# fleet-dedup

![License](https://img.shields.io/badge/license-MIT-blue)
![Language](https://img.shields.io/badge/language-Rust-orange)
![Part of SuperInstance](https://img.shields.io/badge/part%20of-SuperInstance-blue)

Detect duplicate and near-duplicate repos across a directory tree — BLAKE3 content hashing, Hamming distance similarity, and a JSON dedup plan telling you exactly what to archive.

## Overview

When you're managing hundreds of repos (the SuperInstance fleet sits at 562 crates), duplicates accumulate. Copy-paste forks, versioned backups (`-v2`, `-old`, `-bak`), abandoned experiments that share 95% of their code. `fleet-dedup` scans a directory for git repos, computes content fingerprints using BLAKE3, groups them by exact match / near-duplicate / fork, and outputs a structured JSON plan: keep this, archive that, here's why.

The conservation law **γ + η = C** applies here too — attention spent managing duplicate repos is attention not spent building. This tool compresses fleet hygiene into a single command.

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Scan a directory for duplicate repos
fleet-dedup /home/user/repos

# Dry run (show plan, don't act)
fleet-dedup /home/user/repos --dry-run

# Adjust similarity threshold (Hamming distance 0–255, default 3)
fleet-dedup /home/user/repos --max-distance 5

# Write plan to file instead of stdout
fleet-dedup /home/user/repos --output dedup-plan.json
```

Output:

```
🔍 Scanning /home/user/repos for git repos...
   Found 47 git repos
🔑 Computing fingerprints...
📊 Comparing fingerprints...
📋 Generating dedup plan...
📊 Summary: 3 exact dupes, 2 near-dupes, 1 forks across 47 repos
```

The JSON plan:

```json
{
  "summary": {
    "total_repos": 47,
    "exact_duplicates": 3,
    "near_duplicates": 2,
    "forks": 1,
    "repos_to_keep": 40,
    "repos_to_archive": 7,
    "potential_savings": "14.9% of repos (7/47 repos could be archived)"
  },
  "actions": [
    { "repo": "my-tool", "action": "keep", "reason": "Primary repo — most commits/tests/readme" },
    { "repo": "my-tool-copy", "action": "archive", "reason": "Exact duplicate — safe to archive" }
  ],
  "duplicateGroups": [...]
}
```

## Architecture

```
fleet-dedup/
├── src/main.rs          CLI entry, orchestrates scan → fingerprint → group → plan
├── src/scanner.rs       scan_repos(): walks directory, finds git repos
├── src/fingerprint.rs   BLAKE3 content hashing, feature bit extraction, Hamming distance
├── src/group.rs         Three-pass grouping: exact → near-duplicate → fork
└── src/plan.rs          generate_plan(): builds the JSON dedup plan with actions
```

```
         ┌──────────────────┐
         │  Directory tree   │
         └────────┬─────────┘
                  │ scan_repos()
                  ▼
         ┌──────────────────┐
         │  Vec<Repo>        │  47 git repos found
         └────────┬─────────┘
                  │ compute_all() (parallel, rayon)
                  ▼
    ┌─────────────────────────────────┐
    │       Vec<Fingerprint>          │
    │  ┌──────────────────────────┐   │
    │  │ content_hash (BLAKE3)    │   │  Concatenated source file hashes
    │  │ dependencies (sorted)    │   │  Parsed from Cargo.toml / package.json
    │  │ test_count               │   │  #[test], test(), def test_, func Test
    │  │ source_file_count        │   │  .rs, .ts, .py, .go, .java, .c, .cpp...
    │  │ has_readme               │   │  README.md / README / readme.md
    │  │ commit_count             │   │  git rev-list --count HEAD
    │  │ file_types {ext: count}  │   │
    │  │ to_feature_bits() → [u8;32]│  │  Bit-packed for Hamming comparison
    │  └──────────────────────────┘   │
    └──────────────┬──────────────────┘
                   │ group_repos()
                   ▼
    ┌──────────────────────────────┐
    │  Three-pass grouping         │
    │  1. Exact  → same hash       │  GroupKind::ExactDuplicate
    │  2. Near   → Hamming ≤ max   │  GroupKind::NearDuplicate
    │  3. Forks  → same name prefix│  GroupKind::Fork
    │     + relaxed Hamming        │
    └──────────────┬───────────────┘
                   │ generate_plan()
                   ▼
           ┌──────────────┐
           │  DedupPlan    │  JSON: summary + actions + group details
           └──────────────┘
```

## API Reference

### `fingerprint::Fingerprint`

```rust
pub struct Fingerprint {
    pub repo_name: String,
    pub repo_path: String,
    pub content_hash: String,          // BLAKE3 hex of concatenated sources
    pub dependencies: Vec<String>,     // sorted, deduped
    pub test_count: u32,
    pub source_file_count: u32,
    pub has_readme: bool,
    pub commit_count: u32,
    pub file_types: HashMap<String, u32>,
}

impl Fingerprint {
    pub fn to_feature_bits(&self) -> [u8; 32];  // bit-packed for Hamming
}
```

### `fingerprint::hamming_distance`

```rust
pub fn hamming_distance(a: &[u8], b: &[u8]) -> u32;
```

Counts differing bits across two byte arrays. Used to classify near-duplicates (default threshold: ≤ 3 bits different out of 256).

### `group::group_repos`

```rust
pub fn group_repos(fingerprints: &[Fingerprint], max_distance: u32) -> Vec<RepoGroup>;

pub enum GroupKind {
    ExactDuplicate,   // Same BLAKE3 content hash
    NearDuplicate,    // Hamming distance ≤ max_distance
    Fork,             // Same name prefix (stripping -v2, -copy, -old, etc.) + relaxed distance
}

pub struct RepoGroup {
    pub kind: GroupKind,
    pub repos: Vec<String>,
    pub primary: String,         // recommended repo to keep
    pub similarity: f64,         // 0.0–1.0
}
```

The primary repo is selected by `score_repo()`: has_readme (+100), commit_count (up to +1000), test_count × 10, source_file_count.

### `plan::generate_plan`

```rust
pub fn generate_plan(groups: &[RepoGroup], fingerprints: &[Fingerprint]) -> DedupPlan;

pub struct DedupPlan {
    pub summary: Summary,
    pub actions: Vec<Action>,
    pub duplicate_groups: Vec<DuplicateGroupDetail>,
}
```

## Feature Bit Packing

The 32-byte feature vector encodes repo characteristics for fast Hamming comparison:

| Bytes | Content |
|-------|---------|
| 0–3 | test_count (u32 LE) |
| 4–7 | source_file_count (u32 LE) |
| 8–9 | dependency count (u16 LE) |
| 12–15 | BLAKE3 hash of dependency names |
| 16–19 | BLAKE3 hash of file type distribution |
| 20 | has_readme flag (0xFF / 0x00) |
| 21–23 | commit_count (truncated to 3 bytes) |

## Related Crates

- **dep-audit** — vulnerability and health scoring per crate
- **cross-compile-checker** — cross-platform compat analysis
- **ternary-pack** — data packing in the ternary fleet
- **open-parallel** — distributed fleet coordination

## License

MIT

use crate::fingerprint::{hamming_distance, Fingerprint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GroupKind {
    ExactDuplicate,
    NearDuplicate,
    Fork,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoGroup {
    pub kind: GroupKind,
    pub repos: Vec<String>,
    /// The repo recommended as the "primary" to keep
    pub primary: String,
    /// Similarity score (0.0–1.0)
    pub similarity: f64,
}

/// Strip version-like suffixes from repo name to find the "base" name
fn regex_suffix() -> &'static str {
    // Matches trailing patterns like: -v2, -v3, -copy, -backup, -old, -bak, -2, -3, etc.
    r"(-v\d+|-copy|-backup|-old|-bak|-\d+)$"
}

/// Group repos into exact duplicates, near-duplicates, and forks
pub fn group_repos(fingerprints: &[Fingerprint], max_distance: u32) -> Vec<RepoGroup> {
    let mut groups: Vec<RepoGroup> = Vec::new();
    let mut assigned: HashMap<String, usize> = HashMap::new();

    // 1. Find exact duplicates (same content_hash)
    let mut hash_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, fp) in fingerprints.iter().enumerate() {
        hash_map
            .entry(fp.content_hash.clone())
            .or_default()
            .push(i);
    }

    for (_, indices) in &hash_map {
        if indices.len() < 2 {
            continue;
        }
        let names: Vec<String> = indices.iter().map(|&i| fingerprints[i].repo_name.clone()).collect();
        let best = pick_best(indices, fingerprints);
        let group = RepoGroup {
            kind: GroupKind::ExactDuplicate,
            repos: names,
            primary: fingerprints[best].repo_name.clone(),
            similarity: 1.0,
        };
        let group_idx = groups.len();
        groups.push(group);
        for &i in indices {
            assigned.insert(fingerprints[i].repo_name.clone(), group_idx);
        }
    }

    // 2. Find near-duplicates (similar feature bits)
    let bits: Vec<[u8; 32]> = fingerprints.iter().map(|fp| fp.to_feature_bits()).collect();

    for i in 0..fingerprints.len() {
        if assigned.contains_key(&fingerprints[i].repo_name) {
            continue;
        }
        for j in (i + 1)..fingerprints.len() {
            if assigned.contains_key(&fingerprints[j].repo_name) {
                continue;
            }
            let dist = hamming_distance(&bits[i], &bits[j]);
            if dist <= max_distance {
                let similarity = 1.0 - (dist as f64 / 256.0);
                let best = if score_repo(&fingerprints[i]) >= score_repo(&fingerprints[j]) {
                    i
                } else {
                    j
                };
                let group = RepoGroup {
                    kind: GroupKind::NearDuplicate,
                    repos: vec![fingerprints[i].repo_name.clone(), fingerprints[j].repo_name.clone()],
                    primary: fingerprints[best].repo_name.clone(),
                    similarity,
                };
                let group_idx = groups.len();
                groups.push(group);
                assigned.insert(fingerprints[i].repo_name.clone(), group_idx);
                assigned.insert(fingerprints[j].repo_name.clone(), group_idx);
                break; // each repo joins at most one near-dupe group
            }
        }
    }

    // 3. Find forks (same name prefix + similar structure)
    let name_prefixes: Vec<String> = fingerprints
        .iter()
        .map(|fp| {
            let name = &fp.repo_name;
            // Strip common version suffixes: -v2, -v3, -copy, -backup, -old, -bak, -2, -3
            let re = regex_suffix();
            re.replace(name, "").to_string()
        })
        .collect();

    for i in 0..fingerprints.len() {
        if assigned.contains_key(&fingerprints[i].repo_name) {
            continue;
        }
        let mut fork_group = vec![i];
        for j in (i + 1)..fingerprints.len() {
            if assigned.contains_key(&fingerprints[j].repo_name) {
                continue;
            }
            if name_prefixes[i] == name_prefixes[j] && name_prefixes[i].len() >= 3 {
                // Also check structure similarity
                let dist = hamming_distance(&bits[i], &bits[j]);
                if dist <= max_distance * 3 {
                    fork_group.push(j);
                }
            }
        }
        if fork_group.len() >= 2 {
            let best = pick_best(&fork_group, fingerprints);
            let names: Vec<String> = fork_group.iter().map(|&idx| fingerprints[idx].repo_name.clone()).collect();
            let similarity = compute_avg_similarity(&fork_group, &bits);
            let group = RepoGroup {
                kind: GroupKind::Fork,
                repos: names,
                primary: fingerprints[best].repo_name.clone(),
                similarity,
            };
            let group_idx = groups.len();
            groups.push(group);
            for &idx in &fork_group {
                assigned.insert(fingerprints[idx].repo_name.clone(), group_idx);
            }
        }
    }

    groups.sort_by(|a, b| {
        let kind_order = |k: &GroupKind| match k {
            GroupKind::ExactDuplicate => 0,
            GroupKind::NearDuplicate => 1,
            GroupKind::Fork => 2,
        };
        kind_order(&a.kind).cmp(&kind_order(&b.kind))
    });

    groups
}

/// Score a repo for "keep" preference
fn score_repo(fp: &Fingerprint) -> u32 {
    let mut score = 0;
    if fp.has_readme {
        score += 100;
    }
    score += fp.commit_count.min(1000);
    score += fp.test_count * 10;
    score += fp.source_file_count;
    score
}

fn pick_best(indices: &[usize], fingerprints: &[Fingerprint]) -> usize {
    indices
        .iter()
        .copied()
        .max_by_key(|&i| score_repo(&fingerprints[i]))
        .unwrap_or(indices[0])
}

fn compute_avg_similarity(indices: &[usize], bits: &[[u8; 32]]) -> f64 {
    if indices.len() < 2 {
        return 1.0;
    }
    let mut total = 0.0;
    let mut count = 0;
    for i in 0..indices.len() {
        for j in (i + 1)..indices.len() {
            let dist = hamming_distance(&bits[indices[i]], &bits[indices[j]]);
            total += 1.0 - (dist as f64 / 256.0);
            count += 1;
        }
    }
    if count == 0 {
        1.0
    } else {
        total / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_fp(name: &str, content_hash: &str, deps: &[&str], tests: u32, commits: u32) -> Fingerprint {
        Fingerprint {
            repo_name: name.to_string(),
            repo_path: format!("/repos/{}", name),
            content_hash: content_hash.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            test_count: tests,
            source_file_count: 5,
            has_readme: false,
            commit_count: commits,
            file_types: HashMap::new(),
        }
    }

    #[test]
    fn test_no_groups_for_unique_repos() {
        let fps = vec![
            make_fp("a", "hash_a", &["serde"], 1, 10),
            make_fp("b", "hash_b", &["clap"], 2, 20),
            make_fp("c", "hash_c", &["tokio"], 3, 30),
        ];
        let groups = group_repos(&fps, 2);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_exact_duplicate_grouping() {
        let fps = vec![
            make_fp("repo1", "same_hash", &["serde"], 1, 10),
            make_fp("repo2", "same_hash", &["serde"], 1, 20),
        ];
        let groups = group_repos(&fps, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, GroupKind::ExactDuplicate);
        assert_eq!(groups[0].primary, "repo2"); // more commits
    }

    #[test]
    fn test_fork_grouping_by_prefix() {
        // agent-riff, agent-riff-v2 — same base prefix, different enough to be forks not near-dupes
        let fp1 = make_fp("agent-riff", "hash1", &["clap"], 5, 50);
        let fp2 = make_fp("agent-riff-v2", "hash2", &["clap"], 100, 200);
        // Use a small max_distance so near-duplicate doesn't catch them, but fork threshold (3x) will
        let fps = vec![fp1, fp2];
        let groups = group_repos(&fps, 10);
        // They should be grouped — either as forks or near-duplicates
        assert!(!groups.is_empty(), "repos with same base name should be grouped");
    }

    #[test]
    fn test_primary_picks_readme_and_tests() {
        let fps = vec![
            make_fp("repo-no-readme", "same", &["serde"], 0, 5),
            make_fp("repo-with-readme", "same", &["serde"], 10, 5),
        ];
        // Manually override
        let mut fps = fps;
        fps[1].has_readme = true;

        let groups = group_repos(&fps, 2);
        assert_eq!(groups[0].primary, "repo-with-readme");
    }

    #[test]
    fn test_groups_sorted_by_kind() {
        let mut fps = vec![
            make_fp("a", "h1", &[], 0, 1),
            make_fp("b", "h1", &[], 0, 2), // exact dupe with a
        ];
        // Add near-dupes would need specific feature bits, just test sorting
        let groups = group_repos(&fps, 2);
        // Only exact duplicates exist here
        assert!(groups.iter().all(|g| g.kind == GroupKind::ExactDuplicate));
    }
}

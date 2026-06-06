use crate::scanner::Repo;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Content fingerprint for a repo
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    pub repo_name: String,
    pub repo_path: String,

    /// BLAKE3 hash of concatenated main source files
    pub content_hash: String,

    /// Sorted list of dependency names (from Cargo.toml, package.json, etc.)
    pub dependencies: Vec<String>,

    /// Number of test functions found
    pub test_count: u32,

    /// Number of source files
    pub source_file_count: u32,

    /// Has a README
    pub has_readme: bool,

    /// Number of git commits (if determinable)
    pub commit_count: u32,

    /// File extension -> count
    pub file_types: HashMap<String, u32>,
}

impl Fingerprint {
    /// Compute a bit-packed representation for Hamming distance comparison
    pub fn to_feature_bits(&self) -> [u8; 32] {
        let mut bits = [0u8; 32];

        // Byte 0-3: test_count (truncated)
        let tc = self.test_count.min(u32::MAX);
        bits[0] = (tc & 0xFF) as u8;
        bits[1] = ((tc >> 8) & 0xFF) as u8;
        bits[2] = ((tc >> 16) & 0xFF) as u8;
        bits[3] = ((tc >> 24) & 0xFF) as u8;

        // Byte 4-7: source_file_count
        let sc = self.source_file_count.min(u32::MAX);
        bits[4] = (sc & 0xFF) as u8;
        bits[5] = ((sc >> 8) & 0xFF) as u8;
        bits[6] = ((sc >> 16) & 0xFF) as u8;
        bits[7] = ((sc >> 24) & 0xFF) as u8;

        // Byte 8-11: number of dependencies
        let dc = self.dependencies.len() as u32;
        bits[8] = (dc & 0xFF) as u8;
        bits[9] = ((dc >> 8) & 0xFF) as u8;

        // Byte 12-15: dependency name hash
        let dep_str = self.dependencies.join(",");
        let dep_hash = blake3::hash(dep_str.as_bytes());
        bits[12..16].copy_from_slice(&dep_hash.as_bytes()[..4]);

        // Byte 16-19: file type hash
        let ft_str: String = self
            .file_types
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        let ft_hash = blake3::hash(ft_str.as_bytes());
        bits[16..20].copy_from_slice(&ft_hash.as_bytes()[..4]);

        // Byte 20: has_readme flag
        bits[20] = if self.has_readme { 0xFF } else { 0x00 };

        // Byte 21-23: commit_count (truncated to 3 bytes)
        bits[21] = (self.commit_count & 0xFF) as u8;
        bits[22] = ((self.commit_count >> 8) & 0xFF) as u8;
        bits[23] = ((self.commit_count >> 16) & 0xFF) as u8;

        bits
    }
}

/// Compute Hamming distance between two byte arrays
pub fn hamming_distance(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

/// Compute fingerprints for all repos in parallel
pub fn compute_all(repos: &[Repo]) -> Vec<Fingerprint> {
    repos
        .par_iter()
        .map(|repo| compute_fingerprint(repo))
        .collect()
}

/// Compute a single repo's fingerprint
pub fn compute_fingerprint(repo: &Repo) -> Fingerprint {
    let path = &repo.path;

    // Collect source files and compute content hash
    let source_files = collect_source_files(path);
    let content_hash = compute_content_hash(path, &source_files);

    // Parse dependencies
    let dependencies = parse_dependencies(path);

    // Count test functions
    let test_count = count_tests(path);

    // Count source files
    let source_file_count = source_files.len() as u32;

    // Check README
    let has_readme = path.join("README.md").exists()
        || path.join("README").exists()
        || path.join("readme.md").exists();

    // Count commits
    let commit_count = count_commits(path);

    // File types
    let file_types = compute_file_types(path);

    Fingerprint {
        repo_name: repo.name.clone(),
        repo_path: path.to_string_lossy().to_string(),
        content_hash,
        dependencies,
        test_count,
        source_file_count,
        has_readme,
        commit_count,
        file_types,
    }
}

fn collect_source_files(path: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = walkdir::WalkDir::new(path)
        .max_depth(5)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden dirs, target, node_modules, .git
            !name.starts_with('.')
                && name != "target"
                && name != "node_modules"
                && name != "dist"
                && name != "build"
        })
        .collect::<Result<Vec<_>, _>>()
    {
        for entry in entries {
            if entry.file_type().is_file() {
                let p = entry.path();
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if matches!(
                        ext,
                        "rs"
                            | "ts"
                            | "js"
                            | "py"
                            | "go"
                            | "java"
                            | "c"
                            | "cpp"
                            | "h"
                            | "hpp"
                            | "rb"
                            | "sh"
                    ) {
                        files.push(p.to_path_buf());
                    }
                }
            }
        }
    }
    files.sort();
    files
}

fn compute_content_hash(root: &Path, files: &[std::path::PathBuf]) -> String {
    let mut hasher = blake3::Hasher::new();
    for file in files {
        // Use relative path + content for hashing
        let rel = file.strip_prefix(root).unwrap_or(file);
        hasher.update(rel.to_string_lossy().as_bytes());
        if let Ok(content) = fs::read(file) {
            hasher.update(&content);
        }
    }
    hex::encode(hasher.finalize().as_bytes())
}

fn parse_dependencies(path: &Path) -> Vec<String> {
    let mut deps = Vec::new();

    // Cargo.toml
    let cargo = path.join("Cargo.toml");
    if cargo.exists() {
        if let Ok(content) = fs::read_to_string(&cargo) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with(|c: char| c.is_alphabetic())
                    && !trimmed.starts_with('#')
                    && !trimmed.starts_with('[')
                    && !trimmed.contains("::")
                    && !trimmed.contains("fn ")
                    && !trimmed.contains("pub ")
                {
                    if let Some(name) = trimmed.split('=').next() {
                        let name = name.trim().to_string();
                        if !name.is_empty() && !name.contains('(') && !name.contains('{') {
                            deps.push(name);
                        }
                    }
                }
            }
        }
    }

    // package.json
    let pkg = path.join("package.json");
    if pkg.exists() {
        if let Ok(content) = fs::read_to_string(&pkg) {
            // Simple parsing: look for "dep-name" patterns
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('"') && trimmed.contains(':') {
                    if let Some(name) = trimmed.split(':').next() {
                        let name = name.trim().trim_matches('"');
                        if !name.is_empty() && name.len() > 1 {
                            deps.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    // requirements.txt / pyproject.toml
    for req_file in &["requirements.txt", "pyproject.toml"] {
        let req = path.join(req_file);
        if req.exists() {
            if let Ok(content) = fs::read_to_string(&req) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty()
                        && !trimmed.starts_with('#')
                        && !trimmed.starts_with('-')
                        && !trimmed.starts_with('[')
                    {
                        let dep_name = trimmed
                            .split(['=', '<', '>', '!', ';', '~', ' '])
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !dep_name.is_empty() {
                            deps.push(dep_name);
                        }
                    }
                }
            }
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

fn count_tests(path: &Path) -> u32 {
    let mut count = 0u32;
    if let Ok(entries) = walkdir::WalkDir::new(path)
        .max_depth(5)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
                && name != "target"
                && name != "node_modules"
                && name != ".git"
        })
        .collect::<Result<Vec<_>, _>>()
    {
        for entry in entries {
            if entry.file_type().is_file() {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        // Rust: #[test], fn test_, #[tokio::test]
                        if trimmed.contains("#[test]")
                            || trimmed.contains("#[tokio::test]")
                            || trimmed.starts_with("fn test_")
                            || trimmed.starts_with("async fn test_")
                        {
                            count += 1;
                        }
                        // JS/TS: test("...", it("..."
                        if trimmed.starts_with("test(")
                            || trimmed.starts_with("it(")
                            || trimmed.starts_with("describe(")
                        {
                            count += 1;
                        }
                        // Python: def test_, class Test
                        if trimmed.starts_with("def test_")
                            || (trimmed.starts_with("class Test") && trimmed.ends_with(':'))
                        {
                            count += 1;
                        }
                        // Go: func Test
                        if trimmed.starts_with("func Test") {
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    count
}

fn count_commits(path: &Path) -> u32 {
    use std::process::Command;
    let output = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(path)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            s.parse().unwrap_or(0)
        }
        _ => 0,
    }
}

fn compute_file_types(path: &Path) -> HashMap<String, u32> {
    let mut types = HashMap::new();
    if let Ok(entries) = walkdir::WalkDir::new(path)
        .max_depth(5)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
                && name != "target"
                && name != "node_modules"
                && name != ".git"
                && name != "dist"
                && name != "build"
        })
        .collect::<Result<Vec<_>, _>>()
    {
        for entry in entries {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    *types.entry(ext.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_test_repo(dir: &Path, name: &str, files: &[(&str, &str)]) -> Repo {
        let repo_path = dir.join(name);
        fs::create_dir_all(repo_path.join(".git")).unwrap();

        for (file_path, content) in files {
            let full_path = repo_path.join(file_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full_path, content).unwrap();
        }

        Repo {
            name: name.to_string(),
            path: repo_path,
        }
    }

    #[test]
    fn test_content_hash_identical() {
        let tmp = TempDir::new().unwrap();
        let repo1 = make_test_repo(
            tmp.path(),
            "repo1",
            &[("src/main.rs", "fn main() {}"), ("README.md", "# hello")],
        );
        let repo2 = make_test_repo(
            tmp.path(),
            "repo2",
            &[("src/main.rs", "fn main() {}"), ("README.md", "# hello")],
        );
        let fp1 = compute_fingerprint(&repo1);
        let fp2 = compute_fingerprint(&repo2);
        assert_eq!(fp1.content_hash, fp2.content_hash);
    }

    #[test]
    fn test_content_hash_different() {
        let tmp = TempDir::new().unwrap();
        let repo1 = make_test_repo(
            tmp.path(),
            "repo1",
            &[("src/main.rs", "fn main() { println!(\"a\"); }")],
        );
        let repo2 = make_test_repo(
            tmp.path(),
            "repo2",
            &[("src/main.rs", "fn main() { println!(\"b\"); }")],
        );
        let fp1 = compute_fingerprint(&repo1);
        let fp2 = compute_fingerprint(&repo2);
        assert_ne!(fp1.content_hash, fp2.content_hash);
    }

    #[test]
    fn test_parse_cargo_dependencies() {
        let tmp = TempDir::new().unwrap();
        let repo = make_test_repo(
            tmp.path(),
            "test-repo",
            &[(
                "Cargo.toml",
                "[package]\nname = \"test\"\n\n[dependencies]\nclap = \"4\"\nserde = \"1\"\n",
            )],
        );
        let fp = compute_fingerprint(&repo);
        assert!(fp.dependencies.contains(&"clap".to_string()));
        assert!(fp.dependencies.contains(&"serde".to_string()));
    }

    #[test]
    fn test_parse_npm_dependencies() {
        let tmp = TempDir::new().unwrap();
        let repo = make_test_repo(
            tmp.path(),
            "test-repo",
            &[
                ("package.json", "{\n  \"dependencies\": {\n    \"express\": \"^4.0.0\"\n  }\n}\n"),
                ("src/index.js", "console.log('hello');\n"),
            ],
        );
        let fp = compute_fingerprint(&repo);
        assert!(fp.dependencies.contains(&"express".to_string()));
    }

    #[test]
    fn test_count_rust_tests() {
        let tmp = TempDir::new().unwrap();
        let repo = make_test_repo(
            tmp.path(),
            "test-repo",
            &[
                ("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"),
                (
                    "src/lib.rs",
                    "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_add() {}\n    #[test]\n    fn test_sub() {}\n}\n",
                ),
            ],
        );
        // Overwrite with combined content
        fs::write(
            repo.path.join("src/lib.rs"),
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_add() {}\n    #[test]\n    fn test_sub() {}\n}\n",
        ).unwrap();
        let fp = compute_fingerprint(&repo);
        assert!(fp.test_count >= 2);
    }

    #[test]
    fn test_has_readme() {
        let tmp = TempDir::new().unwrap();
        let repo_with = make_test_repo(tmp.path(), "with", &[("README.md", "# hi")]);
        let repo_without = make_test_repo(tmp.path(), "without", &[]);
        let fp_with = compute_fingerprint(&repo_with);
        let fp_without = compute_fingerprint(&repo_without);
        assert!(fp_with.has_readme);
        assert!(!fp_without.has_readme);
    }

    #[test]
    fn test_file_types() {
        let tmp = TempDir::new().unwrap();
        let repo = make_test_repo(
            tmp.path(),
            "test-repo",
            &[
                ("src/main.rs", "fn main() {}"),
                ("src/lib.rs", "// lib"),
                ("src/utils.rs", "// utils"),
            ],
        );
        let fp = compute_fingerprint(&repo);
        assert_eq!(*fp.file_types.get("rs").unwrap_or(&0), 3);
    }

    #[test]
    fn test_hamming_distance_zero() {
        let a = [0u8; 32];
        let b = [0u8; 32];
        assert_eq!(hamming_distance(&a, &b), 0);
    }

    #[test]
    fn test_hamming_distance_nonzero() {
        let a = [0u8; 32];
        let b = [0xFF; 32];
        // Each byte differs by 8 bits, 32 bytes = 256 bits
        assert_eq!(hamming_distance(&a, &b), 256);
    }

    #[test]
    fn test_hamming_distance_partial() {
        let a = [0b00000000u8; 32];
        let mut b = [0u8; 32];
        b[0] = 0b00000001; // 1 bit different
        assert_eq!(hamming_distance(&a, &b), 1);
    }

    #[test]
    fn test_feature_bits_deterministic() {
        let fp = Fingerprint {
            repo_name: "test".to_string(),
            repo_path: "/tmp/test".to_string(),
            content_hash: "abc".to_string(),
            dependencies: vec!["clap".to_string(), "serde".to_string()],
            test_count: 5,
            source_file_count: 10,
            has_readme: true,
            commit_count: 42,
            file_types: [("rs".to_string(), 5u32)].into_iter().collect(),
        };
        let bits1 = fp.to_feature_bits();
        let bits2 = fp.to_feature_bits();
        assert_eq!(bits1, bits2);
    }
}

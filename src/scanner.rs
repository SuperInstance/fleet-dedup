use std::path::{Path, PathBuf};

/// A discovered git repo
#[derive(Debug, Clone, serde::Serialize)]
pub struct Repo {
    pub path: PathBuf,
    pub name: String,
}

/// Scan a directory recursively for git repos (directories containing .git)
pub fn scan_repos(root: &Path) -> Result<Vec<Repo>, std::io::Error> {
    let mut repos = Vec::new();
    scan_repos_recursive(root, &mut repos)?;
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(repos)
}

fn scan_repos_recursive(dir: &Path, repos: &mut Vec<Repo>) -> Result<(), std::io::Error> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // skip unreadable dirs
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Skip hidden dirs at the top level
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') {
            continue;
        }

        // Check if this is a git repo
        if path.join(".git").is_dir() {
            repos.push(Repo {
                name: name.to_string(),
                path: path.clone(),
            });
            // Don't recurse into git repos — we won't find nested ones
        } else {
            // Recurse into non-git directories
            scan_repos_recursive(&path, repos)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_repo(dir: &Path, name: &str) -> PathBuf {
        let repo = dir.join(name);
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        repo
    }

    #[test]
    fn test_scan_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let repos = scan_repos(tmp.path()).unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn test_scan_single_repo() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "my-repo");
        let repos = scan_repos(tmp.path()).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
    }

    #[test]
    fn test_scan_multiple_repos() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "repo-a");
        make_repo(tmp.path(), "repo-b");
        make_repo(tmp.path(), "repo-c");
        let repos = scan_repos(tmp.path()).unwrap();
        assert_eq!(repos.len(), 3);
    }

    #[test]
    fn test_scan_skips_hidden_dirs() {
        let tmp = TempDir::new().unwrap();
        let hidden = tmp.path().join(".hidden-repo");
        fs::create_dir_all(hidden.join(".git")).unwrap();
        let repos = scan_repos(tmp.path()).unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn test_scan_nested_directory() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("subdir");
        fs::create_dir_all(&nested).unwrap();
        make_repo(&nested, "nested-repo");
        let repos = scan_repos(tmp.path()).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "nested-repo");
    }

    #[test]
    fn test_scan_does_not_recurse_into_git_repos() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("outer");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // Nested git repo inside a git repo
        fs::create_dir_all(repo.join("submodule/.git")).unwrap();
        let repos = scan_repos(tmp.path()).unwrap();
        // Should only find the outer repo, not the submodule
        assert_eq!(repos.len(), 1);
    }

    #[test]
    fn test_scan_results_sorted() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "zebra");
        make_repo(tmp.path(), "alpha");
        make_repo(tmp.path(), "middle");
        let repos = scan_repos(tmp.path()).unwrap();
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }
}

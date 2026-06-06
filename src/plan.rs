use crate::fingerprint::Fingerprint;
use crate::group::{GroupKind, RepoGroup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct DedupPlan {
    pub summary: Summary,
    pub actions: Vec<Action>,
    #[serde(rename = "duplicateGroups")]
    pub duplicate_groups: Vec<DuplicateGroupDetail>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    pub total_repos: usize,
    pub exact_duplicates: usize,
    pub near_duplicates: usize,
    pub forks: usize,
    pub repos_to_keep: usize,
    pub repos_to_archive: usize,
    pub potential_savings: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Action {
    pub repo: String,
    pub action: String,
    pub reason: String,
    pub group_id: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateGroupDetail {
    pub id: usize,
    pub kind: String,
    pub repos: Vec<RepoDetail>,
    pub primary: String,
    pub similarity: f64,
    pub suggestion: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoDetail {
    pub name: String,
    pub path: String,
    pub source_files: u32,
    pub tests: u32,
    pub commits: u32,
    pub has_readme: bool,
}

pub fn generate_plan(groups: &[RepoGroup], fingerprints: &[Fingerprint]) -> DedupPlan {
    let fp_map: HashMap<&str, &Fingerprint> = fingerprints
        .iter()
        .map(|fp| (fp.repo_name.as_str(), fp))
        .collect();

    let mut actions = Vec::new();
    let mut duplicate_groups = Vec::new();
    let mut repos_to_keep = std::collections::HashSet::new();
    let mut repos_to_archive = std::collections::HashSet::new();

    let mut exact_count = 0;
    let mut near_count = 0;
    let mut fork_count = 0;

    for (idx, group) in groups.iter().enumerate() {
        match group.kind {
            GroupKind::ExactDuplicate => exact_count += 1,
            GroupKind::NearDuplicate => near_count += 1,
            GroupKind::Fork => fork_count += 1,
        }

        // Primary repo: keep
        repos_to_keep.insert(group.primary.clone());
        actions.push(Action {
            repo: group.primary.clone(),
            action: "keep".to_string(),
            reason: format!(
                "Primary repo for {} group — most commits/tests/readme",
                match group.kind {
                    GroupKind::ExactDuplicate => "exact-duplicate",
                    GroupKind::NearDuplicate => "near-duplicate",
                    GroupKind::Fork => "fork",
                }
            ),
            group_id: Some(idx),
        });

        // Other repos: archive
        for repo_name in &group.repos {
            if repo_name == &group.primary {
                continue;
            }
            repos_to_archive.insert(repo_name.clone());
            let suggestion = match group.kind {
                GroupKind::ExactDuplicate => {
                    "Exact duplicate — safe to archive, consider git remote consolidation".to_string()
                }
                GroupKind::NearDuplicate => {
                    "Near-duplicate — review before archiving, may contain unique changes".to_string()
                }
                GroupKind::Fork => {
                    "Fork — consider merging unique commits into primary, then archive".to_string()
                }
            };
            actions.push(Action {
                repo: repo_name.clone(),
                action: "archive".to_string(),
                reason: suggestion,
                group_id: Some(idx),
            });
        }

        // Build detail
        let details: Vec<RepoDetail> = group
            .repos
            .iter()
            .map(|name| {
                let fp = fp_map.get(name.as_str()).cloned();
                let default = Fingerprint {
                    repo_name: name.clone(),
                    repo_path: String::new(),
                    content_hash: String::new(),
                    dependencies: vec![],
                    test_count: 0,
                    source_file_count: 0,
                    has_readme: false,
                    commit_count: 0,
                    file_types: HashMap::new(),
                };
                let fp = fp.unwrap_or(&default);
                RepoDetail {
                    name: fp.repo_name.clone(),
                    path: fp.repo_path.clone(),
                    source_files: fp.source_file_count,
                    tests: fp.test_count,
                    commits: fp.commit_count,
                    has_readme: fp.has_readme,
                }
            })
            .collect();

        let kind_str = match group.kind {
            GroupKind::ExactDuplicate => "exact_duplicate",
            GroupKind::NearDuplicate => "near_duplicate",
            GroupKind::Fork => "fork",
        };

        let merge_suggestion = match group.kind {
            GroupKind::ExactDuplicate => format!(
                "Archive {} repo(s), keep {} as canonical",
                group.repos.len() - 1,
                group.primary
            ),
            GroupKind::NearDuplicate => format!(
                "Review differences between repos, merge unique work into {}, then archive others",
                group.primary
            ),
            GroupKind::Fork => format!(
                "Consolidate forks into {}, cherry-pick any unique commits from others",
                group.primary
            ),
        };

        duplicate_groups.push(DuplicateGroupDetail {
            id: idx,
            kind: kind_str.to_string(),
            repos: details,
            primary: group.primary.clone(),
            similarity: group.similarity,
            suggestion: merge_suggestion,
        });
    }

    let total_duped = repos_to_archive.len();
    let savings_pct = if fingerprints.is_empty() {
        0.0
    } else {
        (total_duped as f64 / fingerprints.len() as f64) * 100.0
    };

    DedupPlan {
        summary: Summary {
            total_repos: fingerprints.len(),
            exact_duplicates: exact_count,
            near_duplicates: near_count,
            forks: fork_count,
            repos_to_keep: fingerprints.len() - total_duped,
            repos_to_archive: total_duped,
            potential_savings: format!(
                "{:.1}% of repos ({}/{} repos could be archived)",
                savings_pct, total_duped, fingerprints.len()
            ),
        },
        actions,
        duplicate_groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::GroupKind;

    fn make_fp(name: &str) -> Fingerprint {
        Fingerprint {
            repo_name: name.to_string(),
            repo_path: format!("/repos/{}", name),
            content_hash: "test_hash".to_string(),
            dependencies: vec![],
            test_count: 5,
            source_file_count: 10,
            has_readme: true,
            commit_count: 42,
            file_types: HashMap::new(),
        }
    }

    #[test]
    fn test_empty_plan() {
        let plan = generate_plan(&[], &[]);
        assert_eq!(plan.summary.total_repos, 0);
        assert_eq!(plan.actions.len(), 0);
    }

    #[test]
    fn test_single_exact_duplicate_group() {
        let groups = vec![RepoGroup {
            kind: GroupKind::ExactDuplicate,
            repos: vec!["repo-a".to_string(), "repo-b".to_string()],
            primary: "repo-a".to_string(),
            similarity: 1.0,
        }];
        let fps = vec![make_fp("repo-a"), make_fp("repo-b")];
        let plan = generate_plan(&groups, &fps);
        assert_eq!(plan.summary.exact_duplicates, 1);
        assert_eq!(plan.summary.repos_to_archive, 1);
        assert_eq!(plan.summary.repos_to_keep, 1);
    }

    #[test]
    fn test_keep_action_has_reason() {
        let groups = vec![RepoGroup {
            kind: GroupKind::NearDuplicate,
            repos: vec!["x".to_string(), "y".to_string()],
            primary: "x".to_string(),
            similarity: 0.95,
        }];
        let fps = vec![make_fp("x"), make_fp("y")];
        let plan = generate_plan(&groups, &fps);
        let keep = plan.actions.iter().find(|a| a.action == "keep").unwrap();
        assert!(!keep.reason.is_empty());
    }

    #[test]
    fn test_archive_action_for_non_primary() {
        let groups = vec![RepoGroup {
            kind: GroupKind::Fork,
            repos: vec!["main".to_string(), "fork1".to_string()],
            primary: "main".to_string(),
            similarity: 0.8,
        }];
        let fps = vec![make_fp("main"), make_fp("fork1")];
        let plan = generate_plan(&groups, &fps);
        let archive_actions: Vec<_> = plan
            .actions
            .iter()
            .filter(|a| a.action == "archive")
            .collect();
        assert_eq!(archive_actions.len(), 1);
        assert_eq!(archive_actions[0].repo, "fork1");
    }

    #[test]
    fn test_savings_percentage() {
        // 4 repos, 1 pair of exact duplicates → 1 archive
        let groups = vec![RepoGroup {
            kind: GroupKind::ExactDuplicate,
            repos: vec!["a".to_string(), "b".to_string()],
            primary: "a".to_string(),
            similarity: 1.0,
        }];
        let fps = vec![make_fp("a"), make_fp("b"), make_fp("c"), make_fp("d")];
        let plan = generate_plan(&groups, &fps);
        assert!(plan.summary.potential_savings.contains("25.0%"));
    }
}

//! `local-git-branches` refresh source: a read-only summary of the local branches in a
//! fixed list of git repositories, with each branch's remote state, stack parent and PR.
//!
//! The repositories come from server configuration only (never from panel code):
//! `PANELCRAFT_GIT_REPOS` (paths separated like PATH), or else `git-repos.txt` next to
//! the database, one path per line (`#` starts a comment).
//!
//! Remote comparisons use the last locally fetched origin refs.
use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, task::JoinSet, time::timeout};

type Failure = (StatusCode, String);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
    #[serde(default)]
    account: String,
}

fn fail(code: StatusCode, message: impl Into<String>) -> Failure {
    (code, message.into())
}

fn valid_account(account: &str) -> bool {
    !account.is_empty()
        && account.len() <= 39
        && account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The configured repositories, from the environment or the file beside the database.
fn configured_repos() -> Result<Vec<PathBuf>, Failure> {
    if let Some(list) = std::env::var_os("PANELCRAFT_GIT_REPOS") {
        return Ok(std::env::split_paths(&list)
            .filter(|p| !p.as_os_str().is_empty())
            .collect());
    }
    let db = std::env::var("PANELCRAFT_DB").unwrap_or_else(|_| "data/panelcraft.db".into());
    let file = Path::new(&db)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("git-repos.txt");
    let text = std::fs::read_to_string(&file).map_err(|_| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "No repositories configured. Set PANELCRAFT_GIT_REPOS or list one path per line in {}",
                file.display()
            ),
        )
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

async fn run(
    program: &str,
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    secs: u64,
) -> Result<Option<String>, Failure> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = timeout(Duration::from_secs(secs), cmd.output())
        .await
        .map_err(|_| fail(StatusCode::GATEWAY_TIMEOUT, format!("{program} timed out")))?
        .map_err(|_| {
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Install {program} on the server"),
            )
        })?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

async fn git(dir: &Path, args: &[&str]) -> Result<Option<String>, Failure> {
    let output = run("git", dir, args, &[], 30).await?;
    if output.is_none()
        && !matches!(
            args.first(),
            Some(&"symbolic-ref" | &"rev-parse" | &"remote" | &"merge-base")
        )
    {
        return Err(fail(
            StatusCode::BAD_GATEWAY,
            format!("Git {} failed for {}", args[0], dir.display()),
        ));
    }
    Ok(output)
}

/// Refs listed by `git for-each-ref <prefix> --format=%(refname:short)` plus extra filters.
async fn refs(dir: &Path, args: &[&str]) -> Result<HashSet<String>, Failure> {
    let mut all = vec!["for-each-ref", "--format=%(refname:short)"];
    all.extend_from_slice(args);
    Ok(git(dir, &all)
        .await?
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect())
}

async fn count(dir: &Path, range: &str) -> Result<u64, Failure> {
    Ok(git(dir, &["rev-list", "--count", range])
        .await?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// `owner/name` from the origin URL, for `gh pr list -R`.
fn slug_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}

async fn collect(dir: &Path, account: &str) -> Result<Value, Failure> {
    let repo = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if git(dir, &["rev-parse", "--git-dir"]).await?.is_none() {
        return Err(fail(StatusCode::BAD_REQUEST, "Not a git repository"));
    }
    // We work from dev wherever it exists; otherwise the remote's default branch.
    let default = if git(
        dir,
        &["rev-parse", "--verify", "-q", "refs/remotes/origin/dev"],
    )
    .await?
    .is_some()
    {
        "dev".to_string()
    } else {
        git(
            dir,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
        .await?
        .and_then(|s| s.strip_prefix("origin/").map(str::to_string))
        .or(git(dir, &["symbolic-ref", "--short", "HEAD"]).await?)
        .unwrap_or_else(|| "main".into())
    };
    let roots: HashSet<&str> = [default.as_str(), "main", "master"].into();
    let mut origin_default = format!("origin/{default}");
    let current = git(dir, &["branch", "--show-current"])
        .await?
        .unwrap_or_default();

    let mut worktrees = HashMap::new();
    let mut path = None;
    for line in git(dir, &["worktree", "list", "--porcelain"])
        .await?
        .unwrap_or_default()
        .lines()
    {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if let Some(p) = &path {
                worktrees.insert(b.to_string(), p.clone());
            }
        }
    }
    let heads: HashMap<String, String> = git(
        dir,
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(refname:short) %(objectname)",
        ],
    )
    .await?
    .unwrap_or_default()
    .lines()
    .filter_map(|l| {
        l.split_once(' ')
            .map(|(a, b)| (a.to_string(), b.to_string()))
    })
    .collect();
    let remotes: HashMap<String, String> = git(
        dir,
        &[
            "for-each-ref",
            "refs/remotes/origin",
            "--format=%(refname:short) %(objectname)",
        ],
    )
    .await?
    .unwrap_or_default()
    .lines()
    .filter_map(|l| l.split_once(' '))
    .filter_map(|(r, sha)| {
        r.strip_prefix("origin/")
            .map(|n| (n.to_string(), sha.to_string()))
    })
    .filter(|(n, _)| n != "HEAD")
    .collect();
    if !remotes.contains_key(&default) {
        origin_default = format!("refs/heads/{default}");
    }
    let has_default = remotes.contains_key(&default) || heads.contains_key(&default);
    let merged = if has_default {
        refs(dir, &["refs/heads", "--merged", &origin_default]).await?
    } else {
        HashSet::new()
    };

    // The most relevant PR per head branch: an open one first, then the most recently updated.
    let mut prs: HashMap<String, Value> = HashMap::new();
    let slug = git(dir, &["remote", "get-url", "origin"])
        .await?
        .as_deref()
        .and_then(slug_from_url);

    if let Some(slug) = slug.as_ref().filter(|_| !account.is_empty()) {
        let out = run(
            "gh",
            dir,
            &[
                "pr",
                "list",
                "-R",
                slug,
                "--state",
                "all",
                "--author",
                account,
                "--limit",
                "300",
                "--json",
                "number,headRefName,baseRefName,state,isDraft,url,title,updatedAt",
            ],
            &[],
            45,
        )
        .await?;
        match out.and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok()) {
            Some(list) => {
                if list.len() >= 300 {
                    return Err(fail(
                        StatusCode::BAD_GATEWAY,
                        "PR history reached the 300-result limit; keeping the previous snapshot",
                    ));
                }
                for pr in list {
                    let Some(head) = pr["headRefName"].as_str().map(str::to_string) else {
                        continue;
                    };
                    let open = pr["state"] == "OPEN";
                    let better = match prs.get(&head) {
                        None => true,
                        Some(cur) => {
                            let cur_open = cur["state"] == "OPEN";
                            open && !cur_open
                                || open == cur_open
                                    && pr["updatedAt"].as_str() > cur["updatedAt"].as_str()
                        }
                    };
                    if better {
                        prs.insert(head, pr);
                    }
                }
            }
            None => {
                return Err(fail(
                    StatusCode::BAD_GATEWAY,
                    "Pull requests could not be listed",
                ));
            }
        }
    }

    // Remote heads of open PRs, used to tell whether a branch's work is already inside a PR.
    let mut open_heads: Vec<(String, Value)> = prs
        .iter()
        .filter(|(head, p)| p["state"] == "OPEN" && remotes.contains_key(*head))
        .map(|(head, p)| (format!("origin/{head}"), p["number"].clone()))
        .collect();

    open_heads.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names: Vec<&String> = heads.keys().collect();
    names.sort();
    let mut branches = Vec::new();
    for name in names {
        let sha = &heads[name];
        let remote = match remotes.get(name) {
            None => json!({"state":"none"}),
            Some(r) if r == sha => json!({"state":"match"}),
            Some(_) => {
                let ahead = count(dir, &format!("origin/{name}..{name}")).await?;
                let behind = count(dir, &format!("{name}..origin/{name}")).await?;
                let state = if behind == 0 {
                    "ahead"
                } else if ahead == 0 {
                    "behind"
                } else {
                    "diverged"
                };
                json!({"state":state,"ahead":ahead,"behind":behind})
            }
        };
        let pr = prs.get(name.as_str());
        let is_root = roots.contains(name.as_str());
        let in_default = !is_root && merged.contains(name);
        let mut parent: Option<String> = None;
        if !is_root {
            let open_base = pr
                .filter(|p| p["state"] == "OPEN")
                .and_then(|p| p["baseRefName"].as_str())
                .filter(|b| heads.contains_key(*b) || *b == default);
            if in_default {
                parent = Some(default.clone());
            } else if let Some(base) = open_base {
                parent = Some(base.to_string());
            } else {
                // Nearest local branch (or origin/default) whose tip this branch contains,
                // skipping branches that contain this one (same tip), so two never parent each other.
                let ancestors = refs(dir, &["refs/heads", "--merged", name]).await?;
                let descendants = refs(dir, &["refs/heads", "--contains", name]).await?;
                let mut best: Option<(u64, u8, String)> = None;
                let mut candidates: Vec<(String, String)> = ancestors
                    .iter()
                    .filter(|c| {
                        *c != name && !roots.contains(c.as_str()) && !descendants.contains(*c)
                    })
                    .map(|c| (c.clone(), c.clone()))
                    .collect();
                if has_default {
                    let is_anc = git(dir, &["merge-base", "--is-ancestor", &origin_default, name])
                        .await?
                        .is_some();
                    if is_anc {
                        candidates.push((default.clone(), origin_default.clone()));
                    }
                }
                for (label, reference) in candidates {
                    let n = count(dir, &format!("{reference}..{name}")).await?;
                    let key = (n, u8::from(label != default), label);
                    if best.as_ref().is_none_or(|b| key < *b) {
                        best = Some(key);
                    }
                }
                parent = Some(best.map(|b| b.2).unwrap_or_else(|| default.clone()));
            }
        }
        // Integration: commits not yet in the default branch (ignoring cherry-picked or rebased
        // copies), which open PRs already contain this branch, and how many commits no open PR
        // or default branch covers. Squash-merged work still counts as not integrated.
        let mut subjects = Vec::new();
        let (mut unique, mut uncovered, mut covered_by) = (0, 0, Vec::new());
        if !is_root && has_default {
            unique = git(
                dir,
                &[
                    "rev-list",
                    "--count",
                    "--right-only",
                    "--cherry-pick",
                    &format!("{origin_default}...{name}"),
                ],
            )
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
            if unique > 0 {
                let containing = refs(dir, &["refs/remotes/origin", "--contains", name]).await?;
                for (head, number) in &open_heads {
                    if containing.contains(head) {
                        covered_by.push(
                            json!({"number": number, "head": head.trim_start_matches("origin/")}),
                        );
                    }
                }
                let range = format!("{origin_default}...{name}");
                let mut args = vec![
                    "log",
                    "--format=%h %s",
                    "--cherry-pick",
                    "--right-only",
                    &range,
                    "--not",
                ];
                args.extend(open_heads.iter().map(|(h, _)| h.as_str()));
                args.push("--");
                let log = git(dir, &args)
                    .await?
                    .ok_or_else(|| fail(StatusCode::BAD_GATEWAY, "Git history unavailable"))?;
                uncovered = log.lines().count() as u64;
                subjects.extend(log.lines().take(8).map(str::to_string));
            }
        }
        let ahead_of_parent = match &parent {
            Some(p) if p == &default => count(dir, &format!("{origin_default}..{name}")).await?,
            Some(p) => count(dir, &format!("{p}..{name}")).await?,
            None => 0,
        };
        branches.push(json!({
            "name": name,
            "parent": parent,
            "current": *name == current,
            "worktree": if *name == current { None } else { worktrees.get(name.as_str()).cloned() },
            "ahead_of_parent": ahead_of_parent,
            "merged_into_default": in_default,
            "unique_commits": unique,
            "uncovered_commits": uncovered,
            "uncovered": subjects,
            "covered_by": covered_by,
            "remote": remote,
            "pr": pr.map(|p| json!({
                "number": p["number"], "state": p["state"], "isDraft": p["isDraft"],
                "url": p["url"], "title": p["title"], "baseRefName": p["baseRefName"],
            })),
        }));
    }
    Ok(json!({
        "repo": repo,

        "slug": slug,
        "default": default,

        "branches": branches,
    }))
}

pub async fn local_branches(Json(request): Json<SourceRequest>) -> Result<Json<Value>, Failure> {
    let account = request.account;
    if !account.is_empty() && !valid_account(&account) {
        return Err(fail(StatusCode::BAD_REQUEST, "Invalid GitHub account"));
    }
    let repos = configured_repos()?;
    if repos.is_empty() {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "No repositories configured",
        ));
    }
    if !account.is_empty() {
        let login = run(
            "gh",
            Path::new("."),
            &["api", "--hostname", "github.com", "user", "--jq", ".login"],
            &[],
            30,
        )
        .await?;
        if !login.is_some_and(|s| s.eq_ignore_ascii_case(&account)) {
            return Err(fail(
                StatusCode::CONFLICT,
                format!("Sign in to gh as {account} to refresh PR metadata"),
            ));
        }
    }
    let mut jobs = JoinSet::new();
    for (index, dir) in repos.into_iter().enumerate() {
        let account = account.clone();
        jobs.spawn(async move { collect(&dir, &account).await.map(|v| (index, v)) });
    }
    let mut results = Vec::new();
    while let Some(done) = jobs.join_next().await {
        results.push(
            done.map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Repository scan failed"))??,
        );
    }
    results.sort_by_key(|(i, _)| *i);
    Ok(Json(json!({
        "account": account,
        "prsListed": !account.is_empty(),
        "repos": results.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(
            slug_from_url("https://github.com/example/project.git").as_deref(),
            Some("example/project")
        );
        assert_eq!(
            slug_from_url("git@github.com:Cthutu/panelcraft.git").as_deref(),
            Some("Cthutu/panelcraft")
        );
        assert_eq!(slug_from_url("https://example.com/a/b"), None);
        assert_eq!(slug_from_url("https://github.com/a/b/c"), None);
    }

    #[test]
    fn accounts() {
        assert!(valid_account("example-user"));
        assert!(!valid_account("a b"));
        assert!(!valid_account(""));
    }
}

use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};

// gh --paginate supplies endCursor on each subsequent page.
const QUERY: &str = r#"query($search:String!, $endCursor:String) {
  viewer { login }
  search(type:ISSUE, query:$search, first:50, after:$endCursor) {
    issueCount pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      number title url isDraft reviewDecision updatedAt repository { nameWithOwner }
      reviewRequests(first:100) { pageInfo { hasNextPage } nodes {
        requestedReviewer { ... on User { login } ... on Team { slug organization { login } } ... on Mannequin { login } }
      } }
      latestReviews(first:100) { pageInfo { hasNextPage } nodes { author { login } state } }
    } }
  }
}"#;

#[derive(Deserialize)]
pub struct SourceRequest {
    account: String,
}

pub async fn authored_prs(
    Json(request): Json<SourceRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let account = request.account;
    if account.is_empty()
        || account.len() > 39
        || !account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid GitHub account".into()));
    }
    let search = format!("search=is:pr is:open author:{account} sort:updated-desc");
    let mut gh = Command::new("gh");
    gh.args([
        "api",
        "--hostname",
        "github.com",
        "graphql",
        "--paginate",
        "--slurp",
        "-f",
        &format!("query={QUERY}"),
        "-f",
        &search,
    ])
    .env("GH_PROMPT_DISABLED", "1")
    .stdin(Stdio::null())
    .kill_on_drop(true);
    let output = timeout(Duration::from_secs(45), gh.output())
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "GitHub request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Install GitHub CLI (gh) on the server and sign in".into(),
            )
        })?;
    if !output.status.success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            "GitHub request failed. Check gh auth status and GitHub availability.".into(),
        ));
    }
    let invalid = || {
        (
            StatusCode::BAD_GATEWAY,
            "GitHub returned incomplete pull request data".to_string(),
        )
    };
    let pages: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|_| invalid())?;
    let mut prs = Vec::new();
    let mut count = None;
    for page in pages {
        if !page["data"]["viewer"]["login"]
            .as_str()
            .is_some_and(|login| login.eq_ignore_ascii_case(&account))
        {
            return Err((
                StatusCode::CONFLICT,
                format!("Sign in to gh as {account} before refreshing this panel"),
            ));
        }
        let results = &page["data"]["search"];
        count = results["issueCount"].as_u64();
        for pr in results["nodes"].as_array().ok_or_else(invalid)? {
            if !pr["url"].is_string()
                || ["reviewRequests", "latestReviews"].iter().any(|key| {
                    pr[*key]["pageInfo"]["hasNextPage"] != false || !pr[*key]["nodes"].is_array()
                })
            {
                return Err(invalid());
            }
            prs.push(pr.clone());
        }
    }
    if count != Some(prs.len() as u64) {
        return Err(invalid());
    }
    Ok(Json(json!({"account":account,"pullRequests":prs})))
}

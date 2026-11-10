//! Issues from GitHub and Codeberg.

use super::{NewIssue, create_issue, db_label, format_time, urls, user_id};
use crate::buggregator::issues::text_id;
use crate::buggregator::safe_url;
use crate::state::AppState;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// How many issues a forge page holds.
const GITHUB_PAGE: usize = 100;
const CODEBERG_PAGE: usize = 50;
/// Repositories the project has left behind: archived, or superseded by a
/// rewrite, so their old issues are not synced again.
const RETIRED_REPOS: &[&str] =
    &["https://github.com/magnusmanske/quickstatements", "https://codeberg.org/magnusmanske/sourcemd"];

#[derive(Debug, Clone, Copy)]
pub(super) enum Forge {
    GitHub,
    Codeberg,
}

impl Forge {
    fn site(self) -> &'static str {
        match self {
            Self::GitHub => "GITHUB",
            Self::Codeberg => "CODEBERG",
        }
    }

    /// The repository metadata endpoint, where the forge says whether the
    /// repository is archived. Only Codeberg: GitHub's unauthenticated rate
    /// limit is too low for one more call per repository.
    pub(super) fn repo_api(self, html_url: &str) -> Option<String> {
        match self {
            Self::GitHub => None,
            Self::Codeberg => {
                let path = html_url.split_once("codeberg.org/")?.1;
                let mut parts = path.split('/').filter(|p| !p.is_empty());
                let (owner, repo) = (parts.next()?, parts.next()?);
                Some(format!("https://codeberg.org/api/v1/repos/{owner}/{repo}"))
            }
        }
    }

    /// Whether the repository is archived. A failed check counts as not
    /// archived, so a transient API error does not stop the daily import.
    async fn is_archived(self, state: &AppState, html_url: &str) -> bool {
        let Some(url) = self.repo_api(html_url) else { return false };
        let archived = async {
            let response = state
                .http
                .get(&url)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await?
                .error_for_status()?;
            let json: Value = response.json().await?;
            anyhow::Ok(json["archived"].as_bool().unwrap_or(false))
        }
        .await;
        match archived {
            Ok(archived) => archived,
            Err(e) => {
                tracing::warn!("{url}: {e:#}");
                false
            }
        }
    }

    /// The issues endpoint and page size. Codeberg (Forgejo) URLs are built
    /// from the repository's page, since `api_issues_url` holds that for them.
    pub(super) fn api(self, html_url: &str, api_issues_url: &str) -> Option<(String, usize)> {
        match self {
            Self::GitHub => Some((format!("{api_issues_url}?state=open&per_page={GITHUB_PAGE}"), GITHUB_PAGE)),
            Self::Codeberg => {
                let path = html_url.split_once("codeberg.org/")?.1;
                let mut parts = path.split('/').filter(|p| !p.is_empty());
                let (owner, repo) = (parts.next()?, parts.next()?);
                Some((
                    format!(
                        "https://codeberg.org/api/v1/repos/{owner}/{repo}/issues?type=issues&state=open&limit={CODEBERG_PAGE}"
                    ),
                    CODEBERG_PAGE,
                ))
            }
        }
    }
}

/// Stores new open issues of each repository, and closes those the forge
/// no longer lists as open (the PHP version never closed any).
pub(super) async fn from_forge(state: &AppState, conn: &mut Conn, forge: Forge) -> Result<()> {
    let repos: Vec<(u64, String, String, u64)> = conn
        .exec(
            "SELECT id, html_url, api_issues_url, tool_id FROM git_repo WHERE repo_type=? ORDER BY id",
            (forge.site(),),
        )
        .await?;
    for (repo_id, html_url, api_issues_url, tool_id) in repos {
        if RETIRED_REPOS.contains(&html_url.as_str()) {
            continue;
        }
        if forge.is_archived(state, &html_url).await {
            tracing::info!("{html_url}: repository archived; skipping");
            continue;
        }
        let Some((url, page_size)) = forge.api(&html_url, &api_issues_url) else { continue };
        let open = match open_forge_issues(state, &url, page_size).await {
            Ok(open) => open,
            Err(e) => {
                tracing::warn!("{html_url}: {e:#}");
                continue;
            }
        };
        let mut open_numbers = HashSet::new();
        for json in &open {
            let Some(number) = json["number"].as_u64() else { continue };
            open_numbers.insert(number);
            let Some(issue_url) = safe_url(json["html_url"].as_str().unwrap_or_default()) else {
                tracing::warn!("{html_url}: issue without a usable URL; skipping");
                continue;
            };
            if let Some(issue_id) = known_forge_issue(conn, forge, number, repo_id, &html_url, &issue_url).await? {
                update_forge_issue(conn, issue_id, forge, json, &issue_url).await?;
                continue;
            }
            let issue = new_forge_issue(forge, json, tool_id, &issue_url);
            let mut tx = conn.start_transaction(Default::default()).await?;
            let issue_id = create_issue(&mut tx, &issue).await?;
            ensure_mapping(&mut tx, issue_id, forge.site(), number, repo_id).await?;
            tx.commit().await?;
        }
        let stored: Vec<(u64, u64)> = conn
            .exec(
                "SELECT issue.id, git_issue.git_id FROM issue
                 JOIN git_issue ON git_issue.issue_id=issue.id
                 JOIN git_repo ON git_repo.id=git_issue.git_repo_id
                 WHERE git_issue.site=? AND git_repo.repo_type=? AND git_repo.html_url=?
                   AND issue.status='OPEN'",
                (forge.site(), forge.site(), &html_url),
            )
            .await?;
        // A forge that suddenly returns no issues (a proxy error with HTTP 200,
        // an access change) must not close the whole repository's history.
        if open_numbers.is_empty() && !stored.is_empty() {
            tracing::warn!("{html_url}: no open issues returned; not closing {} stored issues", stored.len());
            continue;
        }
        for (issue_id, _) in stored.iter().filter(|(_, number)| !open_numbers.contains(number)) {
            conn.exec_drop("UPDATE issue SET status='CLOSED' WHERE id=?", (issue_id,)).await?;
        }
        tracing::debug!("{html_url}: {} open, {} stored", open_numbers.len(), stored.len());
    }
    Ok(())
}

/// The issue a forge report belongs to: the natural `(site, url)` first, then
/// the repository mapping (the stable identity across URL changes).
async fn known_forge_issue(
    conn: &mut Conn,
    forge: Forge,
    number: u64,
    repo_id: u64,
    html_url: &str,
    url: &str,
) -> Result<Option<u64>> {
    if !url.is_empty()
        && let Some(id) =
            conn.exec_first("SELECT id FROM issue WHERE site=? AND url=? LIMIT 1", (forge.site(), url)).await?
    {
        return Ok(Some(id));
    }
    if let Some(id) = conn
        .exec_first(
            "SELECT issue_id FROM git_issue WHERE site=? AND git_id=? AND git_repo_id=? LIMIT 1",
            (forge.site(), number, repo_id),
        )
        .await?
    {
        return Ok(Some(id));
    }
    Ok(conn
        .exec_first(
            "SELECT gi.issue_id FROM git_issue gi JOIN git_repo r ON r.id=gi.git_repo_id
             WHERE gi.site=? AND gi.git_id=? AND r.html_url=? LIMIT 1",
            (forge.site(), number, html_url),
        )
        .await?)
}

fn new_forge_issue(forge: Forge, json: &Value, tool_id: u64, url: &str) -> NewIssue {
    let description = json["body"].as_str().unwrap_or_default().to_string();
    NewIssue {
        label: json["title"].as_str().unwrap_or_default().to_string(),
        status: "OPEN".to_string(),
        date_created: iso_time(&json["created_at"]),
        date_last: iso_time(&json["updated_at"]),
        site: forge.site(),
        url: url.to_string(),
        urls: urls(&description),
        description,
        tool: Some(tool_id),
        users: json["user"]["login"].as_str().map(str::to_string).into_iter().collect(),
    }
}

/// Updates an issue the forge already knows, including edits to its body.
async fn update_forge_issue(conn: &mut Conn, issue_id: u64, forge: Forge, json: &Value, url: &str) -> Result<()> {
    let description = json["body"].as_str().unwrap_or_default();
    let title = json["title"].as_str().unwrap_or_default();
    let updated = iso_time(&json["updated_at"]);
    let mut tx = conn.start_transaction(Default::default()).await?;
    let description_id = text_id(&mut tx, description).await?;
    // `IGNORE`: a repository rename can collide with another issue's old URL;
    // the mapping still identifies this one.
    tx.exec_drop(
        "UPDATE IGNORE issue SET status='OPEN', label=?, url=?, description_text_id=?,
           date_last=COALESCE(GREATEST(date_last, ?), date_last, ?) WHERE id=?",
        (db_label(title), url, description_id, updated.as_deref(), updated.as_deref(), issue_id),
    )
    .await?;
    for url in urls(description) {
        tx.exec_drop(
            "INSERT INTO url_issue (url, issue_id) VALUES (?, ?)
             ON DUPLICATE KEY UPDATE url=VALUES(url)",
            (url, issue_id),
        )
        .await?;
    }
    if let Some(login) = json["user"]["login"].as_str() {
        let user_id = user_id(&mut tx, login, forge.site()).await?;
        tx.exec_drop(
            "INSERT INTO user_issue (user_id, issue_id, role) VALUES (?, ?, 'CREATOR')
             ON DUPLICATE KEY UPDATE role=IF(role='CREATOR', 'CREATOR', VALUES(role))",
            (user_id, issue_id),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Maps an issue to its forge number. The unique key is `(site, git_id,
/// git_repo_id)`, so a forge's numbers cannot collide with an older forge's.
async fn ensure_mapping(conn: &mut impl Queryable, issue_id: u64, site: &str, number: u64, repo_id: u64) -> Result<()> {
    conn.exec_drop(
        "INSERT INTO git_issue (issue_id, site, git_id, git_repo_id) VALUES (?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE issue_id=VALUES(issue_id)",
        (issue_id, site, number, repo_id),
    )
    .await?;
    Ok(())
}

/// All open issues (not pull requests), page by page.
async fn open_forge_issues(state: &AppState, url: &str, page_size: usize) -> Result<Vec<Value>> {
    /// A runaway pagination loop must not run forever.
    const MAX_PAGES: usize = 400;
    let mut issues = vec![];
    for page in 1..=MAX_PAGES {
        let request = format!("{url}&page={page}");
        let response = state
            .http
            .get(&request)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?
            .error_for_status()?;
        let status = response.status();
        let text = response.text().await?;
        let json: Value = match serde_json::from_str(&text) {
            Ok(json) => json,
            Err(e) => anyhow::bail!("{request}: HTTP {status}, {e}; body: {}", excerpt(&text)),
        };
        // An error object here would be read as "no open issues", and the caller
        // would close every issue of the repository.
        let Some(batch) = json.as_array() else {
            anyhow::bail!("{request}: HTTP {status}, expected a JSON array; body: {}", excerpt(&text));
        };
        let count = batch.len();
        // Forgejo sets `pull_request` to null on issues; GitHub omits it.
        issues.extend(batch.iter().filter(|i| is_issue(i)).cloned());
        if count < page_size {
            return Ok(issues);
        }
    }
    anyhow::bail!("{url}: more than {MAX_PAGES} pages; refusing to guess")
}

/// The start of a response body, for an error message.
fn excerpt(text: &str) -> String {
    text.chars().take(300).collect()
}

/// Whether a forge issue is an issue, not a pull request. GitHub omits the
/// field for issues; Forgejo sets it to null.
pub(super) fn is_issue(json: &Value) -> bool {
    json["pull_request"].is_null()
}

/// `2024-01-02T03:04:05Z` as the database writes it, in UTC.
pub(super) fn iso_time(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| format_time(&t.with_timezone(&Utc).naive_utc()))
}

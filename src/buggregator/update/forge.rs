//! Issues from GitHub and Codeberg.

use super::{NewIssue, create_issue, db_label, db_url, format_time, urls, user_id};
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

    fn host(self) -> &'static str {
        match self {
            Self::GitHub => "github.com",
            Self::Codeberg => "codeberg.org",
        }
    }

    /// `(owner, repository)` from the repository's web URL. A page below the
    /// repository root (`/issues`, `/src`, a trailing `.git`) names the same
    /// repository; another host or a path with fewer than two segments does
    /// not.
    fn owner_repo(self, html_url: &str) -> Option<(&str, &str)> {
        let (_, rest) = html_url.split_once("://")?;
        let (host, path) = rest.split_once('/')?;
        if !host.eq_ignore_ascii_case(self.host()) {
            return None;
        }
        let mut parts = path.split('/').filter(|part| !part.is_empty());
        let owner = parts.next()?;
        let repo = parts.next()?;
        Some((owner, repo.strip_suffix(".git").unwrap_or(repo)))
    }

    /// The repository metadata endpoint, where the forge says whether the
    /// repository is archived. Only Codeberg: GitHub's unauthenticated rate
    /// limit is too low for one more call per repository.
    pub(super) fn repo_api(self, html_url: &str) -> Option<String> {
        match self {
            Self::GitHub => None,
            Self::Codeberg => {
                let (owner, repo) = self.owner_repo(html_url)?;
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

    /// The issues endpoint and page size, generated from the repository's web
    /// URL. A URL to the issues page is accepted because it names the same
    /// repository.
    pub(super) fn issues_api(self, html_url: &str) -> Option<(String, usize)> {
        let (owner, repo) = self.owner_repo(html_url)?;
        match self {
            Self::GitHub => Some((
                format!("https://api.github.com/repos/{owner}/{repo}/issues?state=open&per_page={GITHUB_PAGE}"),
                GITHUB_PAGE,
            )),
            Self::Codeberg => Some((
                format!(
                    "https://codeberg.org/api/v1/repos/{owner}/{repo}/issues?type=issues&state=open&limit={CODEBERG_PAGE}"
                ),
                CODEBERG_PAGE,
            )),
        }
    }
}

/// A repository from `git_repo`, with the tool its issues belong to.
struct ForgeRepo<'a> {
    id: u64,
    html_url: &'a str,
    tool_id: u64,
}

/// Stores new open issues of each repository, and closes those the forge
/// no longer lists as open (the PHP version never closed any). A repository
/// that fails is skipped, so it cannot keep the others from syncing.
pub(super) async fn from_forge(state: &AppState, conn: &mut Conn, forge: Forge) -> Result<()> {
    let repos: Vec<(u64, String, u64)> =
        conn.exec("SELECT id, html_url, tool_id FROM git_repo WHERE repo_type=? ORDER BY id", (forge.site(),)).await?;
    for (id, html_url, tool_id) in repos {
        let repo = ForgeRepo { id, html_url: &html_url, tool_id };
        if RETIRED_REPOS.contains(&repo.html_url) {
            continue;
        }
        if let Err(e) = sync_repo(state, conn, forge, &repo).await {
            tracing::warn!("{}: {e:#}", repo.html_url);
        }
    }
    Ok(())
}

/// One repository: its open issues, then closing the stored ones it no longer
/// lists. A single broken issue does not keep the rest, and the closures, from
/// syncing.
async fn sync_repo(state: &AppState, conn: &mut Conn, forge: Forge, repo: &ForgeRepo<'_>) -> Result<()> {
    if forge.is_archived(state, repo.html_url).await {
        tracing::info!("{}: repository archived; skipping", repo.html_url);
        return Ok(());
    }
    let Some((url, page_size)) = forge.issues_api(repo.html_url) else {
        tracing::warn!("{}: cannot derive the issues endpoint; skipping", repo.html_url);
        return Ok(());
    };
    let open = open_forge_issues(state, &url, page_size).await?;
    let mut open_numbers = HashSet::new();
    for json in &open {
        let Some(number) = json["number"].as_u64() else { continue };
        open_numbers.insert(number);
        let Some(issue_url) = safe_url(json["html_url"].as_str().unwrap_or_default()) else {
            tracing::warn!("{}: issue without a usable URL; skipping", repo.html_url);
            continue;
        };
        if let Err(e) = store_forge_issue(conn, forge, repo, &issue_url, number, json).await {
            tracing::warn!("{issue_url}: {e:#}");
        }
    }
    let stored: Vec<(u64, u64)> = conn
        .exec(
            "SELECT issue.id, git_issue.git_id FROM issue
             JOIN git_issue ON git_issue.issue_id=issue.id
             JOIN git_repo ON git_repo.id=git_issue.git_repo_id
             WHERE git_issue.site=? AND git_repo.repo_type=? AND git_repo.html_url=?
               AND issue.status='OPEN'",
            (forge.site(), forge.site(), repo.html_url),
        )
        .await?;
    // `open_forge_issues` fails on HTTP errors and on anything but a JSON
    // array, and the repository is skipped then. So an empty list really
    // means no open issues, and every stored issue is closed.
    for issue_id in issues_to_close(&stored, &open_numbers) {
        conn.exec_drop("UPDATE issue SET status='CLOSED' WHERE id=?", (issue_id,)).await?;
    }
    tracing::debug!("{}: {} open, {} stored", repo.html_url, open_numbers.len(), stored.len());
    Ok(())
}

/// Creates or updates one issue from its forge report.
async fn store_forge_issue(
    conn: &mut Conn,
    forge: Forge,
    repo: &ForgeRepo<'_>,
    issue_url: &str,
    number: u64,
    json: &Value,
) -> Result<()> {
    if let Some(issue_id) = known_forge_issue(conn, forge, number, repo.id, repo.html_url, issue_url).await? {
        return update_forge_issue(conn, issue_id, forge, json, issue_url).await;
    }
    let issue = new_forge_issue(forge, json, repo.tool_id, issue_url);
    let mut tx = conn.start_transaction(Default::default()).await?;
    let issue_id = create_issue(&mut tx, &issue).await?;
    ensure_mapping(&mut tx, issue_id, forge.site(), number, repo.id).await?;
    tx.commit().await?;
    Ok(())
}

/// Stored open issues whose number the forge no longer lists as open. An empty
/// `open` set closes all of them: the forge answered successfully.
fn issues_to_close<'a>(stored: &'a [(u64, u64)], open: &'a HashSet<u64>) -> impl Iterator<Item = u64> + 'a {
    stored.iter().filter(move |(_, number)| !open.contains(number)).map(|(issue_id, _)| *issue_id)
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
        url: db_url(url),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repository_urls_name_their_repository() {
        let github = Forge::GitHub;
        assert_eq!(
            github.owner_repo("https://github.com/magnusmanske/quickstatements"),
            Some(("magnusmanske", "quickstatements"))
        );
        assert_eq!(
            github.owner_repo("https://github.com/magnusmanske/quickstatements/issues"),
            Some(("magnusmanske", "quickstatements")),
            "a page below the repository root names the same repository"
        );
        assert_eq!(github.owner_repo("https://github.com/o/r.git"), Some(("o", "r")));
        assert_eq!(github.owner_repo("https://GitHub.com/o/r"), Some(("o", "r")));
        assert_eq!(github.owner_repo("https://codeberg.org/o/r"), None, "another forge");
        assert_eq!(github.owner_repo("https://github.com/o"), None);
        assert_eq!(github.owner_repo("not a URL"), None);
    }

    #[test]
    fn api_urls_follow_the_forge() {
        let github = "https://github.com/o/r/issues";
        assert_eq!(
            Forge::GitHub.issues_api(github),
            Some((format!("https://api.github.com/repos/o/r/issues?state=open&per_page={GITHUB_PAGE}"), GITHUB_PAGE))
        );
        assert_eq!(Forge::GitHub.repo_api(github), None, "GitHub's rate limit is too low for an extra call");

        let codeberg = "https://codeberg.org/o/r/src/branch/main";
        assert_eq!(Forge::Codeberg.repo_api(codeberg).as_deref(), Some("https://codeberg.org/api/v1/repos/o/r"));
        assert_eq!(
            Forge::Codeberg.issues_api(codeberg).map(|(url, _)| url).as_deref(),
            Some("https://codeberg.org/api/v1/repos/o/r/issues?type=issues&state=open&limit=50")
        );
    }

    #[test]
    fn pull_requests_and_times_are_recognised() {
        assert!(is_issue(&json!({"number": 1})), "GitHub omits `pull_request` for issues");
        assert!(is_issue(&json!({"pull_request": null})), "Forgejo sets it to null");
        assert!(!is_issue(&json!({"pull_request": {"url": "https://example.org"}})));

        assert_eq!(iso_time(&json!("2024-01-02T03:04:05Z")).as_deref(), Some("2024-01-02 03:04:05"));
        assert_eq!(
            iso_time(&json!("2024-01-02T05:04:05+02:00")).as_deref(),
            Some("2024-01-02 03:04:05"),
            "times are stored in UTC"
        );
        assert_eq!(iso_time(&json!(null)), None);
        assert_eq!(iso_time(&json!("yesterday")), None);
    }

    #[test]
    fn forge_issues_become_new_issues() {
        let json = json!({
            "number": 7,
            "title": "Broken export",
            "body": "See https://example.org/a and https://example.org/a, or https://example.org/b",
            "created_at": "2024-01-02T03:04:05Z",
            "updated_at": "2024-01-03T04:05:06Z",
            "user": {"login": "Magnus"}
        });
        let issue = new_forge_issue(Forge::Codeberg, &json, 42, "https://codeberg.org/o/r/issues/7");
        assert_eq!(issue.label, "Broken export");
        assert_eq!(issue.status, "OPEN");
        assert_eq!(issue.site, "CODEBERG");
        assert_eq!(issue.url, "https://codeberg.org/o/r/issues/7");
        assert_eq!(issue.tool, Some(42));
        assert_eq!(issue.users, vec!["Magnus"]);
        assert_eq!(issue.urls, vec!["https://example.org/a", "https://example.org/b"], "URLs are deduplicated");
        assert_eq!(issue.date_created.as_deref(), Some("2024-01-02 03:04:05"));
        assert_eq!(issue.date_last.as_deref(), Some("2024-01-03 04:05:06"));

        let minimal = new_forge_issue(Forge::GitHub, &json!({}), 1, "https://github.com/o/r/issues/1");
        assert_eq!(minimal.label, "");
        assert_eq!(minimal.description, "");
        assert!(minimal.users.is_empty());
        assert_eq!(minimal.date_created, None);
    }

    #[test]
    fn issues_gone_upstream_are_closed() {
        let stored = vec![(1, 10), (2, 11), (3, 12)];
        let open = HashSet::from([11]);
        assert_eq!(issues_to_close(&stored, &open).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(
            issues_to_close(&stored, &HashSet::new()).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "all upstream issues closed: every stored issue is closed"
        );
        assert_eq!(issues_to_close(&stored, &HashSet::from([10, 11, 12])).count(), 0);
    }

    #[test]
    fn excerpts_stop_at_the_limit() {
        assert_eq!(excerpt("short"), "short");
        assert_eq!(excerpt(&"ä".repeat(400)).chars().count(), 300, "counted in characters, not bytes");
    }
}

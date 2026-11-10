//! The daily buggregator update: issues from wiki talk pages, GitHub and
//! Codeberg; tool data from Toolhub; and some tidying up. Each step runs
//! even if an earlier one failed.

use super::MAINTAINER;
use super::issues::{self, text_id};
use super::safe_url;
use crate::mediawiki;
use crate::state::AppState;
use crate::wiki::{Wiki, encode_title};
use anyhow::Result;
use chrono::{DateTime, Months, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// How MySQL shows an unknown date.
const ZERO_TIME: &str = "0000-00-00 00:00:00";
const GITHUB_PAGE: usize = 100;
const CODEBERG_PAGE: usize = 50;
const TOOLHUB_SEARCH: &str = "https://toolhub.wikimedia.org/api/search/tools/?format=json&ordering=-score&page=1&page_size=1000&q=%22Magnus+Manske%22";
/// Tool names too common to recognise a tool by.
const IGNORE_TOOL_NAMES: &[&str] = &["data"];
/// Repositories the project has left behind: archived, or superseded by a
/// rewrite, so their old issues are not synced again.
const RETIRED_REPOS: &[&str] =
    &["https://github.com/magnusmanske/quickstatements", "https://codeberg.org/magnusmanske/sourcemd"];
/// Tries per tool for its web status: the Toolforge proxy now and then
/// answers 503 for a connection it dropped, though the tool is fine.
const STATUS_ATTEMPTS: usize = 3;
const STATUS_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
/// Addresses checked at once; the job should finish well within its window.
const WEB_STATUS_CONCURRENT: usize = 8;

pub async fn update(state: &AppState) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let tools = ToolMatcher::load(&mut conn).await?;
    let steps: [(&str, Result<()>); 6] = [
        ("wiki pages", from_wiki_pages(state, &mut conn, &tools).await),
        ("GitHub", from_forge(state, &mut conn, Forge::GitHub).await),
        ("Codeberg", from_forge(state, &mut conn, Forge::Codeberg).await),
        ("Toolhub", toolhub(state, &mut conn).await),
        ("Wikidata tool items", wikidata_tool_items(state, &mut conn).await),
        ("maintenance", tidy(&mut conn, &tools).await),
    ];
    for (step, result) in steps {
        if let Err(e) = result {
            tracing::error!("buggregator update, {step}: {e:#}");
        }
    }
    Ok(())
}

/// An issue to store.
#[derive(Debug, Default)]
struct NewIssue {
    label: String,
    status: String,
    date_created: Option<String>,
    date_last: Option<String>,
    site: &'static str,
    url: String,
    description: String,
    tool: u64,
    /// The first one created the issue.
    users: Vec<String>,
    urls: Vec<String>,
}

async fn create_issue(conn: &mut impl Queryable, issue: &NewIssue) -> Result<u64> {
    let description_id = text_id(conn, &issue.description).await?;
    let time = |t: &Option<String>| t.clone().unwrap_or_else(|| ZERO_TIME.to_string());
    conn.exec_drop(
        "INSERT INTO issue (label, status, date_created, date_last, site, url, description_text_id, tool, priority)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'NORMAL')
         ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)",
        (
            db_label(&issue.label),
            &issue.status,
            time(&issue.date_created),
            time(&issue.date_last),
            issue.site,
            mb3(&issue.url),
            description_id,
            issue.tool,
        ),
    )
    .await?;
    let issue_id = super::last_id(conn).await?;
    for url in &issue.urls {
        conn.exec_drop(
            "INSERT INTO url_issue (url, issue_id) VALUES (?, ?)
             ON DUPLICATE KEY UPDATE url=VALUES(url)",
            (url, issue_id),
        )
        .await?;
    }
    let mut seen = HashSet::new();
    for (i, name) in issue.users.iter().enumerate() {
        let user_id = user_id(conn, name, issue.site).await?;
        if seen.insert(user_id) {
            let role = if i == 0 { "CREATOR" } else { "UNKNOWN" };
            conn.exec_drop(
                "INSERT INTO user_issue (user_id, issue_id, role) VALUES (?, ?, ?)
                 ON DUPLICATE KEY UPDATE role=IF(role='CREATOR', 'CREATOR', VALUES(role))",
                (user_id, issue_id, role),
            )
            .await?;
        }
    }
    Ok(issue_id)
}

/// Text for a `utf8mb3` column, which has no room for characters beyond the
/// Basic Multilingual Plane (such as emoji).
fn mb3(text: &str) -> String {
    text.chars().filter(|c| u32::from(*c) <= 0xFFFF).collect()
}

/// Text for the `varchar(255)` label column; truncated the same way when
/// looking an issue up, so a long heading does not create a new row each run.
fn db_label(text: &str) -> String {
    mb3(text).chars().take(255).collect()
}

async fn user_id(conn: &mut impl Queryable, name: &str, site: &str) -> Result<u64> {
    let name = mb3(&if site == "WIKI" { name.replace('_', " ") } else { name.to_string() });
    let name = name.trim();
    conn.exec_drop(
        "INSERT INTO user (name, site) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)",
        (name, site),
    )
    .await?;
    super::last_id(conn).await
}

// ── Wiki talk pages ────────────────────────────────────────────────────────

/// Every section of the watched talk pages is an issue. Sections are
/// refreshed on every run, so replies, dates and descriptions do not go stale.
async fn from_wiki_pages(state: &AppState, conn: &mut Conn, tools: &ToolMatcher) -> Result<()> {
    let pages: Vec<(u64, String, String, u64)> = conn.query("SELECT id, wiki, page, tool_hint FROM wiki_page").await?;
    for (page_id, dbname, page, tool_hint) in pages {
        let wiki = Wiki::from_dbname(&dbname)?;
        let text = mediawiki::page_wikitext(&state.http, &wiki, &page).await?.unwrap_or_default();
        for (label, wikitext) in sections(&text) {
            let known: Option<(u64, String, String)> = conn
                .exec_first(
                    "SELECT issue.id, issue.status, issue.date_last FROM issue JOIN wiki_issue ON issue_id=issue.id
                     WHERE site='WIKI' AND CONVERT(label USING utf8mb4)=? AND wiki_page_id=?",
                    (db_label(&label), page_id),
                )
                .await?;
            let times = signature_times(&wikitext);
            let issue = NewIssue {
                url: format!("https://{}/wiki/{}#{}", wiki.host(), encode_title(&page), encode_title(&label)),
                tool: tools.find(&label).or_else(|| tools.find(&wikitext)).unwrap_or(tool_hint),
                status: "OPEN".to_string(),
                date_created: times.iter().min().map(format_time),
                date_last: times.iter().max().map(format_time),
                site: "WIKI",
                description: trim_markup(&wikitext),
                users: wiki_users(&wikitext),
                urls: urls(&wikitext),
                label,
            };
            match known {
                Some((issue_id, status, old_last)) => {
                    update_wiki_issue(conn, issue_id, &issue, &wikitext).await?;
                    reopen_on_reply(conn, issue_id, &status, &old_last, &issue, &wikitext).await?;
                }
                None => {
                    let mut tx = conn.start_transaction(Default::default()).await?;
                    let issue_id = create_issue(&mut tx, &issue).await?;
                    tx.exec_drop(
                        "INSERT INTO wiki_issue (wiki_page_id, issue_id, wikitext) VALUES (?, ?, ?)
                         ON DUPLICATE KEY UPDATE wikitext=VALUES(wikitext)",
                        (page_id, issue_id, &wikitext),
                    )
                    .await?;
                    tx.commit().await?;
                }
            }
        }
    }
    Ok(())
}

/// Refreshes a known wiki issue without touching its status (closing is
/// `tidy`'s decision). Missing users and URLs are added; existing ones stay.
async fn update_wiki_issue(conn: &mut Conn, issue_id: u64, issue: &NewIssue, wikitext: &str) -> Result<()> {
    let mut tx = conn.start_transaction(Default::default()).await?;
    let description_id = text_id(&mut tx, &issue.description).await?;
    let time = |t: &Option<String>| t.clone().unwrap_or_else(|| ZERO_TIME.to_string());
    tx.exec_drop(
        "UPDATE issue SET label=?, url=?, description_text_id=?,
           date_created=IF(date_created=?, ?, date_created),
           date_last=GREATEST(date_last, ?),
           tool=IF(tool=0, ?, tool)
         WHERE id=?",
        (
            db_label(&issue.label),
            mb3(&issue.url),
            description_id,
            ZERO_TIME,
            time(&issue.date_created),
            time(&issue.date_last),
            issue.tool,
            issue_id,
        ),
    )
    .await?;
    tx.exec_drop("UPDATE wiki_issue SET wikitext=? WHERE issue_id=?", (wikitext, issue_id)).await?;
    for url in &issue.urls {
        tx.exec_drop(
            "INSERT INTO url_issue (url, issue_id) VALUES (?, ?)
             ON DUPLICATE KEY UPDATE url=VALUES(url)",
            (url, issue_id),
        )
        .await?;
    }
    let mut seen = HashSet::new();
    for (i, name) in issue.users.iter().enumerate() {
        let user_id = user_id(&mut tx, name, issue.site).await?;
        if seen.insert(user_id) {
            let role = if i == 0 { "CREATOR" } else { "UNKNOWN" };
            tx.exec_drop(
                "INSERT INTO user_issue (user_id, issue_id, role) VALUES (?, ?, ?)
                 ON DUPLICATE KEY UPDATE role=IF(role='CREATOR', 'CREATOR', VALUES(role))",
                (user_id, issue_id, role),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// A thread that got a reply after the maintainer's last word was closed by
/// `tidy`; a new reply reopens it. Manual closes by anyone else stay closed.
async fn reopen_on_reply(
    conn: &mut Conn,
    issue_id: u64,
    status: &str,
    old_last: &str,
    issue: &NewIssue,
    wikitext: &str,
) -> Result<()> {
    if status != "CLOSED" || issue.date_last.as_deref().is_none_or(|new| new <= old_last) {
        return Ok(());
    }
    let last_close: Option<(String, String)> = conn
        .exec_first(
            "SELECT u.name, t.text FROM log l
             JOIN user u ON u.id=l.user_id JOIN text t ON t.id=l.text_id
             WHERE l.issue_id=? AND l.event='STATUS' ORDER BY l.id DESC LIMIT 1",
            (issue_id,),
        )
        .await?;
    let closed_by_maintenance = last_close.is_some_and(|(user, text)| user == MAINTAINER && text == "CLOSED");
    let maintainer = regex(r"\bMagnus[ _]Manske\b");
    let last_line_is_maintainer = wikitext.trim().lines().last().is_some_and(|line| maintainer.is_match(line));
    if closed_by_maintenance && !last_line_is_maintainer {
        issues::set_status(conn, issue_id, "OPEN", MAINTAINER).await?;
    }
    Ok(())
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*={2,}\s*(.+?)\s*={2,}\s*$"));
static SIGNATURE_TIME: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(\d{2}):(\d{2}), (\d{1,2})\.* (\S+) (\d{4}) \([A-Z]+\)"));
static URL: LazyLock<Regex> = LazyLock::new(|| regex(r"\bhttps?://[^\s()<>]+(?:\([\w\d]+\)|([^[:punct:]\s]|/))"));
static USER_LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"\[\[(?:User|Benutzer):(.+?)(?:\||\]\])"));

/// `(heading, text)` of each section with a heading; trimmed.
fn sections(text: &str) -> Vec<(String, String)> {
    let mut sections = vec![];
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(heading) = HEADING.captures(line).and_then(|c| c.get(1)) {
            sections.extend(current.take());
            current = Some((heading.as_str().trim().to_string(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    sections.extend(current);
    sections.into_iter().map(|(label, body)| (label, body.trim().to_string())).filter(|(l, _)| !l.is_empty()).collect()
}

/// When the comments were signed: `12:34, 5 May 2024 (UTC)`, also German.
fn signature_times(wikitext: &str) -> Vec<NaiveDateTime> {
    SIGNATURE_TIME
        .captures_iter(wikitext)
        .filter_map(|c| {
            let number = |i: usize| c.get(i)?.as_str().parse::<u32>().ok();
            let month = month_number(c.get(4)?.as_str())?;
            let year = i32::try_from(number(5)?).ok()?;
            let date = NaiveDate::from_ymd_opt(year, month, number(3)?)?;
            Some(date.and_time(NaiveTime::from_hms_opt(number(1)?, number(2)?, 0)?))
        })
        .collect()
}

fn month_number(name: &str) -> Option<u32> {
    let short: String = name.to_lowercase().chars().take(3).collect();
    let months = [
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("mär", 3),
        ("apr", 4),
        ("may", 5),
        ("mai", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("okt", 10),
        ("nov", 11),
        ("dec", 12),
        ("dez", 12),
    ];
    months.iter().find(|(m, _)| *m == short).map(|(_, n)| *n)
}

fn format_time(time: &NaiveDateTime) -> String {
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

static PIPED_OR_ANCHOR_LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"\[\[(.+?)[#|].*?\]\]"));
static LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"\[\[(.+?)\]\]"));
static TEMPLATE: LazyLock<Regex> = LazyLock::new(|| regex(r"\{\{(.+?)\}\}"));
static TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"<.*?>"));

/// Wikitext with links as their targets, and no templates or tags.
fn trim_markup(wikitext: &str) -> String {
    let text = PIPED_OR_ANCHOR_LINK.replace_all(wikitext, "$1");
    let text = LINK.replace_all(&text, "$1");
    let text = TEMPLATE.replace_all(&text, "");
    TAG.replace_all(&text, "").into_owned()
}

fn urls(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    URL.find_iter(text).map(|m| m.as_str().to_string()).filter(|url| seen.insert(url.clone())).collect()
}

/// Users linked in the text, in order; the first one opened the section.
fn wiki_users(wikitext: &str) -> Vec<String> {
    USER_LINK
        .captures_iter(wikitext)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|name| !name.is_empty())
        .collect()
}

// ── GitHub and Codeberg ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Forge {
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
    fn repo_api(self, html_url: &str) -> Option<String> {
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
    fn api(self, html_url: &str, api_issues_url: &str) -> Option<(String, usize)> {
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
async fn from_forge(state: &AppState, conn: &mut Conn, forge: Forge) -> Result<()> {
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
        date_created: Some(iso_time(&json["created_at"])),
        date_last: Some(iso_time(&json["updated_at"])),
        site: forge.site(),
        url: url.to_string(),
        urls: urls(&description),
        description,
        tool: tool_id,
        users: json["user"]["login"].as_str().map(str::to_string).into_iter().collect(),
    }
}

/// Updates an issue the forge already knows, including edits to its body.
async fn update_forge_issue(conn: &mut Conn, issue_id: u64, forge: Forge, json: &Value, url: &str) -> Result<()> {
    let description = json["body"].as_str().unwrap_or_default();
    let title = json["title"].as_str().unwrap_or_default();
    let mut tx = conn.start_transaction(Default::default()).await?;
    let description_id = text_id(&mut tx, description).await?;
    // `IGNORE`: a repository rename can collide with another issue's old URL;
    // the mapping still identifies this one.
    tx.exec_drop(
        "UPDATE IGNORE issue SET status='OPEN', label=?, url=?, description_text_id=?,
           date_last=GREATEST(date_last, ?) WHERE id=?",
        (db_label(title), mb3(url), description_id, iso_time(&json["updated_at"]), issue_id),
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
fn is_issue(json: &Value) -> bool {
    json["pull_request"].is_null()
}

/// `2024-01-02T03:04:05Z` as the database writes it, in UTC.
fn iso_time(value: &Value) -> String {
    value
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map_or_else(|| ZERO_TIME.to_string(), |t| format_time(&t.with_timezone(&Utc).naive_utc()))
}

// ── Toolhub, Wikidata ──────────────────────────────────────────────────────

/// Links tools to their Toolhub entries, and takes descriptions (where
/// missing) and keywords from there.
pub async fn toolhub(state: &AppState, conn: &mut Conn) -> Result<()> {
    let known: HashSet<String> =
        conn.query("SELECT DISTINCT toolhub FROM tool WHERE toolhub!=''").await?.into_iter().collect();
    let json: Value = state.http.get(TOOLHUB_SEARCH).send().await?.error_for_status()?.json().await?;
    for entry in json["results"].as_array().into_iter().flatten() {
        let text = |key: &str| entry[key].as_str().unwrap_or_default().trim().to_string();
        let name = text("name");
        if name.is_empty() {
            continue;
        }
        if !known.contains(&name) {
            let title = text("title");
            let url = text("url").replace("http:", "https:");
            let candidates: Vec<u64> = conn
                .exec(
                    "SELECT id FROM tool WHERE toolhub='' AND (url=? OR name IN (?, ?, ?, ?))",
                    (
                        url.as_str(),
                        title.as_str(),
                        title.replace(' ', "_"),
                        title.replace(' ', "-"),
                        title.replace(' ', ""),
                    ),
                )
                .await?;
            match candidates.as_slice() {
                [id] => conn.exec_drop("UPDATE tool SET toolhub=? WHERE id=?", (&name, id)).await?,
                [] => {
                    tracing::info!("Toolhub tool not found here: {title} / {name}: {url}");
                    continue;
                }
                _ => {
                    tracing::info!("Toolhub tool matches more than one tool: {title} / {name}: {url}");
                    continue;
                }
            }
        }
        let description = text("description");
        if !description.is_empty() {
            conn.exec_drop("UPDATE tool SET description=? WHERE description='' AND toolhub=?", (&description, &name))
                .await?;
        }
        for keyword in entry["keywords"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            conn.exec_drop(
                "INSERT IGNORE INTO keywords (tool_id, keyword) SELECT id, ? FROM tool WHERE toolhub=?",
                (keyword.trim(), &name),
            )
            .await?;
        }
    }
    Ok(())
}

/// Reports Wikidata items of Magnus' tools that no tool here points to.
async fn wikidata_tool_items(state: &AppState, conn: &mut Conn) -> Result<()> {
    let known: HashSet<String> =
        conn.query("SELECT value FROM tool_kv WHERE `key`='item'").await?.into_iter().collect();
    let query = "SELECT ?q { ?q wdt:P31 wd:Q20726407 ; wdt:P178 wd:Q13520818 }";
    let json: Value = state
        .http
        .get("https://query.wikidata.org/sparql")
        .query(&[("query", query), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for binding in json["results"]["bindings"].as_array().into_iter().flatten() {
        let q = binding["q"]["value"].as_str().and_then(|u| u.rsplit('/').next()).unwrap_or_default();
        if !q.is_empty() && !known.contains(q) {
            tracing::info!("No key/value pair for tool https://www.wikidata.org/wiki/{q}");
        }
    }
    Ok(())
}

// ── Maintenance ────────────────────────────────────────────────────────────

/// Recognises tools by name in a text: when exactly one (unambiguous) tool name occurs.
struct ToolMatcher(Vec<(Regex, u64)>);

impl ToolMatcher {
    async fn load(conn: &mut Conn) -> Result<Self> {
        let tools: Vec<(u64, String)> = conn.query("SELECT id, name FROM tool").await?;
        let mut by_name: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for (id, name) in tools {
            by_name.entry(name.replace('_', " ").to_lowercase().trim().to_string()).or_default().push(id);
        }
        let matchers = by_name
            .into_iter()
            .filter(|(name, ids)| ids.len() == 1 && !name.is_empty() && !IGNORE_TOOL_NAMES.contains(&name.as_str()))
            .filter_map(|(name, ids)| {
                Some((Regex::new(&format!(r"(?i)\b{}\b", regex::escape(&name))).ok()?, *ids.first()?))
            })
            .collect();
        Ok(Self(matchers))
    }

    fn find(&self, text: &str) -> Option<u64> {
        let found: HashSet<u64> = self.0.iter().filter(|(re, _)| re.is_match(text)).map(|(_, id)| *id).collect();
        (found.len() == 1).then(|| found.into_iter().next()).flatten()
    }
}

/// Dates, tools and closing of issues, where the update could not settle them.
pub async fn maintenance(conn: &mut Conn) -> Result<()> {
    let tools = ToolMatcher::load(conn).await?;
    tidy(conn, &tools).await
}

async fn tidy(conn: &mut Conn, tools: &ToolMatcher) -> Result<()> {
    // Wiki issues without dates: from the signatures.
    let undated: Vec<(u64, String)> = conn
        .query(format!(
            "SELECT issue.id, wikitext FROM issue JOIN wiki_issue ON issue_id=issue.id
             WHERE site='WIKI' AND status='OPEN' AND date_created='{ZERO_TIME}'"
        ))
        .await?;
    for (issue_id, wikitext) in undated {
        let times = signature_times(&wikitext);
        if let (Some(first), Some(last)) = (times.iter().min(), times.iter().max()) {
            conn.exec_drop(
                "UPDATE issue SET date_created=?, date_last=? WHERE id=?",
                (format_time(first), format_time(last), issue_id),
            )
            .await?;
        }
    }

    // Issues without a tool: by name in the label or text, else the wiki page's hint.
    let untooled: Vec<(u64, String, String, u64)> = conn
        .query(
            "SELECT issue.id, label, COALESCE(wikitext, (SELECT text FROM text WHERE text.id=description_text_id), ''),
             COALESCE(tool_hint, 0)
             FROM issue LEFT JOIN wiki_issue ON issue_id=issue.id LEFT JOIN wiki_page ON wiki_page.id=wiki_page_id
             WHERE tool=0 AND status='OPEN'",
        )
        .await?;
    for (issue_id, label, text, hint) in untooled {
        let tool = tools.find(&label).or_else(|| tools.find(&text)).unwrap_or(hint);
        if tool != 0 {
            conn.exec_drop("UPDATE issue SET tool=? WHERE id=? AND tool=0", (tool, issue_id)).await?;
        }
    }

    // Wiki issues where the maintainer had the last word a month ago are done.
    let cutoff = Utc::now().naive_utc().checked_sub_months(Months::new(1)).map(|t| format_time(&t)).unwrap_or_default();
    let replied: Vec<(u64, String)> = conn
        .exec(
            "SELECT issue.id, wikitext FROM issue JOIN wiki_issue ON wiki_issue.issue_id=issue.id
             JOIN user_issue ON user_issue.issue_id=issue.id JOIN user ON user.id=user_issue.user_id
             WHERE issue.site='WIKI' AND issue.status='OPEN' AND issue.date_last!=? AND issue.date_last<?
             AND user.name=? AND user.site='WIKI'",
            (ZERO_TIME, cutoff, MAINTAINER),
        )
        .await?;
    let maintainer = regex(r"\bMagnus[ _]Manske\b");
    for (issue_id, wikitext) in replied {
        if wikitext.trim().lines().last().is_some_and(|line| maintainer.is_match(line)) {
            issues::set_status(conn, issue_id, "CLOSED", MAINTAINER).await?;
        }
    }
    Ok(())
}

/// A `tool` row for the address check: id, active, web interface, subdomain,
/// URL (its own or its service's; `None` when neither is set), service id,
/// status, and since.
type ToolRow = (u64, bool, bool, String, Option<String>, Option<u64>, Option<i64>, Option<i64>);
/// A tool answering at an address, with its status and since before this run.
type ToolStatus = (u64, Option<i64>, Option<i64>);

/// Sets each tool's `http_status` to what its web address answers, after
/// redirects (`0`: no answer, after a few tries), and `http_status_since` to
/// when that answer started, for the alerts. Tools that have no address, or
/// no web interface, get neither.
pub async fn web_status(conn: &mut Conn) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(crate::state::USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let tools: Vec<ToolRow> = conn
        .query(
            "SELECT t.id, t.is_active, t.has_webservice, t.subdomain,
                    COALESCE(NULLIF(s.url,''), NULLIF(t.url,'')), s.id, t.http_status, t.http_status_since
             FROM tool t LEFT JOIN dash_service s ON s.tool_id=t.id AND s.is_active=1",
        )
        .await?;
    // One request per address, however many tools answer there (Reasonator has several).
    let mut by_address: BTreeMap<String, Vec<ToolStatus>> = BTreeMap::new();
    let mut unchecked = vec![];
    for (id, is_active, has_webservice, subdomain, url, service, status, since) in tools {
        let url = url.unwrap_or_default();
        match crate::buggregator::address(&subdomain, &url) {
            Some(address) if is_active && (has_webservice || service.is_some()) => {
                by_address.entry(address).or_default().push((id, status, since));
            }
            _ => unchecked.push(id),
        }
    }
    for id in unchecked {
        conn.exec_drop("UPDATE tool SET http_status=NULL, http_status_since=NULL WHERE id=?", (id,)).await?;
    }
    let now = Utc::now().timestamp();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(WEB_STATUS_CONCURRENT));
    let mut tasks = tokio::task::JoinSet::new();
    for (address, tools) in by_address {
        let client = client.clone();
        let semaphore = semaphore.clone();
        tasks.spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else { return None };
            let status = web_answer(&client, &address).await;
            Some((address, tools, status))
        });
    }
    while let Some(result) = tasks.join_next().await {
        let Some((address, tools, status)) = result? else { continue };
        for (id, previous, previous_since) in tools {
            // An unchanged answer keeps its start; one that becomes fine has none.
            let since = if status == 200 {
                None
            } else if previous == Some(i64::from(status)) {
                previous_since
            } else {
                Some(now)
            };
            conn.exec_drop(
                "UPDATE tool SET http_status=?, http_status_since=? WHERE id=?",
                (i64::from(status), since, id),
            )
            .await?;
        }
        tracing::debug!("{address}: HTTP {status}");
    }
    Ok(())
}

/// What `url` answers, after redirects; `0` if it does not answer at all.
async fn web_answer(client: &reqwest::Client, url: &str) -> u16 {
    let mut status = 0;
    for attempt in 0..STATUS_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(STATUS_RETRY_AFTER).await;
        }
        status = answer_once(client, url).await;
        if status == 200 {
            break;
        }
    }
    status
}

/// HEAD first; some servers reject it, so ask for the first byte instead.
async fn answer_once(client: &reqwest::Client, url: &str) -> u16 {
    match client.head(url).send().await {
        Ok(response) if matches!(response.status().as_u16(), 405 | 501) => {
            let status = client
                .get(url)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send()
                .await
                .map_or(0, |r| r.status().as_u16());
            // A partial answer for the one requested byte means the address is fine.
            if status == 206 { 200 } else { status }
        }
        Ok(response) => response.status().as_u16(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talk_page_sections() {
        let text = "Intro\n== First ==\nHi [[User:Some_One|me]] 12:34, 5 May 2024 (UTC)\n\n=== Sub: x ===\nmore\n==Second==\nok";
        assert_eq!(
            sections(text),
            vec![
                ("First".to_string(), "Hi [[User:Some_One|me]] 12:34, 5 May 2024 (UTC)".to_string()),
                ("Sub: x".to_string(), "more".to_string()),
                ("Second".to_string(), "ok".to_string()),
            ]
        );
    }

    #[test]
    fn signatures() {
        let text = "a 12:34, 5 May 2024 (UTC) b 09:05, 17. Mär. 2025 (CET) c 10:00, 1 Foo 2024 (UTC)";
        let times: Vec<String> = signature_times(text).iter().map(format_time).collect();
        assert_eq!(times, vec!["2024-05-05 12:34:00", "2025-03-17 09:05:00"]);
    }

    #[test]
    fn markup_users_and_urls() {
        assert_eq!(trim_markup("See [[Foo#bar|this]], [[Baz]] {{tl|x}} <b>now</b>"), "See Foo, Baz  now");
        assert_eq!(wiki_users("by [[User:A B|A]] and [[Benutzer:C]] and [[User talk:D]]"), vec!["A B", "C"]);
        assert_eq!(
            urls("see https://x.org/a?b=1. and (http://y.org/z) and https://x.org/a?b=1"),
            vec!["https://x.org/a?b=1", "http://y.org/z"]
        );
    }

    #[test]
    fn forge_urls() {
        let (url, _) = Forge::Codeberg.api("https://codeberg.org/magnusmanske/petscan_rs", "").unwrap();
        assert_eq!(
            url,
            "https://codeberg.org/api/v1/repos/magnusmanske/petscan_rs/issues?type=issues&state=open&limit=50"
        );
        assert!(Forge::Codeberg.api("https://github.com/x/y", "").is_none());
        assert_eq!(
            Forge::Codeberg.repo_api("https://codeberg.org/magnusmanske/petscan_rs"),
            Some("https://codeberg.org/api/v1/repos/magnusmanske/petscan_rs".to_string())
        );
        assert!(Forge::GitHub.repo_api("https://github.com/x/y").is_none());
        assert_eq!(iso_time(&Value::from("2024-01-02T03:04:05Z")), "2024-01-02 03:04:05");
        assert_eq!(iso_time(&Value::Null), ZERO_TIME);
    }

    #[test]
    fn forge_issues_exclude_pull_requests() {
        let github_issue = serde_json::json!({"number": 1});
        let forgejo_issue = serde_json::json!({"number": 2, "pull_request": null});
        let pull_request = serde_json::json!({"number": 3, "pull_request": {"url": "x"}});
        assert!(is_issue(&github_issue));
        assert!(is_issue(&forgejo_issue));
        assert!(!is_issue(&pull_request));
    }

    #[test]
    fn basic_plane_only() {
        assert_eq!(mb3("Bug 🐛 in Grüße"), "Bug  in Grüße");
        assert_eq!(db_label(&"x".repeat(300)).chars().count(), 255);
    }

    #[test]
    fn tool_names() {
        let matcher = ToolMatcher(vec![(regex(r"(?i)\bpetscan\b"), 1), (regex(r"(?i)\breasonator\b"), 2)]);
        assert_eq!(matcher.find("PetScan is slow"), Some(1));
        assert_eq!(matcher.find("PetScan and Reasonator"), None);
        assert_eq!(matcher.find("petscanning"), None);
    }
}

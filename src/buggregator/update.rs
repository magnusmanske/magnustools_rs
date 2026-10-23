//! The daily buggregator update: issues from wiki talk pages, GitHub and
//! Codeberg; tool data from Toolhub; and some tidying up. Each step runs
//! even if an earlier one failed.

use super::issues::text_id;
use crate::mediawiki;
use crate::state::AppState;
use crate::wiki::{Wiki, encode_title};
use anyhow::{Context, Result};
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
/// The sites the `user` table has users of.
const USER_SITES: &[&str] = &["WIKI", "GITHUB"];
/// Whose replies close a wiki issue after a month without further discussion.
const MAINTAINER: &str = "Magnus Manske";

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

async fn create_issue(conn: &mut Conn, issue: &NewIssue) -> Result<u64> {
    let description_id = text_id(conn, &issue.description).await?;
    let time = |t: &Option<String>| t.clone().unwrap_or_else(|| ZERO_TIME.to_string());
    conn.exec_drop(
        "INSERT INTO issue (label, status, date_created, date_last, site, url, description_text_id, tool, priority)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'NORMAL')",
        (
            mb3(&issue.label),
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
    let issue_id = conn.last_insert_id().context("no issue id")?;
    for url in &issue.urls {
        conn.exec_drop("INSERT IGNORE INTO url_issue (url, issue_id) VALUES (?, ?)", (url, issue_id)).await?;
    }
    // `user.site` knows only these sites.
    if !USER_SITES.contains(&issue.site) {
        return Ok(issue_id);
    }
    let mut seen = HashSet::new();
    for (i, name) in issue.users.iter().enumerate() {
        let user_id = user_id(conn, name, issue.site).await?;
        if seen.insert(user_id) {
            let role = if i == 0 { "CREATOR" } else { "UNKNOWN" };
            conn.exec_drop(
                "INSERT IGNORE INTO user_issue (user_id, issue_id, role) VALUES (?, ?, ?)",
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

async fn user_id(conn: &mut Conn, name: &str, site: &str) -> Result<u64> {
    let name = mb3(&if site == "WIKI" { name.replace('_', " ") } else { name.to_string() });
    let name = name.trim();
    if let Some(id) = conn.exec_first("SELECT id FROM user WHERE name=? AND site=?", (name, site)).await? {
        return Ok(id);
    }
    conn.exec_drop("INSERT INTO user (name, site) VALUES (?, ?)", (name, site)).await?;
    conn.last_insert_id().context("no user id")
}

// ── Wiki talk pages ────────────────────────────────────────────────────────

/// Every section of the watched talk pages is an issue.
async fn from_wiki_pages(state: &AppState, conn: &mut Conn, tools: &ToolMatcher) -> Result<()> {
    let pages: Vec<(u64, String, String, u64)> = conn.query("SELECT id, wiki, page, tool_hint FROM wiki_page").await?;
    for (page_id, dbname, page, tool_hint) in pages {
        let wiki = Wiki::from_dbname(&dbname)?;
        let text = mediawiki::page_wikitext(&state.http, &wiki, &page).await?.unwrap_or_default();
        for (label, wikitext) in sections(&text) {
            let known: Option<u64> = conn
                .exec_first(
                    "SELECT issue.id FROM issue JOIN wiki_issue ON issue_id=issue.id
                     WHERE site='WIKI' AND CONVERT(label USING utf8mb4)=? AND wiki_page_id=?",
                    (&label, page_id),
                )
                .await?;
            if known.is_some() {
                continue;
            }
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
            let issue_id = create_issue(conn, &issue).await?;
            conn.exec_drop(
                "INSERT IGNORE INTO wiki_issue (wiki_page_id, issue_id, wikitext) VALUES (?, ?, ?)",
                (page_id, issue_id, &wikitext),
            )
            .await?;
        }
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
    let mut urls: Vec<String> = URL.find_iter(text).map(|m| m.as_str().to_string()).collect();
    urls.dedup();
    urls
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
        .exec("SELECT id, html_url, api_issues_url, tool_id FROM git_repo WHERE repo_type=?", (forge.site(),))
        .await?;
    for (repo_id, html_url, api_issues_url, tool_id) in repos {
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
            let known: Option<u64> = conn
                .exec_first(
                    "SELECT issue.id FROM issue JOIN git_issue ON issue_id=issue.id
                     WHERE site=? AND git_id=? AND git_repo_id=?",
                    (forge.site(), number, repo_id),
                )
                .await?;
            if let Some(issue_id) = known {
                conn.exec_drop(
                    "UPDATE issue SET status='OPEN', label=?, date_last=GREATEST(date_last, ?) WHERE id=?",
                    (mb3(json["title"].as_str().unwrap_or_default()), iso_time(&json["updated_at"]), issue_id),
                )
                .await?;
                continue;
            }
            let description = json["body"].as_str().unwrap_or_default().to_string();
            let issue = NewIssue {
                label: json["title"].as_str().unwrap_or_default().to_string(),
                status: "OPEN".to_string(),
                date_created: Some(iso_time(&json["created_at"])),
                date_last: Some(iso_time(&json["updated_at"])),
                site: forge.site(),
                url: json["html_url"].as_str().unwrap_or_default().to_string(),
                urls: urls(&description),
                description,
                tool: tool_id,
                users: json["user"]["login"].as_str().map(str::to_string).into_iter().collect(),
            };
            let issue_id = create_issue(conn, &issue).await?;
            conn.exec_drop(
                "INSERT IGNORE INTO git_issue (issue_id, git_id, git_repo_id) VALUES (?, ?, ?)",
                (issue_id, number, repo_id),
            )
            .await?;
        }
        let stored: Vec<(u64, u64)> = conn
            .exec(
                "SELECT issue.id, git_id FROM issue JOIN git_issue ON issue_id=issue.id
                 WHERE site=? AND git_repo_id=? AND status='OPEN'",
                (forge.site(), repo_id),
            )
            .await?;
        for (issue_id, _) in stored.iter().filter(|(_, number)| !open_numbers.contains(number)) {
            conn.exec_drop("UPDATE issue SET status='CLOSED' WHERE id=?", (issue_id,)).await?;
        }
    }
    Ok(())
}

/// All open issues (not pull requests), page by page.
async fn open_forge_issues(state: &AppState, url: &str, page_size: usize) -> Result<Vec<Value>> {
    let mut issues = vec![];
    for page in 1.. {
        let json: Value = state
            .http
            .get(format!("{url}&page={page}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)?
            .json()
            .await?;
        let batch = json.as_array().cloned().unwrap_or_default();
        let count = batch.len();
        issues.extend(batch.into_iter().filter(|i| i.get("pull_request").is_none()));
        if count < page_size {
            break;
        }
    }
    Ok(issues)
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
            conn.exec_drop("UPDATE issue SET status='CLOSED' WHERE id=?", (issue_id,)).await?;
        }
    }
    Ok(())
}

/// Sets each tool's `http_status` to what its web address answers (without following redirects).
pub async fn web_status(conn: &mut Conn) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(crate::state::USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    conn.query_drop("UPDATE tool SET http_status=NULL WHERE subdomain=''").await?;
    let subdomains: Vec<String> = conn.query("SELECT DISTINCT subdomain FROM tool WHERE subdomain!=''").await?;
    for subdomain in subdomains {
        let status =
            client.head(format!("https://{subdomain}.toolforge.org")).send().await.ok().map(|r| r.status().as_u16());
        conn.exec_drop("UPDATE tool SET http_status=? WHERE subdomain=?", (status, &subdomain)).await?;
    }
    Ok(())
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
            urls("see https://x.org/a?b=1. and (http://y.org/z)"),
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
        assert_eq!(iso_time(&Value::from("2024-01-02T03:04:05Z")), "2024-01-02 03:04:05");
        assert_eq!(iso_time(&Value::Null), ZERO_TIME);
    }

    #[test]
    fn basic_plane_only() {
        assert_eq!(mb3("Bug 🐛 in Grüße"), "Bug  in Grüße");
    }

    #[test]
    fn tool_names() {
        let matcher = ToolMatcher(vec![(regex(r"(?i)\bpetscan\b"), 1), (regex(r"(?i)\breasonator\b"), 2)]);
        assert_eq!(matcher.find("PetScan is slow"), Some(1));
        assert_eq!(matcher.find("PetScan and Reasonator"), None);
        assert_eq!(matcher.find("petscanning"), None);
    }
}

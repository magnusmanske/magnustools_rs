//! The daily buggregator update: issues from wiki talk pages, GitHub and
//! Codeberg; tool data from Toolhub; and some tidying up. Each step runs
//! even if an earlier one failed.

mod forge;
mod tidy;
mod toolhub;
mod webstatus;
mod wiki;

pub use tidy::maintenance;
pub use toolhub::toolhub;
pub use webstatus::web_status;

use super::issues::text_id;
use crate::state::AppState;
use anyhow::Result;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Tool names too common to recognise a tool by.
const IGNORE_TOOL_NAMES: &[&str] = &["data"];

pub async fn update(state: &AppState) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let tools = ToolMatcher::load(&mut conn).await?;
    let steps: [(&str, Result<()>); 6] = [
        ("wiki pages", wiki::from_wiki_pages(state, &mut conn, &tools).await),
        ("GitHub", forge::from_forge(state, &mut conn, forge::Forge::GitHub).await),
        ("Codeberg", forge::from_forge(state, &mut conn, forge::Forge::Codeberg).await),
        ("Toolhub", toolhub::toolhub(state, &mut conn).await),
        ("Wikidata tool items", toolhub::wikidata_tool_items(state, &mut conn).await),
        ("maintenance", tidy::tidy(&mut conn, &tools).await),
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
    tool: Option<u64>,
    /// The first one created the issue.
    users: Vec<String>,
    urls: Vec<String>,
}

async fn create_issue(conn: &mut impl Queryable, issue: &NewIssue) -> Result<u64> {
    let description_id = text_id(conn, &issue.description).await?;
    conn.exec_drop(
        "INSERT INTO issue (label, status, date_created, date_last, site, url, description_text_id, tool, priority)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'NORMAL')
         ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)",
        (
            db_label(&issue.label),
            &issue.status,
            issue.date_created.as_deref(),
            issue.date_last.as_deref(),
            issue.site,
            &issue.url,
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

/// Text for the `varchar(255)` label column; truncated the same way when
/// looking an issue up, so a long heading does not create a new row each run.
fn db_label(text: &str) -> String {
    text.chars().take(255).collect()
}

async fn user_id(conn: &mut impl Queryable, name: &str, site: &str) -> Result<u64> {
    let name = if site == "WIKI" { name.replace('_', " ") } else { name.to_string() };
    let name = name.trim();
    conn.exec_drop(
        "INSERT INTO user (name, site) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)",
        (name, site),
    )
    .await?;
    super::last_id(conn).await
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

#[cfg(test)]
mod tests {
    use super::forge::{Forge, is_issue, iso_time};
    use super::*;
    use serde_json::Value;

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
        assert_eq!(iso_time(&Value::from("2024-01-02T03:04:05Z")), Some("2024-01-02 03:04:05".to_string()));
        assert_eq!(iso_time(&Value::Null), None);
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
    fn labels_and_unicode() {
        assert_eq!(db_label("Bug 🐛 in Grüße"), "Bug 🐛 in Grüße");
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

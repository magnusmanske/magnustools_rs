//! Issues from the watched wiki talk pages.

use super::{
    NewIssue, ToolMatcher, create_issue, db_label, format_time, regex, sections, signature_times, trim_markup, urls,
    user_id, wiki_users,
};
use crate::buggregator::MAINTAINER;
use crate::buggregator::issues::{self, text_id};
use crate::mediawiki;
use crate::state::AppState;
use crate::wiki::{Wiki, encode_title};
use anyhow::Result;
use std::collections::HashSet;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Every section of the watched talk pages is an issue. Sections are
/// refreshed on every run, so replies, dates and descriptions do not go stale.
pub(super) async fn from_wiki_pages(state: &AppState, conn: &mut Conn, tools: &ToolMatcher) -> Result<()> {
    let pages: Vec<(u64, String, String, Option<u64>)> =
        conn.query("SELECT id, wiki, page, tool_hint FROM wiki_page").await?;
    for (page_id, dbname, page, tool_hint) in pages {
        let wiki = Wiki::from_dbname(&dbname)?;
        let text = mediawiki::page_wikitext(&state.http, &wiki, &page).await?.unwrap_or_default();
        for (label, wikitext) in sections(&text) {
            // `date_last` is a SQL timestamp; mysql_common does not convert
            // its `Date` value to `String`, so format it in SQL for the
            // textual comparison in `reopen_on_reply`.
            let known: Option<(u64, String, Option<String>)> = conn
                .exec_first(
                    "SELECT issue.id, issue.status, DATE_FORMAT(issue.date_last, '%Y-%m-%d %H:%i:%s')
                     FROM issue JOIN wiki_issue ON issue_id=issue.id
                     WHERE site='WIKI' AND label=? AND wiki_page_id=?",
                    (db_label(&label), page_id),
                )
                .await?;
            let times = signature_times(&wikitext);
            let issue = NewIssue {
                url: format!("https://{}/wiki/{}#{}", wiki.host(), encode_title(&page), encode_title(&label)),
                tool: tools.find(&label).or_else(|| tools.find(&wikitext)).or(tool_hint),
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
                    reopen_on_reply(conn, issue_id, &status, old_last.as_deref(), &issue, &wikitext).await?;
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
    tx.exec_drop(
        "UPDATE issue SET label=?, url=?, description_text_id=?,
           date_created=COALESCE(date_created, ?),
           date_last=COALESCE(GREATEST(date_last, ?), date_last, ?),
           tool=COALESCE(tool, ?)
         WHERE id=?",
        (
            db_label(&issue.label),
            &issue.url,
            description_id,
            issue.date_created.as_deref(),
            issue.date_last.as_deref(),
            issue.date_last.as_deref(),
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
    old_last: Option<&str>,
    issue: &NewIssue,
    wikitext: &str,
) -> Result<()> {
    let newer = match (issue.date_last.as_deref(), old_last) {
        (Some(new), Some(old)) => new > old,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if status != "CLOSED" || !newer {
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

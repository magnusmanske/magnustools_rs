//! Dates, tools, and closing/reopening issues, where the update could not
//! settle them.

use super::{ToolMatcher, format_time, regex, signature_times};
use crate::buggregator::MAINTAINER;
use crate::buggregator::issues;
use anyhow::Result;
use chrono::{Months, Utc};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// The standalone maintenance command, without the network imports.
pub async fn maintenance(conn: &mut Conn) -> Result<()> {
    let tools = ToolMatcher::load(conn).await?;
    tidy(conn, &tools).await
}

pub(super) async fn tidy(conn: &mut Conn, tools: &ToolMatcher) -> Result<()> {
    // Wiki issues without dates: from the signatures.
    let undated: Vec<(u64, String)> = conn
        .query(
            "SELECT issue.id, wikitext FROM issue JOIN wiki_issue ON issue_id=issue.id
             WHERE site='WIKI' AND status='OPEN' AND date_created IS NULL",
        )
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
    let untooled: Vec<(u64, String, String, Option<u64>)> = conn
        .query(
            "SELECT issue.id, label, COALESCE(wikitext, (SELECT text FROM text WHERE text.id=description_text_id), ''),
             tool_hint
             FROM issue LEFT JOIN wiki_issue ON issue_id=issue.id LEFT JOIN wiki_page ON wiki_page.id=wiki_page_id
             WHERE tool IS NULL AND status='OPEN'",
        )
        .await?;
    for (issue_id, label, text, hint) in untooled {
        let tool = tools.find(&label).or_else(|| tools.find(&text)).or(hint);
        if let Some(tool) = tool {
            conn.exec_drop("UPDATE issue SET tool=? WHERE id=? AND tool IS NULL", (tool, issue_id)).await?;
        }
    }

    // Wiki issues where the maintainer had the last word a month ago are done.
    let cutoff = Utc::now().naive_utc().checked_sub_months(Months::new(1)).map(|t| format_time(&t)).unwrap_or_default();
    let replied: Vec<(u64, String)> = conn
        .exec(
            "SELECT issue.id, wikitext FROM issue JOIN wiki_issue ON wiki_issue.issue_id=issue.id
             JOIN user_issue ON user_issue.issue_id=issue.id JOIN user ON user.id=user_issue.user_id
             WHERE issue.site='WIKI' AND issue.status='OPEN' AND issue.date_last IS NOT NULL AND issue.date_last<?
             AND user.name=? AND user.site='WIKI'",
            (cutoff, MAINTAINER),
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

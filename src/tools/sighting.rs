//! Shared parts of the sighting tools: wiki and category input, links to
//! flagged revisions, capped replica queries, and RSS.

use crate::replicas::{display_title, placeholders};
use crate::wiki::{Wiki, encode_component};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use wikimisc::escape;
use wikimisc::mysql_async::prelude::{FromRow, Queryable};
use wikimisc::mysql_async::{Conn, Params as DbParams, Value as DbValue};

pub(super) const DEFAULT_LANGUAGE: &str = "de";
pub(super) const DEFAULT_PROJECT: &str = "wikipedia";
pub(super) const NS_ARTICLE: i64 = 0;
pub(super) const MAX_DEPTH: u32 = 20;
/// Pages rendered for one request; a huge category tree must not use unbounded memory.
pub(super) const MAX_PAGES: usize = 10_000;
/// The old subdomain's paths redirect here.
pub(super) const TOOL_BASE: &str = "https://magnustools.toolforge.org";
/// Values per `IN (…)` list, as the replica queries use.
const CHUNK: usize = 5_000;

pub(super) fn default_language() -> String {
    DEFAULT_LANGUAGE.to_string()
}

pub(super) fn default_project() -> String {
    DEFAULT_PROJECT.to_string()
}

/// The category depth, with the old tools' default; invalid input falls back.
pub(super) fn depth(value: Option<&str>, default: u32) -> u32 {
    value.and_then(|value| value.trim().parse().ok()).unwrap_or(default).min(MAX_DEPTH)
}

pub(super) fn is_rss(mode: Option<&str>) -> bool {
    mode.is_some_and(|mode| mode.eq_ignore_ascii_case("rss"))
}

/// The pending changes of a category (a database title), as the old tools linked them.
pub(super) fn pending_changes_url(wiki: &Wiki, category: &str) -> String {
    format!(
        "{}?title=Special:PendingChanges&namespace={NS_ARTICLE}&category={}",
        wiki.index_url(),
        encode_component(&display_title(category))
    )
}

/// A page (a database title); `no_redirect` links to the redirect page itself.
pub(super) fn page_url(wiki: &Wiki, title: &str, no_redirect: bool) -> String {
    let url = wiki.page_url(&display_title(title));
    if no_redirect { format!("{url}?redirect=no") } else { url }
}

/// The diff from the last sighted revision `stable` to the current one.
pub(super) fn diff_url(wiki: &Wiki, title: &str, stable: u64) -> String {
    format!("{}?diff=cur&oldid={stable}&diffonly=0", wiki.page_url(&display_title(title)))
}

/// As [`crate::replicas::select_in`], but at most `cap` rows; the flag says
/// whether there were more. The last row of `sql` must be `IN`.
pub(super) async fn select_capped<T: FromRow + Send + 'static, V: Into<DbValue> + Clone>(
    conn: &mut Conn,
    sql: &str,
    suffix: &str,
    params: &[DbValue],
    values: &[V],
    cap: usize,
) -> Result<(Vec<T>, bool), crate::replicas::ReplicasError> {
    let mut rows = vec![];
    for chunk in values.chunks(CHUNK) {
        let remaining = cap.saturating_sub(rows.len());
        if remaining == 0 {
            return Ok((rows, true));
        }
        // One more than needed shows whether another chunk would have added rows.
        let statement = format!("{sql} ({}) {suffix} LIMIT {}", placeholders(chunk.len()), remaining + 1);
        let mut all = params.to_vec();
        all.extend(chunk.iter().cloned().map(Into::into));
        let mut batch: Vec<T> = conn.exec(statement, DbParams::Positional(all)).await?;
        let truncated = batch.len() > remaining;
        batch.truncate(remaining);
        rows.extend(batch);
        if truncated {
            return Ok((rows, true));
        }
    }
    Ok((rows, false))
}

/// The start of an RSS feed; `title` and `link` describe it.
pub(super) fn rss_header(title: &str, link: &str, language: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><rss version=\"2.0\"><channel>\
         <title>{}</title><link>{}</link><description>{}</description>\
         <language>{}</language><ttl>5</ttl>",
        escape::xml(title),
        escape::xml(link),
        escape::xml(title),
        escape::xml(language)
    )
}

/// One RSS item; `description` may be HTML and is escaped, `title` and `link` as text.
pub(super) fn rss_item(title: &str, link: &str, description: &str) -> String {
    format!(
        "<item><title>{}</title><link>{}</link><guid isPermaLink=\"false\">{}</guid>\
         <description>{}</description></item>",
        escape::xml(title),
        escape::xml(link),
        rss_guid(description),
        escape::xml(description)
    )
}

pub(super) fn rss_footer() -> &'static str {
    "</channel></rss>"
}

pub(super) fn rss_response(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], body).into_response()
}

fn rss_guid(value: &str) -> String {
    use sha1::{Digest, Sha1};
    format!("{:x}", Sha1::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_falls_back_and_is_capped() {
        assert_eq!(depth(None, 1), 1);
        assert_eq!(depth(Some("3"), 1), 3);
        assert_eq!(depth(Some("deep"), 1), 1);
        assert_eq!(depth(Some("999"), 1), MAX_DEPTH);
    }

    #[test]
    fn urls_display_titles() {
        let wiki = Wiki::new("de", "wikipedia").unwrap();
        assert_eq!(
            pending_changes_url(&wiki, "Leuchtturm_in_Schottland"),
            "https://de.wikipedia.org/w/index.php?title=Special:PendingChanges&namespace=0&category=Leuchtturm%20in%20Schottland"
        );
        assert_eq!(
            page_url(&wiki, "Leuchtturm_in_Schottland", true),
            "https://de.wikipedia.org/wiki/Leuchtturm_in_Schottland?redirect=no"
        );
        assert_eq!(
            diff_url(&wiki, "Leuchtturm_in_Schottland", 42),
            "https://de.wikipedia.org/wiki/Leuchtturm_in_Schottland?diff=cur&oldid=42&diffonly=0"
        );
    }

    #[test]
    fn rss_escapes_and_has_a_stable_guid() {
        let item = rss_item("A & B", "https://example.org/?a=1&b=2", "<a href=\"x&y\">t</a>");
        assert!(item.contains("<title>A &amp; B</title>"));
        assert!(item.contains("<link>https://example.org/?a=1&amp;b=2</link>"));
        assert!(item.contains("<description>&lt;a href=&quot;x&amp;y&quot;&gt;t&lt;/a&gt;</description>"));
        assert_eq!(rss_guid("x"), rss_guid("x"));
        assert_ne!(rss_guid("x"), rss_guid("y"));
    }
}

//! `no_information.php` of `add-information`: files without an
//! {{Information}} or similar template, optionally only those of one
//! uploader, in title order.
//!
//! Such files are rare and not indexed, so this scans files in chunks, for
//! a while at most, and links to where it stopped.

use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wikimisc::mysql_async::prelude::Queryable;

/// A file with one of these needs no other.
pub(super) const TEMPLATES: &[&str] = &[
    "Information",
    "Painting",
    "Blason-fr-en",
    "Blason-fr-en-it",
    "Blason-xx",
    "COAInformation",
    "Artwork",
    "Art_Photo",
    "Photograph",
    "Book",
    "Map",
    "Musical_work",
    "Specimen",
];
const LINK_TABLES: &[&str] = &["page", "templatelinks", "linktarget"];
const DEFAULT_MAX: usize = 100;
const MAX_MAX: usize = 500;
/// Files checked per query.
const CHUNK: usize = 10_000;
/// Scanning stops after this, with a link to go on.
const TIME_BUDGET: Duration = Duration::from_secs(20);

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    /// The first title to look at.
    startswith: String,
    /// The last title looked at, from a "next" link.
    after: Option<String>,
    user: String,
    max: Option<String>,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "commons".to_string(),
            project: "wikimedia".to_string(),
            startswith: String::new(),
            after: None,
            user: String::new(),
            max: None,
            doit: None,
        }
    }
}

/// Where a scan starts, in title order.
enum Start {
    From(String),
    After(String),
}

struct Scan {
    files: Vec<String>,
    scanned: usize,
    /// Where the next scan starts, if there are more files.
    after: Option<String>,
}

struct File {
    name: String,
    url: String,
    add_url: String,
}

struct Results {
    files: Vec<File>,
    scanned: usize,
    next_url: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/no_information.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    max: usize,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let max = request.max.as_deref().and_then(|m| m.trim().parse().ok()).unwrap_or(DEFAULT_MAX).clamp(1, MAX_MAX);
    let user = display_title(&db_title(&request.user));
    let start = match &request.after {
        Some(after) => Start::After(db_title(after)),
        None => Start::From(db_title(&request.startswith)),
    };
    let wanted =
        request.doit.is_some() || request.after.is_some() || !request.startswith.trim().is_empty() || !user.is_empty();
    let results = match wanted {
        false => None,
        true => {
            let wiki = Wiki::new(&request.language, &request.project)?;
            let scan = scan(&state, &wiki, &user, start, max).await?;
            Some(results(&wiki, &request, &user, max, scan))
        }
    };
    render(&Page { title: "No information", request: &request, max, results })
}

async fn scan(state: &AppState, wiki: &Wiki, user: &str, mut start: Start, max: usize) -> Result<Scan, AppError> {
    let began = Instant::now();
    let mut links = state.replicas.conn(wiki, LINK_TABLES).await?;
    let sql = "SELECT lt_id FROM linktarget WHERE lt_namespace=10 AND lt_title IN";
    let ids: Vec<u64> = select_in(&mut links, sql, "", &[], TEMPLATES).await?;
    let has_template = match ids.is_empty() {
        true => "0".to_string(),
        false => {
            let ids: Vec<String> = ids.iter().map(u64::to_string).collect();
            format!(
                "EXISTS (SELECT 1 FROM templatelinks WHERE tl_from=page_id AND tl_target_id IN ({}))",
                ids.join(",")
            )
        }
    };
    let mut uploads = match user.is_empty() {
        true => None,
        false => Some(state.replicas.conn(wiki, &["image", "actor"]).await?),
    };

    let mut scan = Scan { files: vec![], scanned: 0, after: None };
    loop {
        let (op, title) = match &start {
            Start::From(title) => (">=", title.as_str()),
            Start::After(title) => (">", title.as_str()),
        };
        // Titles in order, and whether they have a template.
        let chunk: Vec<(String, bool)> = match &mut uploads {
            None => {
                let sql = format!(
                    "SELECT page_title, {has_template} FROM page
                     WHERE page_namespace=6 AND page_is_redirect=0 AND page_title {op} ?
                     ORDER BY page_title LIMIT {CHUNK}"
                );
                links.exec(sql, (title,)).await.map_err(anyhow::Error::from)?
            }
            Some(uploads) => {
                let sql = format!(
                    "SELECT img_name FROM image JOIN actor ON actor_id=img_actor
                     WHERE actor_name=? AND img_name {op} ? ORDER BY img_name LIMIT {CHUNK}"
                );
                let titles: Vec<String> = uploads.exec(sql, (user, title)).await.map_err(anyhow::Error::from)?;
                let sql = format!(
                    "SELECT page_title FROM page WHERE page_namespace=6 AND NOT {has_template} AND page_title IN"
                );
                let without: HashSet<String> =
                    select_in(&mut links, &sql, "", &[], &titles).await?.into_iter().collect();
                titles.into_iter().map(|t| (t.clone(), !without.contains(&t))).collect()
            }
        };
        let last_chunk = chunk.len() < CHUNK;
        let Some(last) = chunk.last().map(|(title, _)| title.clone()) else { return Ok(scan) };
        for (title, has_template) in chunk {
            scan.scanned += 1;
            if !has_template {
                scan.files.push(title);
            }
            if scan.files.len() > max {
                scan.files.truncate(max);
                scan.after = scan.files.last().cloned();
                return Ok(scan);
            }
        }
        if last_chunk {
            return Ok(scan);
        }
        if began.elapsed() > TIME_BUDGET {
            scan.after = Some(last);
            return Ok(scan);
        }
        start = Start::After(last);
    }
}

fn results(wiki: &Wiki, request: &Request, user: &str, max: usize, scan: Scan) -> Results {
    let query = |extra: &[(&str, &str)]| {
        let mut pairs = vec![("language", request.language.trim()), ("project", request.project.trim())];
        pairs.extend_from_slice(extra);
        pairs.iter().map(|(k, v)| format!("{k}={}", encode_component(v))).collect::<Vec<_>>().join("&")
    };
    let max = max.to_string();
    let next_url = scan.after.map(|after| {
        let mut extra = vec![("max", max.as_str()), ("after", after.as_str())];
        if !user.is_empty() {
            extra.push(("user", user));
        }
        format!("/no_information/?{}", query(&extra))
    });
    let files = scan
        .files
        .into_iter()
        .map(|name| File {
            url: wiki.page_url(&format!("File:{name}")),
            add_url: format!("/add_information/?{}", query(&[("image", &name)])),
            name: display_title(&name),
        })
        .collect();
    Results { files, scanned: scan.scanned, next_url }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn links() {
        let request = Request { language: "commons".into(), ..Request::default() };
        let scan = Scan { files: vec!["A_b.jpg".into()], scanned: 7, after: Some("A_b.jpg".into()) };
        let results = results(&Wiki::commons(), &request, "Some One", 1, scan);
        assert_eq!(results.files[0].name, "A b.jpg");
        assert_eq!(results.files[0].url, "https://commons.wikimedia.org/wiki/File:A_b.jpg");
        assert_eq!(results.files[0].add_url, "/add_information/?language=commons&project=wikimedia&image=A_b.jpg");
        assert_eq!(
            results.next_url.as_deref(),
            Some("/no_information/?language=commons&project=wikimedia&max=1&after=A_b.jpg&user=Some%20One")
        );
    }

    #[tokio::test]
    async fn empty_form() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/no_information/?language=commons&project=wikimedia").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"startswith\""));
        assert_eq!(get(dir.path(), "/no_information.php?user=X").await.status.as_u16(), 308);
    }
}

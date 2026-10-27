//! `quick_counter.php`: a user's live and deleted edits per namespace.

use crate::cache::TimedMap;
use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

/// Namespace names per wiki, which hardly ever change.
static NAMESPACES: TimedMap<String, HashMap<i64, String>> = TimedMap::new(Duration::from_secs(86_400), 1000);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    user: String,
    #[serde(default = "default_project")]
    project: String,
}

fn default_project() -> String {
    "en.wikipedia".to_string()
}

struct Row {
    namespace: String,
    live: String,
    deleted: String,
    total: String,
}

#[derive(Template)]
#[template(path = "tools/quick_counter.html")]
struct Page {
    title: &'static str,
    user: String,
    project: String,
    rows: Vec<Row>,
    totals: Option<Row>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page {
        title: "Quick edit counter",
        user: request.user.trim().replace('_', " "),
        project: request.project.trim().to_string(),
        rows: vec![],
        totals: None,
        error: None,
    };
    if page.user.is_empty() {
        return render(&page);
    }
    let wiki = Wiki::from_domain_prefix(&page.project)?;
    let mut conn = state.replicas.conn(&wiki, &["actor", "revision", "page", "archive"]).await?;
    let actor: Option<u64> = conn
        .exec_first("SELECT actor_id FROM actor WHERE actor_name=? AND actor_user IS NOT NULL", (page.user.as_str(),))
        .await
        .map_err(anyhow::Error::from)?;
    let Some(actor) = actor else {
        page.error = Some(format!("There is no user “{}” on {wiki}.", page.user));
        return render(&page);
    };
    let live: Vec<(i64, u64)> = conn
        .exec(
            "SELECT page_namespace, COUNT(*) FROM revision_userindex JOIN page ON page_id=rev_page
             WHERE rev_actor=? GROUP BY page_namespace",
            (actor,),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let deleted: Vec<(i64, u64)> = conn
        .exec("SELECT ar_namespace, COUNT(*) FROM archive_userindex WHERE ar_actor=? GROUP BY ar_namespace", (actor,))
        .await
        .map_err(anyhow::Error::from)?;
    let names = NAMESPACES
        .get_or_fetch(wiki.dbname().to_string(), async || namespace_names(&state.http, &wiki).await)
        .await
        .unwrap_or_default();
    let counts = per_namespace(&live, &deleted);
    page.rows = counts
        .iter()
        .map(|(ns, (l, d))| {
            let name = names.get(ns).map_or_else(|| ns.to_string(), |n| format!("{ns} ({n})"));
            row(name, *l, *d)
        })
        .collect();
    let (live, deleted) = counts.iter().fold((0, 0), |(l, d), (_, (nl, nd))| (l + nl, d + nd));
    page.totals = Some(row("Total".to_string(), live, deleted));
    render(&page)
}

fn row(namespace: String, live: u64, deleted: u64) -> Row {
    Row { namespace, live: thousands(live), deleted: thousands(deleted), total: thousands(live + deleted) }
}

/// `(namespace, (live, deleted))`, most edits first.
fn per_namespace(live: &[(i64, u64)], deleted: &[(i64, u64)]) -> Vec<(i64, (u64, u64))> {
    let mut counts: BTreeMap<i64, (u64, u64)> = BTreeMap::new();
    for (ns, n) in live {
        counts.entry(*ns).or_default().0 += n;
    }
    for (ns, n) in deleted {
        counts.entry(*ns).or_default().1 += n;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|(ns, (l, d))| (std::cmp::Reverse(l + d), *ns));
    counts
}

async fn namespace_names(http: &reqwest::Client, wiki: &Wiki) -> anyhow::Result<HashMap<i64, String>> {
    let json =
        mediawiki::api(http, wiki, &[("action", "query"), ("meta", "siteinfo"), ("siprop", "namespaces")]).await?;
    Ok(json["query"]["namespaces"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, ns)| {
            let name = ns["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Article");
            Some((ns["id"].as_i64()?, name.to_string()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_and_sorted() {
        assert_eq!(per_namespace(&[(0, 10), (2, 3)], &[(0, 1), (4, 5)]), vec![(0, (10, 1)), (4, (0, 5)), (2, (3, 0))]);
    }
}

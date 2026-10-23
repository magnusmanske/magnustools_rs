//! `metronom.php`: page views in one month of the articles a user created.

use crate::replicas::format_date;
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{Datelike, Months, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::Instant;
use wikimisc::http::HttpRetryPolicy;
use wikimisc::mysql_async::prelude::Queryable;

const PAGEVIEWS_API: &str = "https://wikimedia.org/api/rest_v1/metrics/pageviews/per-article";
/// Page views exist from July 2015.
const FIRST_MONTH: (i32, u32) = (2015, 7);
const MAX_ARTICLES: usize = 1000;
const PARALLEL_REQUESTS: usize = 4;
/// The page view service limits the request rate. Whatever is not counted
/// this long after the request came in is reported as such, rather than the
/// page timing out.
const TIME_BUDGET: Duration = Duration::from_secs(200);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    month: Option<String>,
    #[serde(default)]
    user: String,
    doit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Article {
    title: String,
    url: String,
    created: String,
    views: Option<u64>,
}

struct Results {
    articles: Vec<Article>,
    /// Articles started, of which `articles` are the first ones.
    started: String,
    capped: bool,
    total: String,
    failed: usize,
}

#[derive(Template)]
#[template(path = "tools/metronom.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    user: String,
    month: String,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let user = request.user.trim().replace('_', " ");
    let month = match request.month.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(month) => Month::parse(month)?,
        None => Month::previous(),
    };
    let deadline = Instant::now() + TIME_BUDGET;
    let results = match request.doit.is_some() && !user.is_empty() {
        true => Some(results(&state, &request, &user, &month, deadline).await?),
        false => None,
    };
    render(&Page { title: "Metronom", request: &request, user, month: month.label(), results })
}

async fn results(
    state: &AppState,
    request: &Request,
    user: &str,
    month: &Month,
    deadline: Instant,
) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let mut conn = state.replicas.conn(&wiki, &["page", "revision", "actor"]).await?;
    let created: Vec<(String, String)> = conn
        .exec(
            "SELECT page_title, rev_timestamp FROM revision_userindex
             JOIN page ON page_id=rev_page JOIN actor ON actor_id=rev_actor
             WHERE actor_name=? AND rev_parent_id=0 AND page_namespace=0 AND page_is_redirect=0
             ORDER BY rev_timestamp",
            (user,),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let started = thousands(created.len() as u64);
    let capped = created.len() > MAX_ARTICLES;
    let mut articles: Vec<Article> = created
        .into_iter()
        .take(MAX_ARTICLES)
        .map(|(title, timestamp)| Article {
            url: wiki.page_url(&title),
            created: format_date(&timestamp),
            title,
            views: None,
        })
        .collect();

    let project = wiki.host().trim_end_matches(".org").to_string();
    let titles: Vec<String> = articles.iter().map(|a| a.title.clone()).collect();
    let mut views_by_title = HashMap::new();
    for chunk in titles.chunks(PARALLEL_REQUESTS) {
        if Instant::now() >= deadline {
            break;
        }
        let mut requests = JoinSet::new();
        for title in chunk {
            let url = format!(
                "{PAGEVIEWS_API}/{project}/all-access/user/{}/monthly/{}/{}",
                encode_component(title),
                month.first_day(),
                month.last_day()
            );
            let (http, title) = (state.http.clone(), title.clone());
            requests.spawn(async move {
                let views = tokio::time::timeout_at(deadline, views(&http, &url)).await.ok().flatten();
                (title, views)
            });
        }
        while let Some(Ok((title, views))) = requests.join_next().await {
            views_by_title.insert(title, views);
        }
    }
    for article in &mut articles {
        article.views = views_by_title.remove(&article.title).flatten();
    }

    let failed = articles.iter().filter(|a| a.views.is_none()).count();
    let total = articles.iter().filter_map(|a| a.views).sum();
    articles.sort_by_key(|a| std::cmp::Reverse(a.views.unwrap_or_default()));
    for article in &mut articles {
        article.title = article.title.replace('_', " ");
    }
    Ok(Results { articles, started, capped, total: thousands(total), failed })
}

/// Views in the month; no data (e.g. a new article) counts as none. `None` if the API failed.
async fn views(http: &reqwest::Client, url: &str) -> Option<u64> {
    // The API limits the request rate.
    let retry = HttpRetryPolicy::new(4).with_jitter();
    match retry.fetch_json::<Value>(http.get(url)).await {
        Ok(json) => Some(json["items"].as_array()?.iter().filter_map(|i| i["views"].as_u64()).sum()),
        Err(e) if e.status() == Some(404) => Some(0),
        Err(e) => {
            tracing::debug!("page views: {e}");
            None
        }
    }
}

/// A calendar month with page view data.
struct Month(NaiveDate);

impl Month {
    fn parse(yyyymm: &str) -> Result<Self, AppError> {
        let invalid = || AppError::BadRequest(format!("“{yyyymm}” is not a month (YYYYMM, from 201507)."));
        let year: i32 = yyyymm.get(0..4).and_then(|y| y.parse().ok()).ok_or_else(invalid)?;
        let month: u32 = yyyymm.get(4..6).and_then(|m| m.parse().ok()).ok_or_else(invalid)?;
        let date = NaiveDate::from_ymd_opt(year, month, 1).filter(|_| yyyymm.len() == 6).ok_or_else(invalid)?;
        if (year, month) < FIRST_MONTH {
            return Err(invalid());
        }
        Ok(Self(date))
    }

    fn previous() -> Self {
        let today = Utc::now().date_naive();
        let first = today.with_day(1).unwrap_or(today);
        Self(first.checked_sub_months(Months::new(1)).unwrap_or(first))
    }

    fn label(&self) -> String {
        self.0.format("%Y%m").to_string()
    }

    fn first_day(&self) -> String {
        self.0.format("%Y%m%d").to_string()
    }

    fn last_day(&self) -> String {
        let next = self.0.checked_add_months(Months::new(1)).unwrap_or(self.0);
        next.pred_opt().unwrap_or(self.0).format("%Y%m%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        let feb = Month::parse("202402").unwrap();
        assert_eq!(
            (feb.label(), feb.first_day(), feb.last_day()),
            ("202402".into(), "20240201".into(), "20240229".into())
        );
        for bad in ["201506", "2024", "202413", "2024021", "abcdef"] {
            assert!(Month::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Month::previous().first_day().len(), 8);
    }
}

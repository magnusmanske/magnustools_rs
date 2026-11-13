//! `xcontribs.php`: how a user's edits are spread over the Wikimedia
//! projects. The original drew a chart; a table says the same and more.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::{Wiki, ucfirst};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;

const THRESHOLDS: &[u64] = &[1, 5, 10, 25, 50, 100, 1000, 10000];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    user: String,
}

struct Project {
    dbname: String,
    edits: String,
    url: String,
}

struct Results {
    projects: Vec<Project>,
    wikis: usize,
    total: String,
    average: String,
    largest: String,
    largest_share: String,
    theil: Option<String>,
    thresholds: Vec<(String, usize)>,
}

#[derive(Template)]
#[template(path = "tools/xcontribs.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    user: String,
    central_url: String,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let user = ucfirst(&request.user.trim().replace('_', " ")).into_owned();
    let results = match user.is_empty() {
        true => None,
        false => Some(results(&state, &user).await?),
    };
    let central_url = Wiki::new("meta", "wikimedia")?.page_url(&format!("Special:CentralAuth/{user}"));
    render(&Page { title: "xContribs [Erwin85]", request: &request, user, central_url, results })
}

async fn results(state: &AppState, user: &str) -> Result<Results, AppError> {
    let meta = Wiki::new("meta", "wikimedia")?;
    let json = mediawiki::api(
        &state.http,
        &meta,
        &[("action", "query"), ("meta", "globaluserinfo"), ("guiuser", user), ("guiprop", "merged")],
    )
    .await?;
    let mut merged: Vec<(String, String, u64)> = json["query"]["globaluserinfo"]["merged"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|account| {
            let dbname = account["wiki"].as_str()?;
            let url = account["url"].as_str()?;
            let edits = account["editcount"].as_u64().unwrap_or(0);
            (edits > 0).then(|| (dbname.to_string(), url.to_string(), edits))
        })
        .collect();
    merged.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let counts: Vec<u64> = merged.iter().map(|(_, _, edits)| *edits).collect();
    let total: u64 = counts.iter().sum();
    let project_count = counts.len();
    let largest = counts.first().copied().unwrap_or(0);
    let average = if project_count > 0 { total as f64 / project_count as f64 } else { 0.0 };
    let theil = theil(&counts, average);
    let thresholds = THRESHOLDS
        .iter()
        .map(|threshold| (thousands(*threshold), counts.iter().filter(|edits| **edits > *threshold).count()))
        .collect();
    let projects = merged
        .into_iter()
        .map(|(dbname, url, edits)| Project {
            url: format!(
                "{}/wiki/Special:Contributions/{}",
                url.trim_end_matches('/'),
                crate::wiki::percent_encode(user)
            ),
            dbname,
            edits: thousands(edits),
        })
        .collect();
    Ok(Results {
        projects,
        wikis: project_count,
        total: thousands(total),
        average: format!("{average:.2}"),
        largest: thousands(largest),
        largest_share: if total > 0 {
            format!("{:.0}%", 100.0 * largest as f64 / total as f64)
        } else {
            "0%".to_string()
        },
        theil,
        thresholds,
    })
}

/// The Theil index of the edit counts; `None` when there is nothing to compare.
fn theil(counts: &[u64], average: f64) -> Option<String> {
    if counts.len() < 2 || average <= 0.0 {
        return None;
    }
    let n = counts.len() as f64;
    let sum: f64 = counts
        .iter()
        .map(|count| {
            let ratio = *count as f64 / average;
            ratio * ratio.ln()
        })
        .sum();
    Some(format!("{:.2}", sum / (n * n.ln())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theil_of_equal_and_unequal_counts() {
        assert_eq!(theil(&[10, 10], 10.0).as_deref(), Some("0.00"));
        assert!(theil(&[90, 10], 50.0).unwrap().parse::<f64>().unwrap() > 0.0);
        assert_eq!(theil(&[10], 10.0), None);
    }
}

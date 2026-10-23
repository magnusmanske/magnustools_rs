//! `randomarticle.php` and `random_image_commons_subcat.php`: redirect to a
//! random article, or a random Commons file, in a category tree.

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::sync::Arc;

const DEFAULT_DEPTH: u32 = 3;
const MAX_DEPTH: u32 = 15;

#[derive(Debug, Deserialize)]
pub struct ArticleRequest {
    #[serde(default = "default_language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    categories: String,
    d: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageRequest {
    #[serde(default)]
    category: String,
    d: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

#[derive(Template)]
#[template(path = "tools/random_page.html")]
struct Page<'a> {
    title: &'static str,
    action: &'static str,
    /// Articles: the site can be chosen; images are always on Commons.
    site: Option<(&'a str, &'a str)>,
    category_field: &'static str,
    category: &'a str,
    depth: u32,
    error: Option<String>,
}

fn depth(d: &Option<String>) -> u32 {
    d.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH)
}

pub async fn article(
    State(state): State<Arc<AppState>>,
    Params(request): Params<ArticleRequest>,
) -> Result<Response, AppError> {
    let depth = depth(&request.d);
    let mut page = Page {
        title: "Random article",
        action: "/randomarticle/",
        site: Some((&request.lang, &request.family)),
        category_field: "categories",
        category: request.categories.trim(),
        depth,
        error: None,
    };
    if page.category.is_empty() {
        return Ok(render(&page)?.into_response());
    }
    let wiki = Wiki::new(&request.lang, &request.family)?;
    match state.replicas.random_page_in_tree(&wiki, page.category, depth, 0).await? {
        Some(title) => Ok(redirect(&wiki.page_url(&title))),
        None => {
            page.error = Some("There are no articles in this category tree.".to_string());
            Ok(render(&page)?.into_response())
        }
    }
}

pub async fn image(
    State(state): State<Arc<AppState>>,
    Params(request): Params<ImageRequest>,
) -> Result<Response, AppError> {
    let depth = depth(&request.d);
    let mut page = Page {
        title: "Random image",
        action: "/random_image_commons_subcat/",
        site: None,
        category_field: "category",
        category: request.category.trim(),
        depth,
        error: None,
    };
    if page.category.is_empty() {
        return Ok(render(&page)?.into_response());
    }
    let commons = Wiki::commons();
    match state.replicas.random_page_in_tree(&commons, page.category, depth, 6).await? {
        Some(title) => Ok(redirect(&commons.page_url(&format!("File:{title}")))),
        None => {
            page.error = Some("There are no files in this category tree.".to_string());
            Ok(render(&page)?.into_response())
        }
    }
}

/// Never cached, so that every visit is a new draw.
fn redirect(url: &str) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Redirect::to(url)).into_response()
}

//! `transparent.php`: find a translator. Users of a wiki who say (by babel
//! box) they are native speakers of another language, most recently active
//! first.

use crate::replicas::{CATEGORY_TABLES, display_title, format_date, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_SHOWN: usize = 100;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    planguage: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    tlanguage: String,
    doit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Translator {
    name: String,
    url: String,
    last_edit: String,
}

#[derive(Template)]
#[template(path = "tools/transparent.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    category_url: String,
    translators: Option<Vec<Translator>>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = request.tlanguage.trim().to_lowercase();
    let mut page =
        Page { title: "Find a translator", request: &request, category_url: String::new(), translators: None };
    if request.doit.is_none() || language.is_empty() {
        return render(&page);
    }
    if !is_language_code(&language) {
        return Err(AppError::BadRequest(format!("“{language}” is not a language code.")));
    }
    let wiki = Wiki::new(&request.planguage, &request.project)?;
    let category = format!("User_{language}-N");
    page.category_url = wiki.page_url(&format!("Category:{category}"));

    let mut categories = state.replicas.conn(&wiki, CATEGORY_TABLES).await?;
    let user_pages: Vec<String> = categories
        .exec(
            "SELECT page_title FROM page JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
             WHERE lt_namespace=14 AND lt_title=? AND page_namespace=2",
            (category.as_str(),),
        )
        .await
        .map_err(anyhow::Error::from)?;
    // Babel boxes on subpages do not name the user.
    let users: Vec<String> = user_pages.iter().filter(|p| !p.contains('/')).map(|p| display_title(p)).collect();

    let mut core = state.replicas.conn(&wiki, &["recentchanges", "actor"]).await?;
    let sql = "SELECT actor_name, MAX(rc_timestamp) FROM recentchanges_userindex JOIN actor ON actor_id=rc_actor
        WHERE actor_name IN";
    let mut active: Vec<(String, String)> = select_in(&mut core, sql, "GROUP BY actor_name", &[], &users).await?;
    active.sort_by(|a, b| b.1.cmp(&a.1));
    page.translators = Some(
        active
            .into_iter()
            .take(MAX_SHOWN)
            .map(|(name, timestamp)| Translator {
                url: wiki.page_url(&format!("User:{name}")),
                name,
                last_edit: format_date(&timestamp),
            })
            .collect(),
    );
    render(&page)
}

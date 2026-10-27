//! `spacer.php`: links to spacecraft databases for an international
//! designator (COSPAR ID). The databases are listed on a wiki page, as
//! `* [https://…%ID%… Name]`.

use crate::cache::TimedCache;
use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const CONFIG_PAGE: &str = "Benutzer:Magnus Manske/Spacer";
static CONFIG: TimedCache<String> = TimedCache::new(Duration::from_secs(600));

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    id: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Link {
    name: String,
    url: String,
}

#[derive(Template)]
#[template(path = "tools/spacer.html")]
struct Page {
    title: &'static str,
    id: String,
    links: Vec<Link>,
    config_url: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let wiki = Wiki::new("de", "wikipedia")?;
    let id = request.id.trim().to_string();
    let links = match id.is_empty() {
        true => vec![],
        false => {
            let config = CONFIG
                .get_or_fetch(async || {
                    mediawiki::page_wikitext(&state.http, &wiki, CONFIG_PAGE).await.map(Option::unwrap_or_default)
                })
                .await?;
            links(&config, &id)
        }
    };
    render(&Page { title: "Spacer", id, links, config_url: wiki.page_url(CONFIG_PAGE) })
}

/// The configured links, with the ID filled in. Only web links count.
fn links(config: &str, id: &str) -> Vec<Link> {
    let id = encode_component(id);
    let mut links: Vec<Link> = vec![];
    for line in config.lines() {
        let Some(inner) = line.trim().strip_prefix('*').map(str::trim) else { continue };
        let Some((url, name)) =
            inner.strip_prefix('[').and_then(|l| l.strip_suffix(']')).and_then(|l| l.split_once(' '))
        else {
            continue;
        };
        if !url.starts_with("https://") && !url.starts_with("http://") {
            continue;
        }
        let link = Link { name: name.trim().to_string(), url: url.replace("%ID%", &id) };
        match links.iter_mut().find(|l| l.name == link.name) {
            Some(existing) => *existing = link,
            None => links.push(link),
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_from_the_config_page() {
        let config = "Intro\n* [https://www.n2yo.com/database/?q=%ID%#results N2YO]\n\
            *[https://celestrak.org/NORAD/elements/table.php?INTDES=%ID% Celestrak]\n\
            * [javascript:alert(1) Evil]\n* not a link";
        assert_eq!(
            links(config, "1957-001A"),
            vec![
                Link { name: "N2YO".into(), url: "https://www.n2yo.com/database/?q=1957-001A#results".into() },
                Link {
                    name: "Celestrak".into(),
                    url: "https://celestrak.org/NORAD/elements/table.php?INTDES=1957-001A".into()
                },
            ]
        );
    }

    #[test]
    fn ids_cannot_break_out_of_the_url() {
        let config = "* [https://example.org/?q=%ID% X]";
        assert_eq!(links(config, "a'&b c")[0].url, "https://example.org/?q=a%27%26b%20c");
    }
}

//! `cas.php`: links for a CAS registry number (chemistry), from the wiki's
//! `Template:CasTemplate`, with `{cas}`, `{cas9}` and `{title}` filled in.

use crate::cache::TimedMap;
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

const TEMPLATE: &str = "Template:CasTemplate";
/// The Wikipedias that have the template.
const WIKIS_WITH_TEMPLATE: &[&str] = &["de", "fr", "it", "pl"];
/// The template, rendered, per wiki: it rarely changes, and was the one
/// request each page made.
static TEMPLATES: TimedMap<String, Option<String>> = TimedMap::new(Duration::from_secs(600), 1000);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    cas: String,
    #[serde(default)]
    title: String,
}

fn default_language() -> String {
    "de".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

/// A CAS registry number: 2-7 digits, 2 digits, a check digit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cas(String);

impl Cas {
    fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('-').collect();
        let digits = |p: &str, len: std::ops::RangeInclusive<usize>| {
            len.contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit())
        };
        match parts.as_slice() {
            [a, b, c] if digits(a, 2..=7) && digits(b, 2..=2) && digits(c, 1..=1) => Some(Self(parts.join("-"))),
            _ => None,
        }
    }

    /// Without dashes, zero-padded to nine digits.
    fn nine_digits(&self) -> String {
        format!("{:0>9}", self.0.replace('-', ""))
    }
}

#[derive(Template)]
#[template(path = "tools/cas.html")]
struct Page {
    title: &'static str,
    language: String,
    cas: String,
    name: String,
    links_html: Option<String>,
    error: Option<String>,
    wikis_with_template: &'static [&'static str],
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page {
        title: "CAS number links",
        language: request.language.clone(),
        cas: request.cas.trim().to_string(),
        name: request.title.trim().to_string(),
        links_html: None,
        error: None,
        wikis_with_template: WIKIS_WITH_TEMPLATE,
    };
    if !page.cas.is_empty() {
        match Cas::parse(&page.cas) {
            None => page.error = Some(format!("“{}” is not a CAS registry number.", page.cas)),
            Some(cas) => {
                let wiki = Wiki::new(&request.language, &request.project)?;
                let template = TEMPLATES
                    .get_or_fetch(wiki.host().to_string(), async || {
                        mediawiki::page_html(&state.http, &wiki, TEMPLATE).await
                    })
                    .await?;
                match template.as_deref() {
                    Some(html) => page.links_html = Some(fill_in(html, &cas, &page.name)),
                    None => page.error = Some(format!("{wiki} has no {TEMPLATE}.")),
                }
            }
        }
    }
    render(&page)
}

/// The template's placeholders all sit in link URLs.
fn fill_in(html: &str, cas: &Cas, name: &str) -> String {
    html.replace("{cas}", &cas.0).replace("{cas9}", &cas.nine_digits()).replace("{title}", &encode_component(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_numbers() {
        assert_eq!(Cas::parse(" 7732-18-5 "), Some(Cas("7732-18-5".into())));
        assert_eq!(Cas::parse("7732-18-5").unwrap().nine_digits(), "007732185");
        for bad in ["", "7732185", "7732-18-55", "1-18-5", "77a2-18-5", "7732-18-5\"><script>"] {
            assert_eq!(Cas::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn placeholders() {
        let html = r#"<a href="https://x.org/?a={cas}&b={cas9}&t={title}">x</a>"#;
        let cas = Cas::parse("50-00-0").unwrap();
        assert_eq!(
            fill_in(html, &cas, "Form\"aldehyde"),
            r#"<a href="https://x.org/?a=50-00-0&b=000050000&t=Form%22aldehyde">x</a>"#
        );
    }
}

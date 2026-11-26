//! `isin.php`: links to financial databases for an ISIN (International
//! Securities Identification Number), from the wiki's
//! `User:Magnus Manske/ISIN` page, with `%ISIN%` filled in.

use crate::cache::TimedMap;
use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, default_language, default_project};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const CONFIG_PAGE: &str = "User:Magnus Manske/ISIN";
/// The page, rendered, per wiki: it rarely changes, and was the one request
/// each page made.
static PAGES: TimedMap<String, Option<String>> = TimedMap::new(Duration::from_secs(600), 1000);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    isin: String,
}

/// An ISIN: two country letters, nine alphanumeric characters, and a Luhn
/// check digit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Isin(String);

impl Isin {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_uppercase();
        let bytes = s.as_bytes();
        if bytes.len() != 12
            || !(bytes[0].is_ascii_alphabetic() && bytes[1].is_ascii_alphabetic())
            || !bytes[2..].iter().all(u8::is_ascii_alphanumeric)
            || !bytes[11].is_ascii_digit()
            || !luhn_valid(&s)
        {
            return None;
        }
        Some(Self(s))
    }
}

/// The Luhn check digit, over the letters expanded to two digits (A=10, …, Z=35).
fn luhn_valid(s: &str) -> bool {
    let Some(digits) = expanded_digits(s) else { return false };
    let sum: u32 = digits
        .into_iter()
        .rev()
        .enumerate()
        .map(|(i, mut digit)| {
            if i % 2 == 1 {
                digit *= 2;
                if digit > 9 {
                    digit -= 9;
                }
            }
            digit
        })
        .sum();
    sum.is_multiple_of(10)
}

fn expanded_digits(s: &str) -> Option<Vec<u32>> {
    let mut digits = Vec::with_capacity(s.len() * 2);
    for c in s.chars() {
        let n = c.to_digit(36)?;
        match n {
            0..=9 => digits.push(n),
            _ => digits.extend([n / 10, n % 10]),
        }
    }
    Some(digits)
}

#[derive(Template)]
#[template(path = "tools/isin.html")]
struct Page {
    title: &'static str,
    language: String,
    project: String,
    isin: String,
    name: String,
    links_html: Option<String>,
    config_url: Option<String>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page {
        title: "ISIN links",
        language: request.language.trim().to_lowercase(),
        project: request.project.trim().to_lowercase(),
        isin: request.isin.trim().to_ascii_uppercase(),
        name: request.title.trim().to_string(),
        links_html: None,
        config_url: None,
        error: None,
    };
    if page.isin.is_empty() {
        return render(&page);
    }
    let Some(isin) = Isin::parse(&page.isin) else {
        page.error = Some(format!("“{}” is not an ISIN.", page.isin));
        return render(&page);
    };
    page.isin = isin.0.clone();
    let wiki = Wiki::new(&page.language, &page.project)?;
    let template = PAGES
        .get_or_fetch(wiki.host().to_string(), async || mediawiki::page_html(&state.http, &wiki, CONFIG_PAGE).await)
        .await?;
    match template.as_deref() {
        Some(html) => {
            page.links_html = Some(fill_in(html, &isin));
            page.config_url = Some(wiki.page_url(CONFIG_PAGE));
        }
        None => page.error = Some(format!("{wiki} has no {CONFIG_PAGE}.")),
    }
    render(&page)
}

/// The page's placeholders all sit in link URLs.
fn fill_in(html: &str, isin: &Isin) -> String {
    html.replace("%ISIN%", &isin.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isin_numbers() {
        for good in ["US0378331005", "ES0178430E18", "CH0012332372", " us0378331005 "] {
            assert!(Isin::parse(good).is_some(), "{good}");
        }
        assert_eq!(Isin::parse(" us0378331005 ").unwrap().0, "US0378331005");
        for bad in [
            "",
            "US037833100",
            "US03783310055",
            "US0378331004",
            "1S0378331005",
            "US03783310O5",
            "US0378331005\"><script>",
        ] {
            assert_eq!(Isin::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn placeholders() {
        let html = r#"<a href="https://x.org/?isin=%ISIN%&x=%ISIN%">x</a>"#;
        let isin = Isin::parse("US0378331005").unwrap();
        assert_eq!(fill_in(html, &isin), r#"<a href="https://x.org/?isin=US0378331005&x=US0378331005">x</a>"#);
    }
}

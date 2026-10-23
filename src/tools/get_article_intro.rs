//! `get_article_intro.php`: the first paragraph of an article as plain
//! text, as JSONP. Without a title, a form explaining the API.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, jsonp, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde::Deserialize;
use std::sync::{Arc, LazyLock};

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "lang")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    title: String,
    callback: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

#[derive(Template)]
#[template(path = "tools/get_article_intro.html")]
struct Page {
    title: &'static str,
    language: String,
    project: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    if request.title.trim().is_empty() {
        let page = Page { title: "Get article intro", language: request.language, project: request.project };
        return Ok(render(&page)?.into_response());
    }
    let wiki = Wiki::new(&request.language, &request.project)?;
    let text = mediawiki::page_wikitext(&state.http, &wiki, &request.title).await?.unwrap_or_default();
    let language = request.language.trim().to_lowercase();
    let intro = dewikify(initial_paragraph(&text, &language), &language);
    // The old tool dropped anything not allowed in a callback name, rather than refusing it.
    let callback =
        request.callback.map(|c| c.chars().filter(|c| c.is_ascii_alphanumeric() || "_.".contains(*c)).collect());
    Ok(jsonp(intro, callback).into_response())
}

/// The first line that looks like running text: not a template, table,
/// list, heading, HTML, or image.
fn initial_paragraph<'a>(text: &'a str, language: &str) -> &'a str {
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line.starts_with("{{") {
            // Skip to the end of a multi-line template.
            let mut current = line;
            while !current.ends_with("}}") {
                match lines.next() {
                    Some(next) => current = next,
                    None => break,
                }
            }
            continue;
        }
        if line.starts_with("{|") {
            let mut current = line;
            while !current.starts_with("|}") {
                match lines.next() {
                    Some(next) => current = next,
                    None => break,
                }
            }
            continue;
        }
        if line.is_empty()
            || line.starts_with("--")
            || line.starts_with("}}")
            || line.starts_with([':', '*', '#', '=', '<', '!', '|'])
        {
            continue;
        }
        if is_image(line) || (language == "eo" && line.contains(">>")) {
            continue;
        }
        return line;
    }
    ""
}

fn is_image(line: &str) -> bool {
    static FILE_LINK: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"(?i)^\[\[\s*(file|image|datei|bild|fichier|archivo|imagen|immagine|plik|afbeelding)\s*:")
    });
    let lower = line.to_lowercase();
    FILE_LINK.is_match(line)
        || ["|thumb|", "|frame|", "|right|", "|miniatur|", "|hochkant="].iter().any(|s| lower.contains(s))
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

static COMMENT: LazyLock<Regex> = LazyLock::new(|| regex(r"<!--.*?-->"));
static REF_SELF_CLOSING: LazyLock<Regex> = LazyLock::new(|| regex(r"<ref[^>]+/>"));
static REF: LazyLock<Regex> = LazyLock::new(|| regex(r"<ref.+?</ref>"));
static PIPED_LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"\[\[([^|\]]+)\|([^\]]+)\]\]"));
static FR_DATE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\{\{date\|([^|}]+)\|([^|}]+)\|([^|}]+)\}\}"));
static EMPTY_PARENS: LazyLock<Regex> = LazyLock::new(|| regex(r"\(\s*\)"));
static PAREN_PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| regex(r"\([ ,;.]+"));
static SPACE_BEFORE_PAREN: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+\)"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
static QUOTES: LazyLock<Regex> = LazyLock::new(|| regex(r"''+"));

/// Wikitext to plain text, roughly: no comments or references, links as their text, no templates.
fn dewikify(paragraph: &str, language: &str) -> String {
    let mut p = COMMENT.replace_all(paragraph, "").into_owned();
    p = REF_SELF_CLOSING.replace_all(&p, "").into_owned();
    p = REF.replace_all(&p, "").into_owned();
    p = PIPED_LINK.replace_all(&p, "$2").into_owned();
    p = p.replace("[[", "").replace("]]", "");
    if language == "fr" {
        p = FR_DATE.replace_all(&p, "${1} ${2} ${3}").into_owned();
    }
    p = remove_templates(&p);
    p = EMPTY_PARENS.replace_all(&p, "").into_owned();
    p = p.replace("[]", "").replace("}}", "");
    p = PAREN_PUNCTUATION.replace_all(&p, "(").into_owned();
    p = SPACE_BEFORE_PAREN.replace_all(&p, ")").into_owned();
    p = EMPTY_PARENS.replace_all(&p, " ").into_owned();
    p = SPACES.replace_all(&p, " ").into_owned();
    QUOTES.replace_all(&p, "").trim().to_string()
}

/// Cuts from each `{{` to the next `}}`; leftover `}}` of nested templates go later.
fn remove_templates(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        out.push_str(before);
        rest = after.split_once("}}").map_or("", |(_, tail)| tail);
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = "{{Infobox person\n| name = Douglas Adams\n}}\n[[File:Douglas adams portrait.jpg|thumb|Adams]]\n\n\
        '''Douglas Noël Adams''' (11 March 1952 – 11 May 2001) was an English [[author]], [[Humour|humorist]], \
        and [[screenwriter]],<!-- comment --> best known for ''[[The Hitchhiker's Guide to the Galaxy]]''.<ref name=\"x\">Ref</ref>{{sfn|Smith|2001}}\n\nSecond.";

    #[test]
    fn first_paragraph() {
        assert!(initial_paragraph(ARTICLE, "en").starts_with("'''Douglas Noël Adams'''"));
        assert_eq!(initial_paragraph("{{x}}\n== H ==\n* list", "en"), "");
        assert_eq!(initial_paragraph("{|\n| cell\n|}\nText", "en"), "Text");
    }

    #[test]
    fn plain_text() {
        assert_eq!(
            dewikify(initial_paragraph(ARTICLE, "en"), "en"),
            "Douglas Noël Adams (11 March 1952 – 11 May 2001) was an English author, humorist, and screenwriter, \
             best known for The Hitchhiker's Guide to the Galaxy."
        );
        assert_eq!(dewikify("Born {{date|1|mai|1900}} ({{lang|x}}) in Paris", "fr"), "Born 1 mai 1900 in Paris");
        assert_eq!(dewikify("A {{outer|{{inner}}|x}} B", "en"), "A |x B");
    }
}

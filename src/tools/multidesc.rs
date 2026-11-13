//! `multidesc.php`: descriptions for a Commons file from the articles that
//! use it, or a first paragraph per language for a gallery.

use crate::mediawiki::page_contents;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_title, upper_first};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, LazyLock};
use wikimisc::mysql_async::prelude::Queryable;

/// Languages followed; the old tool could follow a very long chain.
const MAX_LANGUAGES: usize = 100;
const MAX_PAGES_PER_LANGUAGE: usize = 200;

static REF: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?is)<ref\b[^>]*>.*?</ref>|<ref\b[^>]*/>").ok());
static FRENCH_DATE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)\{\{date\|([^|}]+)\|([^|}]+)\|([^|}]+)\}\}").ok());

#[derive(Debug, Deserialize)]
pub struct Request {
    baseon: Option<String>,
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    directlinks: bool,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    onlynew: bool,
    image: Option<String>,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    raw: bool,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    js: bool,
}

struct Missing {
    language: String,
    title: String,
    edit_url: String,
    links: Vec<(String, String, String)>,
}

#[derive(Template)]
#[template(path = "tools/multidesc.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    baseon: String,
    output: String,
    missing: Vec<Missing>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let baseon = request.baseon.as_deref().unwrap_or("en").trim().to_lowercase();
    let title = request.title.as_deref().unwrap_or_default().trim();
    let image = request.image.as_deref().unwrap_or_default().trim();
    let raw = request.raw || request.js;
    let mut page = Page {
        title: "MultiDesc",
        request: &request,
        baseon: baseon.clone(),
        output: String::new(),
        missing: vec![],
        error: None,
    };
    if image.is_empty() && title.is_empty() {
        return Ok(render(&page)?.into_response());
    }

    if !image.is_empty() {
        match image_output(&state, image, request.onlynew).await {
            Ok(output) => page.output = output,
            Err(e) => page.error = Some(e.to_string()),
        }
    } else {
        match article_output(&state, &baseon, title, request.directlinks).await {
            Ok((output, missing)) => {
                page.output = output;
                page.missing = missing;
            }
            Err(e) => page.error = Some(e.to_string()),
        }
    }

    if raw {
        let body = if request.js {
            let escaped = page.output.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
            format!("function whatisthat_get_descriptions () {{\nreturn \"{escaped}\";\n}}")
        } else {
            page.output.clone()
        };
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response());
    }
    Ok(render(&page)?.into_response())
}

async fn image_output(state: &AppState, image: &str, onlynew: bool) -> Result<String, AppError> {
    let commons = Wiki::commons();
    let image = normalize_image(image);
    let mut conn = state.replicas.conn(&commons, &["globalimagelinks"]).await?;
    let rows: Vec<(String, String)> = conn
        .exec(
            "SELECT gil_wiki, gil_page_title FROM globalimagelinks
             WHERE gil_to=? AND gil_page_namespace_id=0 ORDER BY gil_wiki, gil_page_title",
            (image.as_str(),),
        )
        .await
        .map_err(anyhow::Error::from)?;

    let already = if onlynew { used_templates(state, &commons, &image).await? } else { BTreeSet::new() };
    let mut by_wiki: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (wiki, page) in rows {
        by_wiki.entry(wiki).or_default().push(page);
    }

    let mut lines = vec![];
    for (dbname, pages) in by_wiki {
        let Some((language, wiki)) = wikipedia_for_dbname(&dbname) else { continue };
        if already.contains(&upper_first(&language).to_lowercase()) {
            continue;
        }
        let pages: Vec<&str> = pages.iter().take(MAX_PAGES_PER_LANGUAGE).map(String::as_str).collect();
        let contents = page_contents(&state.http, &wiki, &pages).await?;
        let mut descriptions = vec![];
        for page in pages {
            let key = crate::mediawiki::title_key(page);
            let Some(content) = contents.get(&key) else { continue };
            descriptions.extend(caption_from_wikitext(&content.text, &image));
        }
        if descriptions.is_empty() {
            continue;
        }
        let description = clean_description(&descriptions.join(" "));
        if !description.is_empty() {
            lines.push(format!("{{{{{language}|{description}}}}}"));
        }
    }
    Ok(lines.join("\n"))
}

async fn used_templates(state: &AppState, wiki: &Wiki, image: &str) -> Result<BTreeSet<String>, AppError> {
    let title = format!("File:{}", crate::replicas::display_title(image));
    let Some(content) = crate::mediawiki::page_content(&state.http, wiki, &title).await? else {
        return Ok(BTreeSet::new());
    };
    Ok(template_names(&content.text).into_iter().map(|name| name.to_lowercase()).collect())
}

/// A Wikipedia for a `globalimagelinks` wiki database name, with its
/// language code (renamed wikis keep their old database names).
fn wikipedia_for_dbname(dbname: &str) -> Option<(String, Wiki)> {
    let language = match dbname {
        "be_x_oldwiki" => "be-tarask".to_string(),
        "zh_classicalwiki" => "lzh".to_string(),
        "zh_yuewiki" => "yue".to_string(),
        "zh_min_nanwiki" => "nan".to_string(),
        "roa_rupwiki" => "rup".to_string(),
        "bat_smgwiki" => "sgs".to_string(),
        "fiu_vrowiki" => "vro".to_string(),
        "nowiki" => "no".to_string(),
        _ => dbname.strip_suffix("wiki")?.replace('_', "-"),
    };
    let wiki = Wiki::new(&language, "wikipedia").ok()?;
    wiki.host().ends_with(".wikipedia.org").then_some((language, wiki))
}

async fn article_output(
    state: &AppState,
    baseon: &str,
    title: &str,
    directlinks: bool,
) -> Result<(String, Vec<Missing>), AppError> {
    let base_wiki = Wiki::new(baseon, "wikipedia").map_err(|e| AppError::BadRequest(e.to_string()))?;
    let mut queue = VecDeque::from([(baseon.to_string(), title.to_string())]);
    let mut seen = BTreeSet::new();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut titles: BTreeMap<String, String> = BTreeMap::new();
    let mut links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    while let Some((language, page_title)) = queue.pop_front() {
        if !seen.insert(language.clone()) || seen.len() > MAX_LANGUAGES {
            continue;
        }
        let wiki = match Wiki::new(&language, "wikipedia") {
            Ok(wiki) => wiki,
            Err(_) if language == baseon => base_wiki.clone(),
            Err(_) => continue,
        };
        let (text, langlinks) = article_and_links(&state.http, &wiki, &page_title).await?;
        texts.insert(language.clone(), text);
        titles.insert(language.clone(), page_title);
        for (link_language, link_title) in langlinks {
            links.entry(language.clone()).or_default().insert(link_language.clone());
            if !seen.contains(&link_language) {
                queue.push_back((link_language, link_title));
            }
        }
    }

    let mut lines = vec![];
    for (language, text) in &texts {
        let paragraph = initial_paragraph(text, language);
        if paragraph.is_empty() {
            continue;
        }
        let paragraph = clean_paragraph(&paragraph, language, directlinks, titles.get(language));
        if !paragraph.is_empty() {
            lines.push(format!("{{{{{language}|{paragraph}}}}}"));
        }
    }
    for (language, title) in &titles {
        lines.push(format!("[[{language}:{title}]]"));
    }

    let mut missing = vec![];
    for (language, title) in &titles {
        let linked = links.get(language).cloned().unwrap_or_default();
        let absent: Vec<(String, String, String)> = titles
            .iter()
            .filter(|(other, _)| *other != language && !linked.contains(*other))
            .map(|(other, title)| {
                let url = Wiki::new(other, "wikipedia").map(|wiki| wiki.page_url(title)).unwrap_or_default();
                (other.clone(), title.clone(), url)
            })
            .collect();
        if absent.is_empty() {
            continue;
        }
        let wiki = Wiki::new(language, "wikipedia");
        let edit_url = wiki.map_or_else(
            |_| String::new(),
            |wiki| format!("{}?action=edit&title={}", wiki.index_url(), encode_title(title)),
        );
        missing.push(Missing { language: language.clone(), title: title.clone(), edit_url, links: absent });
    }
    Ok((lines.join("\n"), missing))
}

async fn article_and_links(
    http: &reqwest::Client,
    wiki: &Wiki,
    title: &str,
) -> Result<(String, Vec<(String, String)>), AppError> {
    let json = crate::mediawiki::api(
        http,
        wiki,
        &[
            ("action", "query"),
            ("prop", "revisions|langlinks"),
            ("rvprop", "content"),
            ("rvslots", "main"),
            ("lllimit", "500"),
            ("redirects", "1"),
            ("titles", title),
        ],
    )
    .await?;
    let page = &json["query"]["pages"][0];
    let text = page["revisions"][0]["slots"]["main"]["content"].as_str().unwrap_or_default().to_string();
    let links = page["langlinks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|link| {
            let language = link["lang"].as_str()?;
            let title = link["title"].as_str().or_else(|| link["*"].as_str())?;
            Some((language.to_string(), title.to_string()))
        })
        .collect();
    Ok((text, links))
}

/// The first line of the first paragraph, skipping templates, tables, images
/// and lists, as the original did.
fn initial_paragraph(text: &str, language: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let mut line = lines[index].trim();
        index += 1;
        if line.is_empty() {
            continue;
        }
        if line.starts_with("{{") {
            if line.ends_with("}}") {
                continue;
            }
            while index < lines.len() {
                line = lines[index].trim();
                index += 1;
                if line.ends_with("}}") {
                    break;
                }
            }
            continue;
        }
        if line.starts_with("--")
            || line.starts_with(':')
            || line.starts_with('*')
            || line.starts_with('#')
            || line.starts_with('=')
            || line.starts_with('<')
            || line.starts_with('!')
            || line.starts_with('|')
            || line.starts_with("}}")
        {
            continue;
        }
        if line.starts_with("{|") {
            while index < lines.len() {
                line = lines[index].trim();
                index += 1;
                if line.starts_with("|}") {
                    break;
                }
            }
            continue;
        }
        let lower = line.to_lowercase();
        if lower.contains("[[file:") || lower.contains("[[image:") || lower.contains("[[datei:") {
            continue;
        }
        if lower.contains("|thumb|")
            || lower.contains("|frame|")
            || lower.contains("|right|")
            || lower.contains("|miniatur|")
            || lower.contains("|hochkant=")
        {
            continue;
        }
        if language == "eo" && lower.contains(">>") {
            continue;
        }
        return line.to_string();
    }
    String::new()
}

fn clean_paragraph(paragraph: &str, language: &str, directlinks: bool, title: Option<&String>) -> String {
    let mut paragraph =
        REF.as_ref().map_or_else(|| paragraph.to_string(), |re| re.replace_all(paragraph, "").to_string());
    let prefix = format!("[[{language}:");
    paragraph = paragraph.replace("[[", &prefix);

    let mut parts = paragraph.split(prefix.as_str());
    let mut out = parts.next().unwrap_or_default().trim().to_string();
    out.push(' ');
    for part in parts {
        if let Some((e1, rest)) = part.split_once("]]") {
            let mut e1 = e1.trim().to_string();
            if !e1.contains('|') {
                e1.push('|');
            }
            out.push_str(&prefix);
            out.push_str(&e1);
            out.push_str("]]");
            out.push_str(rest);
        } else {
            out.push_str(&prefix);
            out.push_str(part);
        }
    }
    let mut paragraph = out;
    if language == "fr"
        && let Some(date) = FRENCH_DATE.as_ref()
    {
        paragraph = date.replace_all(&paragraph, "${1} ${2} ${3}").to_string();
    }
    paragraph = remove_templates(&paragraph);
    paragraph = paragraph.replace("(  )", "").replace("()", "");
    paragraph = paragraph.replace("[]", "").replace("}}", "");
    if directlinks && let Some(title) = title {
        paragraph.push_str(&format!(" (&rarr;[[:{language}:{title}|{title}]])"));
    }
    paragraph.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn remove_templates(text: &str) -> String {
    let mut text = text.to_string();
    while let Some(start) = text.find("{{") {
        let Some(end) = text[start + 2..].find("}}") else { break };
        let after = start + 2 + end + 2;
        text = format!("{}{}", text.get(..start).unwrap_or_default(), text.get(after..).unwrap_or_default());
    }
    text
}

fn template_names(text: &str) -> Vec<String> {
    let mut names = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(['|', '}']) else { break };
        let name = after[..end].trim();
        if !name.is_empty() {
            names.push(name.to_string());
        }
        rest = &after[end..];
    }
    names
}

/// The captions of `image` in a page's wikitext.
fn caption_from_wikitext(text: &str, image: &str) -> Vec<String> {
    let image = normalized_file(image);
    let mut captions = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = find_link_end(after) else { break };
        let link = &after[..end];
        rest = &after[end + 2..];
        let mut parts = link.split('|');
        let Some(target) = parts.next() else { continue };
        if !target.to_lowercase().starts_with("file:") && !target.to_lowercase().starts_with("image:") {
            continue;
        }
        let target = target.split_once(':').map_or(target, |(_, target)| target);
        if normalized_file(target) != image {
            continue;
        }
        let caption = parts.rfind(|part| !is_image_option(part)).map(str::trim).unwrap_or_default();
        if !caption.is_empty() {
            captions.push(caption.to_string());
        }
    }
    captions
}

fn is_image_option(parameter: &str) -> bool {
    let lower = parameter.trim().to_lowercase();
    lower.is_empty()
        || [
            "thumb",
            "thumbnail",
            "frame",
            "framed",
            "frameless",
            "border",
            "left",
            "right",
            "center",
            "centre",
            "none",
            "baseline",
            "sub",
            "super",
            "top",
            "text-top",
            "middle",
            "bottom",
            "text-bottom",
            "upright",
        ]
        .contains(&lower.as_str())
        || lower.starts_with("alt=")
        || lower.starts_with("link=")
        || lower.starts_with("lang=")
        || lower.starts_with("page=")
        || lower.starts_with("class=")
        || lower.starts_with("upright=")
        || lower.starts_with("thumbtime=")
        || lower.starts_with("start=")
        || lower.starts_with("end=")
        || lower.ends_with("px")
        || (lower.contains('x') && lower.chars().all(|c| c.is_ascii_digit() || c == 'x'))
}

fn normalized_file(name: &str) -> String {
    name.trim().trim_start_matches(':').replace('_', " ").to_lowercase()
}

/// The byte offset of the `]]` that closes the link whose `[[` was consumed,
/// ignoring nested links in the caption.
fn find_link_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with("[[") {
            depth += 1;
            index += 2;
        } else if rest.starts_with("]]") {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
            index += 2;
        } else {
            index += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

fn normalize_image(image: &str) -> String {
    let image = image.trim();
    let image = match image.split_once(':') {
        Some((namespace, rest))
            if namespace.eq_ignore_ascii_case("file") || namespace.eq_ignore_ascii_case("image") =>
        {
            rest
        }
        _ => image,
    };
    image.trim().replace(' ', "_")
}

fn clean_description(description: &str) -> String {
    let description = description.replace('|', "/").split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() || description.ends_with('.') { description } else { format!("{description}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_skip_templates_and_images() {
        let text = "{{Infobox|a=b}}\n\n[[File:X.jpg|thumb|A caption]]\nThe first paragraph.";
        assert_eq!(initial_paragraph(text, "en"), "The first paragraph.");
        assert_eq!(initial_paragraph("== Heading ==\nText.", "en"), "Text.");
        assert_eq!(initial_paragraph("", "en"), "");
    }

    #[test]
    fn captions_come_from_file_links() {
        let text = "x [[File:A.jpg|thumb|right|200px|A caption]] y [[Image:A.jpg|thumb|Other]]";
        assert_eq!(caption_from_wikitext(text, "A.jpg"), vec!["A caption", "Other"]);
        assert!(caption_from_wikitext(text, "B.jpg").is_empty());
        let nested = "[[File:A.jpg|thumb|A [[linked]] caption]]";
        assert_eq!(caption_from_wikitext(nested, "A.jpg"), vec!["A [[linked]] caption"]);
    }

    #[test]
    fn templates_are_removed() {
        assert_eq!(remove_templates("a {{b|c}} d"), "a  d");
        assert_eq!(remove_templates("a {{b|c"), "a {{b|c");
    }

    #[test]
    fn wiki_databases_map_to_languages() {
        assert_eq!(wikipedia_for_dbname("enwiki").map(|(l, _)| l).as_deref(), Some("en"));
        assert_eq!(wikipedia_for_dbname("be_x_oldwiki").map(|(l, _)| l).as_deref(), Some("be-tarask"));
        assert!(wikipedia_for_dbname("commonswiki").is_none());
    }

    #[test]
    fn paragraphs_are_clean_markup() {
        let clean = clean_paragraph("A [[link]] and {{a template}}.", "en", false, None);
        assert_eq!(clean, "A [[en:link|]] and .");
    }
}

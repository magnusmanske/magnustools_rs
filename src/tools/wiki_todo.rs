//! Wiki ToDo (the `wiki-todo` tool's `index.php`): checks a random article
//! for things to do, using the checks and interface texts from
//! `meta:Wiki-ToDo/<language>`.
//!
//! The original crashed on the current replica schema (`imagelinks.il_to` is
//! gone), keyed its sections on English headings (so the Finnish page was
//! dropped), suggested the long-removed Persondata template, and linked to a
//! dead image-search tool. This port fixes those, and renders the
//! configuration's markup through a small allowlist.

use crate::mediawiki::{self, Api};
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, percent_encode, wiki_urlencode};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::escape;

/// The Action API's limit for anonymous callers, for every list here.
const MAX_LIST: usize = 500;
/// Outgoing links are fetched to the end, as the original did; this bounds it.
const MAX_LINKS: usize = 5000;
/// Random draws before giving up on finding suggestions.
const MAX_ATTEMPTS: usize = 5;
/// The original only listed this many missing articles to start.
const MAX_NEW_ARTICLES: usize = 50;
const NS_FILE: i64 = 6;
const NS_TEMPLATE: i64 = 10;
const NS_CATEGORY: i64 = 14;

/// `Misc`: one string per language.
const MISC_KEYS: &[&str] = &["disamb_key", "disamb_template", "listmarkers", "persondata", "stubpattern"];
/// `Multi`: `|`-separated values.
const MULTI_KEYS: &[&str] = &["geo_indicators", "coord_templates", "bio_indicators", "skipcat"];

#[derive(Debug, Deserialize)]
pub struct Request {
    language: Option<String>,
    project: Option<String>,
    #[serde(default)]
    title: String,
    doit: Option<String>,
}

/// The interface texts of the configuration, as safe HTML.
struct Labels {
    analysis: String,
    view: String,
    edit: String,
    things_todo: String,
    nothing_wrong: String,
    supported_languages: String,
    edit_interface_text: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            analysis: "Analysis".to_string(),
            view: "View".to_string(),
            edit: "edit".to_string(),
            things_todo: "Things to do".to_string(),
            nothing_wrong: "There seems to be nothing wrong with this article.".to_string(),
            supported_languages: "Supported languages".to_string(),
            edit_interface_text: "Edit interface text".to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "tools/wiki_todo.html")]
struct Page {
    title: &'static str,
    labels: Labels,
    language: String,
    project: String,
    title_input: String,
    result: Option<Analysis>,
    tried: usize,
    error: Option<String>,
    meta_url: String,
    edit_meta_url: String,
}

/// What one article's analysis found; the message fields are safe HTML.
struct Analysis {
    title: String,
    view_url: String,
    edit_url: String,
    items: Vec<String>,
    notes: Vec<String>,
    found: Vec<String>,
    skipped: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let title = request.title.trim().to_string();
    let language = normalized(request.language.as_deref(), "en");
    let project = normalized(request.project.as_deref(), "wikipedia");
    let mut page = Page {
        title: "Wiki ToDo",
        labels: Labels::default(),
        edit_meta_url: format!("{}/{}", META_PAGE, percent_encode(&language)),
        language,
        project,
        title_input: title.clone(),
        result: None,
        tried: 1,
        error: None,
        meta_url: META_PAGE.to_string(),
    };
    // The original analyzed one on every visit; the other tools show a form
    // first, and a result once they have parameters.
    let run = request.doit.as_deref().is_some_and(|doit| !doit.is_empty())
        || request.language.is_some()
        || request.project.is_some()
        || !title.is_empty();
    if !run {
        return render_page(page);
    }

    let wiki = match Wiki::new(&page.language, &page.project) {
        Ok(wiki) => wiki,
        Err(e) => {
            page.error = Some(e.to_string());
            return render_page(page);
        }
    };
    let Some(config) = load_config(&state, &page.language).await? else {
        page.error = Some(format!(
            "There is no Wiki ToDo configuration for the language “{}”. See the linked list of supported languages.",
            page.language
        ));
        return render_page(page);
    };
    page.labels = Labels::from(&config);
    let api = state.api(&wiki).await?;
    let language = page.language.clone();

    if title.is_empty() {
        for attempt in 1..=MAX_ATTEMPTS {
            page.tried = attempt;
            let title = random_title(&api).await?;
            if let Some(analysis) = analyze(&state, &config, &api, &wiki, &language, &title).await? {
                let done = analysis.skipped.is_none() && !analysis.found.is_empty();
                page.result = Some(analysis);
                if done {
                    break;
                }
            }
        }
        if page.result.is_none() {
            page.error = Some("Could not pick a random article to check; please try again.".to_string());
        }
    } else {
        match analyze(&state, &config, &api, &wiki, &language, &title).await {
            Ok(Some(analysis)) => page.result = Some(analysis),
            Ok(None) => page.error = Some(format!("“{title}” is a redirect; use its target.")),
            Err(AppError::BadRequest(message)) => page.error = Some(message),
            Err(e) => return Err(e),
        }
    }
    render_page(page)
}

const META_PAGE: &str = "https://meta.wikimedia.org/wiki/Wiki-ToDo";

fn normalized(value: Option<&str>, default: &str) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_lowercase(),
        None => default.to_string(),
    }
}

fn render_page(page: Page) -> Result<Response, AppError> {
    Ok(([(header::CACHE_CONTROL, "no-store")], render(&page)?).into_response())
}

async fn load_config(state: &AppState, language: &str) -> Result<Option<Config>, AppError> {
    let meta = Wiki::new("meta", "wikipedia")?;
    let title = format!("Wiki-ToDo/{language}");
    let text = mediawiki::page_wikitext(&state.http, &meta, &title).await?;
    Ok(text.map(|text| Config::parse(&text)))
}

async fn random_title(api: &Api) -> Result<String, AppError> {
    let params = api.params_into(&[("action", "query"), ("list", "random"), ("rnnamespace", "0"), ("rnlimit", "1")]);
    let json = api.get_query_api_json(&params).await?;
    json["query"]["random"][0]["title"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::Upstream(anyhow::anyhow!("the API returned no random page")))
}

/// `None` for a redirect, which a random draw retries.
async fn analyze(
    state: &AppState,
    config: &Config,
    api: &Api,
    wiki: &Wiki,
    language: &str,
    title: &str,
) -> Result<Option<Analysis>, AppError> {
    let limit = MAX_LIST.to_string();
    let params = api.params_into(&[
        ("action", "query"),
        ("titles", title),
        ("prop", "info|images|templates|categories|extlinks|langlinks"),
        ("imlimit", &limit),
        ("tllimit", &limit),
        ("tlnamespace", "10"),
        ("cllimit", &limit),
        ("ellimit", &limit),
        ("lllimit", &limit),
        ("list", "backlinks"),
        ("bltitle", title),
        ("blnamespace", "0"),
        ("blfilterredir", "nonredirects"),
        ("bllimit", &limit),
    ]);
    let (json, links) = tokio::join!(api.get_query_api_json(&params), outgoing_links(api, title));
    let json = json?;
    let links = links?;
    let Some(page) = json["query"]["pages"].as_array().and_then(|pages| pages.first()) else {
        return Err(AppError::Upstream(anyhow::anyhow!("the API returned no page for “{title}”")));
    };
    if page.get("missing").is_some() || page.get("invalid").is_some() {
        return Err(AppError::BadRequest(format!("There is no article “{title}”.")));
    }
    if page.get("redirect").is_some() {
        return Ok(None);
    }
    if page["ns"].as_i64() != Some(0) {
        return Err(AppError::BadRequest(format!("“{title}” is not an article.")));
    }
    let title = page["title"].as_str().unwrap_or(title).to_string();
    let view_url = wiki.page_url(&title);
    let edit_url = format!("{}?action=edit&title={}", wiki.index_url(), wiki_urlencode(&title));

    let mut images = entry_titles(page, "images", NS_FILE, true);
    let templates = entry_titles(page, "templates", NS_TEMPLATE, true);
    let categories = entry_titles(page, "categories", NS_CATEGORY, true);
    let backlinks: Vec<String> = json["query"]["backlinks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["ns"].as_i64() == Some(0))
        .filter_map(|entry| entry["title"].as_str().map(str::to_string))
        .collect();
    let external_links = page["extlinks"].as_array().map_or(0, Vec::len);
    let language_links = page["langlinks"].as_array().map_or(0, Vec::len);
    let length = page["length"].as_u64().unwrap_or(0);
    let touched = page["touched"].as_str().unwrap_or_default();

    let (used_in_templates, existing) = database(state, wiki, &images, &links).await?;
    images.retain(|image| !used_in_templates.contains(&db_title(image)));

    let items = vec![
        count_item(config, images.len(), "images_sg", "images_pl"),
        count_item_with_more(config, backlinks.len(), "linksin_sg", "linksin_pl"),
        outgoing_item(config, &links, &existing),
        count_item(config, external_links, "linksext_sg", "linksext_pl"),
        count_item(config, language_links, "linkslang_sg", "linkslang_pl"),
        count_item(config, templates.len(), "templates_sg", "templates_pl"),
        count_item(config, categories.len(), "categories_sg", "categories_pl"),
        format!("{length} bytes"),
        format!("{}: {}", config.label("lastchanged", "Last changed"), format_timestamp(touched)),
    ];

    if let Some(skip) = config.multi("skipcat").iter().find(|skip| categories.contains(skip)) {
        return Ok(Some(Analysis {
            title,
            view_url,
            edit_url,
            items,
            notes: vec![],
            found: vec![],
            skipped: Some(finding(config.filled("skip_cat", &[("$1", skip)]))),
        }));
    }

    let is_disambig = category_matches(config, &categories);
    let mut notes = vec![];
    let mut found = vec![];
    if is_disambig {
        check_disambiguation(config, wiki, &title, &templates, &backlinks, &mut notes, &mut found);
    }
    if found.is_empty() {
        check_biography(config, language, &categories, &templates, &images, &title, &mut notes, &mut found);
    }
    if found.is_empty() {
        check_place(config, &categories, &templates, &images, &title, &mut notes, &mut found);
    }
    if found.is_empty() {
        check_species(config, &templates, &images, &title, &mut notes, &mut found);
    }
    check_common(
        config,
        wiki,
        &title,
        &categories,
        &backlinks,
        &links,
        &existing,
        external_links,
        language_links,
        length,
        is_disambig,
        &mut found,
    );
    check_wiktionary(state, config, language, &title, &templates, is_disambig, &mut found).await;

    Ok(Some(Analysis { title, view_url, edit_url, items, notes, found, skipped: None }))
}

/// The article's links in the article namespace, following continuation.
async fn outgoing_links(api: &Api, title: &str) -> Result<Vec<String>, AppError> {
    let mut params = api.params_into(&[
        ("action", "query"),
        ("titles", title),
        ("prop", "links"),
        ("plnamespace", "0"),
        ("pllimit", &MAX_LIST.to_string()),
    ]);
    let mut links = vec![];
    loop {
        let json = api.get_query_api_json(&params).await?;
        let page = json["query"]["pages"].as_array().and_then(|pages| pages.first());
        for link in page.into_iter().flat_map(|page| page["links"].as_array().into_iter().flatten()) {
            if link["ns"].as_i64() == Some(0)
                && let Some(title) = link["title"].as_str()
            {
                links.push(title.to_string());
            }
        }
        let Some(cont) = json.get("continue").and_then(|c| c.get("plcontinue")).and_then(Json::as_str) else {
            break;
        };
        params.insert("plcontinue".to_string(), cont.to_string());
        if links.len() >= MAX_LINKS {
            links.truncate(MAX_LINKS);
            break;
        }
    }
    Ok(links)
}

fn count_item(config: &Config, count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", config.plural(singular, plural, count))
}

fn count_item_with_more(config: &Config, count: usize, singular: &str, plural: &str) -> String {
    let more = match count >= MAX_LIST {
        true => format!(" {}", config.text("ormore")),
        false => String::new(),
    };
    format!("{count}{more} {}", config.plural(singular, plural, count))
}

fn outgoing_item(config: &Config, links: &[String], existing: &HashSet<String>) -> String {
    let count = links.iter().filter(|link| existing.contains(*link)).count();
    let mut item = count_item(config, links.len(), "linksout_sg", "linksout_pl");
    if count > 0 {
        let message = match count == 1 {
            true => "ofwhich_exists_sg",
            false => "ofwhich_exists_pl",
        };
        item.push_str(&config.filled(message, &[("$1", &count.to_string())]));
    }
    item
}

fn format_timestamp(timestamp: &str) -> String {
    timestamp.replace('T', " ").trim_end_matches('Z').trim().to_string()
}

fn category_matches(config: &Config, categories: &[String]) -> bool {
    // `Disambiguation` as the key also covers `Disambiguation pages`.
    let key = config.misc("disamb_key").to_lowercase();
    !key.is_empty() && categories.iter().any(|category| category.to_lowercase().starts_with(&key))
}

/// The article titles of a property, in `namespace`, optionally without a
/// namespace prefix (templates, categories, files have localized ones).
fn entry_titles(page: &Json, key: &str, namespace: i64, strip: bool) -> Vec<String> {
    page[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["ns"].as_i64() == Some(namespace))
        .filter_map(|entry| entry["title"].as_str())
        .map(|title| match strip {
            true => without_namespace(title),
            false => title,
        })
        .map(str::to_string)
        .collect()
}

fn without_namespace(title: &str) -> &str {
    title.split_once(':').map_or(title, |(_, rest)| rest)
}

/// `(images used in templates, existing outgoing links)` from the replicas;
/// `imagelinks` only has the target ID since the `il_to` column was removed.
async fn database(
    state: &AppState,
    wiki: &Wiki,
    images: &[String],
    links: &[String],
) -> Result<(HashSet<String>, HashSet<String>), AppError> {
    let mut used = HashSet::new();
    let mut existing = HashSet::new();
    if !images.is_empty() {
        // Commons' link tables are on another server than its file tables.
        match state.replicas.conn(wiki, &["imagelinks", "linktarget"]).await {
            Ok(mut conn) => {
                let keys: Vec<String> = images.iter().map(|image| db_title(image)).collect();
                let sql = "SELECT DISTINCT lt_title FROM imagelinks JOIN linktarget ON lt_id=il_target_id
                           WHERE il_from_namespace=10 AND lt_namespace=6 AND lt_title IN";
                match select_in::<String, _>(&mut conn, sql, "", &[], &keys).await {
                    Ok(rows) => used = rows.into_iter().collect(),
                    Err(e) => tracing::warn!("images used in templates: {e}"),
                }
            }
            Err(e) => tracing::warn!("images used in templates: {e}"),
        }
    }
    if !links.is_empty() {
        let mut conn = state.replicas.conn(wiki, &["page"]).await?;
        let keys: Vec<String> = links.iter().map(|link| db_title(link)).collect();
        let sql = "SELECT page_title FROM page WHERE page_namespace=0 AND page_title IN";
        let rows: Vec<String> = select_in(&mut conn, sql, "", &[], &keys).await?;
        existing = rows.into_iter().map(|title| display_title(&title)).collect();
    }
    Ok((used, existing))
}

// The checks -----------------------------------------------------------------

fn check_disambiguation(
    config: &Config,
    wiki: &Wiki,
    title: &str,
    templates: &[String],
    backlinks: &[String],
    notes: &mut Vec<String>,
    found: &mut Vec<String>,
) {
    notes.push(config.text("disambiguation_page"));
    // `Disambig` as the key also covers `Disambiguation` and its variants.
    let template = config.misc("disamb_template").to_lowercase();
    let has_template = !template.is_empty() && templates.iter().any(|t| t.to_lowercase().starts_with(&template));
    if !template.is_empty() && !has_template {
        found.push(finding(config.text("disambig_template_missing")));
    }

    let short = title.split_once(" (").map_or(title, |(short, _)| short);
    let marker = format!("({})", config.misc("disamb_key").to_lowercase());
    let short_plural = format!("{short}s");
    let short_special = format!("{short} (");
    let links: Vec<String> = backlinks
        .iter()
        .filter(|link| last_word(link.as_str()) != marker)
        .filter(|link| link.as_str() != short && link.as_str() != short_plural)
        .filter(|link| !link.starts_with(&short_special))
        .map(|link| article_link(wiki, link.as_str()))
        .collect();
    if !links.is_empty() {
        let message = markup(config.raw("disamb_fix_links"), &[("$", Substitution::Html(&links.join(", ")))]);
        found.push(finding(message));
    }
}

#[allow(clippy::too_many_arguments)]
fn check_biography(
    config: &Config,
    language: &str,
    categories: &[String],
    templates: &[String],
    images: &[String],
    title: &str,
    notes: &mut Vec<String>,
    found: &mut Vec<String>,
) {
    let indicators = config.multi("bio_indicators");
    let mut is_bio = false;
    let mut birth = false;
    for category in categories {
        let first = first_word(category);
        let last = last_word(category);
        if (language == "en" && last == "births") || (language == "de" && first == "geboren") {
            birth = true;
        }
        if indicators.iter().any(|indicator| indicator == &first || indicator == &last) {
            is_bio = true;
        }
    }
    // For German, the birth category starts with "Geboren", not ends with it.
    if !is_bio && !birth {
        return;
    }
    notes.push(config.text("article_bio"));
    if !has_infobox(templates) && language == "en" {
        found.push(finding(config.text("bio_infobox")));
    }
    if !birth {
        found.push(finding(config.text("bio_birth")));
    }
    append_image(config, title, images, found);
}

fn check_place(
    config: &Config,
    categories: &[String],
    templates: &[String],
    images: &[String],
    title: &str,
    notes: &mut Vec<String>,
    found: &mut Vec<String>,
) {
    let indicators = config.multi("geo_indicators");
    let is_place = categories.iter().any(|category| {
        let first = first_word(category);
        let singular = singular(&first);
        indicators.iter().any(|indicator| indicator == &first || indicator == &singular)
    });
    if !is_place {
        return;
    }
    notes.push(config.text("article_geo"));
    if !has_infobox(templates) {
        found.push(finding(config.text("no_infobox")));
    }
    let coordinate_templates = config.multi("coord_templates");
    let has_coordinate = templates.iter().any(|template| {
        let first = first_word(template);
        coordinate_templates.iter().any(|name| name == &first)
    });
    if !has_coordinate {
        found.push(finding(config.text("no_coord")));
    }
    append_image(config, title, images, found);
}

fn check_species(
    config: &Config,
    templates: &[String],
    images: &[String],
    title: &str,
    notes: &mut Vec<String>,
    found: &mut Vec<String>,
) {
    if !templates.iter().any(|template| first_word(template) == "taxobox") {
        return;
    }
    notes.push(config.text("article_organism"));
    append_image(config, title, images, found);
}

fn append_image(config: &Config, title: &str, images: &[String], found: &mut Vec<String>) {
    if !images.is_empty() {
        return;
    }
    let url = commons_image_search(title);
    found.push(finding(config.filled("search_image", &[("$1", &url)])));
}

#[allow(clippy::too_many_arguments)]
fn check_common(
    config: &Config,
    wiki: &Wiki,
    title: &str,
    categories: &[String],
    backlinks: &[String],
    links: &[String],
    existing: &HashSet<String>,
    external_links: usize,
    language_links: usize,
    length: u64,
    is_disambig: bool,
    found: &mut Vec<String>,
) {
    let search = google_search(wiki, title);
    for (category, message) in &config.tags {
        if categories.iter().any(|c| c == category) {
            found.push(finding(markup(message, &[("$1", Substitution::Text(&search))])));
        }
    }
    if external_links == 0 && !is_disambig {
        found.push(finding(config.filled("no_external_links", &[("$1", &search)])));
    }
    if categories.is_empty() {
        found.push(finding(config.text("no_categories")));
    }
    if !is_disambig && backlinks.is_empty() {
        let search_page =
            format!("{}?search={}&fulltext=Search", wiki.page_url("Special:Search"), percent_encode(title));
        found.push(finding(config.filled("orphan", &[("$1", &search_page)])));
    }
    if links.is_empty() {
        found.push(finding(config.text("dead_end")));
    } else if length > 1000 && length / (links.len() as u64) > 150 {
        found.push(finding(config.text("few_links")));
    }
    if language_links == 0 {
        let search = format!("{search}+site:wikipedia.org");
        found.push(finding(config.filled("langlinks", &[("$1", &search)])));
    }
    if !is_disambig {
        let is_stub =
            config.stub.as_ref().is_some_and(|stub| categories.iter().any(|category| stub.is_match(category)));
        if is_stub {
            found.push(finding(config.filled("stub", &[("$1", &search)])));
        } else if length < 1000 {
            found.push(finding(config.filled("short", &[("$1", &search)])));
        } else if length > 50_000 && !starts_with_list_marker(config, title) {
            found.push(finding(config.text("long")));
        }
    }
    append_new_articles(config, wiki, links, existing, found);
}

fn append_new_articles(
    config: &Config,
    wiki: &Wiki,
    links: &[String],
    existing: &HashSet<String>,
    found: &mut Vec<String>,
) {
    let missing: Vec<&String> = links.iter().filter(|link| !existing.contains(*link)).collect();
    if missing.is_empty() || missing.len() >= MAX_NEW_ARTICLES {
        return;
    }
    let label = config.plural("startarticles_sg", "startarticles_pl", missing.len());
    let links: Vec<String> = missing.iter().map(|link| edit_link(wiki, link)).collect();
    found.push(finding(format!("{label} {}.", links.join(", "))));
}

/// A Wiktionary page with the same title suggests a missing link to it.
async fn check_wiktionary(
    state: &AppState,
    config: &Config,
    language: &str,
    title: &str,
    templates: &[String],
    is_disambig: bool,
    found: &mut Vec<String>,
) {
    if templates.iter().any(|template| matches!(template.as_str(), "Wiktionary" | "Wiktionarypar")) {
        return;
    }
    let Ok(wiktionary) = Wiki::new(language, "wiktionary") else { return };
    if !wiktionary.host().ends_with(".wiktionary.org") {
        return;
    }
    let short = match is_disambig {
        true => title.split_once(" (").map_or(title, |(short, _)| short),
        false => title,
    };
    let api = match state.api(&wiktionary).await {
        Ok(api) => api,
        Err(e) => {
            tracing::debug!("Wiktionary check: {e:#}");
            return;
        }
    };
    let params = api.params_into(&[("action", "query"), ("titles", short), ("prop", "info")]);
    let json = match api.get_query_api_json(&params).await {
        Ok(json) => json,
        Err(e) => {
            tracing::debug!("Wiktionary check: {e:#}");
            return;
        }
    };
    let exists = json["query"]["pages"].as_array().and_then(|pages| pages.first()).is_some_and(|page| {
        page.get("missing").is_none() && page.get("invalid").is_none() && page.get("pageid").is_some()
    });
    if !exists {
        return;
    }
    let label = config.text("wiktionary_pendant");
    let link = format!(
        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
        escape::html(&wiktionary.page_url(short)),
        escape::html(&label)
    );
    let message = markup(
        config.raw("no_wiktionary_link"),
        &[("$1", Substitution::Text(short)), ("$2", Substitution::Html(&link))],
    );
    found.push(finding(message));
}

// Word heuristics ------------------------------------------------------------

fn first_word(text: &str) -> String {
    text.split_whitespace().next().unwrap_or_default().to_lowercase()
}

fn last_word(text: &str) -> String {
    text.split_whitespace().last().unwrap_or_default().to_lowercase()
}

/// `word`, or its singular for the plural forms the heuristics see.
fn singular(word: &str) -> String {
    match word.strip_suffix("ies") {
        Some(stem) => format!("{stem}y"),
        None => word.strip_suffix('s').unwrap_or(word).to_string(),
    }
}

fn has_infobox(templates: &[String]) -> bool {
    templates.iter().any(|template| matches!(first_word(template).as_str(), "infobox" | "taxobox"))
}

fn starts_with_list_marker(config: &Config, title: &str) -> bool {
    let marker = config.misc("listmarkers");
    !marker.is_empty() && title.to_lowercase().starts_with(&marker.to_lowercase())
}

// Links ----------------------------------------------------------------------

fn commons_image_search(title: &str) -> String {
    format!(
        "https://commons.wikimedia.org/w/index.php?search={}&title=Special:MediaSearch&type=image",
        percent_encode(title)
    )
}

fn google_search(wiki: &Wiki, title: &str) -> String {
    format!(
        "https://www.google.com/search?q=-site:static.wikipedia.org+-site:{}+{}",
        wiki.host(),
        percent_encode(title)
    )
}

fn article_link(wiki: &Wiki, title: &str) -> String {
    link(&wiki.page_url(title), title)
}

fn edit_link(wiki: &Wiki, title: &str) -> String {
    link(&format!("{}?action=edit&title={}", wiki.index_url(), wiki_urlencode(title)), title)
}

fn link(url: &str, text: &str) -> String {
    format!(
        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
        escape::html(url),
        escape::html(&display_title(text))
    )
}

/// A message beginning with `!` is emphasized, as in the original.
fn finding(html: String) -> String {
    match html.strip_prefix('!') {
        Some(rest) => format!("<strong>{rest}</strong>"),
        None => html,
    }
}

// The configuration page -----------------------------------------------------

#[derive(Debug, Default)]
struct Config {
    messages: HashMap<String, String>,
    misc: HashMap<String, String>,
    multi: HashMap<String, Vec<String>>,
    tags: Vec<(String, String)>,
    stub: Option<Regex>,
}

/// A placeholder value: escaped text, or HTML this code built itself.
enum Substitution<'a> {
    Text(&'a str),
    Html(&'a str),
}

impl Config {
    /// The page's `; key : value` and `* key | value` lines. Section headings
    /// are ignored: the original only knew their English names, so the Finnish
    /// page's `Käyttöliittymä` section was silently dropped.
    fn parse(text: &str) -> Self {
        let mut config = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(';') {
                let Some((key, value)) = rest.split_once(':') else { continue };
                let (key, value) = (key.trim(), value.trim());
                if key.is_empty() {
                    continue;
                }
                if MULTI_KEYS.contains(&key) {
                    let values = match value.is_empty() {
                        true => vec![],
                        false => value
                            .split('|')
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                            .collect(),
                    };
                    config.multi.insert(key.to_string(), values);
                } else if MISC_KEYS.contains(&key) {
                    config.misc.insert(key.to_string(), value.to_string());
                } else {
                    config.messages.insert(key.to_string(), value.to_string());
                }
            } else if let Some(rest) = line.strip_prefix('*') {
                let Some((key, value)) = rest.split_once('|') else { continue };
                let (key, value) = (key.trim(), value.trim());
                if !key.is_empty() && !value.is_empty() {
                    config.tags.push((key.to_string(), value.to_string()));
                }
            }
        }
        config.stub = config.misc.get("stubpattern").and_then(|pattern| parse_pattern(pattern));
        config
    }

    fn raw(&self, key: &str) -> &str {
        self.messages.get(key).map_or("", String::as_str)
    }

    fn misc(&self, key: &str) -> &str {
        self.misc.get(key).map_or("", String::as_str)
    }

    fn multi(&self, key: &str) -> &[String] {
        self.multi.get(key).map_or(&[], Vec::as_slice)
    }

    /// An interface text, as safe HTML.
    fn text(&self, key: &str) -> String {
        markup(self.raw(key), &[])
    }

    /// As [`text`](Self::text), with `$1`, … replaced by escaped values.
    fn filled(&self, key: &str, values: &[(&str, &str)]) -> String {
        let values: Vec<(&str, Substitution)> =
            values.iter().map(|(key, value)| (*key, Substitution::Text(value))).collect();
        markup(self.raw(key), &values)
    }

    fn plural(&self, singular: &str, plural: &str, count: usize) -> String {
        self.text(match count == 1 {
            true => singular,
            false => plural,
        })
    }

    fn label(&self, key: &str, fallback: &str) -> String {
        let text = self.text(key);
        match text.is_empty() {
            true => fallback.to_string(),
            false => text,
        }
    }
}

impl From<&Config> for Labels {
    fn from(config: &Config) -> Self {
        Self {
            analysis: config.label("analysis", "Analysis"),
            view: config.label("view", "View"),
            edit: config.label("edit", "edit"),
            things_todo: config.label("things_todo", "Things to do"),
            nothing_wrong: config.label("nothing_wrong", "There seems to be nothing wrong with this article."),
            supported_languages: config.label("supported_languages", "Supported languages"),
            edit_interface_text: config.label("edit_interface_text", "Edit interface text"),
        }
    }
}

/// A PCRE pattern like `/\bstub\b/i`; an invalid pattern is ignored.
fn parse_pattern(raw: &str) -> Option<Regex> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (body, flags) = match raw.strip_prefix('/') {
        Some(inner) => {
            let end = inner.rfind('/')?;
            (inner.get(..end)?, inner.get(end + 1..)?)
        }
        None => (raw, ""),
    };
    let mut builder = RegexBuilder::new(body);
    builder.case_insensitive(flags.contains('i'));
    builder.multi_line(flags.contains('m'));
    builder.dot_matches_new_line(flags.contains('s'));
    builder.build().ok()
}

/// Configuration markup to safe HTML: text is escaped, entities decoded, and
/// only links, emphasis, line breaks, and `<nowiki>` (whose tags go) remain.
fn markup(raw: &str, values: &[(&str, Substitution<'_>)]) -> String {
    let raw = escape::unescape_html(raw);
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_ref();
    while let Some(start) = rest.find('<') {
        push_text(&mut out, rest.get(..start).unwrap_or_default(), values);
        let after = rest.get(start..).unwrap_or_default();
        let is_tag = after
            .get(1..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!');
        if !is_tag {
            push_text(&mut out, "<", values);
            rest = after.get(1..).unwrap_or_default();
            continue;
        }
        let Some(end) = after.find('>') else {
            push_text(&mut out, after, values);
            rest = "";
            break;
        };
        push_tag(&mut out, after.get(1..end).unwrap_or_default(), values);
        rest = after.get(end + 1..).unwrap_or_default();
    }
    push_text(&mut out, rest, values);
    out
}

fn push_text(out: &mut String, text: &str, values: &[(&str, Substitution<'_>)]) {
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        out.push_str(&escape::html(rest.get(..pos).unwrap_or_default()));
        let after = rest.get(pos..).unwrap_or_default();
        match values.iter().find(|(key, _)| after.starts_with(*key)) {
            Some((key, value)) => {
                match value {
                    Substitution::Text(text) => out.push_str(&escape::html(text)),
                    Substitution::Html(html) => out.push_str(html),
                }
                rest = after.get(key.len()..).unwrap_or_default();
            }
            None => {
                out.push('$');
                rest = after.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(&escape::html(rest));
}

fn push_tag(out: &mut String, tag: &str, values: &[(&str, Substitution<'_>)]) {
    let closing = tag.starts_with('/');
    let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    match (name.as_str(), closing) {
        ("a", false) => {
            let Some(href) = href(tag) else { return };
            let url = substitute_attribute(href, values);
            if let Some(url) = safe_url(&url) {
                out.push_str("<a href=\"");
                out.push_str(&escape::html(url));
                out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
            }
        }
        ("a", true) => out.push_str("</a>"),
        ("i", false) => out.push_str("<i>"),
        ("i", true) => out.push_str("</i>"),
        ("b", false) => out.push_str("<b>"),
        ("b", true) => out.push_str("</b>"),
        ("br", false) => out.push_str("<br>"),
        // `<nowiki>` and unknown tags go; their content stays as text.
        _ => {}
    }
}

fn href(tag: &str) -> Option<&str> {
    let lower = tag.to_ascii_lowercase();
    let start = lower.find("href")?;
    let rest = tag.get(start + 4..)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    match rest.chars().next()? {
        quote @ ('\'' | '"') => rest.get(1..)?.split(quote).next(),
        _ => rest.split_whitespace().next(),
    }
}

fn substitute_attribute(text: &str, values: &[(&str, Substitution<'_>)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        out.push_str(rest.get(..pos).unwrap_or_default());
        let after = rest.get(pos..).unwrap_or_default();
        match values.iter().find(|(key, _)| after.starts_with(*key)) {
            Some((key, Substitution::Text(value))) => {
                out.push_str(value);
                rest = after.get(key.len()..).unwrap_or_default();
            }
            Some((key, Substitution::Html(_))) => rest = after.get(key.len()..).unwrap_or_default(),
            None => {
                out.push('$');
                rest = after.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

fn safe_url(url: &str) -> Option<&str> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    match lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("//") {
        true => Some(url),
        false => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
=== Käyttöliittymä ===
; analysis : Analysis
; images_sg : image
; images_pl : images
; ofwhich_exists_sg : , of which $1 exist
; skip_cat : This is in <i>$1</i>, skipping.

=== Misc ===
; disamb_key : Disambiguation
; disamb_template : Disambig
; listmarkers : List
; stubpattern : /\bstub\b/i

=== Multi ===
; geo_indicators : mountain|city
; coord_templates : coord
; bio_indicators : births|deaths
; skipcat :

=== Tags ===
* All articles lacking sources | Tagged for lacking sources. <a target='_blank' href='$1'>Find some</a>!
* All pages needing cleanup | Needs cleanup.
"#;

    #[test]
    fn config_is_section_agnostic() {
        // The Finnish page's `Käyttöliittymä` heading: the original dropped it.
        let config = Config::parse(PAGE);
        assert_eq!(config.text("analysis"), "Analysis");
        assert_eq!(config.misc("disamb_key"), "Disambiguation");
        assert_eq!(config.multi("geo_indicators").join("|"), "mountain|city");
        assert_eq!(config.tags.len(), 2);
        assert!(config.stub.as_ref().is_some_and(|stub| stub.is_match("Stub articles")));
    }

    #[test]
    fn messages_are_safe_html() {
        assert_eq!(
            markup("This <i>must</i> be <a href='$1'>found</a>!", &[("$1", Substitution::Text("https://example.org"))]),
            "This <i>must</i> be <a href=\"https://example.org\" target=\"_blank\" rel=\"noopener noreferrer\">found</a>!"
        );
        assert_eq!(markup("&#123;&#123;Disambig}}", &[]), "{{Disambig}}");
        assert_eq!(markup("a < b > c", &[]), "a &lt; b &gt; c");
        assert_eq!(markup("<nowiki>{{x}}</nowiki>", &[]), "{{x}}");
        assert_eq!(markup("<script>alert(1)</script>", &[]), "alert(1)");
        assert_eq!(markup("<a href='javascript:alert(1)'>x</a>", &[]), "x</a>");
        assert_eq!(markup("<b onclick='x'>x</b>", &[]), "<b>x</b>");
    }

    #[test]
    fn filled_and_bold() {
        let config = Config::parse(PAGE);
        assert_eq!(config.filled("skip_cat", &[("$1", "Foo")]), "This is in <i>Foo</i>, skipping.");
        assert_eq!(finding("!Needs work".to_string()), "<strong>Needs work</strong>");
        assert_eq!(config.filled("ofwhich_exists_sg", &[("$1", "2")]), ", of which 2 exist");
    }

    #[test]
    fn words_and_plurals() {
        assert_eq!(first_word("Geboren 1952"), "geboren");
        assert_eq!(last_word("1952 births"), "births");
        assert_eq!(singular("cities"), "city");
        assert_eq!(singular("mountains"), "mountain");
        assert_eq!(singular("city"), "city");
    }

    #[test]
    fn property_titles() {
        let page = serde_json::json!({
            "images": [{"ns": 6, "title": "File:A.jpg"}],
            "templates": [{"ns": 10, "title": "Template:T"}],
            "categories": [{"ns": 14, "title": "Category:C"}],
            "links": [{"ns": 0, "title": "Star Trek: Voyager"}, {"ns": 1, "title": "Talk:X"}],
        });
        assert_eq!(entry_titles(&page, "images", NS_FILE, true), vec!["A.jpg"]);
        assert_eq!(entry_titles(&page, "templates", NS_TEMPLATE, true), vec!["T"]);
        assert_eq!(entry_titles(&page, "categories", NS_CATEGORY, true), vec!["C"]);
        // Article titles keep their colons.
        assert_eq!(entry_titles(&page, "links", 0, false), vec!["Star Trek: Voyager"]);
    }

    #[tokio::test]
    async fn form_renders_without_a_config_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let page = crate::web::test_support::get(dir.path(), "/wiki_todo/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Wiki ToDo</h1>"));
        assert!(page.body.contains("name=\"language\""));
    }
}

//! `icommons` (formerly its own tool): Commons for small screens, from the
//! iPhone's early days. The picture of the day, search, file descriptions,
//! categories, and random files or categories; in the original look.

use super::commonsapi;
use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params};
use crate::wiki::{Wiki, encode_component, encode_title, is_language_code, upper_first};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const NS_FILE: &str = "6";
const NS_CATEGORY: &str = "14";
/// As the API gives them to tools without the bot right.
const MAX_MEMBERS: &str = "500";
const SEARCH_LIMIT: &str = "10";
/// Titles per `imageinfo` request.
const SIZE_BATCH: usize = 50;
/// Thumbnail boxes: the large picture, search results, category grid.
const LARGE: (f64, f64) = (320.0, 356.0 * 2.0 / 3.0);
const SMALL: (f64, f64) = (150.0, 100.0);
const GRID: (f64, f64) = (320.0 / 4.0 - 2.0, 320.0 / 4.0 - 2.0);
const GRID_COLUMNS: usize = 4;

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));
static NOINCLUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<noinclude.*</noinclude>").expect("valid regex"));

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    mode: String,
    search: String,
    image: String,
    category: String,
    language: String,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            mode: "main".to_string(),
            search: String::new(),
            image: String::new(),
            category: String::new(),
            language: "en".to_string(),
        }
    }
}

struct Thumb {
    src: String,
    width: u32,
    ogg: bool,
    /// To the file's description here.
    link: Option<String>,
}

impl Thumb {
    /// The image, linked if it has a link; Ogg videos get a width for the player.
    fn html(&self) -> String {
        let escape = |s: &str| wikimisc::escape::html(s).into_owned();
        let width = match self.ogg {
            true => format!(" width='{}'", self.width.saturating_sub(4)),
            false => String::new(),
        };
        let img = format!(
            "<img border='0' src=\"{}\" style='display: block; margin-left: auto; margin-right: auto'{width} />",
            escape(&self.src)
        );
        match &self.link {
            Some(link) => format!("<a href=\"{}\">{img}</a>", escape(link)),
            None => img,
        }
    }
}

struct Hit {
    thumb: Thumb,
    name: String,
    file_url: String,
}

/// A name, and its page here.
struct Link {
    name: String,
    url: String,
}

enum Body {
    Main {
        thumb: Option<Thumb>,
        /// HTML.
        description: String,
    },
    Search(Vec<Hit>),
    Description {
        name: String,
        thumb: Option<Thumb>,
        /// Heading, HTML.
        rows: Vec<(&'static str, String)>,
    },
    Category {
        name: String,
        parents: Vec<Link>,
        subcategories: Vec<Link>,
        rows: Vec<Vec<Option<Thumb>>>,
        has_video: bool,
        truncated: bool,
    },
}

#[derive(Template)]
#[template(path = "tools/icommons.html")]
struct Page {
    body: Body,
    search: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let http = &state.http;
    let search = request.search.trim().to_string();
    let body = match request.mode.as_str() {
        "search" if !search.is_empty() => search_results(http, &search).await?,
        "desc" => description(http, &request.image).await?,
        "category" if !request.category.trim().is_empty() => category(http, &request.category).await?,
        "random_image" => match random_page(http, NS_FILE).await? {
            Some(image) => description(http, &image).await?,
            None => main_page(http, &request.language).await?,
        },
        "random_category" => match random_page(http, NS_CATEGORY).await? {
            Some(name) => category(http, &name).await?,
            None => main_page(http, &request.language).await?,
        },
        _ => main_page(http, &request.language).await?,
    };
    let page = Page { body, search };
    page.render().map(Html).map_err(|e| AppError::Internal(format!("template: {e}")))
}

async fn main_page(http: &reqwest::Client, language: &str) -> anyhow::Result<Body> {
    let commons = Wiki::commons();
    let language = if is_language_code(language) { language } else { "en" };
    let date = Utc::now().format("%Y-%m-%d");
    let image = mediawiki::page_wikitext(http, &commons, &format!("Template:Potd/{date}"))
        .await?
        .map(|text| NOINCLUDE.replace_all(&text, "").trim().to_string())
        .unwrap_or_default();
    let html = mediawiki::page_html(http, &commons, &format!("Template:Potd/{date} ({language})")).await?;
    let description = external_links(&COMMENT.replace_all(&html.unwrap_or_default(), "")).trim().to_string();
    let thumb = match image.is_empty() {
        true => None,
        false => thumbs(http, std::slice::from_ref(&image), LARGE, true).await?.remove(&image),
    };
    let description = if description.is_empty() { wikimisc::escape::html(&image).into_owned() } else { description };
    Ok(Body::Main { thumb, description })
}

async fn search_results(http: &reqwest::Client, search: &str) -> anyhow::Result<Body> {
    let params = [("action", "opensearch"), ("namespace", NS_FILE), ("limit", SEARCH_LIMIT), ("search", search)];
    let json = mediawiki::api(http, &Wiki::commons(), &params).await?;
    let names: Vec<String> =
        json[1].as_array().into_iter().flatten().filter_map(|t| t.as_str()).map(without_namespace).collect();
    let mut thumbs = thumbs(http, &names, SMALL, true).await?;
    let hits = names
        .into_iter()
        .filter_map(|name| {
            let thumb = thumbs.remove(&name)?;
            let file_url = Wiki::commons().page_url(&format!("File:{name}"));
            Some(Hit { thumb, name: display_name(&name), file_url })
        })
        .collect();
    Ok(Body::Search(hits))
}

async fn description(http: &reqwest::Client, image: &str) -> anyhow::Result<Body> {
    let image = upper_first(image.trim().replace('_', " ").trim());
    let name = display_name(&image);
    let thumb = thumbs(http, std::slice::from_ref(&image), LARGE, false).await?.remove(&image);
    let Some(file) = commonsapi::summary(http, &image).await? else {
        return Ok(Body::Description { name, thumb, rows: vec![("Commons", "No such file.".to_string())] });
    };
    let html = |text: &str| external_links(&COMMENT.replace_all(text, ""));
    let escape = |text: &str| wikimisc::escape::html(text).into_owned();
    let extension = image.rsplit_once('.').map_or("", |(_, e)| e).to_uppercase();
    let file_type = match extension.as_str() {
        "JPG" => "JPEG".to_string(),
        "OGG" => "<span style='color:red'>OGG</span>".to_string(),
        other => escape(other),
    };
    let mut rows = vec![];
    let mut add_if = |heading, value: String| {
        if !value.is_empty() {
            rows.push((heading, value));
        }
    };
    add_if("Description", html(&file.description));
    add_if("Author", html(&file.author));
    add_if("Source", html(&file.source));
    if let Some((lat, lon)) = &file.location {
        let (lat, lon) = (escape(lat), escape(lon));
        add_if(
            "Location",
            format!("<a class='external' href='//maps.google.com/maps?q={lat},{lon}'>{lat} , {lon}</a>"),
        );
    }
    rows.push(("Type", file_type));
    rows.push((
        "Size",
        format!("{}&times;{} ({} bytes)", escape(&file.width), escape(&file.height), escape(&file.size)),
    ));
    rows.push(("Uploaded", format!("{} by<br/>{}", escape(&file.upload_date), escape(&file.uploader))));
    let page = Wiki::commons().page_url(&format!("File:{image}"));
    rows.push(("Commons", format!("<a class='external' href='{}'>Description page</a>", escape(&page))));
    let mut add_if = |heading, value: String| {
        if !value.is_empty() {
            rows.push((heading, value));
        }
    };
    add_if("License(s)", escape(&file.licenses.join(", ")));
    add_if("Permission", html(&file.permission));
    let categories: Vec<String> =
        file.categories.iter().map(|c| format!("<a href=\"{}\">{}</a>", escape(&category_url(c)), escape(c))).collect();
    add_if("Categories", categories.join(", "));
    Ok(Body::Description { name, thumb, rows })
}

async fn category(http: &reqwest::Client, name: &str) -> anyhow::Result<Body> {
    let name = name.trim().replace('_', " ");
    let commons = Wiki::commons();
    let title = format!("Category:{name}");
    let params = [("action", "query"), ("prop", "categories"), ("titles", title.as_str()), ("cllimit", "max")];
    let json = mediawiki::api(http, &commons, &params).await?;
    let parents = json["query"]["pages"][0]["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["title"].as_str())
        .map(|t| category_link(&without_namespace(t)))
        .collect();
    let subcategories = members(http, &title, NS_CATEGORY).await?.iter().map(|c| category_link(c)).collect();
    let images = members(http, &title, NS_FILE).await?;
    let truncated = images.len().to_string() == MAX_MEMBERS;
    let mut thumbs = thumbs(http, &images, GRID, true).await?;
    let has_video = images.iter().any(|i| is_ogg(i));
    let cells: Vec<Option<Thumb>> = images.iter().map(|i| thumbs.remove(i)).collect();
    let rows = cells.into_iter().fold(Vec::<Vec<Option<Thumb>>>::new(), |mut rows, cell| {
        match rows.last_mut().filter(|r| r.len() < GRID_COLUMNS) {
            Some(row) => row.push(cell),
            None => rows.push(vec![cell]),
        }
        rows
    });
    Ok(Body::Category { name, parents, subcategories, rows, has_video, truncated })
}

/// Pages of a namespace in a category, without namespace; the first 500.
async fn members(http: &reqwest::Client, category: &str, namespace: &str) -> anyhow::Result<Vec<String>> {
    let params = [
        ("action", "query"),
        ("list", "categorymembers"),
        ("cmtitle", category),
        ("cmnamespace", namespace),
        ("cmlimit", MAX_MEMBERS),
    ];
    let json = mediawiki::api(http, &Wiki::commons(), &params).await?;
    Ok(json["query"]["categorymembers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["title"].as_str())
        .map(without_namespace)
        .collect())
}

/// A random page of a namespace, without namespace.
async fn random_page(http: &reqwest::Client, namespace: &str) -> anyhow::Result<Option<String>> {
    let params = [("action", "query"), ("list", "random"), ("rnnamespace", namespace), ("rnlimit", "1")];
    let json = mediawiki::api(http, &Wiki::commons(), &params).await?;
    Ok(json["query"]["random"][0]["title"].as_str().map(without_namespace))
}

/// Thumbnails fitting `(width, height)`, by file name; files the wiki does
/// not have, or without a size (audio), get none.
async fn thumbs(
    http: &reqwest::Client,
    images: &[String],
    (max_width, max_height): (f64, f64),
    link: bool,
) -> anyhow::Result<HashMap<String, Thumb>> {
    let mut thumbs = HashMap::new();
    for batch in images.chunks(SIZE_BATCH) {
        let titles = batch.iter().map(|i| format!("File:{i}")).collect::<Vec<_>>().join("|");
        let params = [("action", "query"), ("prop", "imageinfo"), ("iiprop", "size"), ("titles", titles.as_str())];
        let json = mediawiki::api(http, &Wiki::commons(), &params).await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            let (Some(title), info) = (page["title"].as_str(), &page["imageinfo"][0]) else { continue };
            let (Some(width), Some(height)) = (info["width"].as_f64(), info["height"].as_f64()) else { continue };
            if width == 0.0 || height == 0.0 {
                continue;
            }
            let name = without_namespace(title);
            let fitted = fit_width(width, height, max_width, max_height);
            let mut src = format!("//commons.wikimedia.org/wiki/Special:Redirect/file/{}", encode_title(&name));
            if fitted <= width {
                src.push_str(&format!("?width={fitted}"));
            }
            let link = link.then(|| format!("?mode=desc&image={}", encode_component(&name)));
            let thumb = Thumb { src, width: fitted as u32, ogg: is_ogg(&name), link };
            thumbs.insert(name, thumb);
        }
    }
    // Under the names asked for, as the API may normalise them.
    Ok(images
        .iter()
        .filter_map(|i| thumbs.remove(&upper_first(&i.replace('_', " "))).map(|t| (i.clone(), t)))
        .collect())
}

/// The width of `width`×`height` scaled to fit the box, rounded down.
fn fit_width(width: f64, height: f64, max_width: f64, max_height: f64) -> f64 {
    let fitted = if max_width / width * height > max_height { width / height * max_height } else { max_width };
    fitted.floor()
}

fn without_namespace(title: &str) -> String {
    title.split_once(':').map_or(title, |(_, name)| name).to_string()
}

/// A file name without extension, for display.
fn display_name(image: &str) -> String {
    upper_first(image.rsplit_once('.').map_or(image, |(name, _)| name).trim())
}

fn is_ogg(image: &str) -> bool {
    image.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("ogg"))
}

fn category_url(name: &str) -> String {
    format!("?mode=category&category={}", encode_component(name))
}

fn category_link(name: &str) -> Link {
    Link { name: name.to_string(), url: category_url(name) }
}

/// Links in Commons' HTML open on Commons, marked as such.
fn external_links(html: &str) -> String {
    html.replace("<a ", "<a class=\"external\" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn helpers() {
        assert_eq!(fit_width(4000.0, 3000.0, LARGE.0, LARGE.1), 316.0);
        assert_eq!(fit_width(1000.0, 200.0, LARGE.0, LARGE.1), 320.0);
        assert_eq!(fit_width(100.0, 100.0, GRID.0, GRID.1), 78.0);
        assert_eq!(display_name("sunset over Leith.JPG"), "Sunset over Leith");
        assert_eq!(without_namespace("File:A:b.jpg"), "A:b.jpg");
        assert!(is_ogg("Video.OGG") && !is_ogg("Ogg.png"));
        assert_eq!(category_url("Lakes of Scotland"), "?mode=category&category=Lakes%20of%20Scotland");
        assert_eq!(external_links("<a href=\"x\">"), "<a class=\"external\" href=\"x\">");
    }

    #[test]
    fn page_markup() {
        let thumb =
            Thumb { src: "//c/x.ogg?width=78".into(), width: 78, ogg: true, link: Some("?mode=desc&image=x".into()) };
        let body = Body::Category {
            name: "A <b>".into(),
            parents: vec![category_link("P")],
            subcategories: vec![],
            rows: vec![vec![Some(thumb), None]],
            has_video: true,
            truncated: false,
        };
        let html = Page { body, search: "\"><x".into() }.render().unwrap();
        assert!(html.contains("<div class='sect1'><b>A &#60;b&#62;</b></div>"));
        assert!(html.contains("<a href=\"?mode=category&#38;category=P\">P</a>"));
        assert!(html.contains("style=\"border:2px solid red\""));
        assert!(html.contains(" width='74'"));
        assert!(html.contains("value=\"&#34;&#62;&#60;x\""));
        assert!(html.contains("The red border indicates Ogg Theora videos"));
    }

    #[tokio::test]
    async fn old_address() {
        let dir = tempfile::tempdir().unwrap();
        let old = get(dir.path(), "/icommons.php?mode=desc&image=X.jpg").await;
        assert_eq!(old.headers["location"], "/icommons/?mode=desc&image=X.jpg");
    }
}

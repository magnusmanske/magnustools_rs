//! `icommons` (formerly its own tool): Commons for small screens, from the
//! iPhone's early days. The picture of the day, search, file descriptions,
//! categories, and random files or categories; in the original look.

use super::commonsapi;
use crate::mediawiki::{self, strip_namespace};
use crate::state::AppState;
use crate::web::{AppError, Params};
use crate::wiki::{Wiki, encode_component, encode_title, is_language_code, upper_first};
use askama::Template;
use axum::extract::State;
use axum::http::HeaderName;
use axum::response::{Html, IntoResponse};
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const NS_FILE: &str = "6";
const NS_CATEGORY: &str = "14";
/// As the API gives them to tools without the bot right.
const MAX_MEMBERS: usize = 500;
const SEARCH_LIMIT: &str = "10";
/// Titles per `imageinfo` request.
const SIZE_BATCH: usize = 50;
/// Thumbnail boxes: the large picture, search results, category grid.
const LARGE: (f64, f64) = (320.0, 356.0 * 2.0 / 3.0);
const SMALL: (f64, f64) = (150.0, 100.0);
const GRID: (f64, f64) = (320.0 / 4.0 - 2.0, 320.0 / 4.0 - 2.0);
const GRID_COLUMNS: usize = 4;
/// A copy of Commons: nothing for search engines to index or crawl.
const ROBOTS: (HeaderName, &str) = (HeaderName::from_static("x-robots-tag"), "noindex, nofollow");

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));

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
    /// With a link to the description here, if `link`.
    fn new(image: &str, src: String, width: u32, link: bool) -> Self {
        let link = link.then(|| format!("?mode=desc&image={}", encode_component(image)));
        Self { src, width, ogg: is_ogg(image), link }
    }

    /// The image, linked if it has a link; Ogg videos get a width for the player.
    fn html(&self) -> String {
        let escape = |s: &str| wikimisc::escape::html(s).into_owned();
        let width = match self.ogg {
            true => format!(" width='{}'", self.width.saturating_sub(4)),
            false => String::new(),
        };
        let img = format!(
            "<img border='0' src=\"{}\" loading='lazy' style='display: block; margin-left: auto; margin-right: auto'{width} />",
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
) -> Result<impl IntoResponse, AppError> {
    let http = &state.http;
    let search = request.search.trim().to_string();
    let body = match request.mode.as_str() {
        "search" if !search.is_empty() => search_results(http, &search).await?,
        "desc" if !request.image.trim().is_empty() => description(http, &request.image).await?,
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
    let html = page.render().map_err(|e| AppError::Internal(format!("template: {e}")))?;
    Ok(([ROBOTS], Html(html)))
}

async fn main_page(http: &reqwest::Client, language: &str) -> anyhow::Result<Body> {
    let commons = Wiki::commons();
    let language = if is_language_code(language) { language } else { "en" };
    // Transcluded, the day's templates give the file name and the caption.
    let date = Utc::now().format("%Y-%m-%d");
    let image = transclude_if_exists(&format!("Potd/{date}"));
    let caption = transclude_if_exists(&format!("Potd/{date} ({language})"));
    let (image, html) = tokio::try_join!(
        mediawiki::expand_templates(http, &commons, &image),
        mediawiki::parse_to_html(http, &commons, &caption),
    )?;
    let image = image.trim().to_string();
    let description = external_links(&COMMENT.replace_all(&html, "")).trim().to_string();
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
    let image = image.trim();
    let image = upper_first(strip_namespace(image, &["file", "image"]).unwrap_or(image).replace('_', " ").trim());
    let name = display_name(&image);
    let Some(file) = commonsapi::summary(http, &image).await? else {
        return Ok(Body::Description { name, thumb: None, rows: vec![("Commons", "No such file.".to_string())] });
    };
    let size = |s: &str| s.parse().unwrap_or_default();
    let thumb = redirect_thumb(&image, size(&file.width), size(&file.height), LARGE);
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
    let (json, subcategories, images) = tokio::try_join!(
        mediawiki::api(http, &commons, &params),
        members(http, &title, NS_CATEGORY),
        members(http, &title, NS_FILE),
    )?;
    let parents = json["query"]["pages"][0]["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["title"].as_str())
        .map(|t| category_link(&without_namespace(t)))
        .collect();
    let subcategories = subcategories.iter().map(|c| category_link(c)).collect();
    let truncated = images.len() == MAX_MEMBERS;
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
    let limit = MAX_MEMBERS.to_string();
    let params = [
        ("action", "query"),
        ("list", "categorymembers"),
        ("cmtitle", category),
        ("cmnamespace", namespace),
        ("cmlimit", limit.as_str()),
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

/// Thumbnails fitting `frame`, by file name; files the wiki does not
/// have, or without a size (audio), get none.
async fn thumbs(
    http: &reqwest::Client,
    images: &[String],
    frame: (f64, f64),
    link: bool,
) -> anyhow::Result<HashMap<String, Thumb>> {
    let mut requests = tokio::task::JoinSet::new();
    for batch in images.chunks(SIZE_BATCH) {
        let (http, batch) = (http.clone(), batch.to_vec());
        requests.spawn(async move { thumb_urls(&http, &batch, frame).await });
    }
    let mut thumbs = HashMap::new();
    while let Some(urls) = requests.join_next().await {
        for (name, url, width) in urls?? {
            let thumb = Thumb::new(&name, url, width, link);
            thumbs.insert(name, thumb);
        }
    }
    Ok(thumbs)
}

/// The thumbnail URLs and widths of files that have a size, under the names
/// asked for: the wiki scales them, so the browser needs no redirect.
/// POSTed, as 50 long titles can be too much for a URL.
async fn thumb_urls(
    http: &reqwest::Client,
    images: &[String],
    (max_width, max_height): (f64, f64),
) -> anyhow::Result<Vec<(String, String, u32)>> {
    let titles = images.iter().map(|i| format!("File:{i}")).collect::<Vec<_>>().join("|");
    let (width, height) = (max_width.floor().to_string(), max_height.floor().to_string());
    let params = [
        ("action", "query"),
        ("prop", "imageinfo"),
        ("iiprop", "size|url"),
        ("iiurlwidth", width.as_str()),
        ("iiurlheight", height.as_str()),
        ("titles", titles.as_str()),
    ];
    let json = mediawiki::api_post(http, &Wiki::commons(), &params).await?;
    // The wiki's form of a title, and the form asked for.
    let asked: HashMap<&str, &str> = json["query"]["normalized"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["to"].as_str()?, n["from"].as_str()?)))
        .collect();
    Ok(json["query"]["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let title = page["title"].as_str()?;
            let info = &page["imageinfo"][0];
            // Without a size (audio), the "thumbnail" is a file type icon.
            info["width"].as_u64().filter(|w| *w > 0)?;
            info["height"].as_u64().filter(|h| *h > 0)?;
            let url = info["thumburl"].as_str()?.to_string();
            let width = u32::try_from(info["thumbwidth"].as_u64()?).ok()?;
            Some((without_namespace(asked.get(title).copied().unwrap_or(title)), url, width))
        })
        .collect())
}

/// A thumbnail of `image` fitting `frame`, by way of `Special:Redirect`,
/// for a file whose size is known; none without a size.
fn redirect_thumb(image: &str, width: f64, height: f64, (max_width, max_height): (f64, f64)) -> Option<Thumb> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let fitted = fit_width(width, height, max_width, max_height);
    let mut src = format!("//commons.wikimedia.org/wiki/Special:Redirect/file/{}", encode_title(image));
    if fitted <= width {
        src.push_str(&format!("?width={fitted}"));
    }
    Some(Thumb::new(image, src, fitted as u32, false))
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

/// `{{template}}`, or nothing if there is no such template (rather than a red link).
fn transclude_if_exists(template: &str) -> String {
    format!("{{{{#ifexist:Template:{template}|{{{{{template}}}}}}}}}")
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
        assert_eq!(
            transclude_if_exists("Potd/2026-10-27"),
            "{{#ifexist:Template:Potd/2026-10-27|{{Potd/2026-10-27}}}}"
        );
        assert_eq!(external_links("<a href=\"x\">"), "<a class=\"external\" href=\"x\">");
        let small = redirect_thumb("A b.jpg", 100.0, 50.0, LARGE).unwrap();
        assert_eq!(small.src, "//commons.wikimedia.org/wiki/Special:Redirect/file/A_b.jpg");
        assert_eq!(redirect_thumb("A.jpg", 4000.0, 3000.0, LARGE).unwrap().src.rsplit_once('=').unwrap().1, "316");
        assert!(redirect_thumb("Sound.ogg", 0.0, 0.0, LARGE).is_none());
        let linked = Thumb::new("A b.jpg", "x".into(), 10, true);
        assert_eq!(linked.link.as_deref(), Some("?mode=desc&image=A%20b.jpg"));
        assert!(linked.html().contains(" loading='lazy' "));
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

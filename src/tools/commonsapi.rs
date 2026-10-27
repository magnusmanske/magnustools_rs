//! `commonsapi.php`: the 2008 "Commons API", an XML summary of Commons
//! files for re-users: URLs, size, description per language, author,
//! source, location, categories, and licences with their terms.
//!
//! The original scraped the file description pages, whose markup has since
//! changed, so most fields came back empty. This keeps the XML format but
//! takes the data from the MediaWiki API (`extmetadata`, `categories`).
//! Licence terms still come from the wiki page `Commons:Commons API`.
//! `format=json` gives the same as JSON (or JSONP, with `callback`).

use crate::cache::TimedCache;
use crate::mediawiki::{self, strip_namespace};
use crate::state::AppState;
use crate::web::{AppError, Params, jsonp, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

const API_VERSION: &str = "0.92";
const LICENSE_PAGE: &str = "Commons:Commons API";
const LICENSE_PAGE_CACHE: Duration = Duration::from_secs(3600);
const LANGUAGE_NAMES_CACHE: Duration = Duration::from_secs(86_400);
const MAX_IMAGES: usize = 50;
const EXTMETADATA: &str = "ImageDescription|DateTimeOriginal|Artist|Credit|Permission|GPSLatitude|GPSLongitude";
/// Output field, `extmetadata` field.
const FIELDS: &[(&str, &str)] =
    &[("date", "DateTimeOriginal"), ("author", "Artist"), ("source", "Credit"), ("permission", "Permission")];

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    image: String,
    thumbwidth: Option<String>,
    thumbheight: Option<String>,
    #[serde(default)]
    languages: String,
    versions: Option<String>,
    meta: Option<String>,
    sdc: Option<String>,
    #[serde(default)]
    format: String,
    callback: Option<String>,
}

struct Options {
    thumb_width: Option<u32>,
    thumb_height: Option<u32>,
    languages: Vec<String>,
    versions: bool,
    meta: bool,
    /// Structured data: captions and "depicts" statements.
    sdc: bool,
}

#[derive(Template)]
#[template(path = "tools/commonsapi.html")]
struct HelpPage {
    title: &'static str,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let images: Vec<String> = request
        .image
        .split('|')
        .map(|i| strip_namespace(i, &["file", "image"]).unwrap_or(i).trim().replace(' ', "_"))
        .filter(|i| !i.is_empty())
        .take(MAX_IMAGES)
        .collect();
    if images.is_empty() {
        return Ok(render(&HelpPage { title: "Commons API" })?.into_response());
    }
    let positive = |v: &Option<String>| v.as_deref().and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0);
    let thumb_width = positive(&request.thumbwidth);
    let options = Options {
        thumb_width,
        thumb_height: thumb_width.and(positive(&request.thumbheight)),
        languages: request.languages.split('|').map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
        versions: request.versions.is_some(),
        meta: request.meta.is_some(),
        sdc: request.sdc.is_some(),
    };
    let license_data = license_data(&state.http).await?;
    let mut files = vec![];
    for image in &images {
        files.push((image, fetch(&state.http, image, &options).await?));
    }
    let language_names = language_names(&state.http, &files).await?;

    let mut response = Element::new("response").attribute("version", API_VERSION);
    let multiple = images.len() > 1;
    for (image, file) in &files {
        let elements = match file {
            Some(file) => file_elements(image, file, &options, &license_data, &language_names),
            None => vec![Element::text("error", &format!("File does not exist : {image}"))],
        };
        match multiple {
            true => response.push(Element::new("image").attribute("name", &ucfirst(image)).children(elements)),
            false => response.children.extend(elements),
        }
    }
    if request.format.trim().eq_ignore_ascii_case("json") {
        return Ok(jsonp(serde_json::json!({ "response": response.to_json() }), request.callback).into_response());
    }
    let mut xml = r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string();
    response.write_xml(&mut xml);
    Ok(([(header::CONTENT_TYPE, "text/xml; charset=utf-8")], xml).into_response())
}

/// What other tools show of a file (iCommons): the fields of the XML response.
pub(super) struct Summary {
    pub(super) author: String,
    pub(super) source: String,
    pub(super) permission: String,
    pub(super) upload_date: String,
    pub(super) uploader: String,
    pub(super) width: String,
    pub(super) height: String,
    pub(super) size: String,
    /// Latitude and longitude.
    pub(super) location: Option<(String, String)>,
    /// In English, else the default language; HTML.
    pub(super) description: String,
    pub(super) licenses: Vec<String>,
    /// All of them, licence categories included.
    pub(super) categories: Vec<String>,
}

/// The summary of `image` (without namespace); `None` if there is no such file.
pub(super) async fn summary(http: &reqwest::Client, image: &str) -> anyhow::Result<Option<Summary>> {
    let options =
        Options { thumb_width: None, thumb_height: None, languages: vec![], versions: false, meta: false, sdc: false };
    let license_data = license_data(http).await?;
    let image = image.trim().replace(' ', "_");
    let Some(file) = fetch(http, &image, &options).await? else { return Ok(None) };
    let elements = file_elements(&image, &file, &options, &license_data, &HashMap::new());
    let element = |name: &str| elements.iter().find(|e| e.name == name);
    let summary = element("file");
    let text = |name: &str| summary.and_then(|s| s.child(name)).map(|e| e.text.clone()).unwrap_or_default();
    let location = summary.and_then(|s| s.child("location")).and_then(|l| {
        let coordinate = |name: &str| l.child(name).map(|e| e.text.clone());
        Some((coordinate("lat")?, coordinate("lon")?))
    });
    let languages = element("description").map(|d| d.children.as_slice()).unwrap_or_default();
    let language = |code: &str| languages.iter().find(|l| l.attributes.iter().any(|(k, v)| k == "code" && v == code));
    let licenses = element("licenses").map(|l| l.children.as_slice()).unwrap_or_default();
    Ok(Some(Summary {
        author: text("author"),
        source: text("source"),
        permission: text("permission"),
        upload_date: text("upload_date"),
        uploader: text("uploader"),
        width: text("width"),
        height: text("height"),
        size: text("size"),
        location,
        description: language("en").or_else(|| language("default")).map(|l| l.text.clone()).unwrap_or_default(),
        licenses: licenses.iter().filter_map(|l| l.child("name")).map(|n| n.text.clone()).collect(),
        categories: file.categories,
    }))
}

/// What the API knows about one file.
struct File {
    info: Map<String, Value>,
    versions: Vec<Map<String, Value>>,
    categories: Vec<String>,
    /// The MediaInfo entity, for `sdc`.
    entity: Option<Value>,
}

async fn fetch(http: &reqwest::Client, image: &str, options: &Options) -> anyhow::Result<Option<File>> {
    let commons = Wiki::commons();
    let title = format!("File:{image}");
    let mut iiprop = "timestamp|user|url|size|sha1|extmetadata".to_string();
    if options.meta {
        iiprop.push_str("|metadata");
    }
    let width = options.thumb_width.map(|w| w.to_string());
    let height = options.thumb_height.map(|h| h.to_string());
    let mut params = vec![
        ("action", "query"),
        ("titles", title.as_str()),
        ("prop", "imageinfo|categories"),
        ("iiprop", iiprop.as_str()),
        ("iiextmetadatamultilang", "1"),
        ("iiextmetadatafilter", EXTMETADATA),
        ("cllimit", "max"),
    ];
    thumb_params(&mut params, &width, &height);
    let json = mediawiki::api(http, &commons, &params).await?;
    let page = &json["query"]["pages"][0];
    let Some(info) = page["imageinfo"][0].as_object() else { return Ok(None) };
    let entity = match (options.sdc, page["pageid"].as_u64()) {
        (true, Some(id)) => {
            let id = format!("M{id}");
            let json = mediawiki::api(http, &commons, &[("action", "wbgetentities"), ("ids", id.as_str())]).await?;
            json["entities"].get(&id).filter(|e| e.get("missing").is_none()).cloned()
        }
        _ => None,
    };
    let categories = page["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["title"].as_str())
        .map(|t| t.split_once(':').map_or(t, |(_, name)| name).to_string())
        .collect();

    let mut versions = vec![];
    if options.versions {
        let mut params = vec![
            ("action", "query"),
            ("titles", title.as_str()),
            ("prop", "imageinfo"),
            ("iiprop", "timestamp|user|url|size|sha1"),
            ("iilimit", "500"),
        ];
        thumb_params(&mut params, &width, &height);
        let json = mediawiki::api(http, &commons, &params).await?;
        versions = json["query"]["pages"][0]["imageinfo"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|v| v.iter().filter(|(k, _)| k.as_str() != "url").map(|(k, v)| (k.clone(), v.clone())).collect())
            .collect();
    }
    Ok(Some(File { info: info.clone(), versions, categories, entity }))
}

fn thumb_params<'a>(params: &mut Vec<(&'a str, &'a str)>, width: &'a Option<String>, height: &'a Option<String>) {
    if let Some(width) = width {
        params.push(("iiurlwidth", width));
    }
    if let Some(height) = height {
        params.push(("iiurlheight", height));
    }
}

/// Autonyms of the description languages of all files, by code.
async fn language_names(
    http: &reqwest::Client,
    files: &[(&String, Option<File>)],
) -> anyhow::Result<HashMap<String, String>> {
    let codes: BTreeSet<String> = files
        .iter()
        .filter_map(|(_, f)| f.as_ref())
        .flat_map(|f| descriptions(&f.info))
        .map(|(code, _)| code)
        .filter(|code| code != "default")
        .collect();
    if codes.is_empty() {
        return Ok(HashMap::new());
    }
    let autonyms = AUTONYMS.get_or_fetch(async || all_autonyms(http).await).await?;
    Ok(codes.into_iter().filter_map(|code| autonyms.get(&code).map(|name| (code, name.clone()))).collect())
}

static AUTONYMS: TimedCache<HashMap<String, String>> = TimedCache::new(LANGUAGE_NAMES_CACHE);

/// The autonyms of all languages the wiki knows (some 1,500), by code: one
/// request a day instead of one per response.
async fn all_autonyms(http: &reqwest::Client) -> anyhow::Result<HashMap<String, String>> {
    let params = [("action", "query"), ("meta", "languageinfo"), ("liprop", "autonym"), ("licode", "*")];
    let json = mediawiki::api(http, &Wiki::commons(), &params).await?;
    Ok(json["query"]["languageinfo"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(code, info)| info["autonym"].as_str().map(|name| (code.clone(), name.to_string())))
        .collect())
}

fn extmetadata<'a>(info: &'a Map<String, Value>, field: &str) -> &'a Value {
    info.get("extmetadata").map_or(&Value::Null, |e| &e[field]["value"])
}

/// Descriptions by language code, with `default` (English, or the only one) last.
fn descriptions(info: &Map<String, Value>) -> Vec<(String, String)> {
    let mut descriptions: Vec<(String, String)> = match extmetadata(info, "ImageDescription") {
        Value::String(text) => vec![("default".to_string(), absolute_links(text))],
        Value::Object(languages) => languages
            .iter()
            .filter(|(code, _)| !code.starts_with('_'))
            .filter_map(|(code, text)| text.as_str().map(|t| (code.clone(), absolute_links(t))))
            .collect(),
        _ => vec![],
    };
    if !descriptions.iter().any(|(code, _)| code == "default")
        && let Some((_, english)) = descriptions.iter().find(|(code, _)| code == "en")
    {
        descriptions.push(("default".to_string(), english.clone()));
    }
    descriptions
}

/// The API writes links to Wikimedia sites protocol-relative.
fn absolute_links(html: &str) -> String {
    html.replace("href=\"//", "href=\"https://")
}

/// The elements describing one file.
fn file_elements(
    image: &str,
    file: &File,
    options: &Options,
    licenses: &LicenseData,
    language_names: &HashMap<String, String>,
) -> Vec<Element> {
    let info = &file.info;
    let text = |key: &str| info.get(key).map(scalar);
    let categories = Categories::new(&file.categories, licenses);
    let mut elements = vec![];

    let mut summary = Element::new("file");
    summary.add("name", &image.replace('_', " "));
    summary.add("title", &format!("File:{image}"));
    let mut urls = Element::new("urls");
    urls.add("file", &text("url").unwrap_or_default());
    urls.add("description", &format!("https://commons.wikimedia.org/wiki/File:{image}"));
    if let Some(thumbnail) = text("thumburl") {
        urls.add("thumbnail", &thumbnail);
    }
    summary.push(urls);
    for (tag, key) in [("size", "size"), ("width", "width"), ("height", "height"), ("uploader", "user")] {
        summary.add(tag, &text(key).unwrap_or_else(|| "0".to_string()));
    }
    for (tag, key) in [("upload_date", "timestamp"), ("sha1", "sha1")] {
        summary.add(tag, &text(key).unwrap_or_default());
    }
    let coordinate = |field| extmetadata(info, field).as_str().map(str::trim).filter(|v| !v.is_empty());
    if let (Some(lat), Some(lon)) = (coordinate("GPSLatitude"), coordinate("GPSLongitude")) {
        summary.push(Element::new("location").children(vec![Element::text("lat", lat), Element::text("lon", lon)]));
    }
    for (tag, field) in FIELDS {
        if let Some(value) = extmetadata(info, field).as_str() {
            summary.add(tag, &absolute_links(value));
        }
    }
    for (tag, value) in &categories.flags {
        summary.add(tag, value);
    }
    elements.push(summary);

    let metadata = info.get("metadata").and_then(Value::as_array).filter(|m| !m.is_empty());
    if let (true, Some(metadata)) = (options.meta, metadata) {
        let items = metadata
            .iter()
            .map(|item| {
                Element::text("item", &scalar(&item["value"]))
                    .attribute("name", item["name"].as_str().unwrap_or_default())
            })
            .collect();
        elements.push(Element::new("meta").children(items));
    }

    let mut descriptions = descriptions(info);
    if !options.languages.is_empty() {
        descriptions =
            options.languages.iter().filter_map(|l| descriptions.iter().find(|(code, _)| code == l).cloned()).collect();
    }
    let descriptions = descriptions
        .iter()
        .map(|(code, text)| {
            let language = Element::text("language", text).attribute("code", code);
            match language_names.get(code) {
                Some(name) => language.attribute("name", name),
                None => language,
            }
        })
        .collect();
    elements.push(Element::new("description").children(descriptions));

    let other = categories.other.iter().map(|c| Element::text("category", c)).collect();
    elements.push(Element::new("categories").children(other));

    let mut list = Element::new("licenses");
    if categories.self_made {
        list = list.attribute("selfmade", "1");
    }
    for license in &categories.licenses {
        let mut element = Element::new("license");
        element.add("name", license);
        for (key, value) in licenses.terms.get(license).into_iter().flatten() {
            element.add(key, value);
        }
        list.push(element);
    }
    elements.push(list);

    if options.versions {
        let versions = file
            .versions
            .iter()
            .map(|version| {
                let mut element = Element::new("version");
                for (key, value) in version {
                    element.add(key, &scalar(value));
                }
                element
            })
            .collect();
        elements.push(Element::new("versions").children(versions));
    }
    if let Some(entity) = &file.entity {
        elements.push(structured_data(entity));
    }
    elements
}

/// Captions and "depicts" (P180) statements of a MediaInfo entity.
fn structured_data(entity: &Value) -> Element {
    let captions = entity["labels"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(code, label)| {
            Element::text("caption", label["value"].as_str().unwrap_or_default()).attribute("code", code)
        })
        .collect();
    let depicts = entity["statements"]["P180"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["mainsnak"]["datavalue"]["value"]["id"].as_str())
        .map(|id| Element::new("entity").attribute("id", id))
        .collect();
    Element::new("structured_data")
        .children(vec![Element::new("captions").children(captions), Element::new("depicts").children(depicts)])
}

/// A file's categories, sorted into licences, flags and the rest.
#[derive(Debug, Default, PartialEq)]
struct Categories {
    licenses: Vec<String>,
    other: Vec<String>,
    /// `qualityimage`, `featuredpicture`, `pictureoftheday` (year only, as `YYYY-00-00`).
    flags: Vec<(&'static str, String)>,
    self_made: bool,
}

impl Categories {
    fn new(categories: &[String], license_data: &LicenseData) -> Self {
        let mut result = Self::default();
        let mut sorted: Vec<&String> = categories.iter().filter(|c| !license_data.ignore.contains(*c)).collect();
        sorted.sort();
        sorted.dedup();
        for category in sorted {
            let lower = category.to_lowercase();
            if category == "Quality images" {
                result.flags.push(("qualityimage", "1".to_string()));
            } else if category == "Featured pictures on Wikimedia Commons" {
                result.flags.push(("featuredpicture", "1".to_string()));
            } else if let Some(rest) = category.strip_prefix("Pictures of the day") {
                let year: String = rest.chars().filter(char::is_ascii_digit).take(4).collect();
                if !result.flags.iter().any(|(k, _)| *k == "pictureoftheday") {
                    result.flags.push(("pictureoftheday", format!("{year}-00-00")));
                }
            } else if category == "Self-published work" {
                result.self_made = true;
            } else if ["pd", "gfdl", "cc-"].iter().any(|p| lower.starts_with(p))
                || license_data.terms.contains_key(category.as_str())
            {
                let license = match category.len().checked_sub(5).filter(|_| lower.ends_with("-self")) {
                    Some(end) => {
                        result.self_made = true;
                        category.get(..end).unwrap_or(category)
                    }
                    None => category,
                };
                if !result.licenses.iter().any(|l| l == license) {
                    result.licenses.push(license.to_string());
                }
            } else {
                result.other.push(category.clone());
            }
        }
        result
    }
}

/// From the wiki page `Commons:Commons API`: terms per licence category,
/// and categories to leave out.
#[derive(Debug, Default, Clone, PartialEq)]
struct LicenseData {
    /// Key/value pairs in page order, per licence.
    terms: HashMap<String, Vec<(String, String)>>,
    ignore: Vec<String>,
}

impl LicenseData {
    fn parse(wikitext: &str) -> Self {
        let mut data = Self::default();
        let mut headings: [String; 9] = Default::default();
        for line in wikitext.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line.starts_with('=') {
                let mut level = 0;
                let mut heading = line;
                while heading.starts_with('=') && level < headings.len() - 1 {
                    heading = strip_first_and_last(heading);
                    level += 1;
                }
                let mut heading = heading.trim().replace('_', " ");
                if level == 2 {
                    heading = heading.to_lowercase();
                }
                for deeper in headings.iter_mut().skip(level + 1) {
                    deeper.clear();
                }
                if let Some(slot) = headings.get_mut(level) {
                    *slot = heading;
                }
                continue;
            }
            match headings[2].as_str() {
                "license data" => {
                    let Some(pair) = line.strip_prefix(';') else { continue };
                    let (key, value) = pair.split_once(':').unwrap_or((pair, ""));
                    let key = key.trim().replace(' ', "_");
                    let license = headings[3].clone();
                    if key == "based_on" {
                        let base = data.terms.get(value.trim()).cloned().unwrap_or_default();
                        data.terms.insert(license, base);
                    } else {
                        let terms = data.terms.entry(license).or_default();
                        match terms.iter_mut().find(|(k, _)| *k == key) {
                            Some(entry) => entry.1 = value.to_string(),
                            None => terms.push((key, value.to_string())),
                        }
                    }
                }
                "ignore categories" => {
                    if let Some(category) = line.strip_prefix('*') {
                        data.ignore.push(category.replace('_', " ").trim().to_string());
                    }
                }
                _ => {}
            }
        }
        data
    }
}

fn strip_first_and_last(s: &str) -> &str {
    let mut chars = s.chars();
    chars.next();
    chars.next_back();
    chars.as_str()
}

static LICENSE_DATA: TimedCache<LicenseData> = TimedCache::new(LICENSE_PAGE_CACHE);

async fn license_data(http: &reqwest::Client) -> anyhow::Result<Arc<LicenseData>> {
    LICENSE_DATA
        .get_or_fetch(async || {
            let text = mediawiki::page_wikitext(http, &Wiki::commons(), LICENSE_PAGE).await?.unwrap_or_default();
            Ok(LicenseData::parse(&text))
        })
        .await
}

/// Elements that are lists in JSON, even with a single entry.
const JSON_LISTS: &[&str] = &["image", "category", "language", "license", "version", "item", "caption", "entity"];
/// Elements holding such a list, with its name: in JSON, the list is there even when empty.
const JSON_CONTAINERS: &[(&str, &str)] = &[
    ("categories", "category"),
    ("description", "language"),
    ("licenses", "license"),
    ("meta", "item"),
    ("versions", "version"),
    ("captions", "caption"),
    ("depicts", "entity"),
];

/// An element of the response, written as XML or JSON.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    text: String,
    children: Vec<Element>,
}

impl Element {
    /// A tag name from data is made a valid XML name.
    fn new(name: &str) -> Self {
        Self { name: element_name(name), ..Default::default() }
    }

    fn text(name: &str, text: &str) -> Self {
        Self { text: text.to_string(), ..Self::new(name) }
    }

    fn attribute(mut self, name: &str, value: &str) -> Self {
        self.attributes.push((name.to_string(), value.to_string()));
        self
    }

    fn children(mut self, children: Vec<Element>) -> Self {
        self.children = children;
        self
    }

    fn push(&mut self, child: Element) {
        self.children.push(child);
    }

    fn add(&mut self, name: &str, text: &str) {
        self.push(Self::text(name, text));
    }

    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    fn write_xml(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (name, value) in &self.attributes {
            out.push_str(&format!(" {name}=\"{}\"", attribute(value)));
        }
        if self.text.is_empty() && self.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        out.push_str(&text_escape(&self.text));
        for child in &self.children {
            child.write_xml(out);
        }
        out.push_str(&format!("</{}>", self.name));
    }

    /// As the PHP version turned its XML into JSON: a plain element becomes
    /// its text; attributes become keys, with the text as `value`; repeated
    /// elements become lists. Unlike there, an empty list is an empty list, not `""`.
    fn to_json(&self) -> Value {
        let list_name = JSON_CONTAINERS.iter().find(|(name, _)| *name == self.name).map(|(_, list)| *list);
        if self.children.is_empty() && self.attributes.is_empty() && list_name.is_none() {
            return Value::String(self.text.clone());
        }
        let mut map: Map<String, Value> =
            self.attributes.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        if let Some(list_name) = list_name {
            map.insert(list_name.to_string(), Value::Array(vec![]));
        }
        if self.children.is_empty() && !self.text.trim().is_empty() {
            map.insert("value".to_string(), Value::String(self.text.clone()));
        }
        for child in &self.children {
            let value = child.to_json();
            match map.get_mut(&child.name) {
                Some(Value::Array(list)) => list.push(value),
                Some(single) => *single = Value::Array(vec![single.take(), value]),
                None if JSON_LISTS.contains(&child.name.as_str()) => {
                    map.insert(child.name.clone(), Value::Array(vec![value]));
                }
                None => {
                    map.insert(child.name.clone(), value);
                }
            }
        }
        Value::Object(map)
    }
}

fn element_name(name: &str) -> String {
    let name: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    match name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        true => name,
        false => format!("_{name}"),
    }
}

/// As PHP's DOM wrote text: `&`, `<` and `>` escaped.
fn text_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn attribute(text: &str) -> String {
    text_escape(text).replace('"', "&quot;")
}

/// A JSON value as element text; structures as JSON.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LICENSE_PAGE_TEXT: &str = "Intro\n== Ignore categories ==\n* Media_with locations\n\n\
        == License data ==\n* explanation\n=== CC-BY-2.5 ===\n;full_name:Creative Commons Attribution V2.5\n\
        ;attribute_author:1\n;license_info_url:http://creativecommons.org/licenses/by/2.5/\n\
        === CC-BY-SA-2.5 ===\n;based_on:CC-BY-2.5\n;full_name:Creative Commons Attribution Share-Alike V2.5\n\
        ;keep_under_similar_license:1\n";

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn license_page() {
        let data = LicenseData::parse(LICENSE_PAGE_TEXT);
        assert_eq!(data.ignore, vec!["Media with locations"]);
        assert_eq!(
            data.terms["CC-BY-SA-2.5"],
            pairs(&[
                ("full_name", "Creative Commons Attribution Share-Alike V2.5"),
                ("attribute_author", "1"),
                ("license_info_url", "http://creativecommons.org/licenses/by/2.5/"),
                ("keep_under_similar_license", "1"),
            ])
        );
    }

    #[test]
    fn categories_sorted_out() {
        let data = LicenseData::parse(LICENSE_PAGE_TEXT);
        let categories: Vec<String> = [
            "Yarra River",
            "CC-BY-SA-2.5",
            "Media with locations",
            "Quality images",
            "Pictures of the day (2008)",
            "GFDL-self",
            "PD-old",
            "Self-published work",
        ]
        .iter()
        .map(|c| c.to_string())
        .collect();
        let sorted = Categories::new(&categories, &data);
        assert_eq!(sorted.licenses, vec!["CC-BY-SA-2.5", "GFDL", "PD-old"]);
        assert_eq!(sorted.other, vec!["Yarra River"]);
        assert_eq!(
            sorted.flags,
            vec![("pictureoftheday", "2008-00-00".to_string()), ("qualityimage", "1".to_string())]
        );
        assert!(sorted.self_made);
    }

    #[test]
    fn descriptions_with_default() {
        let info = json!({"extmetadata": {"ImageDescription": {"value": {
            "de": "Blick <a href=\"//commons.wikimedia.org/wiki/X\">X</a>", "en": "View", "_type": "lang"}}}});
        let info = info.as_object().unwrap();
        assert_eq!(
            descriptions(info),
            pairs(&[
                ("de", "Blick <a href=\"https://commons.wikimedia.org/wiki/X\">X</a>"),
                ("en", "View"),
                ("default", "View")
            ])
        );
        let single = json!({"extmetadata": {"ImageDescription": {"value": "Just one"}}});
        assert_eq!(descriptions(single.as_object().unwrap()), pairs(&[("default", "Just one")]));
    }

    #[test]
    fn file_xml() {
        let info = json!({
            "timestamp": "2008-05-22T00:26:08Z", "user": "Dha", "size": 2045024, "width": 3334, "height": 1219,
            "url": "https://upload.wikimedia.org/x/Yarra_Panorama.jpg?a=1&b=2", "sha1": "cc03",
            "extmetadata": {
                "GPSLatitude": {"value": "-37.8"}, "GPSLongitude": {"value": "144.9"},
                "Artist": {"value": "<a href=\"//commons.wikimedia.org/wiki/User:Diliff\">Diliff</a>"},
                "ImageDescription": {"value": {"en": "View & more", "_type": "lang"}}
            }
        });
        let file = File {
            info: info.as_object().unwrap().clone(),
            versions: vec![],
            categories: vec!["CC-BY-SA-2.5".into(), "Self-published work".into(), "Yarra River".into()],
            entity: None,
        };
        let options = Options {
            thumb_width: None,
            thumb_height: None,
            languages: vec![],
            versions: false,
            meta: false,
            sdc: false,
        };
        let names = HashMap::from([("en".to_string(), "English".to_string())]);
        let elements =
            file_elements("Yarra_Panorama.jpg", &file, &options, &LicenseData::parse(LICENSE_PAGE_TEXT), &names);
        let mut xml = String::new();
        elements.iter().for_each(|e| e.write_xml(&mut xml));
        assert_eq!(
            xml,
            "<file><name>Yarra Panorama.jpg</name><title>File:Yarra_Panorama.jpg</title><urls>\
             <file>https://upload.wikimedia.org/x/Yarra_Panorama.jpg?a=1&amp;b=2</file>\
             <description>https://commons.wikimedia.org/wiki/File:Yarra_Panorama.jpg</description></urls>\
             <size>2045024</size><width>3334</width><height>1219</height><uploader>Dha</uploader>\
             <upload_date>2008-05-22T00:26:08Z</upload_date><sha1>cc03</sha1>\
             <location><lat>-37.8</lat><lon>144.9</lon></location>\
             <author>&lt;a href=\"https://commons.wikimedia.org/wiki/User:Diliff\"&gt;Diliff&lt;/a&gt;</author></file>\
             <description><language code=\"en\" name=\"English\">View &amp; more</language>\
             <language code=\"default\">View &amp; more</language></description>\
             <categories><category>Yarra River</category></categories>\
             <licenses selfmade=\"1\"><license><name>CC-BY-SA-2.5</name>\
             <full_name>Creative Commons Attribution Share-Alike V2.5</full_name><attribute_author>1</attribute_author>\
             <license_info_url>http://creativecommons.org/licenses/by/2.5/</license_info_url>\
             <keep_under_similar_license>1</keep_under_similar_license></license></licenses>"
        );
    }

    #[test]
    fn json_as_the_php_version_made_it() {
        let mut response = Element::new("response").attribute("version", API_VERSION);
        response.push(Element::new("categories").children(vec![Element::text("category", "A")]));
        response.push(Element::text("language", "View").attribute("code", "en"));
        response.push(Element::text("language", "Blick").attribute("code", "de"));
        response.push(Element::new("licenses").attribute("selfmade", "1"));
        response.push(Element::text("sha1", ""));
        assert_eq!(
            response.to_json(),
            json!({
                "version": API_VERSION,
                "categories": {"category": ["A"]},
                "language": [{"code": "en", "value": "View"}, {"code": "de", "value": "Blick"}],
                "licenses": {"selfmade": "1", "license": []},
                "sha1": ""
            })
        );
        let mut xml = String::new();
        response.write_xml(&mut xml);
        assert!(xml.ends_with(r#"<licenses selfmade="1"/><sha1/></response>"#), "{xml}");
    }

    #[test]
    fn captions_and_depicts() {
        let entity = json!({
            "labels": {"en": {"language": "en", "value": "A warthog"}},
            "statements": {"P180": [{"mainsnak": {"datavalue": {"value": {"id": "Q1"}}}}]}
        });
        let mut xml = String::new();
        structured_data(&entity).write_xml(&mut xml);
        assert_eq!(
            xml,
            r#"<structured_data><captions><caption code="en">A warthog</caption></captions><depicts><entity id="Q1"/></depicts></structured_data>"#
        );
    }

    #[test]
    fn names() {
        assert_eq!(element_name("full_name"), "full_name");
        assert_eq!(element_name("1 bad<name>"), "_1_bad_name_");
        assert_eq!(attribute("a\"<b>&"), "a&quot;&lt;b&gt;&amp;");
    }
}

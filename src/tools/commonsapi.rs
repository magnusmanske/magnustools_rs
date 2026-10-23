//! `commonsapi.php`: the 2008 "Commons API", an XML summary of Commons
//! files for re-users: URLs, size, description per language, author,
//! source, location, categories, and licences with their terms.
//!
//! The original scraped the file description pages, whose markup has since
//! changed, so most fields came back empty. This keeps the XML format but
//! takes the data from the MediaWiki API (`extmetadata`, `categories`).
//! Licence terms still come from the wiki page `Commons:Commons API`.

use crate::cache::TimedCache;
use crate::mediawiki::{self, strip_namespace};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
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
}

struct Options {
    thumb_width: Option<u32>,
    thumb_height: Option<u32>,
    languages: Vec<String>,
    versions: bool,
    meta: bool,
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
    };
    let license_data = license_data(&state.http).await?;
    let mut files = vec![];
    for image in &images {
        files.push((image, fetch(&state.http, image, &options).await?));
    }
    let language_names = language_names(&state.http, &files).await?;

    let mut xml = Xml::default();
    xml.raw(&format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><response version=\"{API_VERSION}\">"));
    let multiple = images.len() > 1;
    for (image, file) in &files {
        if multiple {
            xml.raw(&format!("<image name=\"{}\">", attribute(&ucfirst(image))));
        }
        match file {
            Some(file) => write_file(&mut xml, image, file, &options, &license_data, &language_names),
            None => xml.element("error", &format!("File does not exist : {image}")),
        }
        if multiple {
            xml.raw("</image>");
        }
    }
    xml.raw("</response>");
    Ok(([(header::CONTENT_TYPE, "text/xml; charset=utf-8")], xml.0).into_response())
}

/// What the API knows about one file.
struct File {
    info: Map<String, Value>,
    versions: Vec<Map<String, Value>>,
    categories: Vec<String>,
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
    Ok(Some(File { info: info.clone(), versions, categories }))
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
        .collect();
    let codes: Vec<String> = codes.into_iter().filter(|c| c != "default").collect();
    if codes.is_empty() {
        return Ok(HashMap::new());
    }
    let codes = codes.join("|");
    let params = [("action", "query"), ("meta", "languageinfo"), ("liprop", "autonym"), ("licode", codes.as_str())];
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

fn write_file(
    xml: &mut Xml,
    image: &str,
    file: &File,
    options: &Options,
    licenses: &LicenseData,
    language_names: &HashMap<String, String>,
) {
    let info = &file.info;
    let text = |key: &str| info.get(key).map(scalar);
    let categories = Categories::new(&file.categories, licenses);

    xml.raw("<file>");
    xml.element("name", &image.replace('_', " "));
    xml.element("title", &format!("File:{image}"));
    xml.raw("<urls>");
    xml.element("file", &text("url").unwrap_or_default());
    xml.element("description", &format!("https://commons.wikimedia.org/wiki/File:{image}"));
    if let Some(thumbnail) = text("thumburl") {
        xml.element("thumbnail", &thumbnail);
    }
    xml.raw("</urls>");
    for (tag, key) in [("size", "size"), ("width", "width"), ("height", "height"), ("uploader", "user")] {
        xml.element(tag, &text(key).unwrap_or_else(|| "0".to_string()));
    }
    for (tag, key) in [("upload_date", "timestamp"), ("sha1", "sha1")] {
        match text(key) {
            Some(value) => xml.element(tag, &value),
            None => xml.raw(&format!("<{tag}/>")),
        }
    }
    let coordinate = |field| extmetadata(info, field).as_str().map(str::trim).filter(|v| !v.is_empty());
    if let (Some(lat), Some(lon)) = (coordinate("GPSLatitude"), coordinate("GPSLongitude")) {
        xml.raw(&format!("<location><lat>{}</lat><lon>{}</lon></location>", text_escape(lat), text_escape(lon)));
    }
    for (tag, field) in FIELDS {
        if let Some(value) = extmetadata(info, field).as_str() {
            xml.element(tag, &absolute_links(value));
        }
    }
    for (tag, value) in &categories.flags {
        xml.element(tag, value);
    }
    xml.raw("</file>");

    let metadata = info.get("metadata").and_then(Value::as_array).filter(|m| !m.is_empty());
    if let (true, Some(metadata)) = (options.meta, metadata) {
        xml.raw("<meta>");
        for item in metadata {
            let name = item["name"].as_str().unwrap_or_default();
            xml.raw(&format!("<item name=\"{}\">{}</item>", attribute(name), text_escape(&scalar(&item["value"]))));
        }
        xml.raw("</meta>");
    }

    xml.raw("<description>");
    let mut descriptions = descriptions(info);
    if !options.languages.is_empty() {
        descriptions =
            options.languages.iter().filter_map(|l| descriptions.iter().find(|(code, _)| code == l).cloned()).collect();
    }
    for (code, text) in &descriptions {
        let name = language_names.get(code).map(|n| format!(" name=\"{}\"", attribute(n))).unwrap_or_default();
        xml.raw(&format!("<language code=\"{}\"{name}>{}</language>", attribute(code), text_escape(text)));
    }
    xml.raw("</description>");

    xml.raw("<categories>");
    for category in &categories.other {
        xml.element("category", category);
    }
    xml.raw("</categories>");

    xml.raw(if categories.self_made { "<licenses selfmade=\"1\">" } else { "<licenses>" });
    for license in &categories.licenses {
        xml.raw("<license>");
        xml.element("name", license);
        for (key, value) in licenses.terms.get(license).into_iter().flatten() {
            xml.element(key, value);
        }
        xml.raw("</license>");
    }
    xml.raw("</licenses>");

    if options.versions {
        xml.raw("<versions>");
        for version in &file.versions {
            xml.raw("<version>");
            for (key, value) in version {
                xml.element(key, &scalar(value));
            }
            xml.raw("</version>");
        }
        xml.raw("</versions>");
    }
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

/// The response under construction.
#[derive(Default)]
struct Xml(String);

impl Xml {
    fn raw(&mut self, s: &str) {
        self.0.push_str(s);
    }

    /// `<tag>text</tag>`; a tag name from data is made a valid XML name.
    fn element(&mut self, tag: &str, text: &str) {
        let tag = element_name(tag);
        self.0.push_str(&format!("<{tag}>{}</{tag}>", text_escape(text)));
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
        };
        let options =
            Options { thumb_width: None, thumb_height: None, languages: vec![], versions: false, meta: false };
        let names = HashMap::from([("en".to_string(), "English".to_string())]);
        let mut xml = Xml::default();
        write_file(&mut xml, "Yarra_Panorama.jpg", &file, &options, &LicenseData::parse(LICENSE_PAGE_TEXT), &names);
        assert_eq!(
            xml.0,
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
    fn names() {
        assert_eq!(element_name("full_name"), "full_name");
        assert_eq!(element_name("1 bad<name>"), "_1_bad_name_");
        assert_eq!(attribute("a\"<b>&"), "a&quot;&lt;b&gt;&amp;");
    }
}

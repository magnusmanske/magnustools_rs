//! `add-information` (formerly its own tool): rewrites a file's description
//! into an {{Information}} (or, for paintings, {{Painting}}) template, and
//! opens the wiki's edit form with the changes, to check and save. The
//! rewrite is the old tool's guesswork, ported with its rules.

use super::no_information::TEMPLATES;
use crate::mediawiki::{self, strip_namespace};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, LazyLock};

const TOOL_URL: &str = "https://magnustools.toolforge.org/add_information/";
/// Exif dates, the most telling first.
const EXIF_DATES: &[&str] = &["DateTimeOriginal", "DateTimeDigitized", "DateTime"];
/// A maintenance category that the new template makes obsolete.
const MISSING_INFOBOX: &str = "[[category:media missing infobox template]]";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

static HAS_TEMPLATE: LazyLock<Regex> = LazyLock::new(|| {
    let names: Vec<String> = TEMPLATES.iter().map(|t| regex::escape(t).replace('_', "[ _]")).collect();
    regex(&format!(r"(?i)\{{\{{\s*(?:template:)?(?:{})\s*[|}}\n]", names.join("|")))
});
static COMMENT: LazyLock<Regex> = LazyLock::new(|| regex(r"(?s)<!--.*?-->"));
/// German descriptions as `Quelle: …` lines, and the headings they become.
static GERMAN_FIELDS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    [
        (r"Beschreibung", "Description"),
        (r"Quelle", "Source"),
        (r"Autor", "Author"),
        (r"Urheber", "Author"),
        (r"Lizenz\w*", "Permission"),
        (r"Genehmigung\w*", "Permission"),
        (r"Andere Version\w*", "Other versions"),
    ]
    .into_iter()
    .map(|(field, heading)| (regex(&format!(r"(?i)^\*?\s*{field}\s*:\s*(.*)$")), heading))
    .collect()
});
/// `{{en|…}}` and the like: a description in some language.
static LANGUAGE_TEMPLATE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\{\{[a-z]{2,3}(-[a-z]+)?\|"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^=+\s*(.+?)\s*=+\s*$"));
static SOURCE_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^(sources?|from)\b\s*(:)?\s*(.*)$"));
static NUMBERED_COPY: LazyLock<Regex> = LazyLock::new(|| regex(r"\s*\(\d\d?\)$"));
static CAMERA_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"IMG\s*_*\d*$"));
/// Dates in a description, the most precise kind first; the first kind found counts.
static DATES: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        regex(r"[0-9]{1,2}\. \w+ [0-9]{4}"),
        regex(r"\w+ [0-9]{1,2},? [0-9]{4}"),
        regex(r"[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}"),
        regex(r"\w{3,15} [0-9]{4}"),
    ]
});
static YEAR: LazyLock<Regex> = LazyLock::new(|| regex(r"(?:^|[^-xa-zA-Z0-9])([0-9]{4})(?:[^-a-zA-Z0-9]|$)"));
static DOTTED_DATE: LazyLock<Regex> = LazyLock::new(|| regex(r"([0-9]{1,2})\.\s*([0-9]{1,2})\.\s*([0-9]{4})"));
static EXIF_DATE: LazyLock<Regex> = LazyLock::new(|| regex(r"[0-9]{4}[-.:][0-9]{2}[-.:][0-9]{2}"));
static URL: LazyLock<Regex> = LazyLock::new(|| regex(r"https?://[A-Za-z0-9\-_./]+"));
static WWW: LazyLock<Regex> = LazyLock::new(|| regex(r"www\.[A-Za-z0-9\-_./]+"));
static PAINTER: LazyLock<Regex> = LazyLock::new(|| regex(r"\bby (\[\[[a-zA-Z0-9\- _]+\]\])"));
static YEAR_ANYWHERE: LazyLock<Regex> = LazyLock::new(|| regex(r"[0-9]{4}"));

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    image: String,
    /// The author of `{{self}}` files; the uploader by default.
    user: String,
    /// Only show the new text.
    debug: Option<String>,
    /// Lines signed by the user make it their own work.
    mynoinfo: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "commons".to_string(),
            project: "wikipedia".to_string(),
            image: String::new(),
            user: String::new(),
            debug: None,
            mynoinfo: None,
        }
    }
}

/// The edit to propose.
struct Edit {
    file: String,
    file_url: String,
    template: &'static str,
    action_url: String,
    wikitext: String,
    summary: String,
    /// The revision this edit is based on, and when editing started, for
    /// the wiki's edit conflict check.
    edit_time: String,
    start_time: String,
    submit: bool,
}

#[derive(Template)]
#[template(path = "tools/add_information.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    edit: Option<Edit>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let image = request.image.trim();
    let image = strip_namespace(image, &["file", "image", "datei", "bild"]).unwrap_or(image).replace('_', " ");
    let edit = match image.is_empty() {
        true => None,
        false => Some(edit(&state, &request, &image).await?),
    };
    render(&Page { title: "Add information", request: &request, edit })
}

async fn edit(state: &AppState, request: &Request, image: &str) -> Result<Edit, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let file = format!("File:{image}");
    let info = file_info(&state.http, &wiki, &file)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("There is no {file} on {wiki}.")))?;
    if HAS_TEMPLATE.is_match(&info.text) {
        return Err(AppError::BadRequest(format!("{file} already has an Information template, or a similar one.")));
    }
    let user = match request.user.trim() {
        "" => info.uploader.as_str(),
        user => user,
    };
    let draft = Draft::new(&Input {
        language: &request.language.trim().to_lowercase(),
        project: &request.project.trim().to_lowercase(),
        image,
        user: &user.replace('_', " "),
        text: &info.text,
        exif_date: info.exif_date.as_deref(),
        own_work: request.mynoinfo.as_deref().is_some_and(|v| !v.is_empty() && v != "0"),
    });
    Ok(Edit {
        file_url: wiki.page_url(&file),
        action_url: format!("{}?title={}&action=submit", wiki.index_url(), encode_title(&file)),
        file,
        template: draft.template,
        summary: format!("Adding {{{{{}}}}} template (using {TOOL_URL} )", draft.template),
        wikitext: draft.wikitext,
        edit_time: info.edit_time,
        start_time: Utc::now().format("%Y%m%d%H%M%S").to_string(),
        submit: request.debug.is_none(),
    })
}

/// What the wiki knows of a file.
struct FileInfo {
    /// Without HTML comments.
    text: String,
    uploader: String,
    /// Of the current revision, as `YYYYMMDDHHMMSS`.
    edit_time: String,
    exif_date: Option<String>,
}

/// `None` if there is no such page.
async fn file_info(http: &reqwest::Client, wiki: &Wiki, file: &str) -> anyhow::Result<Option<FileInfo>> {
    let params = [
        ("action", "query"),
        ("titles", file),
        ("prop", "revisions|imageinfo"),
        ("rvprop", "content|timestamp"),
        ("rvslots", "main"),
        ("iiprop", "user|commonmetadata"),
    ];
    let json = mediawiki::api(http, wiki, &params).await?;
    Ok(parse_file_info(&json["query"]["pages"][0]))
}

fn parse_file_info(page: &Value) -> Option<FileInfo> {
    let revision = &page["revisions"][0];
    let text = revision["slots"]["main"]["content"].as_str()?;
    let info = &page["imageinfo"][0];
    let metadata = info["commonmetadata"].as_array();
    let exif = |name: &str| metadata?.iter().find(|m| m["name"] == name)?["value"].as_str();
    Some(FileInfo {
        text: COMMENT.replace_all(text, "").trim().to_string(),
        uploader: info["user"].as_str().unwrap_or_default().to_string(),
        edit_time: revision["timestamp"].as_str().unwrap_or_default().chars().filter(char::is_ascii_digit).collect(),
        exif_date: EXIF_DATES.iter().find_map(|name| exif(name)).map(str::to_string),
    })
}

struct Input<'a> {
    language: &'a str,
    project: &'a str,
    /// Without namespace.
    image: &'a str,
    user: &'a str,
    text: &'a str,
    exif_date: Option<&'a str>,
    own_work: bool,
}

/// The new description page.
#[derive(Debug)]
struct Draft {
    template: &'static str,
    wikitext: String,
}

/// The parts of a description, each a list of bits of wiki text.
#[derive(Default)]
struct Fields {
    description: Vec<String>,
    source: Vec<String>,
    date: Vec<String>,
    author: Vec<String>,
    permission: Vec<String>,
    other_versions: Vec<String>,
    /// Templates, licenses, and categories, kept below the new template.
    after: Vec<String>,
}

impl Draft {
    fn new(input: &Input) -> Self {
        let german = input.language == "de";
        let painting = input.text.to_lowercase().contains("painting");
        let text = input.text.replace("{{msg:", "{{").replace("}}[[", "}}\n[[").replace("]]{{", "]]\n{{");
        let text = if german { german_fields_as_headings(&text) } else { text };
        let text = one_sentence_per_line(&text).replace("[[Category:", "\n[[Category:");

        let mut fields = Fields::default();
        // Category and signature lines may hold a date too.
        let mut also_dated = String::new();
        let signature = format!("[[user:{0}|{0}]]", input.user).to_lowercase();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let lower = line.to_lowercase();
            if line.starts_with("{{") {
                match LANGUAGE_TEMPLATE.is_match(line) {
                    true => fields.description.push(format!("\n{line}")),
                    false => fields.after.push(line.to_string()),
                }
            } else if lower.contains("[[category:") {
                if lower != MISSING_INFOBOX {
                    fields.after.push(line.to_string());
                }
                also_dated += &format!(" {line}");
            } else if input.own_work && lower.contains(&signature) {
                fields.author.push("~~~".to_string());
                fields.source.push("own work".to_string());
                also_dated += &format!(" {line}");
            } else if lower.contains("[[user:") {
                fields.author.push(line.to_string());
                also_dated += &format!(" {line}");
            } else if lower.contains("{{int:filedesc}}") {
                // The new text brings its own heading.
            } else if lower.contains("{{int:license-header}}") {
                fields.after.push(line.to_string());
            } else if line.starts_with('*') {
                fields.description.push(format!("\n{line}"));
            } else {
                fields.description.push(line.to_string());
            }
        }
        // Categories last.
        fields.after.sort_by_key(|l| l.to_lowercase().starts_with("[[category:"));

        fields.sort_by_headings();
        fields.take_sources_from_description();
        if fields.description.is_empty() {
            fields.description.push(description_from_name(input.image));
        }
        let after = fields.after.join("\n").to_lowercase();
        if fields.source.is_empty() && (after.contains("{{loc|") || after.contains("{{loc}}")) {
            fields.source.push("Library of Congress".to_string());
        }
        let dotted = !matches!(input.language, "commons" | "en") && input.project != "wikimedia";
        fields.date = dates_in(&format!("{} {also_dated}", fields.description.join("\n")), dotted);
        fields.source = fields.source.iter().map(|s| unbracket_url(s)).collect();
        if let Some(exif) = input.exif_date.and_then(|d| EXIF_DATE.find(d)).filter(|_| !painting) {
            let exif = exif.as_str().replace([':', '.'], "-");
            if !(fields.date.len() == 1 && fields.date[0].trim() == exif) {
                let separator = if fields.date.is_empty() { "" } else { "; " };
                fields.date.push(format!("{separator}{{{{According to Exif data|{exif}}}}}"));
            }
        }
        for line in &mut fields.description {
            if line.starts_with(';') && line.contains(':') {
                line.insert(0, '\n');
            }
        }
        if fields.author.is_empty() && text.to_lowercase().contains("{{self") {
            fields.author.push(format!("[[User:{0}|{0}]]", input.user));
            if fields.source.is_empty() {
                fields.source.push("{{own}}".to_string());
            }
        }
        fields.take_url_as_source();

        let painting_title = painting.then(|| fields.take_painter(input.image).unwrap_or_default());
        if fields.description.is_empty() {
            fields.description.push(description_from_name(input.image));
        }
        let (template, wikitext) = match painting_title {
            Some(title) => ("Painting", fields.painting(&title)),
            None => ("Information", fields.information()),
        };
        let wikitext = if german { germanize(&wikitext) } else { wikitext };
        Self { template, wikitext }
    }
}

impl Fields {
    /// Lines under a "Source", "Permission", … heading belong there. The
    /// headings go, as do broken ones, such as a lone `==`.
    fn sort_by_headings(&mut self) {
        let mut heading = String::new();
        let mut description = vec![];
        for line in std::mem::take(&mut self.description) {
            if let Some(c) = HEADING.captures(&line) {
                heading = c[1].to_lowercase();
                continue;
            }
            match heading.as_str() {
                "permission" => self.permission.push(line),
                "source" => self.source.push(line),
                "author" => self.author.push(line),
                "other versions" => self.other_versions.push(line),
                _ => description.push(line),
            }
        }
        description.retain(|l| !(l.starts_with('=') && l.ends_with('=')));
        self.description = description;
    }

    /// "Source: …" and "From …" lines.
    fn take_sources_from_description(&mut self) {
        let mut description = vec![];
        for line in std::mem::take(&mut self.description) {
            let Some(c) = SOURCE_LINE.captures(&line) else {
                description.push(line);
                continue;
            };
            let from = c[1].eq_ignore_ascii_case("from");
            let source = if from || c.get(2).is_some() { c[3].trim() } else { line.trim() };
            if !source.is_empty() {
                self.source.push(source.to_string());
            }
        }
        self.description = description;
    }

    /// Without a source, the first line with a URL gives one.
    fn take_url_as_source(&mut self) {
        if !self.source.is_empty() {
            return;
        }
        let url = |line: &String| {
            URL.find(line)
                .map(|m| m.as_str().to_string())
                .or_else(|| WWW.find(line).map(|m| format!("http://{}", m.as_str())))
        };
        if let Some((i, url)) = self.description.iter().enumerate().find_map(|(i, l)| Some((i, url(l)?))) {
            self.source.push(url);
            self.description.remove(i);
        }
    }

    /// The artist, from "by [[Name]]"; and the title, from the file name.
    fn take_painter(&mut self, image: &str) -> Option<String> {
        if !self.author.is_empty() {
            return None;
        }
        let (i, artist) = self
            .description
            .iter()
            .enumerate()
            .find_map(|(i, l)| Some((i, PAINTER.captures(l)?[1].replace('_', " "))))?;
        self.description.remove(i);
        let name_parts: Vec<String> = artist.replace(['[', ']'], "").split(' ').map(str::to_string).collect();
        self.author.push(artist);

        let mut title = image.rsplit_once('.').map_or(image, |(name, _)| name).to_string();
        for part in name_parts.iter().filter(|p| !p.is_empty()) {
            title = replace_ci(&title, part, "");
        }
        if self.date.is_empty()
            && let Some(year) = YEAR_ANYWHERE.find(&title).map(|m| m.as_str().to_string())
        {
            title = title.replace(&year, "");
            self.date.push(year);
        }
        let title = title.replace("  ", " ").replace(" IBI", "");
        Some(upper_first(title.trim()))
    }

    fn information(&self) -> String {
        format!(
            "=={{{{int:filedesc}}}}==\n{{{{Information\n|Description = {}\n|Source      = {}\n|Date        = {}\n|Author      = {}\n|Permission  = {}\n|other_versions = {}\n}}}}\n\n{}",
            self.description.join(" "),
            self.source.join(" ; "),
            self.date.join(" "),
            self.author.join(" "),
            self.permission.join(" "),
            self.other_versions.join(" "),
            self.after.join("\n"),
        )
    }

    fn painting(&self, title: &str) -> String {
        format!(
            "=={{{{int:filedesc}}}}==\n{{{{Painting\n|Title = {title}\n|Artist = {}\n|Year = {}\n|Technique = \n|Dimensions = \n|Gallery = \n|Location = \n|Notes = {}\n|Source = {}\n|Permission = \n|Other versions = \n}}}}\n\n{}",
            self.author.join(" "),
            self.date.join(" "),
            self.description.join(" "),
            self.source.join(" ; "),
            self.after.join("\n"),
        )
    }
}

/// German Wikipedia's `Beschreibung: …` lines, as headings with English names.
fn german_fields_as_headings(text: &str) -> String {
    let text = replace_ci(&replace_ci(text, "[[Kategorie:", "[[Category:"), "[[Benutzer:", "[[User:");
    let lines = text.lines().map(|line| {
        GERMAN_FIELDS
            .iter()
            .find_map(|(field, heading)| {
                field.captures(line).map(|c| format!("== {heading} ==\n{}\n== dummy ==\n", &c[1]))
            })
            .unwrap_or_else(|| line.to_string())
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// The template in German, for German Wikipedia.
fn germanize(wikitext: &str) -> String {
    let wikitext = replace_ci(&replace_ci(wikitext, "[[Category:", "[[Kategorie:"), "[[User:", "[[Benutzer:");
    [
        ("|Description", "|Beschreibung"),
        ("|Source", "|Quelle"),
        ("|Date", "|Datum"),
        ("|Author", "|Urheber"),
        ("|Permission", "|Genehmigung"),
        ("|other_versions", "|Andere Versionen"),
    ]
    .into_iter()
    .fold(wikitext, |text, (from, to)| text.replace(from, to))
}

/// Breaks the text after each sentence outside brackets, and before each
/// template outside brackets; headings stay as they are.
fn one_sentence_per_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut depth = 0i32;
    for line in text.split_inclusive('\n') {
        if depth == 0 && HEADING.is_match(line.trim()) {
            out.push_str(line);
            continue;
        }
        let mut previous = ' ';
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if depth == 0 => out.push_str("\n{"),
                _ => out.push(c),
            }
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                // Not after a number: "12. März 2005" is a date.
                '.' if depth == 0 && chars.peek() == Some(&' ') && !previous.is_ascii_digit() => out.push('\n'),
                _ => {}
            }
            previous = c;
        }
    }
    out
}

/// `Lake_Hamun (2).jpg` → `Lake Hamun`.
fn description_from_name(image: &str) -> String {
    let name = image.rsplit_once('.').map_or(image, |(name, _)| name).replace('_', " ");
    let name = NUMBERED_COPY.replace(&name, "");
    CAMERA_NAME.replace(&name, "").trim().to_string()
}

/// All dates of the most precise kind there is; else, if `dotted`, the
/// first `d.m.yyyy`; else years.
fn dates_in(text: &str, dotted: bool) -> Vec<String> {
    let found = |re: &Regex| re.find_iter(text).map(|m| m.as_str().to_string()).collect::<Vec<_>>();
    let dotted_date = || {
        let c = DOTTED_DATE.captures(text)?;
        Some(vec![format!("{}-{:0>2}-{:0>2}", &c[3], &c[2], &c[1])])
    };
    DATES
        .iter()
        .map(found)
        .find(|dates| !dates.is_empty())
        .or_else(|| dotted.then(dotted_date).flatten())
        .unwrap_or_else(|| YEAR.captures_iter(text).map(|c| c[1].to_string()).collect())
}

/// `[http://x.org].` → `http://x.org`; anything else stays.
fn unbracket_url(source: &str) -> String {
    let trimmed = source.trim();
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed).trim();
    match trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        Some(url) if !trimmed.contains(' ') => url.to_string(),
        _ => source.to_string(),
    }
}

/// Every `from`, in any case, replaced by `to`.
fn replace_ci(text: &str, from: &str, to: &str) -> String {
    match RegexBuilder::new(&regex::escape(from)).case_insensitive(true).build() {
        Ok(re) => re.replace_all(text, regex::NoExpand(to)).into_owned(),
        Err(_) => text.to_string(),
    }
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn draft(text: &str, image: &str, exif_date: Option<&str>) -> Draft {
        let input = Input {
            language: "commons",
            project: "wikipedia",
            image,
            user: "Uploader",
            text,
            exif_date,
            own_work: false,
        };
        Draft::new(&input)
    }

    fn field<'a>(wikitext: &'a str, name: &str) -> &'a str {
        wikitext
            .lines()
            .find_map(|l| l.strip_prefix(&format!("|{name}")))
            .map_or("", |v| v.trim_start().trim_start_matches('=').trim())
    }

    #[test]
    fn self_made_file_with_exif_date() {
        let text = "[[Category:Media missing infobox template]]\n== {{int:filedesc}} ==\nA banner at Sutter Health Park\n== {{int:license-header}} ==\n{{self|cc-zero}}\n[[Category:Banners]]";
        let d = draft(text, "Banner.png", Some("2019:07:16 08:33:50"));
        assert_eq!(d.template, "Information");
        assert_eq!(field(&d.wikitext, "Description"), "A banner at Sutter Health Park");
        assert_eq!(field(&d.wikitext, "Source"), "{{own}}");
        assert_eq!(field(&d.wikitext, "Author"), "[[User:Uploader|Uploader]]");
        assert_eq!(field(&d.wikitext, "Date"), "{{According to Exif data|2019-07-16}}");
        assert!(!d.wikitext.contains("missing infobox"));
        assert!(d.wikitext.ends_with("}}\n\n== {{int:license-header}} ==\n{{self|cc-zero}}\n[[Category:Banners]]"));
    }

    #[test]
    fn sources_dates_and_language_templates() {
        let text = "{{en|A lake.}} Taken on 12. March 2005. Nice.\nSource: [http://example.org/lake].\n{{PD-old}}";
        let d = draft(text, "Lake.jpg", Some("2005:03:12 10:00:00"));
        assert_eq!(field(&d.wikitext, "Source"), "http://example.org/lake");
        assert_eq!(field(&d.wikitext, "Date"), "12. March 2005 ; {{According to Exif data|2005-03-12}}");
        assert!(d.wikitext.contains("|Description = \n{{en|A lake.}} Taken on 12. March 2005. Nice.\n|"));
        assert!(d.wikitext.ends_with("\n\n{{PD-old}}"));
    }

    #[test]
    fn urls_and_file_names() {
        let d = draft("See www.example.org/x for more", "Lake_Hamun (2).jpg", None);
        assert_eq!(field(&d.wikitext, "Source"), "http://www.example.org/x");
        assert_eq!(field(&d.wikitext, "Description"), "Lake Hamun");
        assert_eq!(description_from_name("Holiday IMG_1234.jpg"), "Holiday");
        assert_eq!(dates_in("Built (1923) by 120327-021", false), vec!["1923"]);
        assert_eq!(dates_in("Built (1923), on 3.4.1923", true), vec!["1923-04-03"]);
        assert_eq!(dates_in("Opened on June 5, 1923", false), vec!["June 5, 1923"]);
        assert_eq!(
            one_sentence_per_line("A b. C (d. e). 12. Mai\n== H. i ==\n"),
            "A b.\n C (d. e).\n 12. Mai\n== H. i ==\n"
        );
    }

    #[test]
    fn paintings() {
        let d = draft("A painting by [[Jan Vermeer]].", "Vermeer Milkmaid 1658.jpg", Some("2010:01:01 00:00:00"));
        assert_eq!(d.template, "Painting");
        assert_eq!(field(&d.wikitext, "Artist"), "[[Jan Vermeer]]");
        assert_eq!(field(&d.wikitext, "Year"), "1658");
        assert_eq!(field(&d.wikitext, "Title"), "Milkmaid");
    }

    #[test]
    fn german_wikipedia() {
        let input = Input {
            language: "de",
            project: "wikipedia",
            image: "Rathaus.jpg",
            user: "Hochlader",
            text: "Beschreibung: Das Rathaus, 3.4.1999\nQuelle: selbst fotografiert\nUrheber: [[Benutzer:Hochlader]]\n[[Kategorie:Rathaus]]",
            exif_date: None,
            own_work: false,
        };
        let d = Draft::new(&input);
        assert_eq!(field(&d.wikitext, "Quelle"), "selbst fotografiert");
        assert_eq!(field(&d.wikitext, "Urheber"), "[[Benutzer:Hochlader]]");
        assert_eq!(field(&d.wikitext, "Datum"), "1999-04-03");
        assert!(d.wikitext.ends_with("[[Kategorie:Rathaus]]"));
    }

    #[test]
    fn existing_templates() {
        assert!(HAS_TEMPLATE.is_match("{{Information\n|Description=x}}"));
        assert!(HAS_TEMPLATE.is_match("x {{ art photo |a=b}}"));
        assert!(HAS_TEMPLATE.is_match("{{Template:Map}}"));
        assert!(!HAS_TEMPLATE.is_match("{{Mapillary|1}} {{Informationen}}"));
    }

    #[test]
    fn file_info_from_the_api() {
        let page = serde_json::json!({
            "revisions": [{"timestamp": "2026-05-06T11:06:39Z", "slots": {"main": {"content": "<!-- moved -->\nText "}}}],
            "imageinfo": [{"user": "Someone", "commonmetadata": [
                {"name": "DateTime", "value": "2019:07:18 14:56:27"},
                {"name": "DateTimeOriginal", "value": "2019:07:16 08:33:50"},
            ]}],
        });
        let info = parse_file_info(&page).unwrap();
        assert_eq!((info.text.as_str(), info.uploader.as_str()), ("Text", "Someone"));
        assert_eq!(info.edit_time, "20260506110639");
        assert_eq!(info.exif_date.as_deref(), Some("2019:07:16 08:33:50"));
        assert!(parse_file_info(&serde_json::json!({"missing": true})).is_none());
    }

    #[tokio::test]
    async fn empty_form() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/add_information/?language=commons").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"image\""));
        assert_eq!(get(dir.path(), "/add_information/?image=X.jpg&language=evil.com%23").await.status.as_u16(), 400);
    }
}

//! `file-siblings.php`: for a file, its neighbours in each of its
//! categories and in the galleries (article pages) that use it.

use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, percent_encode, ucfirst, wiki_urlencode};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use wikimisc::escape::html;
use wikimisc::mysql_async::Value as DbValue;
use wikimisc::mysql_async::prelude::Queryable;

const THUMB_SIZE: u32 = 100;
/// A file can be used on thousands of pages; more rows than this are of no use.
const MAX_GALLERIES: usize = 50;
/// A file can be in very many categories; keep the page responsive.
const MAX_CATEGORIES: usize = 200;
const DEFAULT_LANGUAGE: &str = "commons";

const IGNORED_CATEGORIES: &[(&str, &[&str])] = &[
    ("de.wikipedia", &["GFDL", "PD", "CC", "In SVG konvertieren", "Public-Domain", "Frei-Bild", "Bild-", "Wikipedia:"]),
    (
        "commons.wikimedia",
        &[
            "GFDL",
            "PD",
            "CC",
            "License migration redundant",
            "Self-published work",
            "Media lacking author information",
            "Admin reviewed",
            "Media with locations",
            "Images of low quality",
            "Media requiring renaming",
            "Files moved from",
            "License migration",
            "Files by",
            "Images without source",
            "Flickr images",
        ],
    ),
];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    project: Option<String>,
    #[serde(default)]
    file: String,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    sort_by_date: bool,
    sib_cat: Option<String>,
    sib_gal: Option<String>,
    mode: Option<String>,
    #[serde(default)]
    message: String,
    uselang: Option<String>,
}

fn default_language() -> String {
    DEFAULT_LANGUAGE.to_string()
}

impl Request {
    fn project(&self) -> String {
        crate::wiki::effective_project(&self.language, self.project.as_deref())
    }
}

fn flag(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

#[derive(Debug, Clone)]
struct Thumb {
    url: String,
    src: Option<String>,
    width: u32,
    height: u32,
    label: String,
}

struct Row {
    heading: String,
    heading_url: String,
    note: String,
    prev: Option<Thumb>,
    next: Option<Thumb>,
}

struct Section {
    kind: &'static str,
    rows: Vec<Row>,
}

#[derive(Template)]
#[template(path = "tools/file_siblings.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    project: String,
    file: String,
    sections: Vec<Section>,
    truncated: bool,
    has_rows: bool,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let project = request.project();
    let file = file_title(&request.file);
    let mode = request.mode.as_deref().unwrap_or("html");
    let uselang = request.uselang.as_deref().filter(|l| crate::wiki::is_language_code(l)).unwrap_or_default();
    let mut page = Page {
        title: "File siblings",
        request: &request,
        project: project.clone(),
        file: file.clone(),
        sections: vec![],
        truncated: false,
        has_rows: false,
        error: None,
    };
    if file.is_empty() {
        return Ok(render(&page)?.into_response());
    }
    let wiki = match crate::wiki::wiki_and_project(&request.language, request.project.as_deref()) {
        Ok((_, _, wiki)) => wiki,
        Err(e) => {
            page.error = Some(format!("Not a wiki: {e}."));
            return Ok(render(&page)?.into_response());
        }
    };

    let loaded = load(&state, &wiki, &request, &file).await?;
    page.sections = loaded.sections;
    page.truncated = loaded.truncated;
    page.has_rows = page.sections.iter().any(|section| !section.rows.is_empty());

    let response = match mode {
        "text" => {
            let fragment = fragment(&wiki, &page.sections, &request.message, uselang);
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], fragment).into_response()
        }
        "json" => {
            let fragment = fragment(&wiki, &page.sections, &request.message, uselang);
            let json = serde_json::to_string(&fragment).map_err(|e| AppError::Internal(format!("JSON: {e}")))?;
            let json = json.replace('<', "\\u003c").replace('>', "\\u003e");
            let body = format!("file_siblings_callback({json});");
            ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], body).into_response()
        }
        _ => render(&page)?.into_response(),
    };
    Ok(response)
}

struct Loaded {
    sections: Vec<Section>,
    truncated: bool,
}

async fn load(state: &AppState, wiki: &Wiki, request: &Request, file: &str) -> Result<Loaded, AppError> {
    let mut conn = state.replicas.conn(wiki, &["page", "categorylinks", "imagelinks", "linktarget"]).await?;
    let mut sections = vec![];
    let mut truncated = false;
    if flag(request.sib_cat.as_deref()) || (request.sib_cat.is_none() && request.sib_gal.is_none()) {
        let ignored = ignored_categories(wiki);
        let (rows, more) = category_siblings(&mut conn, wiki, file, request.sort_by_date, &ignored).await?;
        truncated |= more;
        sections.push(Section { kind: "categories", rows });
    }
    if flag(request.sib_gal.as_deref()) || (request.sib_cat.is_none() && request.sib_gal.is_none()) {
        sections.push(Section { kind: "galleries", rows: gallery_siblings(&mut conn, wiki, file).await? });
    }

    let files: Vec<String> = sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .flat_map(|row| [row.prev.as_ref(), row.next.as_ref()])
        .flatten()
        .map(|thumb| thumb.label.replace(' ', "_"))
        .collect();
    let sizes = image_sizes(state, wiki, &files).await?;
    for section in &mut sections {
        for row in &mut section.rows {
            for thumb in [row.prev.as_mut(), row.next.as_mut()].into_iter().flatten() {
                *thumb = thumbnail(wiki, &thumb.label.replace(' ', "_"), &sizes);
            }
        }
    }
    Ok(Loaded { sections, truncated })
}

fn ignored_categories(wiki: &Wiki) -> Vec<String> {
    let key = format!("{}.{}", wiki.dbname().trim_end_matches("wiki"), project_of(wiki));
    IGNORED_CATEGORIES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, categories)| categories.iter().map(|c| c.to_lowercase()).collect())
        .unwrap_or_default()
}

fn project_of(wiki: &Wiki) -> &'static str {
    if wiki.host().contains(".wikimedia.org") { "wikimedia" } else { "wikipedia" }
}

struct CategoryRow {
    target: u64,
    title: String,
    from: u64,
    sortkey: Vec<u8>,
    timestamp: String,
}

async fn category_siblings(
    conn: &mut wikimisc::mysql_async::Conn,
    wiki: &Wiki,
    file: &str,
    by_date: bool,
    ignored: &[String],
) -> Result<(Vec<Row>, bool), AppError> {
    let rows: Vec<(u64, String, u64, Vec<u8>, String)> = conn
        .exec(
            "SELECT cl_target_id, lt_title, cl_from, cl_sortkey, CAST(cl_timestamp AS CHAR)
             FROM page JOIN categorylinks ON cl_from=page_id JOIN linktarget ON lt_id=cl_target_id
             WHERE page_namespace=6 AND page_title=? ORDER BY lt_title LIMIT ?",
            (file, (MAX_CATEGORIES + 1) as u64),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let truncated = rows.len() > MAX_CATEGORIES;
    let mut result = vec![];
    for row in rows.into_iter().take(MAX_CATEGORIES) {
        if is_ignored_category(&row.1, ignored) {
            continue;
        }
        let category = CategoryRow { target: row.0, title: row.1, from: row.2, sortkey: row.3, timestamp: row.4 };
        let prev = neighbour(conn, &category, by_date, true).await?;
        let next = neighbour(conn, &category, by_date, false).await?;
        if prev.is_none() && next.is_none() {
            continue;
        }
        let title = display_title(&category.title);
        result.push(Row {
            heading: format!("Category:{title}"),
            heading_url: wiki.page_url(&format!("Category:{title}")),
            note: String::new(),
            prev: prev.map(|file| Thumb { label: display_title(&file), ..empty_thumb(&file) }),
            next: next.map(|file| Thumb { label: display_title(&file), ..empty_thumb(&file) }),
        });
    }
    Ok((result, truncated))
}

async fn neighbour(
    conn: &mut wikimisc::mysql_async::Conn,
    category: &CategoryRow,
    by_date: bool,
    before: bool,
) -> Result<Option<String>, AppError> {
    let (op, dir) = if before { ("<", "DESC") } else { (">", "ASC") };
    let column = if by_date { "cl_timestamp" } else { "cl_sortkey" };
    let sql = format!(
        "SELECT page_title FROM categorylinks JOIN page ON page_id=cl_from
         WHERE cl_target_id=? AND cl_type='file' AND ({column} {op} ? OR ({column}=? AND cl_from {op} ?))
         ORDER BY {column} {dir}, cl_from {dir} LIMIT 1"
    );
    let value =
        if by_date { DbValue::from(category.timestamp.as_str()) } else { DbValue::from(category.sortkey.clone()) };
    let row: Option<(String,)> = conn
        .exec_first(sql, (category.target, value.clone(), value, category.from))
        .await
        .map_err(anyhow::Error::from)?;
    Ok(row.map(|(title,)| title))
}

async fn gallery_siblings(
    conn: &mut wikimisc::mysql_async::Conn,
    wiki: &Wiki,
    file: &str,
) -> Result<Vec<Row>, AppError> {
    let pages: Vec<(u64, String)> = conn
        .exec(
            "SELECT page_id, page_title FROM imagelinks JOIN linktarget ON lt_id=il_target_id JOIN page ON page_id=il_from
             WHERE lt_namespace=6 AND lt_title=? AND il_from_namespace=0 ORDER BY page_title LIMIT ?",
            (file, MAX_GALLERIES as u64),
        )
        .await
        .map_err(anyhow::Error::from)?;
    if pages.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
    let files: Vec<(u64, String)> = select_in(
        conn,
        "SELECT il_from, lt_title FROM imagelinks JOIN linktarget ON lt_id=il_target_id
         WHERE lt_namespace=6 AND il_from IN",
        "ORDER BY lt_title",
        &[],
        &ids,
    )
    .await?;
    let mut by_page: HashMap<u64, Vec<String>> = HashMap::new();
    for (page_id, title) in files {
        by_page.entry(page_id).or_default().push(title);
    }

    Ok(pages
        .into_iter()
        .filter_map(|(page_id, page_title)| {
            let on_page = by_page.get(&page_id)?;
            let prev = on_page.iter().rev().find(|title| title.as_str() < file).cloned();
            let next = on_page.iter().find(|title| title.as_str() > file).cloned();
            (prev.is_some() || next.is_some()).then(|| Row {
                heading: display_title(&page_title),
                heading_url: wiki.page_url(&display_title(&page_title)),
                note: format!(" [{}]", on_page.len()),
                prev: prev.map(|file| Thumb { label: display_title(&file), ..empty_thumb(&file) }),
                next: next.map(|file| Thumb { label: display_title(&file), ..empty_thumb(&file) }),
            })
        })
        .collect())
}

fn empty_thumb(file: &str) -> Thumb {
    Thumb { url: String::new(), src: None, width: THUMB_SIZE, height: THUMB_SIZE, label: display_title(file) }
}

async fn image_sizes(state: &AppState, wiki: &Wiki, files: &[String]) -> Result<HashMap<String, (u32, u32)>, AppError> {
    if files.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = state.replicas.conn(wiki, &["image"]).await?;
    let rows: Vec<(String, u32, u32)> =
        select_in(&mut conn, "SELECT img_name, img_width, img_height FROM image WHERE img_name IN", "", &[], files)
            .await?;
    Ok(rows.into_iter().map(|(name, width, height)| (name, (width, height))).collect())
}

/// A thumbnail link, scaled to a square of [`THUMB_SIZE`].
fn thumbnail(wiki: &Wiki, file: &str, sizes: &HashMap<String, (u32, u32)>) -> Thumb {
    let mut thumb = empty_thumb(file);
    thumb.url = wiki.page_url(&format!("File:{}", display_title(file)));
    let Some(&(width, height)) = sizes.get(file) else {
        thumb.label = format!("???{}", display_title(file));
        return thumb;
    };
    if width == 0 || height == 0 {
        thumb.label = format!("???{}", display_title(file));
        return thumb;
    }
    let (width, height) = if width > height {
        (THUMB_SIZE, height * THUMB_SIZE / width)
    } else {
        (width * THUMB_SIZE / height, THUMB_SIZE)
    };
    let mut src = format!("//{}/wiki/Special:Redirect/file/{}", wiki.host(), wiki_urlencode(file));
    if !file.to_lowercase().ends_with(".gif") {
        src.push_str(&format!("?width={width}"));
    }
    thumb.src = Some(src);
    thumb.width = width;
    thumb.height = height;
    thumb
}

fn fragment(wiki: &Wiki, sections: &[Section], message: &str, uselang: &str) -> String {
    let mut rows = String::new();
    for section in sections {
        let colour = match section.kind {
            "categories" => "#99E0FF",
            _ => "#62FDCE",
        };
        for row in &section.rows {
            rows.push_str(&format!(
                "<tr><th colspan='2' style='background:{colour}'><a href='{}'>{}</a>{}</th></tr><tr>",
                html(&row.heading_url),
                html(&row.heading),
                html(&row.note)
            ));
            for thumb in [&row.prev, &row.next] {
                rows.push_str("<td align='center' valign='center'>");
                match thumb {
                    Some(thumb) => rows.push_str(&thumb_html(wiki, thumb, uselang)),
                    None => rows.push_str(&empty_thumb_html()),
                }
                rows.push_str("</td>");
            }
            rows.push_str("</tr>");
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let top = if message.is_empty() {
        "<tr><th align='center'>&larr;</th><th align='center'>&rarr;</th></tr>".to_string()
    } else {
        format!("<tr><th colspan='2' align='center'>{}</th></tr>", html(message))
    };
    format!(
        "<div id='file_siblings_div' style='border:2px solid #888888;background:white;margin:5px;padding:2px;position:absolute;right:0px'>\
         <div style='text-align:right'><a href='#' onclick='jQuery(\"#file_siblings_table\").toggle();return false;'>&#8632;</a>&nbsp;\
         <a href='#' onclick='jQuery(\"#file_siblings_div\").remove();return false;'>X</a></div>\
         <table id='file_siblings_table'>{top}{rows}</table></div>"
    )
}

fn thumb_html(wiki: &Wiki, thumb: &Thumb, uselang: &str) -> String {
    let mut url = wiki.page_url(&format!("File:{}", thumb.label));
    if !uselang.is_empty() {
        url.push_str(&format!("?uselang={}", percent_encode(uselang)));
    }
    match &thumb.src {
        Some(src) => format!(
            "<a href='{}'><img border='0' src='{}' width='{}px' height='{}px' alt='{}' title='{}' /></a>",
            html(&url),
            html(src),
            thumb.width,
            thumb.height,
            html(&thumb.label),
            html(&thumb.label)
        ),
        None => format!("{}<br/>{}", "???", html(&thumb.label)),
    }
}

fn empty_thumb_html() -> String {
    format!("<div style='background:#EEEEEE;width:{THUMB_SIZE}px;height:{THUMB_SIZE}px'></div>")
}

/// `File:Some file.jpg` -> `Some_file.jpg`.
fn file_title(input: &str) -> String {
    let input = input.trim();
    let input = match input.split_once(':') {
        Some((namespace, rest))
            if namespace.eq_ignore_ascii_case("file") || namespace.eq_ignore_ascii_case("image") =>
        {
            rest
        }
        _ => input,
    };
    ucfirst(&input.trim().replace(' ', "_")).into_owned()
}

fn is_ignored_category(category: &str, ignored: &[String]) -> bool {
    let category = category.replace('_', " ").to_lowercase();
    ignored.iter().any(|prefix| {
        category == *prefix
            || category.starts_with(&format!("{prefix}-"))
            || category.starts_with(&format!("{prefix} "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_titles() {
        assert_eq!(file_title("File:some file.jpg"), "Some_file.jpg");
        assert_eq!(file_title(" image:Some_file.jpg "), "Some_file.jpg");
        assert_eq!(file_title("Some file.jpg"), "Some_file.jpg");
        assert_eq!(file_title(""), "");
    }

    #[test]
    fn ignored_categories_prefix_match() {
        let ignored = vec!["pd".to_string(), "files by".to_string()];
        assert!(is_ignored_category("PD", &ignored));
        assert!(is_ignored_category("PD-art", &ignored));
        assert!(is_ignored_category("Files_by_subject", &ignored));
        assert!(!is_ignored_category("PDF files", &ignored));
    }

    #[test]
    fn thumbnails_keep_the_aspect_ratio() {
        let wiki = Wiki::new("commons", "wikimedia").unwrap();
        let sizes = HashMap::from([("A.jpg".to_string(), (200, 100))]);
        let thumb = thumbnail(&wiki, "A.jpg", &sizes);
        assert_eq!((thumb.width, thumb.height), (100, 50));
        assert!(thumb.src.unwrap().ends_with("?width=100"));
        let gif = thumbnail(&wiki, "B.gif", &HashMap::from([("B.gif".to_string(), (200, 100))]));
        assert!(!gif.src.unwrap().contains("?width"));
    }
}

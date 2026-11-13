//! `filedupes.php`: files marked as duplicates, either among one user's
//! uploads (same size, dimensions and bit depth) or in categories (a
//! `{{duplicate}}` template in the description).

use crate::mediawiki::{PageContent, page_contents, title_key};
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, percent_encode, ucfirst, wiki_urlencode};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::Value as DbValue;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_CATEGORIES: usize = 20;
const MAX_LIMIT: usize = 100;
const DEFAULT_LIMIT: usize = 25;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    project: Option<String>,
    #[serde(default)]
    username: String,
    #[serde(default, rename = "identicalsonly", deserialize_with = "crate::web::php_bool")]
    identicals_only: bool,
    #[serde(default, deserialize_with = "crate::web::php_bool")]
    showdesc: bool,
    #[serde(default)]
    category: String,
    startat: Option<String>,
    /// The name the form used before the fix; still accepted for bookmarks.
    start: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
    doit: Option<String>,
}

fn default_language() -> String {
    "commons".to_string()
}

impl Request {
    fn startat(&self) -> String {
        self.startat.as_deref().or(self.start.as_deref()).unwrap_or_default().trim().to_string()
    }

    fn paging(&self) -> Paging {
        Paging {
            limit: self
                .limit
                .as_deref()
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, MAX_LIMIT),
            offset: self.offset.as_deref().and_then(|n| n.trim().parse().ok()).unwrap_or(1).max(1),
            startat: self.startat(),
        }
    }

    fn categories(&self) -> Vec<String> {
        self.category
            .replace('|', "\n")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let line = crate::mediawiki::strip_namespace(line, &["category"]).unwrap_or(line);
                db_title(&ucfirst(line))
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct Paging {
    limit: usize,
    offset: usize,
    startat: String,
}

#[derive(Clone, Debug)]
struct ImageInfo {
    name: String,
    size: u64,
    width: u32,
    height: u32,
    bits: Option<u32>,
}

impl ImageInfo {
    fn signature(&self) -> String {
        format!("{}x{}x{}x{}", self.size, self.width, self.height, self.bits.unwrap_or(0))
    }

    fn extension(&self) -> Option<String> {
        self.name.rsplit_once('.').map(|(_, extension)| extension.to_lowercase())
    }
}

struct EditForm {
    action: String,
    text: String,
    summary: String,
    edittime: String,
    starttime: String,
}

struct FileCard {
    name: String,
    page_url: String,
    file_url: String,
    thumb_url: String,
    delete_url: String,
    links_url: String,
    size: u64,
    width: u32,
    height: u32,
    age_days: Option<i64>,
    description: String,
    superseded: Option<EditForm>,
    universal: Option<EditForm>,
}

struct Pair {
    original: FileCard,
    duplicate: FileCard,
    same_size: bool,
    exact: bool,
}

struct CategoryResult {
    name: String,
    pairs: Vec<Pair>,
}

struct UserRow {
    card: FileCard,
    duplicate: bool,
    others: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "tools/filedupes.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    language: String,
    project: String,
    identicals_only: bool,
    show_description: bool,
    paging: Paging,
    user_rows: Vec<UserRow>,
    user_found: bool,
    categories: Vec<CategoryResult>,
    next_url: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = request.language.trim().to_lowercase();
    let project = crate::wiki::effective_project(&language, request.project.as_deref());
    let paging = request.paging();
    let mut page = Page {
        title: "File duplicates",
        request: &request,
        language: language.clone(),
        project: project.clone(),
        identicals_only: request.identicals_only,
        show_description: request.showdesc,
        paging: paging.clone(),
        user_rows: vec![],
        user_found: false,
        categories: vec![],
        next_url: None,
        message: None,
        error: None,
    };
    if request.doit.is_none() {
        return render(&page);
    }
    let wiki = match Wiki::new(&language, &project) {
        Ok(wiki) => wiki,
        Err(e) => {
            page.error = Some(format!("Not a wiki: {e}."));
            return render(&page);
        }
    };
    let categories = request.categories();
    if !request.username.trim().is_empty() {
        let (rows, found) = user_mode(&state, &wiki, &request, request.identicals_only, request.showdesc).await?;
        page.user_rows = rows;
        page.user_found = found;
        if page.user_rows.is_empty() {
            page.message = Some("No duplicates found.".to_string());
        }
    } else if !categories.is_empty() {
        page.categories = category_mode(&state, &wiki, &categories, &paging).await?;
        page.next_url = Some(next_url(&request, &paging));
    } else {
        page.message = Some("Please give a user name or at least one category.".to_string());
    }
    render(&page)
}

async fn user_mode(
    state: &AppState,
    wiki: &Wiki,
    request: &Request,
    identicals_only: bool,
    show_description: bool,
) -> Result<(Vec<UserRow>, bool), AppError> {
    let mut conn = state.replicas.conn(wiki, &["image", "actor"]).await?;
    let rows: Vec<(String, u64, u32, u32, Option<u32>)> = conn
        .exec(
            "SELECT img_name, img_size, img_width, img_height, img_bits FROM image
             WHERE img_actor IN (SELECT actor_id FROM actor WHERE actor_name=?) ORDER BY img_name",
            (request.username.trim(),),
        )
        .await
        .map_err(anyhow::Error::from)?;
    drop(conn);
    let infos: Vec<ImageInfo> = rows
        .into_iter()
        .map(|(name, size, width, height, bits)| ImageInfo { name, size, width, height, bits })
        .collect();
    let mut infos = infos;
    infos.sort_by(|a, b| a.signature().cmp(&b.signature()).then_with(|| a.name.cmp(&b.name)));
    let mut groups: BTreeMap<String, Vec<&ImageInfo>> = BTreeMap::new();
    for info in &infos {
        groups.entry(info.signature()).or_default().push(info);
    }
    let duplicates: HashSet<&String> =
        groups.iter().filter(|(_, files)| files.len() > 1).map(|(signature, _)| signature).collect();

    let wanted: Vec<String> =
        infos.iter().filter(|info| duplicates.contains(&info.signature())).map(|info| info.name.clone()).collect();
    let contents = if show_description { descriptions(state, wiki, &wanted).await? } else { HashMap::new() };

    let mut result = vec![];
    let mut found = false;
    for info in &infos {
        let duplicate = duplicates.contains(&info.signature());
        if !duplicate && identicals_only {
            continue;
        }
        if duplicate {
            found = true;
        }
        let card = file_card(wiki, info, contents.get(&info.name), show_description, None, None)?;
        let others = if duplicate {
            groups[&info.signature()]
                .iter()
                .filter(|other| other.name != info.name)
                .map(|other| {
                    let display = display_title(&other.name);
                    let url = delete_url(wiki, &info.name, &other.name);
                    (display, url)
                })
                .collect()
        } else {
            vec![]
        };
        result.push(UserRow { card, duplicate, others });
    }
    Ok((result, found))
}

async fn category_mode(
    state: &AppState,
    wiki: &Wiki,
    categories: &[String],
    paging: &Paging,
) -> Result<Vec<CategoryResult>, AppError> {
    let mut remaining = paging.limit;
    let mut results = vec![];

    for category in categories.iter().take(MAX_CATEGORIES) {
        if remaining == 0 {
            break;
        }
        let mut conn = state.replicas.conn(wiki, &["page", "categorylinks", "linktarget"]).await?;
        let mut sql = "SELECT p.page_title FROM page p
            JOIN categorylinks cl ON cl.cl_from=p.page_id
            JOIN linktarget lt ON lt.lt_id=cl.cl_target_id
            WHERE p.page_namespace=6 AND lt.lt_namespace=14 AND lt.lt_title=?"
            .to_string();
        let mut params = vec![DbValue::from(category.clone())];
        if !paging.startat.is_empty() {
            sql.push_str(" AND p.page_title>=?");
            params.push(DbValue::from(paging.startat.replace(' ', "_")));
        }
        sql.push_str(" ORDER BY p.page_title LIMIT ? OFFSET ?");
        params.push(DbValue::from(remaining as u64));
        params.push(DbValue::from(paging.offset.saturating_sub(1) as u64));
        let images: Vec<(String,)> =
            conn.exec(sql, wikimisc::mysql_async::Params::Positional(params)).await.map_err(anyhow::Error::from)?;
        drop(conn);
        if images.is_empty() {
            continue;
        }
        let names: Vec<String> = images.into_iter().map(|(name,)| name).collect();
        let contents = descriptions(state, wiki, &names).await?;
        let mut targets = vec![];
        for name in &names {
            if let Some(content) = contents.get(name)
                && let Some(target) = find_duplicate(&content.text)
                && !targets.contains(&target)
            {
                targets.push(target);
            }
        }
        let target_contents = descriptions(state, wiki, &targets).await?;
        let mut image_conn = state.replicas.conn(wiki, &["image"]).await?;
        let mut all_infos = image_infos(&mut image_conn, &names).await?;
        all_infos.extend(image_infos(&mut image_conn, &targets).await?);
        drop(image_conn);

        let mut pairs = vec![];
        for name in &names {
            if remaining == 0 {
                break;
            }
            let Some(content) = contents.get(name) else { continue };
            let Some(target) = find_duplicate(&content.text) else { continue };
            let (Some(original_info), Some(duplicate_info)) = (all_infos.get(name), all_infos.get(&target)) else {
                continue;
            };
            let exact = original_info.extension() == duplicate_info.extension()
                && original_info.width == duplicate_info.width
                && original_info.height == duplicate_info.height;
            let same_size = original_info.size == duplicate_info.size;
            let summary_reason = format!(
                "Was in category \"{}\", {}",
                display_title(category),
                if same_size { "exact duplicate" } else { "although NOT an exact duplicate" }
            );
            let universal =
                universal_form(wiki, &summary_reason, &original_info.name, &duplicate_info.name, &toy_note());
            let mut original = file_card(
                wiki,
                original_info,
                Some(content),
                true,
                (!exact).then(|| superseded_form(wiki, original_info, content, &toy_note())).flatten(),
                Some(universal),
            )?;
            let mut duplicate = file_card(wiki, duplicate_info, target_contents.get(&target), true, None, None)?;
            original.delete_url = delete_url(wiki, &original_info.name, &duplicate_info.name);
            duplicate.delete_url = delete_url(wiki, &duplicate_info.name, &original_info.name);
            pairs.push(Pair { original, duplicate, same_size, exact });
            remaining -= 1;
        }
        results.push(CategoryResult { name: display_title(category), pairs });
    }
    Ok(results)
}

async fn image_infos(conn: &mut Conn, names: &[String]) -> Result<HashMap<String, ImageInfo>, AppError> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, u64, u32, u32, Option<u32>)> = select_in(
        conn,
        "SELECT img_name, img_size, img_width, img_height, img_bits FROM image WHERE img_name IN",
        "",
        &[],
        names,
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, size, width, height, bits)| (name.clone(), ImageInfo { name, size, width, height, bits }))
        .collect())
}

async fn descriptions(
    state: &AppState,
    wiki: &Wiki,
    names: &[String],
) -> Result<HashMap<String, PageContent>, AppError> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let titles: Vec<String> = names.iter().map(|name| format!("File:{}", display_title(name))).collect();
    let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
    let contents = page_contents(&state.http, wiki, &titles).await?;
    Ok(names
        .iter()
        .filter_map(|name| {
            let key = title_key(&format!("File:{}", display_title(name)));
            contents.get(&key).cloned().map(|content| (name.clone(), content))
        })
        .collect())
}

fn file_card(
    wiki: &Wiki,
    info: &ImageInfo,
    content: Option<&PageContent>,
    show_description: bool,
    superseded: Option<EditForm>,
    universal: Option<EditForm>,
) -> Result<FileCard, AppError> {
    let display = display_title(&info.name);
    Ok(FileCard {
        name: display.clone(),
        page_url: wiki.page_url(&format!("File:{display}")),
        file_url: format!("https://{}/wiki/Special:Redirect/file/{}", wiki.host(), wiki_urlencode(&info.name)),
        thumb_url: format!(
            "https://{}/wiki/Special:Redirect/file/{}?width=150",
            wiki.host(),
            wiki_urlencode(&info.name)
        ),
        delete_url: delete_url(wiki, &info.name, ""),
        links_url: format!(
            "{}?title=Special:WhatLinksHere&target={}",
            wiki.index_url(),
            percent_encode(&format!("File:{display}"))
        ),
        size: info.size,
        width: info.width,
        height: info.height,
        age_days: content.and_then(|content| age_days(&content.timestamp)),
        description: if show_description { content.map(|c| c.text.clone()).unwrap_or_default() } else { String::new() },
        superseded,
        universal,
    })
}

fn delete_url(wiki: &Wiki, file: &str, duplicate: &str) -> String {
    let reason = if duplicate.is_empty() {
        "Duplicate".to_string()
    } else {
        format!("Duplicate of [[:File:{}]]", display_title(duplicate))
    };
    format!(
        "{}?title=File:{}&image={}&action=delete&wpReason={}",
        wiki.index_url(),
        wiki_urlencode(&display_title(file)),
        wiki_urlencode(&display_title(file)),
        percent_encode(&reason)
    )
}

fn superseded_form(wiki: &Wiki, info: &ImageInfo, content: &PageContent, note: &str) -> Option<EditForm> {
    let (head, rest) = content.text.split_once("{{duplicate").or_else(|| content.text.split_once("{{Duplicate"))?;
    let text = format!("{head}{{{{superseded{rest}");
    Some(EditForm {
        action: format!(
            "{}?title={}&action=edit",
            wiki.index_url(),
            wiki_urlencode(&format!("File:{}", display_title(&info.name)))
        ),
        text,
        summary: format!(
            "{{{{duplicate}}}} is only for exactly identical images, please don't use it for different file types, they cannot be identical{note}"
        ),
        edittime: content.timestamp.clone(),
        starttime: start_time(),
    })
}

fn universal_form(wiki: &Wiki, reason: &str, original: &str, duplicate: &str, note: &str) -> EditForm {
    EditForm {
        action: format!(
            "{}?title={}&action=edit&section=new",
            wiki.index_url(),
            wiki_urlencode("User:CommonsDelinker/commands")
        ),
        text: format!(
            "{{{{universal replace|{}|{}|reason={}}}}}",
            display_title(original),
            display_title(duplicate),
            reason
        ),
        summary: format!("Dummy section, heading can be deleted{note}"),
        edittime: String::new(),
        starttime: start_time(),
    }
}

fn toy_note() -> String {
    " (using [https://magnustools.toolforge.org/filedupes/ FileDupes])".to_string()
}

fn age_days(timestamp: &str) -> Option<i64> {
    let timestamp = DateTime::parse_from_rfc3339(timestamp).ok()?.with_timezone(&Utc);
    Some(Utc::now().signed_duration_since(timestamp).num_days())
}

/// The first target of a `{{duplicate}}`-like template in `text`.
fn find_duplicate(text: &str) -> Option<String> {
    const TAGS: &[&str] = &["duplicate", "bad name", "badname", "dupe"];
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let body = &after[..end];
        if let Some((name, args)) = body.split_once('|')
            && TAGS.contains(&name.trim().to_lowercase().as_str())
        {
            let target = args.split('|').next().unwrap_or_default().trim();
            let target = target.strip_prefix("1=").unwrap_or(target);
            let target = target.trim_matches(['[', ']']);
            if !target.is_empty() {
                return Some(normalize_file(target));
            }
        }
        rest = &after[end + 2..];
    }
    None
}

fn normalize_file(input: &str) -> String {
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

fn next_url(request: &Request, paging: &Paging) -> String {
    let offset = paging.offset + paging.limit;
    format!(
        "/filedupes/?language={}&project={}&category={}&startat={}&limit={}&offset={}&doit=1{}{}",
        percent_encode(&request.language),
        percent_encode(&crate::wiki::effective_project(&request.language, request.project.as_deref())),
        percent_encode(&request.category),
        percent_encode(&paging.startat),
        paging.limit,
        offset,
        if request.identicals_only { "&identicalsonly=1" } else { "" },
        if request.showdesc { "&showdesc=1" } else { "" }
    )
}

/// Edge-case helper for the time left in a form, as the old edit button had it.
fn start_time() -> String {
    (Utc::now() + Duration::hours(12)).format("%Y%m%d%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_templates_are_recognised() {
        assert_eq!(find_duplicate("x {{duplicate|File:B.jpg}} y").as_deref(), Some("B.jpg"));
        assert_eq!(find_duplicate("x {{Dupe|B.jpg|reason}} y").as_deref(), Some("B.jpg"));
        assert_eq!(find_duplicate("x {{bad name|1=Image:B.jpg}} y").as_deref(), Some("B.jpg"));
        assert_eq!(find_duplicate("x {{duplicate}} y"), None);
        assert_eq!(find_duplicate("x {{other|B.jpg}} y"), None);
    }

    #[test]
    fn request_parameters() {
        let request = Request {
            language: "commons".into(),
            project: None,
            username: String::new(),
            identicals_only: false,
            showdesc: false,
            category: "A|Category:B\nC".into(),
            startat: Some(" X ".into()),
            start: None,
            limit: Some("1000".into()),
            offset: Some("0".into()),
            doit: Some("1".into()),
        };
        let paging = request.paging();
        assert_eq!(crate::wiki::effective_project(&request.language, request.project.as_deref()), "wikimedia");
        assert_eq!(paging.limit, MAX_LIMIT);
        assert_eq!(paging.offset, 1);
        assert_eq!(paging.startat, "X");
        assert_eq!(request.categories(), vec!["A", "B", "C"]);
    }

    #[test]
    fn image_signatures_are_comparable() {
        let info = ImageInfo { name: "A.jpg".into(), size: 10, width: 2, height: 3, bits: None };
        assert_eq!(info.signature(), "10x2x3x0");
        assert_eq!(info.extension().as_deref(), Some("jpg"));
    }
}

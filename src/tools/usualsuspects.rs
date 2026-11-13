//! `usualsuspects.php`: for the files in a category, who uploaded them
//! ("Bad Boys"), or how long their description pages have been left alone
//! ("Bad Old Ones").

use crate::mediawiki::Api;
use crate::regex;
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, ucfirst, wiki_urlencode};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

const NS_FILE: i64 = 6;
const NS_FILE_TALK: i64 = 7;
/// Bad Boys: files examined in the category.
const MAX_UPLOADER_FILES: usize = 10_000;
/// Bad Old Ones: files whose pages and histories are loaded.
const MAX_OLD_FILES: usize = 1_000;
/// Files per API request; each also needs its talk page, so this is 50 titles.
const API_BATCH: usize = 25;
const DEFAULT_LANGUAGE: &str = "de";
const DEFAULT_PROJECT: &str = "wikipedia";
const DEFAULT_MIN_DAYS: u32 = 14;
const DELETE_REASONS: [&str; 5] =
    ["not edited for %d days", "no source", "no license", "no permission", "replaceable fair use"];
const REMOVE_TAGS_SUMMARY: &str = "Image description is satisfactory now, removing \"no source/license\" tag(s)";

/// Wikitext without `{{no source…}}`, `{{no license…}}` and
/// `{{dw no source…}}` templates.
static NO_TAGS: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\{\{\s*(dw[ _])?no[ _](source|license)[^{}]*\}\}\s*"));

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    language: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    category: String,
    min_days: Option<String>,
    limit: Option<String>,
    badboys: Option<String>,
    badoldones: Option<String>,
}

struct Form {
    language: String,
    project: String,
    category: String,
    min_days: u32,
    old_only: bool,
}

struct FileLink {
    name: String,
    url: String,
}

struct Uploader {
    name: String,
    url: String,
    files: Vec<FileLink>,
}

struct BadBoys {
    category: String,
    category_url: String,
    files: usize,
    capped: bool,
    uploaders: Vec<Uploader>,
}

struct Link {
    label: String,
    url: String,
}

struct UsageLine {
    wiki: String,
    wiki_url: String,
    namespaces: String,
}

struct Usage {
    detail_url: String,
    lines: Vec<UsageLine>,
    none: bool,
}

struct EditButton {
    action_url: String,
    text: String,
    summary: String,
    start_time: String,
    edit_time: String,
}

struct OldRow {
    file: String,
    file_url: String,
    edit_url: String,
    thumbnail: String,
    age_days: i64,
    old: bool,
    problem: bool,
    source: String,
    usage: Option<Usage>,
    delete_links: Vec<Link>,
    remove_button: Option<EditButton>,
}

struct OldOnes {
    category: String,
    category_url: String,
    files: usize,
    capped: bool,
    is_commons: bool,
    /// Table cells after the file cell, for the “problem” row.
    columns: u8,
    rows: Vec<OldRow>,
}

#[derive(Template)]
#[template(path = "tools/usualsuspects.html")]
struct Page {
    title: &'static str,
    form: Form,
    bad_boys: Option<BadBoys>,
    old_ones: Option<OldOnes>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = text_or(&request.language, DEFAULT_LANGUAGE);
    let project = match language.eq_ignore_ascii_case("commons") {
        true => "wikimedia".to_string(),
        false => text_or(&request.project, DEFAULT_PROJECT),
    };
    let category = category_title(&request.category);
    let min_days = min_days(request.min_days.as_deref());
    let old_only = request.limit.as_deref() == Some("1");
    let form = Form { language, project, category: display_title(&category), min_days, old_only };

    let (mut bad_boys_results, mut old_ones_results) = (None, None);
    let mode = match (request.badoldones.is_some(), request.badboys.is_some()) {
        (true, _) => Some(Mode::BadOldOnes),
        (_, true) => Some(Mode::BadBoys),
        _ => None,
    };
    if let (Some(mode), false) = (mode, category.is_empty()) {
        let wiki = Wiki::new(&form.language, &form.project)?;
        match mode {
            Mode::BadBoys => bad_boys_results = Some(bad_boys(&state, &wiki, &category).await?),
            Mode::BadOldOnes => {
                old_ones_results = Some(bad_old_ones(&state, &wiki, &category, min_days, old_only).await?)
            }
        }
    }
    render(&Page { title: "The Usual Suspects", form, bad_boys: bad_boys_results, old_ones: old_ones_results })
}

enum Mode {
    BadBoys,
    BadOldOnes,
}

fn text_or(value: &str, default: &str) -> String {
    let value = value.trim();
    match value.is_empty() {
        true => default.to_string(),
        false => value.to_string(),
    }
}

/// `Category:Some name` => `Some_name`, as the database has it.
fn category_title(input: &str) -> String {
    let input = input.trim();
    let without_prefix = match input.get(..9) {
        Some(prefix) if prefix.eq_ignore_ascii_case("category:") => input.get(9..).unwrap_or_default(),
        _ => input,
    };
    ucfirst(&without_prefix.replace(' ', "_")).into_owned()
}

fn min_days(value: Option<&str>) -> u32 {
    value.and_then(|value| value.trim().parse().ok()).filter(|days| *days <= 999_999).unwrap_or(DEFAULT_MIN_DAYS)
}

async fn bad_boys(state: &AppState, wiki: &Wiki, category: &str) -> Result<BadBoys, AppError> {
    let mut files = state.replicas.some_pages_in_tree(wiki, category, 0, NS_FILE, Some(MAX_UPLOADER_FILES + 1)).await?;
    let capped = files.len() > MAX_UPLOADER_FILES;
    files.truncate(MAX_UPLOADER_FILES);

    let mut conn = state.replicas.conn(wiki, &["file", "filerevision", "actor"]).await?;
    let sql = "SELECT file_name, actor_name FROM file
        JOIN filerevision ON fr_id=file_latest JOIN actor ON actor_id=fr_actor
        WHERE file_deleted=0 AND file_name IN";
    let uploads: Vec<(String, String)> = select_in(&mut conn, sql, "", &[], &files).await?;

    let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, user) in uploads {
        by_user.entry(user).or_default().push(file);
    }
    let mut uploaders: Vec<Uploader> = by_user
        .into_iter()
        .map(|(name, mut files)| {
            files.sort();
            Uploader {
                url: wiki.page_url(&format!("User:{name}")),
                name,
                files: files
                    .into_iter()
                    .map(|file| FileLink { name: display_title(&file), url: wiki.page_url(&format!("File:{file}")) })
                    .collect(),
            }
        })
        .collect();
    uploaders.sort_by(|a, b| b.files.len().cmp(&a.files.len()).then(a.name.cmp(&b.name)));

    Ok(BadBoys {
        category: display_title(category),
        category_url: wiki.page_url(&format!("Category:{category}")),
        files: files.len(),
        capped,
        uploaders,
    })
}

async fn bad_old_ones(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    min_days: u32,
    old_only: bool,
) -> Result<OldOnes, AppError> {
    let mut files = state.replicas.pages_in_tree(wiki, category, 2, NS_FILE).await?;
    let capped = files.len() > MAX_OLD_FILES;
    files.truncate(MAX_OLD_FILES);

    let api = state.api(wiki).await?;
    let is_commons = wiki.dbname() == Wiki::commons().dbname();
    let now = Utc::now().timestamp();
    let mut rows = vec![];
    for chunk in files.chunks(API_BATCH) {
        let revisions = latest_revisions(&api, chunk).await?;
        let usage = match is_commons {
            true => Some(global_usage(state, chunk).await?),
            false => None,
        };
        rows.extend(
            chunk
                .iter()
                .filter_map(|file| old_row(wiki, category, file, &revisions, usage.as_ref(), min_days, old_only, now)),
        );
    }

    Ok(OldOnes {
        category: display_title(category),
        category_url: wiki.page_url(&format!("Category:{category}")),
        files: files.len(),
        capped,
        is_commons,
        columns: if is_commons { 3 } else { 2 },
        rows,
    })
}

#[derive(Clone)]
struct Revision {
    timestamp: i64,
    text: String,
}

type RevisionKey = (i64, String);

/// The latest revision of each file page and file talk page, as
/// `(namespace, database title) => revision`.
async fn latest_revisions(api: &Api, files: &[String]) -> Result<HashMap<RevisionKey, Revision>, AppError> {
    let mut titles = Vec::with_capacity(files.len() * 2);
    for file in files {
        titles.push(format!("File:{file}"));
        titles.push(format!("File talk:{file}"));
    }
    let params = api.params_into(&[
        ("action", "query"),
        ("prop", "revisions"),
        ("rvprop", "timestamp|content"),
        ("rvslots", "main"),
        ("titles", &titles.join("|")),
    ]);
    let json = api.get_query_api_json_all(&params).await?;

    let mut revisions = HashMap::new();
    for page in json["query"]["pages"].as_array().into_iter().flatten() {
        let Some(namespace) = page["ns"].as_i64() else { continue };
        if namespace != NS_FILE && namespace != NS_FILE_TALK {
            continue;
        }
        let Some(title) = page["title"].as_str() else { continue };
        let name = db_title(title.split_once(':').map_or(title, |(_, name)| name));
        let Some(revision) = page["revisions"].as_array().and_then(|revisions| revisions.first()) else {
            continue;
        };
        let Some(timestamp) = revision["timestamp"].as_str().and_then(parse_timestamp) else {
            continue;
        };
        let text = revision["slots"]["main"]["content"].as_str().unwrap_or_default().to_string();
        revisions.insert((namespace, name), Revision { timestamp, text });
    }
    Ok(revisions)
}

fn parse_timestamp(timestamp: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp).ok().map(|timestamp| timestamp.timestamp())
}

/// File name => wiki => namespace => number of pages using it.
type Usages = HashMap<String, BTreeMap<String, BTreeMap<String, u64>>>;

async fn global_usage(state: &AppState, files: &[String]) -> Result<Usages, AppError> {
    let commons = Wiki::commons();
    let mut conn = state.replicas.conn(&commons, &["globalimagelinks"]).await?;
    let sql = "SELECT gil_to, gil_wiki, COALESCE(gil_page_namespace,''), COUNT(*) AS cnt
        FROM globalimagelinks WHERE gil_to IN";
    let rows: Vec<(String, String, String, u64)> =
        select_in(&mut conn, sql, "GROUP BY gil_to,gil_wiki,gil_page_namespace", &[], files).await?;
    let mut usages: Usages = HashMap::new();
    for (file, wiki, namespace, count) in rows {
        usages.entry(file).or_default().entry(wiki).or_default().insert(namespace, count);
    }
    Ok(usages)
}

#[allow(clippy::too_many_arguments)]
fn old_row(
    wiki: &Wiki,
    category: &str,
    file: &str,
    revisions: &HashMap<RevisionKey, Revision>,
    usages: Option<&Usages>,
    min_days: u32,
    old_only: bool,
    now: i64,
) -> Option<OldRow> {
    let file_url = wiki.page_url(&format!("File:{file}"));
    let edit_url = format!("{}?title={}&action=edit", wiki.index_url(), wiki_urlencode(&format!("File:{file}")));
    let page = revisions.get(&(NS_FILE, file.to_string()));
    let Some(page) = page else {
        return Some(OldRow {
            file: display_title(file),
            file_url,
            edit_url,
            thumbnail: String::new(),
            age_days: 0,
            old: false,
            problem: true,
            source: String::new(),
            usage: None,
            delete_links: vec![],
            remove_button: None,
        });
    };

    let talk = revisions.get(&(NS_FILE_TALK, file.to_string()));
    let last = page.timestamp.max(talk.map_or(0, |talk| talk.timestamp));
    let age_days = ((now - last) as f64 / 86_400.0).round() as i64;
    if old_only && age_days < i64::from(min_days) {
        return None;
    }

    let source = match talk {
        Some(talk) => format!("{}\n----\n{}", page.text, talk.text),
        None => page.text.clone(),
    };
    let fixed = without_no_tags(&page.text);
    let remove_button = (fixed != page.text.trim()).then(|| edit_button(wiki, file, &fixed, page.timestamp));
    let usage = usages.map(|usages| usage(file, usages.get(file)));
    let delete_links = DELETE_REASONS
        .iter()
        .map(|reason| {
            let label = reason.replace("%d", &age_days.to_string());
            Link { url: delete_url(wiki, category, file, &label), label }
        })
        .collect();
    Some(OldRow {
        file: display_title(file),
        file_url,
        edit_url,
        thumbnail: format!("https://{}/wiki/Special:Redirect/file/{}?width=60", wiki.host(), wiki_urlencode(file)),
        age_days,
        old: age_days >= i64::from(min_days),
        problem: false,
        source,
        usage,
        delete_links,
        remove_button,
    })
}

fn usage(file: &str, usages: Option<&BTreeMap<String, BTreeMap<String, u64>>>) -> Usage {
    let detail_url = Wiki::commons().page_url(&format!("Special:GlobalUsage/{file}"));
    let Some(usages) = usages else {
        return Usage { detail_url, lines: vec![], none: true };
    };
    let mut lines: Vec<UsageLine> = usages
        .iter()
        .map(|(dbname, namespaces)| {
            let host = Wiki::from_dbname(dbname).map(|wiki| wiki.host().to_string()).unwrap_or_else(|_| dbname.clone());
            let counts = namespaces
                .iter()
                .map(|(namespace, count)| {
                    let namespace = match namespace.is_empty() {
                        true => "Article",
                        false => namespace,
                    };
                    format!("{namespace} ({count})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            UsageLine {
                wiki_url: format!("https://{host}/wiki/{}#filelinks", wiki_urlencode(&format!("File:{file}"))),
                wiki: host,
                namespaces: counts,
            }
        })
        .collect();
    lines.sort_by(|a, b| a.wiki.cmp(&b.wiki));
    Usage { detail_url, none: lines.is_empty(), lines }
}

fn delete_url(wiki: &Wiki, category: &str, file: &str, reason: &str) -> String {
    let in_category = format!("In category [[:Category:{category}|{}]]", display_title(category));
    let reason = format!("{in_category}; {reason}");
    let title = format!("File:{file}");
    let query =
        serde_urlencoded::to_string([("title", title.as_str()), ("action", "delete"), ("wpReason", reason.as_str())])
            .unwrap_or_default();
    format!("{}?{query}", wiki.index_url())
}

fn edit_button(wiki: &Wiki, file: &str, text: &str, base_timestamp: i64) -> EditButton {
    let title = format!("File:{file}");
    let query = serde_urlencoded::to_string([("title", title.as_str()), ("action", "edit")]).unwrap_or_default();
    let edit_time =
        DateTime::<Utc>::from_timestamp(base_timestamp, 0).unwrap_or_else(Utc::now).format("%Y%m%d%H%M%S").to_string();
    EditButton {
        action_url: format!("{}?{query}", wiki.index_url()),
        text: text.to_string(),
        summary: REMOVE_TAGS_SUMMARY.to_string(),
        start_time: Utc::now().format("%Y%m%d%H%M%S").to_string(),
        edit_time,
    }
}

fn without_no_tags(text: &str) -> String {
    NO_TAGS.replace_all(text, "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_titles() {
        assert_eq!(category_title(" Category:Foo Bar "), "Foo_Bar");
        assert_eq!(category_title("foo"), "Foo");
        assert_eq!(category_title("Category:"), "");
    }

    #[test]
    fn min_days_falls_back_when_invalid() {
        assert_eq!(min_days(None), DEFAULT_MIN_DAYS);
        assert_eq!(min_days(Some("7")), 7);
        assert_eq!(min_days(Some("0")), 0);
        assert_eq!(min_days(Some("1000000")), DEFAULT_MIN_DAYS);
        assert_eq!(min_days(Some("soon")), DEFAULT_MIN_DAYS);
    }

    #[test]
    fn removes_no_source_and_no_license_templates() {
        let text = "{{no source}} Text {{No license|foo}} {{dw no source}} {{keep}}";
        assert_eq!(without_no_tags(text), "Text {{keep}}");
        assert_eq!(without_no_tags(" {{no_source}} \n"), "");
    }

    #[test]
    fn old_row_uses_the_newest_revision_and_offers_removal() {
        let wiki = Wiki::new("commons", "wikimedia").unwrap();
        let now = 1_700_000_000;
        let file = "A.jpg";
        let mut revisions = HashMap::new();
        revisions.insert(
            (NS_FILE, file.to_string()),
            Revision { timestamp: now - 10 * 86_400, text: "{{no source}} Keep".into() },
        );
        revisions
            .insert((NS_FILE_TALK, file.to_string()), Revision { timestamp: now - 2 * 86_400, text: "Talk".into() });
        let row = old_row(&wiki, "Foo", file, &revisions, None, 14, false, now).unwrap();
        assert_eq!(row.age_days, 2);
        assert!(!row.old);
        assert!(row.source.contains("Talk"));
        assert!(row.remove_button.is_some());
        assert_eq!(row.delete_links.len(), DELETE_REASONS.len());
        assert!(old_row(&wiki, "Foo", file, &revisions, None, 14, true, now).is_none());
    }

    fn page(bad_boys: Option<BadBoys>, old_ones: Option<OldOnes>) -> Page {
        Page {
            title: "The Usual Suspects",
            form: Form {
                language: "en".into(),
                project: "wikipedia".into(),
                category: "Foo".into(),
                min_days: 14,
                old_only: false,
            },
            bad_boys,
            old_ones,
        }
    }

    #[test]
    fn renders_bad_boys() {
        let results = BadBoys {
            category: "Foo".into(),
            category_url: "https://en.wikipedia.org/wiki/Category:Foo".into(),
            files: 2,
            capped: false,
            uploaders: vec![Uploader {
                name: "Alice".into(),
                url: "https://en.wikipedia.org/wiki/User:Alice".into(),
                files: vec![
                    FileLink { name: "A.jpg".into(), url: "https://en.wikipedia.org/wiki/File:A.jpg".into() },
                    FileLink { name: "B.jpg".into(), url: "https://en.wikipedia.org/wiki/File:B.jpg".into() },
                ],
            }],
        };
        let html = page(Some(results), None).render().unwrap();
        assert!(html.contains("Alice"));
        assert!(html.contains("https://en.wikipedia.org/wiki/File:B.jpg"));
        assert!(html.contains("2 potentially bad files"));
    }

    #[test]
    fn renders_bad_old_ones() {
        let results = OldOnes {
            category: "Foo".into(),
            category_url: "https://commons.wikimedia.org/wiki/Category:Foo".into(),
            files: 1,
            capped: false,
            is_commons: true,
            columns: 3,
            rows: vec![OldRow {
                file: "A.jpg".into(),
                file_url: "https://commons.wikimedia.org/wiki/File:A.jpg".into(),
                edit_url: "https://commons.wikimedia.org/w/index.php?title=File:A.jpg&action=edit".into(),
                thumbnail: "https://commons.wikimedia.org/wiki/Special:Redirect/file/A.jpg?width=60".into(),
                age_days: 21,
                old: true,
                problem: false,
                source: "{{no source}}".into(),
                usage: Some(Usage {
                    detail_url: "https://commons.wikimedia.org/wiki/Special:GlobalUsage/A.jpg".into(),
                    none: false,
                    lines: vec![UsageLine {
                        wiki: "en.wikipedia.org".into(),
                        wiki_url: "https://en.wikipedia.org/wiki/File:A.jpg#filelinks".into(),
                        namespaces: "Article (2)".into(),
                    }],
                }),
                delete_links: vec![Link {
                    label: "not edited for 21 days".into(),
                    url: "https://commons.wikimedia.org/w/index.php?title=File:A.jpg&action=delete".into(),
                }],
                remove_button: Some(EditButton {
                    action_url: "https://commons.wikimedia.org/w/index.php?title=File:A.jpg&action=edit".into(),
                    text: "".into(),
                    summary: REMOVE_TAGS_SUMMARY.into(),
                    start_time: "20261108120000".into(),
                    edit_time: "20261101000000".into(),
                }),
            }],
        };
        let html = page(None, Some(results)).render().unwrap();
        assert!(html.contains("not edited for 21 days"));
        assert!(html.contains("Article (2)"));
        assert!(html.contains("Remove “no” tags"));
    }
}

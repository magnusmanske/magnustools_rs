//! `can_i_haz_files.html`: shows a shell script to download all files in a
//! Commons category tree and tar them. The same file list can also be streamed
//! as a tar archive with the (currently unlinked) `format=tar` parameter.

use crate::mediawiki;
use crate::replicas::select_in;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, wiki_urlencode};
use anyhow::Context;
use askama::Template;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tar::{EntryType, Header};
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;
use tokio_util::io::ReaderStream;

const NS_FILE: i64 = 6;
/// Titles per `imageinfo` request, the API limit for normal users.
const MAX_TITLES: usize = 50;
const DEFAULT_DEPTH: u32 = 1;
const MAX_DEPTH: u32 = 12;
const DEFAULT_LIMIT: usize = 1000;
const MAX_LIMIT: usize = 5000;
/// Stop before streaming an archive larger than this; Toolforge should not
/// relay arbitrarily large downloads.
const MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// How often to retry a throttled or failed file request.
const MAX_ATTEMPTS: u32 = 4;
/// How far the downloader may run ahead of the client.
const BUFFER: usize = 1024 * 1024;
const ZEROS: [u8; 1024] = [0; 1024];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    category: String,
    depth: Option<String>,
    limit: Option<String>,
    doit: Option<String>,
    /// `format=tar` streams an archive; there is no form field for it yet.
    format: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/can_i_haz_files.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    limit: usize,
    results: Option<Results>,
    error: Option<String>,
}

struct Results {
    category: String,
    category_url: String,
    depth: u32,
    files: usize,
    truncated: bool,
    script: String,
}

struct File {
    name: String,
    url: String,
    size: u64,
}

struct Plan {
    files: Vec<File>,
    /// The header lines of `_skipped.txt`; set when the limit truncated the list.
    note: String,
    truncated: bool,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let depth = request.depth.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    let limit =
        request.limit.as_deref().and_then(|l| l.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut page = Page { title: "Can I haz files?", request: &request, depth, limit, results: None, error: None };
    let category = request.category.trim();
    if category.is_empty() || request.doit.is_none() {
        return Ok(render(&page)?.into_response());
    }
    if request.format.as_deref() == Some("tar") {
        return tar_response(&state, &mut page, category).await;
    }

    let commons = Wiki::commons();
    let (names, truncated) = names(&state, category, depth, limit).await?;
    let files = existing_files(&state, &names).await?;
    let category_url = format!(
        "https://{}/wiki/Category:{}",
        commons.host(),
        wiki_urlencode(category.trim_start_matches("Category:"))
    );
    let script = script(&commons, &files, category, depth);
    page.results =
        Some(Results { category: category.to_string(), category_url, depth, files: files.len(), truncated, script });
    Ok(render(&page)?.into_response())
}

/// Streams the archive for the (currently unlinked) `format=tar` request.
async fn tar_response(state: &AppState, page: &mut Page<'_>, category: &str) -> Result<Response, AppError> {
    let (files, truncated) = files(state, category, page.depth, page.limit).await?;
    if files.is_empty() {
        page.error = Some("No files found.".to_string());
        return Ok(render(&*page)?.into_response());
    }
    let total: u64 = files.iter().map(|file| file.size).sum();
    if total > MAX_BYTES {
        let gb = total as f64 / (1024.0 * 1024.0 * 1024.0);
        let limit_gb = MAX_BYTES / (1024 * 1024 * 1024);
        page.error = Some(format!(
            "The {} files total {gb:.1} GB, more than the {limit_gb} GB limit. Narrow the category or lower the file limit.",
            files.len()
        ));
        return Ok(render(&*page)?.into_response());
    }

    let name = tar_name(category);
    let (reader, writer) = tokio::io::duplex(BUFFER);
    let mut note =
        format!("# Files not included\n# Category: {}\n# Depth: {}\n", category.replace(['\r', '\n'], " "), page.depth);
    if truncated {
        note.push_str(&format!("# Only the first {} files were included.\n", page.limit));
    }
    let plan = Plan { files, note, truncated };
    let http = state.http.clone();
    tokio::spawn(async move {
        if let Err(e) = write_tar(writer, http, plan).await {
            tracing::warn!("can_i_haz_files: {e:#}");
        }
    });

    let mut response = Body::from_stream(ReaderStream::new(reader)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-tar"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // The archive is mostly already-compressed media; do not spend CPU on it.
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("identity"));
    let disposition = format!("attachment; filename=\"{name}\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::try_from(disposition).map_err(|e| AppError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// The file names in the category tree, sorted and deduplicated.
async fn names(state: &AppState, category: &str, depth: u32, limit: usize) -> Result<(Vec<String>, bool), AppError> {
    let commons = Wiki::commons();
    let mut names =
        state.replicas.some_pages_in_tree(&commons, category, depth, NS_FILE, Some(limit.saturating_add(1))).await?;
    let truncated = names.len() > limit;
    names.truncate(limit);
    names.sort_unstable();
    names.dedup();
    Ok((names, truncated))
}

/// The existing files among `names`, dropping redirects and deleted files.
async fn existing_files(state: &AppState, names: &[String]) -> Result<Vec<String>, AppError> {
    if names.is_empty() {
        return Ok(vec![]);
    }
    let commons = Wiki::commons();
    let mut conn = state.replicas.conn(&commons, &["file"]).await?;
    let rows: Vec<String> =
        select_in(&mut conn, "SELECT file_name FROM file WHERE file_deleted=0 AND file_name IN", "", &[], names)
            .await?;
    let existing: HashSet<String> = rows.into_iter().collect();
    Ok(names.iter().filter(|name| existing.contains(*name)).cloned().collect())
}

/// The existing files in the category tree, with their direct URLs and sizes.
async fn files(state: &AppState, category: &str, depth: u32, limit: usize) -> Result<(Vec<File>, bool), AppError> {
    let (names, truncated) = names(state, category, depth, limit).await?;

    let api = state.api(&Wiki::commons()).await?;
    let mut files = vec![];
    for batch in names.chunks(MAX_TITLES) {
        let titles = batch.iter().map(|name| format!("File:{name}")).collect::<Vec<_>>().join("|");
        let params = HashMap::from([
            ("action".to_string(), "query".to_string()),
            ("prop".to_string(), "imageinfo".to_string()),
            ("iiprop".to_string(), "url|size".to_string()),
            ("titles".to_string(), titles),
        ]);
        let json = api.post_query_api_json(&params).await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            let Some(title) = page["title"].as_str() else { continue };
            let Some(info) = page["imageinfo"].as_array().and_then(|info| info.first()) else { continue };
            let (Some(url), Some(size)) = (info["url"].as_str(), info["size"].as_u64()) else { continue };
            files.push(File { name: mediawiki::database_title(NS_FILE, title), url: url.to_string(), size });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files.dedup_by(|a, b| a.name == b.name);
    Ok((files, truncated))
}

async fn write_tar(mut writer: DuplexStream, http: reqwest::Client, plan: Plan) -> anyhow::Result<()> {
    let Plan { files, note, truncated } = plan;
    let mut skipped: Vec<(String, String)> = vec![];
    let mut next = files.first().map(|file| start(&http, &file.url));
    for (index, file) in files.iter().enumerate() {
        let response = match next.take() {
            Some(handle) => match handle.await {
                Ok(Ok(response)) => Some(response),
                Ok(Err(e)) => {
                    skipped.push((file.name.clone(), e.to_string()));
                    None
                }
                Err(e) => {
                    skipped.push((file.name.clone(), e.to_string()));
                    None
                }
            },
            None => match fetch(&http, &file.url).await {
                Ok(response) => Some(response),
                Err(e) => {
                    skipped.push((file.name.clone(), e.to_string()));
                    None
                }
            },
        };
        // Start the next request before streaming this file's body.
        if let Some(next_file) = files.get(index + 1) {
            next = Some(start(&http, &next_file.url));
        }
        let Some(response) = response else { continue };
        if !response.status().is_success() {
            skipped.push((file.name.clone(), format!("HTTP {}", response.status())));
            continue;
        }
        let header = match header(&file.name, file.size) {
            Ok(header) => header,
            Err(e) => {
                skipped.push((file.name.clone(), e.to_string()));
                continue;
            }
        };
        writer.write_all(&header).await?;
        copy_body(&mut writer, response, file.size).await.with_context(|| format!("downloading {}", file.name))?;
    }

    if truncated || !skipped.is_empty() {
        let mut text = note;
        for (file, reason) in &skipped {
            text.push_str(&format!("{file}: {reason}\n"));
        }
        write_entry(&mut writer, "_skipped.txt", text.as_bytes()).await?;
    }
    writer.write_all(&ZEROS).await?;
    writer.shutdown().await?;
    Ok(())
}

fn start(http: &reqwest::Client, url: &str) -> JoinHandle<anyhow::Result<reqwest::Response>> {
    let http = http.clone();
    let url = url.to_string();
    tokio::spawn(async move { fetch(&http, &url).await })
}

async fn fetch(http: &reqwest::Client, url: &str) -> anyhow::Result<reqwest::Response> {
    let mut delay = Duration::from_millis(250);
    let mut last = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let response = http.get(url).header(reqwest::header::ACCEPT_ENCODING, "identity").send().await?;
        if response.status().is_success() || !retryable(response.status()) || attempt == MAX_ATTEMPTS {
            return Ok(response);
        }
        let wait = retry_after(&response).unwrap_or(delay).min(Duration::from_secs(5));
        last = Some(response);
        tokio::time::sleep(wait).await;
        delay = (delay * 2).min(Duration::from_secs(4));
    }
    last.map_or_else(|| anyhow::bail!("no response"), Ok)
}

fn retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.parse::<u64>().ok().map(Duration::from_secs)
}

async fn copy_body(writer: &mut DuplexStream, mut response: reqwest::Response, size: u64) -> anyhow::Result<()> {
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await? {
        written += chunk.len() as u64;
        anyhow::ensure!(written <= size, "received more than the {size} announced bytes");
        writer.write_all(&chunk).await?;
    }
    anyhow::ensure!(written == size, "received {written} of {size} bytes");
    let padding = (512 - size % 512) % 512;
    if padding > 0 {
        writer.write_all(&ZEROS[..padding as usize]).await?;
    }
    Ok(())
}

async fn write_entry(writer: &mut DuplexStream, name: &str, data: &[u8]) -> anyhow::Result<()> {
    writer.write_all(&header(name, data.len() as u64)?).await?;
    writer.write_all(data).await?;
    let padding = (512 - data.len() % 512) % 512;
    if padding > 0 {
        writer.write_all(&ZEROS[..padding]).await?;
    }
    Ok(())
}

/// The tar header(s) for a file, including a GNU long-name entry if needed.
fn header(name: &str, size: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    if name.len() >= 100 {
        let mut long = Header::new_gnu();
        long.set_entry_type(EntryType::GNULongName);
        long.set_mode(0o644);
        long.set_mtime(0);
        long.set_size((name.len() + 1) as u64);
        long.set_path("././@LongLink")?;
        long.set_cksum();
        out.extend_from_slice(long.as_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        let padding = (512 - out.len() % 512) % 512;
        out.resize(out.len() + padding, 0);
    }
    let mut entry = Header::new_gnu();
    entry.set_entry_type(EntryType::Regular);
    entry.set_mode(0o644);
    entry.set_mtime(0);
    entry.set_size(size);
    entry.set_path(short_name(name))?;
    entry.set_cksum();
    out.extend_from_slice(entry.as_bytes());
    Ok(out)
}

/// A name that fits the classic 100-byte tar field with room for a NUL.
fn short_name(name: &str) -> &str {
    if name.len() < 100 {
        return name;
    }
    let mut end = 99;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn script(commons: &Wiki, files: &[String], category: &str, depth: u32) -> String {
    let stem = tar_name(category);
    // The category goes into a comment; a newline there would end it.
    let display = category.trim().replace(['\r', '\n'], " ");
    let mut script = format!(
        "#!/bin/sh\n# {count} files from {display} (depth {depth}), generated by magnustools.\nset -e\nmkdir -p tmp\n",
        count = files.len(),
    );
    for file in files {
        let url = format!("https://{}/wiki/Special:FilePath/{}", commons.host(), wiki_urlencode(file));
        script.push_str(&format!("curl -L --fail --url {} -o {}\n", quote(&url), quote(&format!("tmp/{file}"))));
    }
    script.push_str(&format!("tar -cvf {} tmp\nrm -rf tmp\n", quote(&stem)));
    script
}

/// A string as a single-quoted shell word.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A safe tar file name from a category name.
fn tar_name(category: &str) -> String {
    let name: String = category
        .trim()
        .trim_start_matches("Category:")
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    let name = name.trim_matches('_');
    format!("{}.tar", if name.is_empty() { "files" } else { name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn file(name: &str) -> String {
        name.to_string()
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn tar_names_are_safe() {
        assert_eq!(tar_name("Felis"), "Felis.tar");
        assert_eq!(tar_name("Category:Black-and-white photographs"), "Black-and-white_photographs.tar");
        assert_eq!(tar_name("A/B"), "A_B.tar");
        assert_eq!(tar_name(""), "files.tar");
    }

    #[test]
    fn a_script_downloads_and_tars() {
        let commons = Wiki::commons();
        let script = script(&commons, &[file("A_b.jpg")], "Felis", 1);
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("Special:FilePath/A_b.jpg"));
        assert!(script.contains("curl -L --fail --url"));
        assert!(script.ends_with("tar -cvf 'Felis.tar' tmp\nrm -rf tmp\n"));
    }

    #[test]
    fn long_names_are_truncated_in_the_plain_header() {
        let long = "ä".repeat(150);
        assert!(short_name(&long).len() < 100);
        assert!(long.starts_with(short_name(&long)));
    }

    #[test]
    fn entries_round_trip() {
        let long = "x".repeat(150);
        let mut bytes = Vec::new();
        for (name, data) in [("short.txt", &b"hello"[..]), (long.as_str(), &b"long"[..])] {
            bytes.extend_from_slice(&header(name, data.len() as u64).unwrap());
            bytes.extend_from_slice(data);
            let padding = (512 - data.len() % 512) % 512;
            bytes.resize(bytes.len() + padding, 0);
        }
        bytes.extend_from_slice(&ZEROS);

        let mut archive = tar::Archive::new(Cursor::new(bytes));
        let mut entries = archive.entries().unwrap();
        for expected in ["short.txt", long.as_str()] {
            let mut entry = entries.next().unwrap().unwrap();
            assert_eq!(entry.path().unwrap().to_string_lossy(), expected);
            let mut content = vec![];
            entry.read_to_end(&mut content).unwrap();
            assert!(!content.is_empty());
        }
        assert!(entries.next().is_none());
    }
}

//! The tools with their own interface (the Buggregator, The Talk Page,
//! GeneaWiki), and the files Commons' Commander gadget loads (`commcomm/`):
//! all they need lives in `public/` and is built into the binary, so they
//! work without the PHP checkout.

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use include_dir::{Dir, include_dir};
use percent_encoding::percent_decode_str;

static PUBLIC: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/public");

/// Middleware in front of the PHP checkout's files: serves an app file if
/// there is one for the path.
pub async fn serve(request: Request, next: Next) -> Response {
    match lookup(request.uri().path()) {
        Some(Found::File(path, contents)) => file(path, contents),
        Some(Found::Directory(path)) => Redirect::permanent(&format!("/{path}/")).into_response(),
        None => next.run(request).await,
    }
}

enum Found {
    File(&'static str, &'static [u8]),
    /// A directory addressed without its trailing slash.
    Directory(String),
}

fn lookup(uri_path: &str) -> Option<Found> {
    let path = percent_decode_str(uri_path).decode_utf8().ok()?;
    let relative = path.trim_start_matches('/');
    if relative.split('/').any(|s| s.starts_with('.')) {
        return None;
    }
    let wanted = match relative.strip_suffix('/') {
        Some(dir) => format!("{dir}/index.html"),
        // Only directories with an index page; others, such as `ts2`, are the checkout's.
        None if PUBLIC.get_file(format!("{relative}/index.html")).is_some() => {
            return Some(Found::Directory(relative.to_string()));
        }
        None => relative.to_string(),
    };
    let file = PUBLIC.get_file(wanted)?;
    Some(Found::File(file.path().to_str()?, file.contents()))
}

fn file(path: &'static str, contents: &'static [u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = (StatusCode::OK, contents).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    // Changes come with deployments, which restart the service.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=600"));
    response
}

#[cfg(test)]
mod tests {
    use super::PUBLIC;
    use crate::web::test_support::get;
    use regex::Regex;

    /// Files from old tools can carry API keys (a Flickr key once came along
    /// in `ts2/js/common.js`); everything here is public, so none may.
    #[test]
    fn no_keys_in_public_files() {
        let key =
            Regex::new(r#"(?i)(api[_-]?key|secret|token|password)\w*\s*[:=]\s*['"][A-Za-z0-9_\-]{16,}['"]"#).unwrap();
        let mut files = vec![];
        let mut dirs = vec![&PUBLIC];
        while let Some(dir) = dirs.pop() {
            files.extend(dir.files());
            dirs.extend(dir.dirs());
        }
        for file in files {
            let text = String::from_utf8_lossy(file.contents());
            assert!(!key.is_match(&text), "{} looks like it contains a key", file.path().display());
        }
        // What it is there for:
        assert!(key.is_match("var flickr_api_key = '0123456789abcdef0123' ;"));
    }

    #[tokio::test]
    async fn apps_from_the_binary() {
        // The PHP checkout is empty here: everything comes from `public/`.
        let dir = tempfile::tempdir().unwrap();
        let index = get(dir.path(), "/thetalkpage/").await;
        assert_eq!(index.status.as_u16(), 200);
        assert!(index.headers["content-type"].to_str().unwrap().starts_with("text/html"));
        let js = get(dir.path(), "/buggregator/vue.js").await;
        assert_eq!(js.status.as_u16(), 200);
        assert!(js.headers["content-type"].to_str().unwrap().contains("javascript"));
        let component = get(dir.path(), "/buggregator/vue_components/service-card.html").await;
        assert!(component.body.contains("by_tool"));
        assert_eq!(get(dir.path(), "/ts2/geneawiki2").await.headers["location"], "/ts2/geneawiki2/");
        assert_eq!(get(dir.path(), "/ts2/css/common.css").await.status.as_u16(), 200);
        assert_eq!(get(dir.path(), "/ts2/css/tablesorter/blue/asc.gif").await.status.as_u16(), 200);
        // Not there: on to the PHP checkout, which has nothing either.
        assert_eq!(get(dir.path(), "/ts2/js/no_such_file.js").await.status.as_u16(), 404);
        assert_eq!(get(dir.path(), "/thetalkpage/../Cargo.toml").await.status.as_u16(), 404);
    }
}

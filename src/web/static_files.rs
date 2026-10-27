//! The PHP tool's static files (`public_html`), served as they are.
//!
//! `public_html` holds much that must never go out: PHP and Perl source,
//! `.git` and `.svn` directories, keys. So only files with an allowlisted
//! extension are served, and nothing under a hidden name.

use super::AppError;
use axum::Router;
use axum::extract::{Request, State};
use axum::handler::HandlerWithoutStateExt;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_http::services::ServeDir;

/// File types that may be served, lowercase.
const EXTENSIONS: &[&str] = &[
    "css", "csv", "dmg", "eot", "geojson", "gif", "htm", "html", "ico", "jpeg", "jpg", "js", "json", "kml", "map",
    "md", "mjs", "osm", "otf", "pdf", "png", "rss", "svg", "tab", "tif", "tiff", "tsv", "ttf", "txt", "webp", "wkt",
    "woff", "woff2", "xml",
];

/// A router serving the apps (built in), then the files under
/// `public_html`, with the themed 404 page for everything else.
pub fn service(public_html: &Path) -> Router {
    let files = ServeDir::new(public_html).not_found_service(super::not_found.into_service());
    let root = Arc::new(public_html.to_path_buf());
    Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn_with_state(root, only_public))
        .layer(middleware::from_fn(super::apps::serve))
}

async fn only_public(State(root): State<Arc<PathBuf>>, request: Request, next: Next) -> Response {
    let public = match classify(request.uri().path()) {
        Verdict::Public => true,
        Verdict::Private => false,
        // A directory, so that `/dir` redirects to `/dir/` (and its index.html).
        Verdict::IfDirectory(relative) => tokio::fs::metadata(root.join(relative)).await.is_ok_and(|m| m.is_dir()),
    };
    if public { next.run(request).await } else { AppError::NotFound.into_response() }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Public,
    Private,
    /// Public only if this path, relative to `public_html`, is a directory.
    IfDirectory(String),
}

fn classify(uri_path: &str) -> Verdict {
    let Ok(path) = percent_decode_str(uri_path).decode_utf8() else {
        return Verdict::Private;
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // A leading dot also covers `..`.
    if segments.iter().any(|s| s.starts_with('.') || s.contains(['\\', '\0'])) {
        return Verdict::Private;
    }
    match segments.last() {
        None => Verdict::Public,
        Some(_) if path.ends_with('/') => Verdict::Public,
        Some(name) if has_public_extension(name) => Verdict::Public,
        Some(_) => Verdict::IfDirectory(segments.join("/")),
    }
}

fn has_public_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn classification() {
        use Verdict::*;
        assert_eq!(classify("/"), Public);
        assert_eq!(classify("/index.html"), Public);
        assert_eq!(classify("/resources/js/common.JS"), Public);
        assert_eq!(classify("/mashwiki/"), Public);
        assert_eq!(classify("/mashwiki"), IfDirectory("mashwiki".into()));
        assert_eq!(classify("/resources/leaflet-0.7.5"), IfDirectory("resources/leaflet-0.7.5".into()));
        assert_eq!(classify("/logger.php"), IfDirectory("logger.php".into()));
        assert_eq!(classify("/resources/js/dracula/.git/config"), Private);
        assert_eq!(classify("/resources/%2Egit/config"), Private);
        assert_eq!(classify("/file_reuse/.svn/"), Private);
        assert_eq!(classify("/../replica.my.cnf"), Private);
        assert_eq!(classify("/%2e%2e/replica.my.cnf"), Private);
        assert_eq!(classify("/a%5C..%5Cb.html"), Private);
        assert_eq!(classify("/%FF.html"), Private);
    }

    fn public_html() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("tool/.git")).unwrap();
        std::fs::write(root.join("page.html"), "<p>main</p>").unwrap();
        std::fs::write(root.join("tool/index.html"), "<p>tool</p>").unwrap();
        std::fs::write(root.join("tool/.git/config"), "secret").unwrap();
        std::fs::write(root.join("tool.php"), "<?php secret").unwrap();
        std::fs::write(root.join("key.pem"), "secret").unwrap();
        dir
    }

    #[tokio::test]
    async fn serves_public_files_only() {
        let dir = public_html();
        let root = dir.path();
        let page = get(root, "/page.html").await;
        assert_eq!((page.status.as_u16(), page.body.as_str()), (200, "<p>main</p>"));
        assert_eq!(get(root, "/tool/").await.body, "<p>tool</p>");
        let redirect = get(root, "/tool").await;
        assert!(redirect.status.is_redirection());
        assert_eq!(redirect.headers["location"], "/tool/");
        for private in ["/tool.php", "/key.pem", "/tool/.git/config", "/tool/%2egit/config", "/missing.html"] {
            let response = get(root, private).await;
            assert_eq!(response.status.as_u16(), 404, "{private}");
            assert!(!response.body.contains("secret"), "{private}");
        }
    }
}

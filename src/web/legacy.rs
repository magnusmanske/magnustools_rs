//! Retired URLs: broken or obsolete pages of the PHP tool that were not
//! ported, and now either redirect to a replacement or are gone for good.

use super::AppError;
use axum::Router;
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;

enum Fate {
    Gone,
    MovedTo(&'static str),
}

/// Exact paths. A trailing `/` marks a directory: everything below it is retired too.
const RETIRED: &[(&str, Fate)] = &[
    ("/multistatus.html", Fate::MovedTo("/")),
    ("/toolstatus.html", Fate::MovedTo("/")),
    ("/hay2.html", Fate::MovedTo("https://hay.toolforge.org/")),
    ("/legacy.php", Fate::Gone),
    ("/update_article_counter.php", Fate::Gone),
    ("/autotools.php", Fate::Gone),
    ("/last_tools.php", Fate::Gone),
    ("/record_ip.php", Fate::Gone),
    ("/test1.php", Fate::Gone),
    ("/test2.html", Fate::Gone),
    ("/test3.html", Fate::Gone),
    ("/test.file", Fate::Gone),
    ("/csstest.html", Fate::Gone),
    ("/file_reuse/", Fate::Gone),
];

pub fn routes() -> Router {
    RETIRED.iter().fold(Router::new(), |router, (path, fate)| {
        let handler = get(move || async move {
            match fate {
                Fate::Gone => AppError::Gone.into_response(),
                Fate::MovedTo(url) => Redirect::permanent(url).into_response(),
            }
        });
        match path.strip_suffix('/') {
            Some(dir) => router
                .route(dir, handler.clone())
                .route(path, handler.clone())
                .route(&format!("{path}{{*rest}}"), handler),
            None => router.route(path, handler),
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn retired_urls() {
        let dir = tempfile::tempdir().unwrap();
        // Even where the file is still there.
        std::fs::write(dir.path().join("test2.html"), "old").unwrap();
        for gone in ["/legacy.php", "/test2.html", "/file_reuse", "/file_reuse/", "/file_reuse/main.js"] {
            assert_eq!(get(dir.path(), gone).await.status.as_u16(), 410, "{gone}");
        }
        let moved = get(dir.path(), "/hay2.html").await;
        assert_eq!(moved.status.as_u16(), 308);
        assert_eq!(moved.headers["location"], "https://hay.toolforge.org/");
    }
}

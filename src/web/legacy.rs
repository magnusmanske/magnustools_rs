//! Retired URLs: broken or obsolete pages of the PHP tools that were not
//! ported, and now either redirect to a replacement or are gone for good.
//! Besides magnustools' own, these are the pages of magnus-toolserver,
//! whose every address redirects to the same path here.

use super::{AppError, Params, moved};
use crate::mediawiki::strip_namespace;
use crate::wiki::encode_component;
use axum::Router;
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use serde::Deserialize;

enum Fate {
    Gone,
    /// Redirects there, keeping the query string.
    MovedTo(&'static str),
}

const FIST: &str = "https://fist.toolforge.org/";
const FLICKR2COMMONS: &str = "https://flickr2commons.toolforge.org/";
const GLAMOROUS: &str = "https://glamtools.toolforge.org/glamorous/";
const TREEVIEWS: &str = "https://glamtools.toolforge.org/treeviews/";
const COMMONSHELPER: &str = "https://commonshelper.toolforge.org/";
const PETSCAN: &str = "https://petscan.wmcloud.org/";

/// Exact paths. A trailing `/*` marks a directory: it and everything below are retired.
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
    ("/file_reuse/*", Fate::Gone),
    // magnus-toolserver
    ("/commonsapi_test.php", Fate::MovedTo("/commonsapi/")),
    ("/missing_images.php", Fate::MovedTo(FIST)),
    ("/watchflickr.php", Fate::MovedTo(FIST)),
    ("/fist_api.php", Fate::MovedTo(FIST)),
    ("/fist_old.php", Fate::MovedTo(FIST)),
    ("/flickr_mass.php", Fate::MovedTo(FLICKR2COMMONS)),
    ("/amalglamate.php", Fate::MovedTo(GLAMOROUS)),
    ("/treeviews.php", Fate::MovedTo(TREEVIEWS)),
    ("/pushforcommons.php", Fate::MovedTo(COMMONSHELPER)),
    ("/pushforcommons2.php", Fate::MovedTo(COMMONSHELPER)),
    ("/files_in_category.php", Fate::MovedTo(PETSCAN)),
    ("/PDator.php", Fate::MovedTo(PETSCAN)),
    ("/familiar.php", Fate::MovedTo("https://sigma.toolforge.org/editorinteract.py")),
    ("/ts2/", Fate::MovedTo("/")),
    ("/ts2/index.html", Fate::MovedTo("/")),
    ("/ts2/main/*", Fate::MovedTo("/")),
    ("/ts2/fist/*", Fate::MovedTo(FIST)),
    ("/ts2/flickr2commons/*", Fate::MovedTo(FLICKR2COMMONS)),
    ("/ts2/glamorous/*", Fate::MovedTo(GLAMOROUS)),
    ("/ts2/treeviews/*", Fate::MovedTo(TREEVIEWS)),
    ("/ts2/reasonator/*", Fate::MovedTo("https://reasonator.toolforge.org/")),
    ("/normdaten/*", Fate::Gone),
    ("/ts2/apis/*", Fate::Gone),
    ("/ts2/authority_control/*", Fate::Gone),
    ("/ts2/findart/*", Fate::Gone),
    ("/ts2/testing/*", Fate::Gone),
    ("/ts2/wdq/*", Fate::Gone),
    ("/ts2/wdstats/*", Fate::Gone),
    ("/ts2/wikiprojects/*", Fate::Gone),
];

/// magnus-toolserver pages that depended on dead services, data or Toolserver
/// features, and libraries that were never pages.
const TOOLSERVER_GONE: &[&str] = &[
    "add_infobox.php",
    "articleweight.php",
    "bibber.php",
    "bibcomm.php",
    "biblebay.php",
    "biblebay_test.php",
    "bibliophil.php",
    "biblotag.js",
    "biblotag.php",
    "bong.js",
    "bookie.php",
    "browser_saver.php",
    "catlitter.php",
    "cfapi.php",
    "class_imagedata.php",
    "class_wikiquery.php",
    "comdel.php",
    "commons_counter.php",
    "creationist.php",
    "darBNDer.php",
    "de_bio.php",
    "dnb.php",
    "filterdone.php",
    "flagellum.php",
    "flickr_my_nogroup.php",
    "image_pages_without_image.php",
    "image_paranoia.php",
    "implicator_bot.php",
    "import_bib.php",
    "infoboxes.php",
    "isbn_usage.js",
    "isbn_usage.php",
    "js_page_titles.php",
    "kmlhook.php",
    "knights_template.php",
    "listimagesurls.php",
    "liturgy.php",
    "marcsman.php",
    "missing_links.php",
    "most_wanted_encyclopaedia_topics.php",
    "my_commons_candidates.php",
    "mygallery.php",
    "narative.php",
    "oggster.php",
    "openlibrary_authors.php",
    "pdf.php",
    "persondata.php",
    "personendaten.php",
    "phpdiff.php",
    "queryclass.php",
    "related_timeline.php",
    "relicenser.php",
    "replace_stuff.php",
    "save_a_blp.php",
    "shadows_commons.php",
    "sifter_api.php",
    "sifterbooks.php",
    "sources.php",
    "spiegelei.php",
    "stockphoto.js",
    "summarizer.php",
    "test.php",
    "transwiki_data.php",
    "twic.php",
    "wf_funs.php",
    "wikilendar.php",
    "xml2json.php",
    "thetalkpage/index_test.html",
];

pub fn routes() -> Router {
    let gone = TOOLSERVER_GONE.iter().map(|name| (format!("/{name}"), &Fate::Gone));
    let retired = RETIRED.iter().map(|(path, fate)| (path.to_string(), fate)).chain(gone);
    let router = retired.fold(Router::new(), |router, (path, fate)| {
        let handler = match fate {
            Fate::Gone => get(|| async { AppError::Gone.into_response() }),
            Fate::MovedTo(url) => moved(url),
        };
        match path.strip_suffix("/*") {
            Some(dir) => router
                .route(dir, handler.clone())
                .route(&format!("{dir}/"), handler.clone())
                .route(&format!("{dir}/{{*rest}}"), handler),
            None => router.route(&path, handler),
        }
    });
    router.route("/commcomm", get(commander)).route("/commcomm/", get(commander))
}

#[derive(Debug, Deserialize)]
struct CommanderRequest {
    category: Option<String>,
}

/// Commons Commander lives on Commons now, as a user script at `Special:Commander`.
/// Its old page took a category to open.
async fn commander(Params(request): Params<CommanderRequest>) -> Redirect {
    const COMMANDER: &str = "https://commons.wikimedia.org/w/index.php?title=Special:Commander";
    let category = request.category.as_deref().map(|c| strip_namespace(c, &["category"]).unwrap_or(c).trim());
    match category.filter(|c| !c.is_empty()) {
        Some(category) => {
            Redirect::permanent(&format!("{COMMANDER}&pane1=cat&cat1={}&run1=1&pane2=cat", encode_component(category)))
        }
        None => Redirect::permanent(COMMANDER),
    }
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

    #[tokio::test]
    async fn toolserver_urls() {
        let dir = tempfile::tempdir().unwrap();
        for gone in ["/persondata.php", "/stockphoto.js", "/normdaten/", "/normdaten/api.php", "/ts2/wdq/index.html"] {
            assert_eq!(get(dir.path(), gone).await.status.as_u16(), 410, "{gone}");
        }
        let root = dir.path();
        let location = async |uri| get(root, uri).await.headers["location"].to_str().unwrap().to_string();
        assert_eq!(
            location("/ts2/glamorous/?category=X").await,
            "https://glamtools.toolforge.org/glamorous/?category=X"
        );
        assert_eq!(location("/ts2/").await, "/");
        assert_eq!(location("/commonsapi_test.php?image=A.jpg").await, "/commonsapi/?image=A.jpg");
        assert_eq!(
            location("/commcomm/?category=Birds%20of%20Ohio").await,
            "https://commons.wikimedia.org/w/index.php?title=Special:Commander&pane1=cat&cat1=Birds%20of%20Ohio&run1=1&pane2=cat"
        );
        // Pages next to retired ones stay.
        let geneawiki = get(dir.path(), "/ts2/geneawiki/").await;
        assert_eq!(geneawiki.status.as_u16(), 200);
        assert!(geneawiki.body.contains("main.js"));
    }
}

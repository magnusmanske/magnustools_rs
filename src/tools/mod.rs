//! The tools, one module each, at the URLs of their PHP originals.

mod add_information;
mod artniw;
mod authority_control;
mod blockfinder;
mod bong;
mod can_i_haz_files;
mod cas;
mod commonsapi;
mod contribs;
mod coord_gen;
mod deep_insight;
mod deep_out_of_sight;
mod dnb;
mod dnb_link_finder;
mod dnb_ratios;
mod dnb_wikisource;
mod filterdone;
mod flickrfree;
mod forest4trees;
mod get_article_intro;
mod get_distinct_authors;
mod herding_sheep;
mod html2wiki;
mod icommons;
mod index;
mod interwiki;
mod isbn2wiki;
pub(crate) mod logger;
mod makeref;
mod map2wp;
mod metronom;
mod missingtopics;
mod misstake;
pub(crate) mod most_wanted;
mod newpages;
mod no_information;
mod not_in_the_other_language;
mod oauth_uploader;
mod prepbio;
mod quick_counter;
mod random_out_of_sight;
mod random_page;
mod redirector;
mod related_places;
mod shortpages;
mod sighting;
mod spacer;
mod subpages;
mod sugart;
mod tab2wiki;
mod talkcatintersect;
mod templator;
mod transparent;
mod unused_files;
mod uploader_distribution;
mod usualsuspects;
mod wiki2playlist;
mod wiki_todo;
mod wp_data_url;
mod xcontribs;

use crate::state::AppState;
use crate::web::moved;
use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{MethodRouter, get};
use std::sync::Arc;

type State = Arc<AppState>;

/// The tools with a page, by title, for the menu of every page.
pub const MENU: &[(&str, &str)] = &[
    ("add_information", "Add information"),
    ("artniw", "Articles without interwikis [Joanjoc]"),
    ("blockfinder", "Block finder [Erwin85]"),
    ("bong", "BONG"),
    ("can_i_haz_files", "Can I haz files?"),
    ("cas", "CAS number links"),
    ("commonsapi", "Commons API"),
    ("contribs", "Contribs [Erwin85]"),
    ("coord_gen", "Coordinate generator"),
    ("deep_insight", "Deep InSight"),
    ("deep_out_of_sight", "Deep out-of-sight"),
    ("get_distinct_authors", "Distinct authors"),
    ("dnb_ratios", "DNB article length ratios"),
    ("dnb_link_finder", "DNB link finder"),
    ("dnb_wikisource", "DNB on Wikisource"),
    ("filterdone", "Filter done"),
    ("transparent", "Find a translator"),
    ("flickrfree", "FlickrFree"),
    ("forest4trees", "Forest for the trees"),
    ("get_article_intro", "Get article intro"),
    ("herding_sheep", "Herding sheep"),
    ("html2wiki", "HTML to wiki"),
    ("icommons", "iCommons"),
    ("isbn2wiki", "ISBN to wiki"),
    ("makeref", "MakeRef"),
    ("map2wp", "Map DNB to Wikipedia"),
    ("metronom", "Metronom"),
    ("missingtopics", "Missing topics"),
    ("most_wanted", "Most wanted"),
    ("newpages", "New pages [Erwin85]"),
    ("no_information", "No information"),
    ("not-in-the-other-language", "Not in the other language"),
    ("oauth_uploader", "OAuth uploader"),
    ("prepbio", "PrepBio"),
    ("quick_counter", "Quick edit counter"),
    ("randomarticle", "Random article"),
    ("random_image_commons_subcat", "Random image"),
    ("random_out_of_sight", "Random out-of-sight"),
    ("misstake", "Ranked missing articles"),
    ("redirector", "Redirector"),
    ("related_places", "Related places"),
    ("shortpages", "Short pages [Erwin85]"),
    ("spacer", "Spacer"),
    ("subpages", "Subpages [Erwin85]"),
    ("sugart", "Suggest articles from interwikis [Joanjoc]"),
    ("tab2wiki", "Table to wiki"),
    ("talkcatintersect", "Talk page category intersect [Erwin85]"),
    ("templator", "Templator"),
    ("usualsuspects", "The Usual Suspects"),
    ("unused_files", "Unused files"),
    ("uploader_distribution", "Uploader distribution"),
    ("wiki2playlist", "Wiki to playlist"),
    ("wiki_todo", "Wiki ToDo"),
    ("wp_data_url", "Wikipedia article with inline images"),
    ("xcontribs", "xContribs [Erwin85]"),
];

pub fn routes() -> Router<State> {
    let router = Router::new()
        .route("/", get(index::handler))
        .route("/index.html", get(index::handler))
        .route("/main.html", moved("/"));
    tools().into_iter().fold(router, |router, (name, handler)| {
        // The logger is called by the tools themselves: neither a use nor traffic of its own.
        let handler = match name {
            "logger" => handler,
            _ => handler.layer(middleware::from_fn(move |request: Request, next: Next| count(name, request, next))),
        };
        tool(router, name, handler)
    })
}

/// Counts each request to a tool for the service dashboard, by status
/// class (`2xx`, …; `http_requests_total` has them for the whole service);
/// and those that bring parameters and succeed as uses (`tool_uses_total`),
/// which the dashboard's maintenance copies into the logger's tables daily.
async fn count(tool: &'static str, request: Request, next: Next) -> Response {
    let with_parameters = logger::has_parameters(request.method(), request.uri());
    let response = next.run(request).await;
    let status = response.status().as_u16();
    wikimisc::metrics::counter!("tool_requests_total", "tool" => tool, "status" => format!("{}xx", status / 100))
        .increment(1);
    if with_parameters && status < 400 {
        wikimisc::metrics::counter!("tool_uses_total", "tool" => tool).increment(1);
    }
    response
}

fn tools() -> Vec<(&'static str, MethodRouter<State>)> {
    vec![
        ("add_information", get(add_information::handler).post(add_information::handler)),
        ("artniw", get(artniw::handler)),
        ("authority_control", get(authority_control::handler).post(authority_control::handler)),
        ("blockfinder", get(blockfinder::handler)),
        ("bong", get(bong::handler)),
        ("can_i_haz_files", get(can_i_haz_files::handler)),
        ("cas", get(cas::handler)),
        ("commonsapi", get(commonsapi::handler)),
        ("contribs", get(contribs::handler)),
        ("coord_gen", get(coord_gen::handler).post(coord_gen::handler)),
        ("deep_insight", get(deep_insight::handler).post(deep_insight::handler)),
        ("deep_out_of_sight", get(deep_out_of_sight::handler).post(deep_out_of_sight::handler)),
        ("filterdone", get(filterdone::handler)),
        ("flickrfree", get(flickrfree::handler)),
        ("forest4trees", get(forest4trees::handler)),
        ("get_article_intro", get(get_article_intro::handler)),
        ("get_distinct_authors", get(get_distinct_authors::handler).post(get_distinct_authors::handler)),
        ("dnb_ratios", get(dnb_ratios::handler)),
        ("dnb_link_finder", get(dnb_link_finder::handler)),
        ("dnb_wikisource", get(dnb_wikisource::handler)),
        ("herding_sheep", get(herding_sheep::handler)),
        ("html2wiki", get(html2wiki::handler).post(html2wiki::handler)),
        ("icommons", get(icommons::handler).post(icommons::handler)),
        ("isbn2wiki", get(isbn2wiki::handler).post(isbn2wiki::handler)),
        ("logger", get(logger::handler).post(logger::handler)),
        ("makeref", get(makeref::handler).post(makeref::handler)),
        ("map2wp", get(map2wp::handler).post(map2wp::handler)),
        ("metronom", get(metronom::handler)),
        ("missingtopics", get(missingtopics::handler).post(missingtopics::handler)),
        ("most_wanted", get(most_wanted::handler)),
        ("misstake", get(misstake::handler).post(misstake::handler)),
        ("newpages", get(newpages::handler)),
        ("no_information", get(no_information::handler)),
        ("not-in-the-other-language", get(not_in_the_other_language::handler)),
        ("oauth_uploader", get(oauth_uploader::handler).post(oauth_uploader::handler)),
        ("prepbio", get(prepbio::handler).post(prepbio::handler)),
        ("quick_counter", get(quick_counter::handler)),
        ("random_image_commons_subcat", get(random_page::image)),
        ("random_out_of_sight", get(random_out_of_sight::handler).post(random_out_of_sight::handler)),
        ("randomarticle", get(random_page::article)),
        ("redirector", get(redirector::handler).post(redirector::handler)),
        ("related_places", get(related_places::handler)),
        ("shortpages", get(shortpages::handler)),
        ("spacer", get(spacer::handler)),
        ("subpages", get(subpages::handler)),
        ("sugart", get(sugart::handler)),
        ("tab2wiki", get(tab2wiki::handler).post(tab2wiki::handler)),
        ("talkcatintersect", get(talkcatintersect::handler)),
        ("templator", get(templator::handler).post(templator::handler)),
        ("transparent", get(transparent::handler).post(transparent::handler)),
        ("unused_files", get(unused_files::handler)),
        ("uploader_distribution", get(uploader_distribution::handler)),
        ("usualsuspects", get(usualsuspects::handler).post(usualsuspects::handler)),
        ("wiki2playlist", get(wiki2playlist::handler)),
        ("wiki_todo", get(wiki_todo::handler).post(wiki_todo::handler)),
        ("wp_data_url", get(wp_data_url::handler)),
        ("xcontribs", get(xcontribs::handler)),
    ]
}

/// A tool at `/<name>/`. Its old addresses, `/<name>.php` and `/<name>`, redirect there.
fn tool(router: Router<State>, name: &str, handler: MethodRouter<State>) -> Router<State> {
    let path = format!("/{name}/");
    router.route(&path, handler).route(&format!("/{name}.php"), moved(&path)).route(&format!("/{name}"), moved(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn menu_has_every_page_in_title_order() {
        // Tools without a page of their own, only an API.
        const API_ONLY: &[&str] = &["authority_control", "logger"];
        let mut pages: Vec<&str> =
            tools().into_iter().map(|(name, _)| name).filter(|n| !API_ONLY.contains(n)).collect();
        let mut menu: Vec<&str> = MENU.iter().map(|(name, _)| *name).collect();
        pages.sort_unstable();
        menu.sort_unstable();
        assert_eq!(menu, pages);
        assert!(MENU.windows(2).all(|w| w[0].1.to_lowercase() < w[1].1.to_lowercase()));
    }

    #[tokio::test]
    async fn old_php_addresses_redirect_to_the_tool() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["/bong.php", "/filterdone.php", "/unused_files.php", "/wiki_todo.php"] {
            let page = get(dir.path(), path).await;
            assert_eq!(page.status.as_u16(), 308, "{path}");
            assert_eq!(page.headers["location"], path.trim_end_matches(".php").to_string() + "/", "{path}");
        }
    }

    #[tokio::test]
    async fn pages_have_the_menu() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/unused_files/").await;
        assert!(page.body.contains(r#"<a href="/no_information/">No information</a>"#));
        assert!(page.body.contains(r#"<a href="/unused_files/" aria-current="page">Unused files</a>"#));
    }

    #[tokio::test]
    async fn erwin85_forms_render() {
        let dir = tempfile::tempdir().unwrap();
        for (path, title) in [
            ("/blockfinder/", "Block finder [Erwin85]"),
            ("/contribs/", "Contribs [Erwin85]"),
            ("/newpages/", "New pages [Erwin85]"),
            ("/shortpages/", "Short pages [Erwin85]"),
            ("/subpages/", "Subpages [Erwin85]"),
            ("/talkcatintersect/", "Talk page category intersect [Erwin85]"),
            ("/xcontribs/", "xContribs [Erwin85]"),
        ] {
            let page = get(dir.path(), path).await;
            assert_eq!(page.status.as_u16(), 200, "{path}");
            assert!(page.body.contains(&format!("<h1>{title}</h1>")), "{path}");
        }
    }

    #[tokio::test]
    async fn rescued_forms_render() {
        let dir = tempfile::tempdir().unwrap();
        for (path, title) in [
            ("/can_i_haz_files/", "Can I haz files?"),
            ("/filterdone/", "Filter done"),
            ("/usualsuspects/", "The Usual Suspects"),
        ] {
            let page = get(dir.path(), path).await;
            assert_eq!(page.status.as_u16(), 200, "{path}");
            assert!(page.body.contains(&format!("<h1>{title}</h1>")), "{path}");
        }
    }

    #[tokio::test]
    async fn other_language_form_renders() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/not-in-the-other-language/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Not in the other language</h1>"));
    }

    #[tokio::test]
    async fn joanjoc_forms_render() {
        let dir = tempfile::tempdir().unwrap();
        for (path, title) in [
            ("/artniw/", "Articles without interwikis [Joanjoc]"),
            ("/sugart/", "Suggest articles from interwikis [Joanjoc]"),
        ] {
            let page = get(dir.path(), path).await;
            assert_eq!(page.status.as_u16(), 200, "{path}");
            assert!(page.body.contains(&format!("<h1>{title}</h1>")), "{path}");
        }
    }

    #[tokio::test]
    async fn isbn_form_renders_and_rejects_a_bad_isbn() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/isbn2wiki/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>ISBN to wiki</h1>"));
        assert!(page.body.contains("{{cite book}}"));
        let page = get(dir.path(), "/isbn2wiki/?isbn=9780140328722").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("Not an ISBN"));
    }

    #[tokio::test]
    async fn old_addresses_redirect() {
        let dir = tempfile::tempdir().unwrap();
        let old = get(dir.path(), "/tab2wiki.php?text=a&doit=1").await;
        assert_eq!(old.status.as_u16(), 308);
        assert_eq!(old.headers["location"], "/tab2wiki/?text=a&doit=1");
        assert_eq!(get(dir.path(), "/tab2wiki").await.headers["location"], "/tab2wiki/");
        let new = get(dir.path(), "/tab2wiki/?text=a%09b&doit=1").await;
        assert_eq!(new.status.as_u16(), 200);
        assert!(new.body.contains("{| border="));
    }
}

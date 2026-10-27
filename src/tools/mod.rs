//! The tools, one module each, at the URLs of their PHP originals.

mod add_information;
mod authority_control;
mod bong;
mod cas;
mod commonsapi;
mod coord_gen;
mod flickrfree;
mod forest4trees;
mod get_article_intro;
mod get_distinct_authors;
mod herding_sheep;
mod html2wiki;
mod icommons;
mod index;
mod logger;
mod makeref;
mod metronom;
mod missingtopics;
mod misstake;
mod no_information;
mod oauth_uploader;
mod prepbio;
mod quick_counter;
mod random_page;
mod redirector;
mod related_places;
mod spacer;
mod tab2wiki;
mod templator;
mod transparent;
mod unused_files;
mod uploader_distribution;
mod wiki2playlist;
mod wp_data_url;

use crate::state::AppState;
use crate::web::moved;
use axum::Router;
use axum::extract::{Request, State as Extract};
use axum::middleware::{self, Next};
use axum::routing::{MethodRouter, get};
use std::sync::Arc;

type State = Arc<AppState>;

/// The tools with a page, by title, for the menu of every page.
pub const MENU: &[(&str, &str)] = &[
    ("add_information", "Add information"),
    ("bong", "BONG"),
    ("cas", "CAS number links"),
    ("commonsapi", "Commons API"),
    ("coord_gen", "Coordinate generator"),
    ("get_distinct_authors", "Distinct authors"),
    ("transparent", "Find a translator"),
    ("flickrfree", "FlickrFree"),
    ("forest4trees", "Forest for the trees"),
    ("get_article_intro", "Get article intro"),
    ("herding_sheep", "Herding sheep"),
    ("html2wiki", "HTML to wiki"),
    ("icommons", "iCommons"),
    ("makeref", "MakeRef"),
    ("metronom", "Metronom"),
    ("missingtopics", "Missing topics"),
    ("no_information", "No information"),
    ("oauth_uploader", "OAuth uploader"),
    ("prepbio", "PrepBio"),
    ("quick_counter", "Quick edit counter"),
    ("randomarticle", "Random article"),
    ("random_image_commons_subcat", "Random image"),
    ("misstake", "Ranked missing articles"),
    ("redirector", "Redirector"),
    ("related_places", "Related places"),
    ("spacer", "Spacer"),
    ("tab2wiki", "Table to wiki"),
    ("templator", "Templator"),
    ("unused_files", "Unused files"),
    ("uploader_distribution", "Uploader distribution"),
    ("wiki2playlist", "Wiki to playlist"),
    ("wp_data_url", "Wikipedia article with inline images"),
];

pub fn routes(state: &State) -> Router<State> {
    let router = Router::new()
        .route("/", get(index::handler))
        .route("/index.html", get(index::handler))
        .route("/main.html", moved("/"));
    tools().into_iter().fold(router, |router, (name, handler)| {
        // The logger counts other tools' uses, not its own.
        let handler = match name {
            "logger" => handler,
            _ => handler.layer(middleware::from_fn_with_state(
                state.clone(),
                move |Extract(state): Extract<State>, request: Request, next: Next| {
                    logger::count_uses(state, name, request, next)
                },
            )),
        };
        tool(router, name, handler)
    })
}

fn tools() -> Vec<(&'static str, MethodRouter<State>)> {
    vec![
        ("add_information", get(add_information::handler).post(add_information::handler)),
        ("authority_control", get(authority_control::handler).post(authority_control::handler)),
        ("bong", get(bong::handler)),
        ("cas", get(cas::handler)),
        ("commonsapi", get(commonsapi::handler)),
        ("coord_gen", get(coord_gen::handler).post(coord_gen::handler)),
        ("flickrfree", get(flickrfree::handler)),
        ("forest4trees", get(forest4trees::handler)),
        ("get_article_intro", get(get_article_intro::handler)),
        ("get_distinct_authors", get(get_distinct_authors::handler).post(get_distinct_authors::handler)),
        ("herding_sheep", get(herding_sheep::handler)),
        ("html2wiki", get(html2wiki::handler).post(html2wiki::handler)),
        ("icommons", get(icommons::handler).post(icommons::handler)),
        ("logger", get(logger::handler).post(logger::handler)),
        ("makeref", get(makeref::handler).post(makeref::handler)),
        ("metronom", get(metronom::handler)),
        ("missingtopics", get(missingtopics::handler).post(missingtopics::handler)),
        ("misstake", get(misstake::handler).post(misstake::handler)),
        ("no_information", get(no_information::handler)),
        ("oauth_uploader", get(oauth_uploader::handler).post(oauth_uploader::handler)),
        ("prepbio", get(prepbio::handler).post(prepbio::handler)),
        ("quick_counter", get(quick_counter::handler)),
        ("random_image_commons_subcat", get(random_page::image)),
        ("randomarticle", get(random_page::article)),
        ("redirector", get(redirector::handler).post(redirector::handler)),
        ("related_places", get(related_places::handler)),
        ("spacer", get(spacer::handler)),
        ("tab2wiki", get(tab2wiki::handler).post(tab2wiki::handler)),
        ("templator", get(templator::handler).post(templator::handler)),
        ("transparent", get(transparent::handler).post(transparent::handler)),
        ("unused_files", get(unused_files::handler)),
        ("uploader_distribution", get(uploader_distribution::handler)),
        ("wiki2playlist", get(wiki2playlist::handler)),
        ("wp_data_url", get(wp_data_url::handler)),
    ]
}

/// A tool at `/<name>/`. Its old address, `/<name>.php`, and `/<name>` redirect there.
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
    async fn pages_have_the_menu() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/unused_files/").await;
        assert!(page.body.contains(r#"<a href="/no_information/">No information</a>"#));
        assert!(page.body.contains(r#"<a href="/unused_files/" aria-current="page">Unused files</a>"#));
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

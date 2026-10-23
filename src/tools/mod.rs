//! The tools, one module each, at the URLs of their PHP originals.

mod authority_control;
mod bong;
mod cas;
mod commonsapi;
mod coord_gen;
mod flickrfree;
mod get_article_intro;
mod get_distinct_authors;
mod herding_sheep;
mod html2wiki;
mod index;
mod logger;
mod metronom;
mod misstake;
mod oauth_uploader;
mod prepbio;
mod quick_counter;
mod random_page;
mod redirector;
mod spacer;
mod tab2wiki;
mod transparent;
mod unused_files;
mod uploader_distribution;
mod wiki2playlist;
mod wp_data_url;

use crate::state::AppState;
use crate::web::moved;
use axum::Router;
use axum::routing::{MethodRouter, get};
use std::sync::Arc;

type State = Arc<AppState>;

pub fn routes() -> Router<State> {
    let tools: Vec<(&str, MethodRouter<State>)> = vec![
        ("authority_control", get(authority_control::handler).post(authority_control::handler)),
        ("bong", get(bong::handler)),
        ("cas", get(cas::handler)),
        ("commonsapi", get(commonsapi::handler)),
        ("coord_gen", get(coord_gen::handler).post(coord_gen::handler)),
        ("flickrfree", get(flickrfree::handler)),
        ("get_article_intro", get(get_article_intro::handler)),
        ("get_distinct_authors", get(get_distinct_authors::handler).post(get_distinct_authors::handler)),
        ("herding_sheep", get(herding_sheep::handler)),
        ("html2wiki", get(html2wiki::handler).post(html2wiki::handler)),
        ("logger", get(logger::handler).post(logger::handler)),
        ("metronom", get(metronom::handler)),
        ("misstake", get(misstake::handler).post(misstake::handler)),
        ("oauth_uploader", get(oauth_uploader::handler).post(oauth_uploader::handler)),
        ("prepbio", get(prepbio::handler).post(prepbio::handler)),
        ("quick_counter", get(quick_counter::handler)),
        ("random_image_commons_subcat", get(random_page::image)),
        ("randomarticle", get(random_page::article)),
        ("redirector", get(redirector::handler).post(redirector::handler)),
        ("spacer", get(spacer::handler)),
        ("tab2wiki", get(tab2wiki::handler).post(tab2wiki::handler)),
        ("transparent", get(transparent::handler).post(transparent::handler)),
        ("unused_files", get(unused_files::handler)),
        ("uploader_distribution", get(uploader_distribution::handler)),
        ("wiki2playlist", get(wiki2playlist::handler)),
        ("wp_data_url", get(wp_data_url::handler)),
    ];
    let router = Router::new()
        .route("/", get(index::handler))
        .route("/index.html", get(index::handler))
        .route("/main.html", moved("/"));
    tools.into_iter().fold(router, |router, (name, handler)| tool(router, name, handler))
}

/// A tool at `/<name>/`. Its old address, `/<name>.php`, and `/<name>` redirect there.
fn tool(router: Router<State>, name: &str, handler: MethodRouter<State>) -> Router<State> {
    let path = format!("/{name}/");
    router.route(&path, handler).route(&format!("/{name}.php"), moved(&path)).route(&format!("/{name}"), moved(&path))
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

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

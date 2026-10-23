//! The tools, one module each, at the URLs of their PHP originals.

mod authority_control;
mod cas;
mod commonsapi;
mod flickrfree;
mod get_article_intro;
mod get_distinct_authors;
mod herding_sheep;
mod html2wiki;
mod index;
mod logger;
mod metronom;
mod oauth_uploader;
mod prepbio;
mod quick_counter;
mod random_page;
mod spacer;
mod tab2wiki;
mod transparent;
mod unused_files;
mod uploader_distribution;
mod wiki2playlist;
mod wp_data_url;

use crate::state::AppState;
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index::handler))
        .route("/index.html", get(index::handler))
        .route("/main.html", get(|| async { axum::response::Redirect::permanent("/") }))
        .route("/authority_control.php", get(authority_control::handler).post(authority_control::handler))
        .route("/cas.php", get(cas::handler))
        .route("/commonsapi.php", get(commonsapi::handler))
        .route("/flickrfree.php", get(flickrfree::handler))
        .route("/get_article_intro.php", get(get_article_intro::handler))
        .route("/get_distinct_authors.php", get(get_distinct_authors::handler).post(get_distinct_authors::handler))
        .route("/herding_sheep.php", get(herding_sheep::handler))
        .route("/html2wiki.php", get(html2wiki::handler).post(html2wiki::handler))
        .route("/logger.php", get(logger::handler).post(logger::handler))
        .route("/metronom.php", get(metronom::handler))
        .route("/oauth_uploader.php", get(oauth_uploader::handler).post(oauth_uploader::handler))
        .route("/prepbio.php", get(prepbio::handler).post(prepbio::handler))
        .route("/quick_counter.php", get(quick_counter::handler))
        .route("/random_image_commons_subcat.php", get(random_page::image))
        .route("/randomarticle.php", get(random_page::article))
        .route("/spacer.php", get(spacer::handler))
        .route("/tab2wiki.php", get(tab2wiki::handler).post(tab2wiki::handler))
        .route("/transparent.php", get(transparent::handler).post(transparent::handler))
        .route("/unused_files.php", get(unused_files::handler))
        .route("/uploader_distribution.php", get(uploader_distribution::handler))
        .route("/wiki2playlist.php", get(wiki2playlist::handler))
        .route("/wp_data_url.php", get(wp_data_url::handler))
}

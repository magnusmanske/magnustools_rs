//! `related_places.php`: the places a page links to, on a map, or as KML or
//! GeoJSON. (The original parsed GeoHack links and left the map to Google;
//! now the coordinates come from the `geo_tags` table.)

use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use wikimisc::escape;
use wikimisc::mysql_async::prelude::Queryable;

/// Namespaces a page can be in: articles and user pages.
const NAMESPACES: &[(i64, &str)] = &[(0, "Article"), (2, "User")];
const MAX_PLACES: usize = 5000;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    namespace: i64,
    title: String,
    /// `kml` or `geojson`; HTML otherwise.
    format: String,
    /// The original's way of asking for KML.
    kml: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            project: "wikipedia".to_string(),
            namespace: 0,
            title: String::new(),
            format: String::new(),
            kml: None,
        }
    }
}

struct Place {
    title: String,
    url: String,
    lat: f64,
    lon: f64,
}

#[derive(Template)]
#[template(path = "tools/related_places.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    namespaces: &'static [(i64, &'static str)],
    places: Option<Vec<Place>>,
    /// The places as GeoJSON, for the map.
    geojson: String,
    /// The query string of this request, for the download links.
    query: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let title = request.title.trim();
    if title.is_empty() {
        return Ok(page(&request, None)?.into_response());
    }
    if !NAMESPACES.iter().any(|(ns, _)| *ns == request.namespace) {
        return Err(AppError::BadRequest("Only articles and user pages can be checked.".to_string()));
    }
    let wiki = Wiki::new(&request.language, &request.project)?;
    let places = places(&state, &wiki, request.namespace, title).await?;
    let format = if request.kml.is_some() { "kml" } else { request.format.as_str() };
    Ok(match format {
        "kml" => ([(header::CONTENT_TYPE, "application/vnd.google-earth.kml+xml")], kml(&wiki, title, &places))
            .into_response(),
        "geojson" => ([(header::CONTENT_TYPE, "application/geo+json")], geojson(&places).to_string()).into_response(),
        _ => page(&request, Some(places))?.into_response(),
    })
}

fn page(request: &Request, places: Option<Vec<Place>>) -> Result<axum::response::Html<String>, AppError> {
    let geojson = places.as_deref().map(|p| geojson(p).to_string()).unwrap_or_default();
    let namespace = request.namespace.to_string();
    let query = serde_urlencoded::to_string([
        ("language", request.language.as_str()),
        ("project", request.project.as_str()),
        ("namespace", namespace.as_str()),
        ("title", request.title.trim()),
    ])
    .unwrap_or_default();
    render(&Page { title: "Related places", request, namespaces: NAMESPACES, places, geojson, query })
}

/// The pages `title` links to that have coordinates (on Earth), by title.
async fn places(state: &AppState, wiki: &Wiki, namespace: i64, title: &str) -> Result<Vec<Place>, AppError> {
    let mut links = state.replicas.conn(wiki, &["page", "pagelinks", "linktarget"]).await?;
    let sql =
        "SELECT DISTINCT lt_title FROM page JOIN pagelinks ON pl_from=page_id JOIN linktarget ON pl_target_id=lt_id
        WHERE page_namespace=? AND page_title=? AND lt_namespace=0";
    let targets: Vec<String> = links.exec(sql, (namespace, db_title(title))).await.map_err(anyhow::Error::from)?;

    let mut geo = state.replicas.conn(wiki, &["page", "geo_tags"]).await?;
    let sql = "SELECT page_title, gt_lat, gt_lon FROM page JOIN geo_tags ON gt_page_id=page_id
        WHERE gt_primary=1 AND gt_globe='earth' AND gt_lat IS NOT NULL AND gt_lon IS NOT NULL
        AND page_namespace=0 AND page_title IN";
    let mut rows: Vec<(String, f64, f64)> = select_in(&mut geo, sql, "", &[], &targets).await?;
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.dedup_by(|a, b| a.0 == b.0);
    Ok(rows
        .into_iter()
        .take(MAX_PLACES)
        .map(|(title, lat, lon)| Place { url: wiki.page_url(&title), title: display_title(&title), lat, lon })
        .collect())
}

fn geojson(places: &[Place]) -> serde_json::Value {
    let features: Vec<_> = places
        .iter()
        .map(|p| {
            json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [p.lon, p.lat]},
                "properties": {"title": p.title, "url": p.url},
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

fn kml(wiki: &Wiki, title: &str, places: &[Place]) -> String {
    let title = display_title(title);
    let mut kml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n<Document>\n\
         <name>{}</name>\n<description>Places {} links to on {wiki}</description>\n",
        escape::xml(&title),
        escape::xml(&title)
    );
    for p in places {
        kml.push_str(&format!(
            "<Placemark><name>{}</name><description>{}</description><Point><coordinates>{},{},0</coordinates></Point></Placemark>\n",
            escape::xml(&p.title),
            escape::xml(&p.url),
            p.lon,
            p.lat
        ));
    }
    kml.push_str("</Document>\n</kml>\n");
    kml
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Vec<Place> {
        vec![Place {
            title: "Bell Rock & Lighthouse".to_string(),
            url: "https://en.wikipedia.org/wiki/Bell_Rock".to_string(),
            lat: 56.43,
            lon: -2.38,
        }]
    }

    #[test]
    fn kml_and_geojson() {
        let kml = kml(&Wiki::new("en", "wikipedia").unwrap(), "List <of> lighthouses", &places());
        assert!(kml.contains("<name>List &lt;of&gt; lighthouses</name>"));
        assert!(kml.contains("<name>Bell Rock &amp; Lighthouse</name>"));
        assert!(kml.contains("<coordinates>-2.38,56.43,0</coordinates>"));
        let json = geojson(&places());
        assert_eq!(json["features"][0]["geometry"]["coordinates"], json!([-2.38, 56.43]));
    }
}

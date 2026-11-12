//! `dnb_ratios.php`: Wikisource DNB articles with exactly one English
//! Wikipedia link, with their live Wikipedia length and cached Wikisource
//! word count.

use super::dnb;
use crate::replicas::{db_title, display_title};
use crate::state::AppState;
use crate::web::{AppError, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const TITLE: &str = "DNB article length ratios";
const NS_TEMPLATE: i64 = 10;
const ATTRIBUTION_CATEGORY: &str = "Attribution_templates";
const MESSAGE_CATEGORY: &str = "Article_message_templates";
const ATTRIBUTION_DEPTH: u32 = 5;
const MESSAGE_DEPTH: u32 = 1;
/// Rows shown; the highest ratios are the interesting ones.
const MAX_ROWS: usize = 2_000;

#[derive(Template)]
#[template(path = "tools/dnb_ratios.html")]
struct Page {
    title: &'static str,
    words_available: bool,
    rows: Vec<Row>,
    truncated: bool,
}

struct Row {
    ws_title: String,
    ws_url: String,
    wp_title: String,
    wp_url: String,
    words: u64,
    ratio: Option<f64>,
    templates: Vec<TemplateUse>,
}

struct TemplateUse {
    title: String,
    attribution: bool,
}

impl Row {
    fn ratio_text(&self) -> String {
        self.ratio.map_or_else(|| "Undefined".to_string(), |ratio| format!("{ratio:.3}"))
    }
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let wikisource = Wiki::new("en", "wikisource")?;
    let wikipedia = Wiki::new("en", "wikipedia")?;
    let words = read_wordcounts(&state.config.dnb_wordcount_file).await;
    let words_available = !words.is_empty();
    let (mut rows, truncated) = results(&state, &wikisource, &wikipedia, &words).await?;
    rows.sort_by(|a, b| {
        b.ratio.partial_cmp(&a.ratio).unwrap_or(Ordering::Equal).then_with(|| a.ws_title.cmp(&b.ws_title))
    });
    let truncated = truncated || rows.len() > MAX_ROWS;
    rows.truncate(MAX_ROWS);
    let attribution =
        state.replicas.pages_in_tree(&wikipedia, ATTRIBUTION_CATEGORY, ATTRIBUTION_DEPTH, NS_TEMPLATE).await?;
    let messages = state.replicas.pages_in_tree(&wikipedia, MESSAGE_CATEGORY, MESSAGE_DEPTH, NS_TEMPLATE).await?;
    add_templates(&state, &wikipedia, &mut rows, &attribution, &messages).await?;
    render(&Page { title: TITLE, words_available, rows, truncated })
}

async fn results(
    state: &AppState,
    wikisource: &Wiki,
    wikipedia: &Wiki,
    words: &HashMap<String, (String, u64)>,
) -> Result<(Vec<Row>, bool), AppError> {
    let mut rows: HashMap<String, Row> = HashMap::new();
    for (ws_title, (wp_title, word_count)) in words {
        if wp_title.is_empty() {
            continue;
        }
        rows.insert(
            wp_title.clone(),
            Row {
                ws_url: wikisource.page_url(&display_title(ws_title)),
                ws_title: display_title(ws_title),
                wp_url: wikipedia.page_url(&display_title(wp_title)),
                wp_title: display_title(wp_title),
                words: *word_count,
                ratio: None,
                templates: Vec::new(),
            },
        );
    }
    if rows.is_empty() {
        return Ok((Vec::new(), false));
    }

    let titles: Vec<String> = rows.keys().cloned().collect();
    let mut conn = state.replicas.conn(wikipedia, &["page", "templatelinks", "linktarget"]).await?;
    let lengths: Vec<(String, u64)> = dnb::select_small(
        &mut conn,
        "SELECT page_title, page_len FROM page WHERE page_namespace=0 AND page_title IN",
        &titles,
    )
    .await?;
    for (title, length) in lengths {
        if let Some(row) = rows.get_mut(&title) {
            row.ratio = (length > 0).then(|| row.words as f64 / length as f64);
        }
    }

    Ok((rows.into_values().collect(), false))
}

/// Adds the attribution and article-message templates of the rows that are shown.
async fn add_templates(
    state: &AppState,
    wikipedia: &Wiki,
    rows: &mut [Row],
    attribution: &[String],
    messages: &[String],
) -> Result<(), AppError> {
    if rows.is_empty() {
        return Ok(());
    }
    let titles: Vec<String> = rows.iter().map(|row| db_title(&row.wp_title)).collect();
    let mut conn = state.replicas.conn(wikipedia, &["page", "templatelinks", "linktarget"]).await?;
    let attribution: HashSet<&str> = attribution.iter().map(String::as_str).collect();
    let messages: HashSet<&str> = messages.iter().map(String::as_str).collect();
    let templates: Vec<(String, String)> = dnb::select_small(
        &mut conn,
        "SELECT p.page_title, lt.lt_title FROM page p
         JOIN templatelinks tl ON tl_from=p.page_id JOIN linktarget lt ON tl_target_id=lt.lt_id
         WHERE p.page_namespace=0 AND lt.lt_namespace=10 AND p.page_title IN",
        &titles,
    )
    .await?;
    let mut by_page: HashMap<String, Vec<TemplateUse>> = HashMap::new();
    for (title, template) in templates {
        let template = match (attribution.contains(template.as_str()), messages.contains(template.as_str())) {
            (true, _) => TemplateUse { title: display_title(&template), attribution: true },
            (false, true) => TemplateUse { title: display_title(&template), attribution: false },
            _ => continue,
        };
        by_page.entry(title).or_default().push(template);
    }
    for row in rows.iter_mut() {
        if let Some(templates) = by_page.remove(&db_title(&row.wp_title)) {
            row.templates = templates;
        }
    }
    Ok(())
}

/// Cached word counts from the old dnbtools job: Wikisource page, Wikipedia
/// page, and words.
async fn read_wordcounts(path: &std::path::Path) -> HashMap<String, (String, u64)> {
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        tracing::warn!("cannot read {}", path.display());
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let page = fields.next()?;
            let wikipedia = fields.next()?;
            let _bytes = fields.next()?;
            let words = fields.next()?.parse().ok()?;
            Some((page.to_string(), (wikipedia.to_string(), words)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wordcounts_ignore_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.tab");
        std::fs::write(&path, "A_(DNB00)\tA\t100\t12\nbad\nB_(DNB00)\tB\t200\tx\nC_(DNB00)\tC\t300\t7\n").unwrap();
        let words = read_wordcounts(&path).await;
        assert_eq!(words.get("A_(DNB00)"), Some(&("A".to_string(), 12)));
        assert_eq!(words.get("C_(DNB00)"), Some(&("C".to_string(), 7)));
        assert_eq!(words.len(), 2);
        assert!(read_wordcounts(&dir.path().join("missing")).await.is_empty());
    }
}

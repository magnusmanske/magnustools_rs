//! Shared parts of the joanjoc tools `artniw` and `sugart`, which look for
//! pages missing an interwiki, optionally inside a category tree and among
//! pages that use a template.

use crate::mediawiki::{Api, Title};
use crate::replicas::db_title;
use crate::state::AppState;
use crate::web::AppError;
use crate::wiki::Wiki;

/// Results shown, as in the original tools.
pub const MAX_RESULTS: usize = 500;
/// Pages to look at in a category tree; a tree can hold far more than one
/// request can check.
pub const MAX_CANDIDATES: usize = 20_000;
/// The original tools offered categories one to three levels down.
pub const MAX_DEPTH: u32 = 3;

/// The old tools' project names; `wiki` is Wikipedia.
pub fn project(project: &str) -> &str {
    match project.trim() {
        "wiki" => "wikipedia",
        other => other,
    }
}

/// A category or template name as the database has it: without a namespace
/// prefix, with underscores, and with the first letter in upper case.
pub fn db_title_without_namespace(input: &str, api: &Api) -> String {
    db_title(&Title::new_from_full(input.trim(), api).with_underscores())
}

/// The category depth, as the form offers it.
pub fn depth(value: Option<&str>) -> u32 {
    value.and_then(|d| d.trim().parse().ok()).unwrap_or(1).clamp(1, MAX_DEPTH)
}

/// The pages of `namespace` in a category tree; `None` if no category was
/// given, so the search is not limited to one.
pub async fn candidates(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
    namespace: i64,
) -> Result<Option<Vec<String>>, AppError> {
    match category.trim() {
        "" => Ok(None),
        category => {
            let pages =
                state.replicas.some_pages_in_tree(wiki, category, depth, namespace, Some(MAX_CANDIDATES)).await?;
            Ok(Some(pages))
        }
    }
}

/// Restricts a query to pages that transclude the template, if one was given.
/// The one `?` it has is the template's database title.
pub fn template_condition(template: Option<&str>) -> &'static str {
    match template {
        Some(_) => {
            "AND p.page_id IN (SELECT tl_from FROM templatelinks
                JOIN linktarget ON lt_id=tl_target_id WHERE lt_namespace=10 AND lt_title=?)"
        }
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects() {
        assert_eq!(project("wiki"), "wikipedia");
        assert_eq!(project(" wiktionary "), "wiktionary");
    }

    #[test]
    fn depths_are_one_to_three() {
        assert_eq!(depth(None), 1);
        assert_eq!(depth(Some("")), 1);
        assert_eq!(depth(Some("2")), 2);
        assert_eq!(depth(Some("9")), MAX_DEPTH);
        assert_eq!(depth(Some("0")), 1);
        assert_eq!(depth(Some("deep")), 1);
    }

    #[test]
    fn templates_only_add_a_condition_when_given() {
        assert!(template_condition(None).is_empty());
        assert!(template_condition(Some("Infobox")).contains("lt_namespace=10"));
    }
}

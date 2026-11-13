//! Wikimedia wikis and page titles; see [`wikimisc::wiki`] and
//! [`wikimisc::title`].

pub use wikimisc::title::*;
pub use wikimisc::wiki::{UnknownWiki, Wiki, is_language_code};

/// The effective project for the old tools' `language`/`project` pair:
/// Commons is always `wikimedia`, and an empty project is `wikipedia`.
pub fn effective_project(language: &str, project: Option<&str>) -> String {
    match project.map(str::trim).filter(|project| !project.is_empty()) {
        Some(project) => project.to_lowercase(),
        None if language.trim().eq_ignore_ascii_case("commons") => "wikimedia".to_string(),
        None => "wikipedia".to_string(),
    }
}

/// The language, effective project and wiki from the `language`/`project`
/// parameters of the old tools. Commons defaults to the wikimedia project.
pub fn wiki_and_project(language: &str, project: Option<&str>) -> Result<(String, String, Wiki), UnknownWiki> {
    let language = language.trim().to_lowercase();
    let project = effective_project(&language, project);
    let wiki = Wiki::new(&language, &project)?;
    Ok((language, project, wiki))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_defaults_and_validation() {
        assert_eq!(
            wiki_and_project(" Commons ", None).unwrap(),
            ("commons".into(), "wikimedia".into(), Wiki::commons())
        );
        assert_eq!(wiki_and_project("de", None).unwrap().1, "wikipedia");
        assert_eq!(wiki_and_project("de", Some("wikisource")).unwrap().1, "wikisource");
        assert!(wiki_and_project("xx", Some("evil")).is_err());
    }
}

//! Wikimedia wikis and page titles; see [`wikimisc::wiki`] and
//! [`wikimisc::title`].

pub use wikimisc::title::*;
pub use wikimisc::wiki::{UnknownWiki, Wiki, is_language_code};

/// The effective project for the old tools' `language`/`project` pair:
/// Commons is always `wikimedia` (whatever `project` says), and an empty
/// project is `wikipedia`.
pub fn effective_project(language: &str, project: Option<&str>) -> String {
    if language.trim().eq_ignore_ascii_case("commons") {
        return COMMONS_PROJECT.to_string();
    }
    match project.map(str::trim).filter(|project| !project.is_empty()) {
        Some(project) => project.to_lowercase(),
        None => DEFAULT_PROJECT.to_string(),
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

/// The default language and project of the old tools' `language`/`project`
/// parameters.
pub const DEFAULT_LANGUAGE: &str = "en";
pub const DEFAULT_PROJECT: &str = "wikipedia";
/// The German Wikipedia, which some of the old tools started on.
pub const DEFAULT_LANGUAGE_DE: &str = "de";
/// The Commons tools' `language`/`project` pair; Commons forces the project.
pub const COMMONS_LANGUAGE: &str = "commons";
pub const COMMONS_PROJECT: &str = "wikimedia";
/// The old tools' `project` value for Wikipedia, which the tools resolve to
/// `wikipedia`.
pub const WIKIPEDIA_PROJECT: &str = "wiki";

/// The defaults above, for `serde(default = …)` on request structs.
pub fn default_language() -> String {
    DEFAULT_LANGUAGE.to_string()
}

pub fn default_de() -> String {
    DEFAULT_LANGUAGE_DE.to_string()
}

pub fn default_project() -> String {
    DEFAULT_PROJECT.to_string()
}

pub fn default_commons_language() -> String {
    COMMONS_LANGUAGE.to_string()
}

pub fn default_commons_project() -> String {
    COMMONS_PROJECT.to_string()
}

pub fn default_wikipedia_project() -> String {
    WIKIPEDIA_PROJECT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Defaults {
        #[serde(default = "default_language")]
        language: String,
        #[serde(default = "default_project")]
        project: String,
    }

    #[test]
    fn request_defaults() {
        assert_eq!((default_language(), default_project()), ("en".into(), "wikipedia".into()));
        assert_eq!(default_de(), "de");
        assert_eq!((default_commons_language(), default_commons_project()), ("commons".into(), "wikimedia".into()));
        assert_eq!(default_wikipedia_project(), "wiki");
        assert!(wiki_and_project(COMMONS_LANGUAGE, Some(COMMONS_PROJECT)).is_ok());
    }

    #[test]
    fn serde_uses_the_request_defaults() {
        let defaults: Defaults = serde_urlencoded::from_str("").unwrap();
        assert_eq!((defaults.language.as_str(), defaults.project.as_str()), (DEFAULT_LANGUAGE, DEFAULT_PROJECT));
    }

    #[test]
    fn project_defaults_and_validation() {
        assert_eq!(
            wiki_and_project(" Commons ", None).unwrap(),
            ("commons".into(), "wikimedia".into(), Wiki::commons())
        );
        assert_eq!(
            wiki_and_project("commons", Some("wikisource")).unwrap(),
            ("commons".into(), "wikimedia".into(), Wiki::commons())
        );
        assert_eq!(wiki_and_project("de", None).unwrap().1, "wikipedia");
        assert_eq!(wiki_and_project("de", Some("wikisource")).unwrap().1, "wikisource");
        assert!(wiki_and_project("xx", Some("evil")).is_err());
    }
}

//! Wikimedia wikis, as named by the `language`/`project` parameters of the
//! old tools. Only Wikimedia hosts can come out of this, so user input can
//! never point a request elsewhere.

use std::fmt;

/// Project families that have one wiki per language, at `<language>.<project>.org`.
const LANGUAGE_PROJECTS: &[&str] =
    &["wikipedia", "wiktionary", "wikisource", "wikibooks", "wikinews", "wikiquote", "wikiversity", "wikivoyage"];

/// "Languages" that name a single wiki, whatever the project.
const SPECIAL_WIKIS: &[(&str, &str)] = &[
    ("commons", "commons.wikimedia.org"),
    ("meta", "meta.wikimedia.org"),
    ("species", "species.wikimedia.org"),
    ("incubator", "incubator.wikimedia.org"),
    ("wikidata", "www.wikidata.org"),
    ("mediawiki", "www.mediawiki.org"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiki {
    host: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown wiki: {language}.{project}")]
pub struct UnknownWiki {
    pub language: String,
    pub project: String,
}

impl Wiki {
    /// `("en", "wikipedia")`, `("commons", <anything>)`, `("de", "wikisource")`, …
    pub fn new(language: &str, project: &str) -> Result<Self, UnknownWiki> {
        let language = language.trim().to_lowercase();
        let project = project.trim().to_lowercase();
        let special = SPECIAL_WIKIS.iter().find(|(name, _)| *name == language || *name == project);
        let host = match special {
            Some((_, host)) => Some(host.to_string()),
            None if LANGUAGE_PROJECTS.contains(&project.as_str()) && is_language_code(&language) => {
                Some(format!("{language}.{project}.org"))
            }
            None => None,
        };
        host.map(|host| Self { host }).ok_or(UnknownWiki { language, project })
    }

    pub fn commons() -> Self {
        Self { host: "commons.wikimedia.org".to_string() }
    }

    pub fn wikidata() -> Self {
        Self { host: "www.wikidata.org".to_string() }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn api_url(&self) -> String {
        format!("https://{}/w/api.php", self.host)
    }

    pub fn index_url(&self) -> String {
        format!("https://{}/w/index.php", self.host)
    }

    pub fn page_url(&self, title: &str) -> String {
        format!("https://{}/wiki/{}", self.host, encode_title(title))
    }
}

impl fmt::Display for Wiki {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.host)
    }
}

/// `en`, `simple`, `zh-min-nan`, `be-tarask`, …
fn is_language_code(code: &str) -> bool {
    (2..=20).contains(&code.len())
        && code.starts_with(|c: char| c.is_ascii_lowercase())
        && !code.ends_with('-')
        && !code.contains("--")
        && code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A page title as it appears in a wiki URL: spaces as underscores, the rest percent-encoded.
pub fn encode_title(title: &str) -> String {
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'_')
        .remove(b'-')
        .remove(b'.')
        .remove(b':')
        .remove(b'/')
        .remove(b'(')
        .remove(b')')
        .remove(b',');
    percent_encoding::utf8_percent_encode(&title.trim().replace(' ', "_"), KEEP).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(language: &str, project: &str) -> Option<String> {
        Wiki::new(language, project).ok().map(|w| w.host().to_string())
    }

    #[test]
    fn hosts() {
        assert_eq!(host("en", "wikipedia").as_deref(), Some("en.wikipedia.org"));
        assert_eq!(host(" DE ", "wikisource").as_deref(), Some("de.wikisource.org"));
        assert_eq!(host("zh-min-nan", "wikipedia").as_deref(), Some("zh-min-nan.wikipedia.org"));
        assert_eq!(host("commons", "wikimedia").as_deref(), Some("commons.wikimedia.org"));
        assert_eq!(host("commons", "wikipedia").as_deref(), Some("commons.wikimedia.org"));
        assert_eq!(host("en", "wikidata").as_deref(), Some("www.wikidata.org"));
        assert_eq!(host("wikidata", "wikipedia").as_deref(), Some("www.wikidata.org"));
    }

    #[test]
    fn nothing_but_wikimedia_hosts() {
        for (language, project) in [
            ("evil.com#", "wikipedia"),
            ("en", "evil"),
            ("en", "wikipedia.org.evil.com"),
            ("a", "wikipedia"),
            ("en-", "wikipedia"),
            ("", "wikipedia"),
            ("en/x", "wikipedia"),
            ("de", "wikimedia"),
        ] {
            assert_eq!(host(language, project), None, "{language}.{project}");
        }
    }

    #[test]
    fn urls() {
        let wiki = Wiki::new("de", "wikipedia").unwrap();
        assert_eq!(wiki.api_url(), "https://de.wikipedia.org/w/api.php");
        assert_eq!(
            wiki.page_url("Benutzer:Magnus Manske/Spacer"),
            "https://de.wikipedia.org/wiki/Benutzer:Magnus_Manske/Spacer"
        );
        assert_eq!(encode_title("A&B?c d"), "A%26B%3Fc_d");
    }
}

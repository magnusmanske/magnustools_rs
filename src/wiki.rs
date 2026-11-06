//! Wikimedia wikis and page titles; see [`wikimisc::wiki`] and
//! [`wikimisc::title`].

pub use wikimisc::title::*;
pub use wikimisc::wiki::{UnknownWiki, Wiki, is_language_code};

/// A page title as it appears in a wiki URL.
///
/// This is the old name for [`wiki_urlencode`].
pub fn encode_title(title: &str) -> String {
    wiki_urlencode(title)
}

/// A value for a URL query or path segment.
///
/// This is the old name for [`percent_encode`].
pub fn encode_component(value: &str) -> String {
    percent_encode(value)
}

/// `s` with its first letter in upper case, as a wiki page title has it.
///
/// This is the old name for [`ucfirst`].
pub fn upper_first(s: &str) -> String {
    ucfirst(s).into_owned()
}

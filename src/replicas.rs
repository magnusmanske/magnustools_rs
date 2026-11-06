//! Wiki replica databases; see [`wikimisc::replicas`].

pub use wikimisc::replicas::*;

/// A title as the database has it: underscores for spaces, first letter upper case.
pub fn db_title(title: &str) -> String {
    wikimisc::title::to_db_key(&wikimisc::title::normalize(title, wikimisc::title::Case::FirstLetter))
}

/// A database title for display: spaces instead of underscores.
pub fn display_title(title: &str) -> String {
    wikimisc::title::to_display(title)
}

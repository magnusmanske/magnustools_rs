//! Looks up a book by ISBN in several catalogues and writes a wiki-markup
//! reference from each, plus one merged entry.

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use bookmeta::{Book, Isbn, Lookup};
use serde::Deserialize;
use std::sync::Arc;

const WD_INFERNAL: &str = "https://wd-infernal.toolforge.org/isbn/isbn/";

/// The citation formats offered; the plain line is the old tool's output.
const FORMATS: &[(&str, &str)] = &[
    ("plain", "Plain wiki line (default)"),
    ("cite_book", "{{cite book}} (English)"),
    ("literatur", "{{Literatur}} (German)"),
    ("ouvrage", "{{Ouvrage}} (French)"),
];

/// Languages worth offering as the book's language in a citation.
const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("de", "German"), ("fr", "French")];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    isbn: String,
    #[serde(default = "default_format")]
    template: String,
    #[serde(default)]
    language: Option<String>,
}

fn default_format() -> String {
    "plain".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Plain,
    CiteBook,
    Literatur,
    Ouvrage,
}

impl Format {
    fn parse(value: &str) -> Self {
        match value {
            "cite_book" => Self::CiteBook,
            "literatur" => Self::Literatur,
            "ouvrage" => Self::Ouvrage,
            _ => Self::Plain,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::CiteBook => "cite_book",
            Self::Literatur => "literatur",
            Self::Ouvrage => "ouvrage",
        }
    }

    /// The language a template is written in; another language is added as a
    /// parameter.
    fn default_language(self) -> &'static str {
        match self {
            Self::Literatur => "de",
            Self::Ouvrage => "fr",
            Self::Plain | Self::CiteBook => "en",
        }
    }
}

/// The requested language if it is one we offer, else the format's own.
fn language_key(requested: Option<&str>, format: Format) -> &'static str {
    requested
        .and_then(|requested| LANGUAGES.iter().find(|(key, _)| *key == requested).map(|(key, _)| *key))
        .unwrap_or_else(|| format.default_language())
}

#[derive(Template)]
#[template(path = "tools/isbn2wiki.html")]
struct Page {
    title: &'static str,
    isbn: String,
    template: &'static str,
    language: &'static str,
    formats: &'static [(&'static str, &'static str)],
    languages: &'static [(&'static str, &'static str)],
    error: Option<String>,
    results: Option<Results>,
}

struct Results {
    isbn: String,
    books: Vec<RenderedBook>,
    merged: Option<RenderedBook>,
    wikidata: Vec<WikidataLink>,
    failures: Vec<&'static str>,
    wd_infernal_url: String,
    plain: bool,
}

struct RenderedBook {
    id: String,
    source: &'static str,
    text: String,
}

struct WikidataLink {
    id: String,
    url: String,
    label: Option<String>,
}

impl Results {
    fn new(lookup: &Lookup, isbn: &Isbn, format: Format, language: &'static str) -> Self {
        let display = isbn.display().to_string();
        let books = lookup
            .by_source()
            .into_iter()
            .flat_map(|(_, books)| books)
            .enumerate()
            .map(|(index, book)| RenderedBook {
                id: format!("book-{}", index + 1),
                source: book.source.label(),
                text: render_book(&display, book, format, language),
            })
            .collect();
        let merged = lookup.merged().map(|book| RenderedBook {
            id: "merged".to_string(),
            source: book.source.label(),
            text: render_book(&display, &book, format, language),
        });
        let wikidata = lookup
            .wikidata
            .iter()
            .map(|item| WikidataLink { id: item.id.clone(), url: item.url(), label: item.label.clone() })
            .collect();
        Self {
            wd_infernal_url: format!("{WD_INFERNAL}{}", isbn.isbn13_digits()),
            isbn: display,
            books,
            merged,
            wikidata,
            failures: lookup.failures.iter().map(|failure| failure.source.label()).collect(),
            plain: format == Format::Plain,
        }
    }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let format = Format::parse(&request.template);
    let language = language_key(request.language.as_deref(), format);
    let mut page = Page {
        title: "ISBN to wiki",
        isbn: request.isbn.trim().to_string(),
        template: format.key(),
        language,
        formats: FORMATS,
        languages: LANGUAGES,
        error: None,
        results: None,
    };
    if !page.isbn.is_empty() {
        match Isbn::parse(&page.isbn) {
            Err(_) => page.error = Some(format!("Not an ISBN: {}.", page.isbn)),
            Ok(isbn) => {
                let lookup = state
                    .isbn2wiki_cache
                    .get_or_fetch(isbn.key(), async || Ok::<_, AppError>(state.book_sources.lookup(&isbn).await))
                    .await?;
                page.results = Some(Results::new(&lookup, &isbn, format, language));
            }
        }
    }
    render(&page)
}

fn render_book(isbn: &str, book: &Book, format: Format, language: &'static str) -> String {
    match format {
        Format::Plain => {
            let wiki = wiki_line(isbn, book);
            format!("{wiki}\n{}", plain_line(&wiki))
        }
        Format::CiteBook => citation_cite_book(isbn, book, language),
        Format::Literatur => citation_literatur(isbn, book, language),
        Format::Ouvrage => citation_ouvrage(isbn, book, language),
    }
}

/// The old tool's line, byte for byte in structure.
fn wiki_line(isbn: &str, book: &Book) -> String {
    let authors = if book.authors.is_empty() {
        "???".to_string()
    } else {
        book.authors.iter().map(|author| format!("[[{}]]", wiki_text(author))).collect::<Vec<_>>().join(", ")
    };
    let mut line = format!("* {authors}: ''{}'' ", wiki_text(&book.display_title()));
    if !book.publisher.is_empty() {
        line.push_str(&format!("[[{}]], ", wiki_text(&book.publisher)));
    }
    if !book.places.is_empty() {
        let places = book.places.iter().map(|place| format!("[[{}]]", wiki_text(place))).collect::<Vec<_>>().join("/");
        line.push_str(&places);
        line.push(' ');
    }
    if !book.year.is_empty() {
        line.push_str(&format!("{}, ", wiki_text(&book.year)));
    }
    line.push_str(&format!("ISBN {isbn}"));
    line
}

fn plain_line(line: &str) -> String {
    line.replace("[[", "").replace("]]", "")
}

fn citation_cite_book(isbn: &str, book: &Book, language: &'static str) -> String {
    let mut fields = author_fields(book, "last", "first");
    fields.push(("title".to_string(), book.title.clone()));
    if let Some(subtitle) = &book.subtitle {
        fields.push(("subtitle".to_string(), subtitle.clone()));
    }
    fields.push(("publisher".to_string(), book.publisher.clone()));
    fields.push(("location".to_string(), book.places.join(", ")));
    fields.push(("date".to_string(), book.year.clone()));
    if let Some(pages) = book.pages {
        fields.push(("pages".to_string(), pages.to_string()));
    }
    fields.push(("isbn".to_string(), book_isbn(isbn, book)));
    if let Some(doi) = &book.ids.crossref_doi {
        fields.push(("doi".to_string(), doi.clone()));
    }
    if language != "en" {
        fields.push(("language".to_string(), language.to_string()));
    }
    template_line("cite book", &fields)
}

fn citation_literatur(isbn: &str, book: &Book, language: &'static str) -> String {
    let mut fields = full_author_fields(book, "Autor", "Autor");
    fields.push(("Titel".to_string(), book.title.clone()));
    if let Some(subtitle) = &book.subtitle {
        fields.push(("TitelErg".to_string(), subtitle.clone()));
    }
    fields.push(("Verlag".to_string(), book.publisher.clone()));
    fields.push(("Ort".to_string(), book.places.join(", ")));
    fields.push(("Datum".to_string(), book.year.clone()));
    if let Some(pages) = book.pages {
        fields.push(("Seiten".to_string(), pages.to_string()));
    }
    fields.push(("ISBN".to_string(), book_isbn(isbn, book)));
    if let Some(doi) = &book.ids.crossref_doi {
        fields.push(("DOI".to_string(), doi.clone()));
    }
    if language != "de" {
        fields.push(("Sprache".to_string(), language.to_string()));
    }
    template_line("Literatur", &fields)
}

fn citation_ouvrage(isbn: &str, book: &Book, language: &'static str) -> String {
    let mut fields = full_author_fields(book, "auteur1", "auteur");
    fields.push(("titre".to_string(), book.title.clone()));
    if let Some(subtitle) = &book.subtitle {
        fields.push(("sous-titre".to_string(), subtitle.clone()));
    }
    fields.push(("éditeur".to_string(), book.publisher.clone()));
    fields.push(("lieu".to_string(), book.places.join(", ")));
    fields.push(("date".to_string(), book.year.clone()));
    if let Some(pages) = book.pages {
        fields.push(("pages".to_string(), pages.to_string()));
    }
    fields.push(("isbn".to_string(), book_isbn(isbn, book)));
    if let Some(doi) = &book.ids.crossref_doi {
        fields.push(("doi".to_string(), doi.clone()));
    }
    if language != "fr" {
        fields.push(("langue".to_string(), language.to_string()));
    }
    template_line("Ouvrage", &fields)
}

fn book_isbn(isbn: &str, book: &Book) -> String {
    book.ids.isbn13.clone().or_else(|| book.ids.isbn10.clone()).unwrap_or_else(|| isbn.to_string())
}

fn author_fields(book: &Book, last: &str, first: &str) -> Vec<(String, String)> {
    let mut fields = vec![];
    for (index, author) in book.authors.iter().enumerate() {
        let (family, given) = split_name(author);
        let number = index + 1;
        fields.push((format!("{last}{number}"), family.to_string()));
        if let Some(given) = given {
            fields.push((format!("{first}{number}"), given.to_string()));
        }
    }
    fields
}

fn full_author_fields(book: &Book, first: &str, prefix: &str) -> Vec<(String, String)> {
    book.authors
        .iter()
        .enumerate()
        .map(|(index, author)| {
            let key = if index == 0 { first.to_string() } else { format!("{prefix}{}", index + 1) };
            (key, author.clone())
        })
        .collect()
}

/// Split a natural-order name into family name and given names, for citation
/// templates that want them separately.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once(' ') {
        Some((given, family)) if !given.is_empty() && !family.is_empty() => (family, Some(given)),
        _ => (name, None),
    }
}

fn template_line(name: &str, fields: &[(String, String)]) -> String {
    let mut line = format!("{{{{{name}");
    for (key, value) in fields {
        if value.trim().is_empty() {
            continue;
        }
        line.push_str(&format!(" | {key} = {}", wiki_text(value)));
    }
    line.push_str("}}");
    line
}

/// Catalogue text safe inside wiki markup: no template braces, link brackets
/// or pipes that would change the generated line.
fn wiki_text(text: &str) -> String {
    text.replace('{', "&#123;")
        .replace('}', "&#125;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace('|', "{{!}}")
        .replace('\'', "&#39;")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bookmeta::Source;

    fn book() -> Book {
        let mut book = Book::new(Source::Dnb);
        book.authors = vec!["Michael H. Kater".to_string()];
        book.title = "Gewagtes Spiel".to_string();
        book.subtitle = Some("Jazz im Nationalsozialismus".to_string());
        book.publisher = "Deutscher Taschenbuch Verlag".to_string();
        book.places = vec!["München".to_string()];
        book.year = "1998".to_string();
        book
    }

    #[test]
    fn default_line_matches_the_old_tool() {
        let line = wiki_line("978-3-423-30666-9", &book());
        assert_eq!(
            line,
            "* [[Michael H. Kater]]: ''Gewagtes Spiel : Jazz im Nationalsozialismus'' \
             [[Deutscher Taschenbuch Verlag]], [[München]] 1998, ISBN 978-3-423-30666-9"
        );
        assert_eq!(
            plain_line(&line),
            "* Michael H. Kater: ''Gewagtes Spiel : Jazz im Nationalsozialismus'' \
             Deutscher Taschenbuch Verlag, München 1998, ISBN 978-3-423-30666-9"
        );
    }

    #[test]
    fn citation_templates() {
        let book = book();
        let cite = citation_cite_book("978-3-423-30666-9", &book, "de");
        assert!(cite.starts_with("{{cite book"));
        assert!(cite.contains("| last1 = Kater"));
        assert!(cite.contains("| first1 = Michael H."));
        assert!(cite.contains("| language = de"));
        assert!(cite.contains("| isbn = 978-3-423-30666-9"));
        let literatur = citation_literatur("978-3-423-30666-9", &book, "de");
        assert!(literatur.starts_with("{{Literatur"));
        assert!(literatur.contains("| Autor = Michael H. Kater"));
        assert!(!literatur.contains("Sprache"));
        let ouvrage = citation_ouvrage("978-3-423-30666-9", &book, "de");
        assert!(ouvrage.starts_with("{{Ouvrage"));
        assert!(ouvrage.contains("| auteur1 = Michael H. Kater"));
        assert!(ouvrage.contains("| langue = de"));
    }

    #[test]
    fn unsafe_delimiters_are_kept_out_of_wikitext() {
        let mut book = book();
        book.title = "A | B {{C}} [[D]]".to_string();
        let line = wiki_line("978-3-423-30666-9", &book);
        assert!(!line.contains("{{C}}"));
        assert!(!line.contains("[[D]]"));
        book.title = "Assassin's Creed".to_string();
        assert!(wiki_line("978-3-423-30666-9", &book).contains("Assassin&#39;s Creed"));
        let cite = citation_cite_book("978-3-423-30666-9", &book, "en");
        assert!(!cite.contains("| B"));
    }

    #[test]
    fn invalid_formats_fall_back_to_plain() {
        assert_eq!(Format::parse("nonsense"), Format::Plain);
        assert_eq!(Format::parse("literatur"), Format::Literatur);
        assert_eq!(language_key(Some("fr"), Format::Plain), "fr");
        assert_eq!(language_key(Some("xx"), Format::Literatur), "de");
    }

    #[test]
    fn results_render() {
        use bookmeta::SourceFailure;
        let isbn = Isbn::parse("978-3-423-30666-9").unwrap();
        let mut other = book();
        other.source = Source::K10plus;
        let lookup = Lookup {
            isbn: isbn.clone(),
            books: vec![book(), other],
            wikidata: vec![],
            failures: vec![SourceFailure { source: Source::Loc, message: "timeout".to_string() }],
        };
        let page = Page {
            title: "ISBN to wiki",
            isbn: isbn.display().to_string(),
            template: "plain",
            language: "de",
            formats: FORMATS,
            languages: LANGUAGES,
            error: None,
            results: Some(Results::new(&lookup, &isbn, Format::Plain, "de")),
        };
        let html = page.render().unwrap();
        assert!(html.contains("Merged entry"));
        assert!(html.contains("No answer from Library of Congress."));
        assert!(html.contains("wd-infernal.toolforge.org/isbn/isbn/9783423306669"));
    }
}

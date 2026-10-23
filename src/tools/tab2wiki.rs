//! `tab2wiki.php`: turns a table pasted from a spreadsheet (tab-separated)
//! into a wiki table.

use crate::web::{AppError, Params, render};
use askama::Template;
use axum::response::Html;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    text: String,
    colhead: Option<String>,
    rowhead: Option<String>,
    compressed: Option<String>,
    sortable: Option<String>,
    doit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    column_headers: bool,
    row_headers: bool,
    compressed: bool,
    sortable: bool,
}

#[derive(Template)]
#[template(path = "tools/tab2wiki.html")]
struct Page {
    title: &'static str,
    text: String,
    options: Options,
    wikitext: Option<String>,
}

pub async fn handler(Params(request): Params<Request>) -> Result<Html<String>, AppError> {
    let submitted = request.doit.is_some();
    let options = match submitted {
        true => Options {
            column_headers: request.colhead.is_some(),
            row_headers: request.rowhead.is_some(),
            compressed: request.compressed.is_some(),
            sortable: request.sortable.is_some(),
        },
        false => Options { column_headers: true, row_headers: false, compressed: true, sortable: true },
    };
    let wikitext = submitted.then(|| convert(&request.text, options));
    render(&Page { title: "Table to wiki", text: request.text, options, wikitext })
}

fn convert(text: &str, options: Options) -> String {
    // Row headers need one cell per line.
    let compressed = options.compressed && !options.row_headers;
    let separator = if compressed { "||" } else { "\n|" };
    let rows = text.replace("\r\n", "\n");
    let rows = rows.trim_matches('\n').replace('\n', "\n|-\n|").replace('\t', separator);
    let sortable = if options.sortable { " class=\"sortable\"" } else { "" };
    let mut wiki = format!("{{| border=\"1\"{sortable}\n|{rows}\n|}}");

    if options.column_headers {
        let (first, rest) = wiki.split_once("\n|-\n").unwrap_or((&wiki, ""));
        let mut first = first.replace("\n|", "\n!");
        if compressed {
            first = first.replace("||", "!!");
        }
        wiki = format!("{first}\n|-\n{rest}");
    }
    if options.row_headers {
        wiki = wiki.replace("\n|-\n|", "\n|-\n!");
        if !options.column_headers {
            let (first, rest) = wiki.split_once("\n|").unwrap_or((&wiki, ""));
            wiki = format!("{first}\n!{rest}");
        }
    }
    wiki
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "a\tb\n1\t2\n";

    fn options(column_headers: bool, row_headers: bool, compressed: bool, sortable: bool) -> Options {
        Options { column_headers, row_headers, compressed, sortable }
    }

    #[test]
    fn default_options() {
        assert_eq!(
            convert(TABLE, options(true, false, true, true)),
            "{| border=\"1\" class=\"sortable\"\n!a!!b\n|-\n|1||2\n|}"
        );
    }

    #[test]
    fn plain() {
        assert_eq!(
            convert("a\tb\r\n1\t2", options(false, false, false, false)),
            "{| border=\"1\"\n|a\n|b\n|-\n|1\n|2\n|}"
        );
    }

    #[test]
    fn row_headers_uncompress() {
        assert_eq!(convert(TABLE, options(false, true, true, false)), "{| border=\"1\"\n!a\n|b\n|-\n!1\n|2\n|}");
        assert_eq!(convert(TABLE, options(true, true, false, false)), "{| border=\"1\"\n!a\n!b\n|-\n!1\n|2\n|}");
    }
}

//! `html2wiki.php`: converts HTML tables (and a few simple tags) to wiki
//! markup. After the "HTML to Wiki Converter - tables" by Borislav Manolov
//! (GPL-2.0-or-later).

use crate::web::{AppError, Params, render};
use askama::Template;
use axum::response::Html;
use regex::{Captures, Regex};
use serde::Deserialize;
use std::sync::LazyLock;

/// Marks a line break while the markup is still being rewritten.
const NL: &str = "=N=";
const MAX_DASHES: usize = 20;

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    html: String,
    dashes: Option<String>,
    oneline: Option<String>,
    escape: Option<String>,
    otherhtml: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    dashes: usize,
    oneline: bool,
    escape: bool,
    other_html: bool,
}

#[derive(Template)]
#[template(path = "tools/html2wiki.html")]
struct Page {
    title: &'static str,
    html: String,
    options: Options,
    wikitext: Option<String>,
}

pub async fn handler(Params(request): Params<Request>) -> Result<Html<String>, AppError> {
    let submitted = !request.html.trim().is_empty();
    let options = Options {
        dashes: request.dashes.and_then(|d| d.trim().parse().ok()).unwrap_or(1).clamp(1, MAX_DASHES),
        oneline: request.oneline.is_some_and(|o| o != "0" && !o.is_empty()),
        escape: request.escape.is_some() || !submitted,
        other_html: request.otherhtml.is_some() || !submitted,
    };
    let wikitext = submitted.then(|| convert(&request.html, options).trim_start_matches('\n').to_string());
    render(&Page { title: "HTML to wiki", html: request.html, options, wikitext })
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

static SPACE_BETWEEN_TAGS: LazyLock<Regex> = LazyLock::new(|| regex(r">\s+<"));
static TABLE_END: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)</table>"));
static ENDS_DROPPED: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)</(caption|tr|th|td)>"));
static TABLE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)<table([^>]*)>"));
static CAPTION: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)<caption>"));
static ROW: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)<tr(.*?)>"));
static HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)<th(.*?)>"));
static CELL: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)<td(.*?)>"));
static LINE_START_SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\n *"));
static ROW_AFTER_TABLE_START: LazyLock<Regex> = LazyLock::new(|| regex(r"\{\|(.*)\n\|-+ *\n"));
static OTHER_TAGS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        ("<b>", "'''"),
        ("</b>", "'''"),
        ("<i>", "''"),
        ("</i>", "''"),
        ("<p>", "\n"),
        ("</p>", ""),
        ("<br/?>", "\n"),
        ("<hr/?>", "\n----\n"),
    ]
    .into_iter()
    .map(|(tag, wiki)| (regex(&format!("(?i){tag}")), wiki))
    .collect()
});

fn convert(html: &str, options: Options) -> String {
    let mut s = html.replace('\r', "");
    if options.escape {
        s = s.replace('!', "&#33;").replace('|', "&#124;");
    }
    s = s.replace('\n', " \n");
    s = SPACE_BETWEEN_TAGS.replace_all(&s, "><").into_owned();
    s = TABLE_END.replace_all(&s, format!("{NL}|}}")).into_owned();
    s = ENDS_DROPPED.replace_all(&s, "").into_owned();
    s = TABLE.replace_all(&s, |c: &Captures| format!("{NL}{{| {}", attributes(c))).into_owned();
    s = CAPTION.replace_all(&s, format!("{NL}|+")).into_owned();
    let dashes = "-".repeat(options.dashes);
    s = ROW.replace_all(&s, |c: &Captures| format!("{NL}|{dashes} {}", attributes(c))).into_owned();
    s = HEADER.replace_all(&s, |c: &Captures| format!("{NL}! {} | ", attributes(c))).into_owned();
    s = CELL.replace_all(&s, |c: &Captures| format!("{NL}| {} | ", attributes(c))).into_owned();
    s = s.replace(&format!("\n{NL}"), "\n").replace(NL, "\n");
    s = LINE_START_SPACES.replace_all(&s, "\n").into_owned();
    s = ROW_AFTER_TABLE_START.replace_all(&s, "{|$1\n").into_owned();
    s = s.replace("!  |", "!").replace("|  |", "|").replace("\\\"", "\"");
    if options.oneline {
        s = one_line_cells(s);
    }
    if options.other_html {
        for (tag, wiki) in OTHER_TAGS.iter() {
            s = tag.replace_all(&s, *wiki).into_owned();
        }
        s = s.replace("\n\n\n", "\n\n");
    }
    s
}

fn attributes(captures: &Captures) -> String {
    captures.get(1).map_or("", |m| m.as_str()).replace('\n', "").trim().to_string()
}

/// Puts the cells of a row on one line (`| a || b`), and joins every other
/// line break that is not the start of a table, row or cell.
fn one_line_cells(s: String) -> String {
    let mut bytes = s.into_bytes();
    let (mut previous_cell, mut previous_header) = (false, false);
    let mut from = 0;
    while let Some(offset) = bytes.get(from..).and_then(|rest| rest.iter().position(|&b| b == b'\n')) {
        let pos = from + offset;
        let next = bytes.get(pos + 1).copied();
        let after = bytes.get(pos + 2).copied();
        match next {
            Some(b'|') => {
                if previous_cell && after == Some(b' ') {
                    bytes.splice(pos..=pos, *b" |");
                } else {
                    previous_cell = after == Some(b' ');
                }
                previous_header = false;
            }
            Some(b'!') => {
                if previous_header {
                    bytes.splice(pos..=pos, *b" !");
                } else {
                    previous_header = true;
                }
                previous_cell = false;
            }
            Some(b'{') if after == Some(b'|') => (previous_cell, previous_header) = (false, false),
            _ => {
                if let Some(b) = bytes.get_mut(pos) {
                    *b = b' ';
                }
            }
        }
        from = pos + 1;
    }
    // Only ASCII bytes were changed, so this is still UTF-8.
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "<table border=1><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>";

    fn options(dashes: usize, oneline: bool, escape: bool, other_html: bool) -> Options {
        Options { dashes, oneline, escape, other_html }
    }

    // Expected values are from the PHP original (with its post-PHP-7 regression fixed).
    #[test]
    fn simple_table() {
        assert_eq!(convert(TABLE, options(1, false, false, false)), "\n{| border=1\n! A\n! B\n|- \n| 1\n| 2\n|}");
        assert_eq!(convert(TABLE, options(1, true, false, false)), "\n{| border=1\n! A !! B\n|- \n| 1 || 2\n|}");
    }

    #[test]
    fn attributes_caption_escaping_and_other_html() {
        let html = "<TABLE class=\"x\">\n<caption>Cap</caption>\n <tr style=\"a\">\n  <td colspan=2><b>bold</b> a|b!</td>\n </tr>\n <tr><td>x</td><td>y<br>z</td></tr>\n</TABLE>";
        assert_eq!(
            convert(html, options(3, false, true, true)),
            "\n{| class=\"x\"\n|+Cap\n|--- style=\"a\"\n| colspan=2 | '''bold''' a&#124;b&#33;\n|--- \n| x\n| y\nz\n|}"
        );
    }

    #[test]
    fn one_line_without_table_attributes() {
        let html = "<table><tr><td>x</td><td>y</td></tr><tr><td>p</td><td>q</td></tr></table>";
        assert_eq!(convert(html, options(2, true, false, true)), "\n{| \n| x || y\n|-- \n| p || q\n|}");
    }

    #[test]
    fn other_html_only() {
        let html = "<p>Hello <i>world</i></p><hr/><p>Bye</p>";
        assert_eq!(convert(html, options(1, false, false, true)), "\nHello ''world''\n----\n\nBye");
    }
}

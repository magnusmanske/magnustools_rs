//! `makeref` (formerly its own tool): fills in a citation template from a
//! form. The citation types, their fields, and their wiki code come from
//! `MakeRef/<Language>` on Meta, which anyone can extend.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;
use regex::{Captures, Regex};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const CONFIG_PREFIX: &str = "MakeRef/";
/// Every citation type gets this field first: the `<ref name>`.
const REF_NAME: &str = "Reference ID|refid|Reuters 2006-07-21";
const BIBTEX: &str = "bibtex";
const ACCESS_DATES: &[&str] = &["access-date", "accessdate", "access_date"];

static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").expect("valid regex"));
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\$([^{}$]+)\}").expect("valid regex"));

/// A kind of reference, such as "CS1 News article".
#[derive(Debug, PartialEq)]
struct RefType {
    /// The name in lower case, with underscores; prefixes the form's field names.
    key: String,
    name: String,
    fields: Vec<Field>,
    /// Wiki code with `{$key}` placeholders, and `<!key>…</!key>` for what
    /// is only there if `key` has a value.
    code: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    key: String,
    description: String,
    /// Lines, as the configuration breaks long examples.
    example: Vec<String>,
    required: bool,
    multiline: bool,
    /// BibTeX fields that can fill this one, in lower case.
    bibtex: Vec<String>,
}

/// The configuration's wiki text: `== Type ==`, then `=== Fields ===` with a
/// table of `|*Description||key=bib.alternative||Example` rows (`*`:
/// required; `@key`: multiline), and `=== Code ===`. Sections with
/// translated names are fields and code by their order.
fn parse_config(wikitext: &str) -> Vec<RefType> {
    let mut types: Vec<RefType> = vec![];
    let mut section = "";
    let mut sections = 0;
    for line in wikitext.lines() {
        let line = line.replace("||", "|");
        let (level, text) = heading(line.trim());
        match level {
            2 => {
                let name = upper_first(text.trim());
                let key = name.to_lowercase().replace(' ', "_");
                types.push(RefType { key, name, fields: vec![], code: String::new() });
                (section, sections) = ("", 0);
            }
            3 => {
                section = match (text.trim().to_lowercase().as_str(), sections) {
                    ("fields", _) | (_, 0) => "fields",
                    ("code", _) | (_, 1) => "code",
                    _ => "",
                };
                sections += 1;
            }
            _ => {
                let Some(current) = types.last_mut() else { continue };
                match section {
                    "fields" => {
                        if let Some(field) = parse_field(text) {
                            match current.fields.iter_mut().find(|f| f.key == field.key) {
                                Some(existing) => *existing = field,
                                None => current.fields.push(field),
                            }
                        }
                    }
                    // Code lines join without line breaks: one line of wiki code.
                    "code" => current.code.push_str(text),
                    _ => {}
                }
            }
        }
    }
    for t in &mut types {
        t.fields.extend(parse_field(REF_NAME));
        t.fields.rotate_right(1);
    }
    types.retain(|t| !t.fields.is_empty() && !t.code.is_empty());
    types
}

/// `== X ==` is `(2, " X ")`: each leading `=` takes a character off either end.
fn heading(mut line: &str) -> (usize, &str) {
    let mut level = 0;
    while let Some(inner) = line.strip_prefix('=') {
        let end = inner.char_indices().last().map_or(0, |(i, _)| i);
        line = &inner[..end];
        level += 1;
    }
    (level, line)
}

fn parse_field(line: &str) -> Option<Field> {
    let line = line.trim();
    if line.is_empty() || ["{|", "|}", "|-"].iter().any(|p| line.starts_with(p)) {
        return None;
    }
    let line = line.strip_prefix('|').unwrap_or(line);
    let (required, line) = match line.strip_prefix('*') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let mut parts = line.splitn(3, '|');
    let description = parts.next().unwrap_or_default().trim().to_string();
    let key = parts.next().unwrap_or_default().trim();
    let example = parts.next().unwrap_or_default().trim();
    let (key, alternatives) = key.split_once('=').unwrap_or((key, ""));
    let (multiline, key) = match key.strip_prefix('@') {
        Some(key) => (true, key),
        None => (false, key),
    };
    if key.is_empty() {
        return None;
    }
    let bibtex = alternatives
        .split(',')
        .filter_map(|a| a.trim().to_lowercase().strip_prefix("bib.").map(str::to_string))
        .collect();
    let example = LINE_BREAK.split(example).map(str::to_string).collect();
    Some(Field { key: key.to_string(), description, example, required, multiline, bibtex })
}

/// The fields of a BibTeX entry, such as `@book{key, title={A {B}}, year=2001}`,
/// with lower case names.
fn parse_bibtex(bibtex: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let Some((_, body)) = bibtex.split_once(['{', '(']) else { return fields };
    let mut chars = body.chars().peekable();
    let mut name = String::new();
    while let Some(c) = chars.next() {
        match c {
            // The citation key, or the end of a value without delimiters.
            ',' => name.clear(),
            '=' => {
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                let value = match chars.peek() {
                    Some('{') => {
                        chars.next();
                        let mut depth = 1;
                        chars
                            .by_ref()
                            .take_while(|&c| {
                                depth += match c {
                                    '{' => 1,
                                    '}' => -1,
                                    _ => 0,
                                };
                                depth > 0
                            })
                            .collect()
                    }
                    Some('"') => {
                        chars.next();
                        chars.by_ref().take_while(|&c| c != '"').collect()
                    }
                    _ => {
                        let mut value = String::new();
                        while let Some(c) = chars.next_if(|&c| c != ',' && c != '}' && c != ')') {
                            value.push(c);
                        }
                        value
                    }
                };
                fields.insert(name.trim().to_lowercase(), clean_bibtex_value(&value));
                name.clear();
            }
            c => name.push(c),
        }
    }
    fields
}

fn clean_bibtex_value(value: &str) -> String {
    let value = value.replace(['{', '}'], "").replace("--", "-");
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The wiki code of a reference.
fn wiki_code(ref_type: &RefType, values: &HashMap<String, String>) -> String {
    let value = |key: &str| values.get(key).map(String::as_str).unwrap_or_default();
    let code = conditionals(&ref_type.code, &|key| !value(key).is_empty());
    let code = PLACEHOLDER.replace_all(&code, |c: &Captures| mediawiki::template_value(value(&c[1])));
    let name = value("refid").replace('"', "");
    let name = match name.trim() {
        "" => String::new(),
        name => format!(" name=\"{name}\""),
    };
    format!("<ref{name}>{}</ref>", code.trim())
}

/// `<!key>…</!key>` is the part between if `has_value(key)`, else nothing.
fn conditionals(code: &str, has_value: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(start) = rest.find("<!") {
        let after = &rest[start + 2..];
        let block = after.split_once('>').and_then(|(key, body)| {
            let close = format!("</!{key}>");
            let (inner, rest) = body.split_once(&close)?;
            (!key.is_empty() && !key.starts_with("--")).then_some((key, inner, rest))
        });
        match block {
            Some((key, inner, after_block)) => {
                out.push_str(&rest[..start]);
                if has_value(key) {
                    out.push_str(&conditionals(inner, has_value));
                }
                rest = after_block;
            }
            // Not a block, such as an HTML comment: as it is.
            None => {
                out.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct Request(HashMap<String, String>);

impl Request {
    fn get(&self, key: &str) -> &str {
        self.0.get(key).map_or("", |v| v.trim())
    }
}

struct Input {
    name: String,
    field: Field,
    value: String,
}

struct TypeLink {
    name: String,
    url: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "tools/makeref.html")]
struct Page {
    title: &'static str,
    language: String,
    config_url: String,
    config_title: String,
    types: Vec<TypeLink>,
    reftype: String,
    type_name: String,
    inputs: Vec<Input>,
    missing: Vec<String>,
    prefilled: bool,
    code: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = match request.get("language").to_lowercase() {
        l if l.is_empty() => "en".to_string(),
        l if is_language_code(&l) => l,
        l => return Err(AppError::BadRequest(format!("“{l}” is not a language code."))),
    };
    let meta = Wiki::new("meta", "wikimedia")?;
    let config_title = format!("{CONFIG_PREFIX}{}", upper_first(&language));
    let wikitext = mediawiki::page_wikitext(&state.http, &meta, &config_title)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("There is no {config_title} on Meta for this language yet.")))?;
    let types = parse_config(&wikitext);
    if types.is_empty() {
        return Err(AppError::BadRequest(format!("{config_title} on Meta defines no types of reference.")));
    }
    let ref_type = match request.get("reftype") {
        "" => types.first(),
        key => types.iter().find(|t| t.key == key),
    }
    .ok_or_else(|| AppError::BadRequest(format!("{config_title} on Meta has no such type of reference.")))?;

    let submitted = !request.get("doit").is_empty();
    let bibtex = parse_bibtex(request.get(&format!("{}_{BIBTEX}", ref_type.key)));
    let mut prefilled = false;
    let mut values = HashMap::new();
    let mut inputs = vec![];
    for field in &ref_type.fields {
        let name = format!("{}_{}", ref_type.key, field.key);
        let mut value = request.get(&name).to_string();
        if value.is_empty()
            && let Some(v) = field.bibtex.iter().find_map(|b| bibtex.get(b)).filter(|v| !v.is_empty())
        {
            value = v.clone();
            prefilled = true;
        }
        if value.is_empty() && !submitted && ACCESS_DATES.contains(&field.key.as_str()) {
            value = Utc::now().format("%Y-%m-%d").to_string();
        }
        values.insert(field.key.clone(), value.clone());
        inputs.push(Input { name, field: field.clone(), value });
    }
    if prefilled {
        // Filled in from it; to check, not to fill in again.
        inputs.iter_mut().filter(|i| i.field.key == BIBTEX).for_each(|i| i.value.clear());
    }
    let missing: Vec<String> = match submitted {
        true => inputs
            .iter()
            .filter(|i| i.field.required && i.value.is_empty())
            .map(|i| i.field.description.clone())
            .collect(),
        false => vec![],
    };
    let code = (submitted && missing.is_empty() && !prefilled).then(|| wiki_code(ref_type, &values));

    let type_links = types
        .iter()
        .map(|t| TypeLink {
            name: t.name.clone(),
            url: format!("/makeref/?language={}&reftype={}", encode_component(&language), encode_component(&t.key)),
            current: t.key == ref_type.key,
        })
        .collect();
    render(&Page {
        title: "MakeRef",
        config_url: meta.page_url(&config_title),
        config_title,
        language,
        types: type_links,
        reftype: ref_type.key.clone(),
        type_name: ref_type.name.clone(),
        inputs,
        missing,
        prefilled,
        code,
    })
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    const CONFIG: &str = "Intro text\n\n== CS1 Journal paper ==\n\n=== Fields ===\n{| border=1\n |-\n |*Paper title||title=bib.title||Self-replication\n |-\n |Pages||pages=bib.pages,bib.page||1043-1046\n |-\n |URL||url||http://example.com/<br />long/path\n |-\n |Date you reviewed the source||access-date||2006-07-23\n |-\n |BibTex||@bibtex||You can use BibTex\n |}\n\n=== Code ===\n{{cite journal\n<!title> |title={$title}</!title>\n<!pages> |pages={$pages}</!pages>\n<!url> |url={$url}</!url> <!-- note -->\n<!nofield> |x={$nofield}</!nofield> }}\n\n== Empty ==\n=== End ===\n";

    #[test]
    fn configuration() {
        let types = parse_config(CONFIG);
        assert_eq!(types.len(), 1);
        let t = &types[0];
        assert_eq!((t.key.as_str(), t.name.as_str()), ("cs1_journal_paper", "CS1 Journal paper"));
        let keys: Vec<&str> = t.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["refid", "title", "pages", "url", "access-date", "bibtex"]);
        assert!(t.fields[1].required && !t.fields[2].required);
        assert_eq!(t.fields[2].bibtex, ["pages", "page"]);
        assert_eq!(t.fields[3].example, ["http://example.com/", "long/path"]);
        assert!(t.fields[5].multiline);
        assert!(t.code.starts_with("{{cite journal<!title> |title={$title}</!title><!pages>"));
        assert_eq!(heading("== 一般網頁 =="), (2, " 一般網頁 "));
        let translated = parse_config(
            "== बातमी ==\n=== क्षेत्रे ===\n|*शीर्षक||title||X\n=== कोड ===\n{{cite news|title={$title}}}\n=== End ===\nignored",
        );
        assert_eq!(translated[0].fields[1].key, "title");
        assert_eq!(translated[0].code, "{{cite news|title={$title}}}");
    }

    #[test]
    fn code() {
        let t = &parse_config(CONFIG)[0];
        let values = HashMap::from([
            ("refid".to_string(), "Smith \"2001\"".to_string()),
            ("title".to_string(), "On [[Prion|prions]] | {{lang|de|x}}".to_string()),
            ("url".to_string(), String::new()),
        ]);
        assert_eq!(
            wiki_code(t, &values),
            "<ref name=\"Smith 2001\">{{cite journal |title=On [[Prion|prions]] {{!}} {{lang|de|x}} <!-- note --> }}</ref>"
        );
        let nested = conditionals("<!a>A<!b>B</!b></!a><!c>C</!c>", &|k| k != "c");
        assert_eq!(nested, "AB");
    }

    #[test]
    fn bibtex() {
        let fields = parse_bibtex(
            "@article{smith01,\n  Title = {The {DNA} of\n  things},\n  author = \"Smith, J. and Doe, A.\",\n  year = 2001,\n  pages = {12--16}\n}",
        );
        assert_eq!(fields["title"], "The DNA of things");
        assert_eq!(fields["author"], "Smith, J. and Doe, A.");
        assert_eq!(fields["year"], "2001");
        assert_eq!(fields["pages"], "12-16");
        assert!(!fields.contains_key("smith01"));
        assert!(parse_bibtex("no entry").is_empty());
    }

    #[tokio::test]
    async fn bad_language() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/makeref/?language=en%2F..%2FX&doit=1").await;
        assert_eq!(page.status.as_u16(), 400);
        assert!(page.body.contains("is not a language code"));
    }
}

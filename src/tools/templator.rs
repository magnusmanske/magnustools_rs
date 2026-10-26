//! `templator` (formerly its own tool): fills in a template, such as an
//! infobox, from a form. The templates and their parameters come from
//! `Templator/<Language>` on Meta, which anyone can extend.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component, is_language_code, upper_first};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const CONFIG_PREFIX: &str = "Templator/";
const DEFAULT_LANGUAGE: &str = "de";
/// Between descriptions, to render them all in one go.
const SPLIT: &str = "TEMPLATORSPLIT";

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));
static SPLIT_HTML: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:<p>)?\s*{SPLIT}\s*(?:</p>)?")).expect("valid regex"));

/// A `==` section: a template, or, without fields, a heading over the next ones.
#[derive(Debug, PartialEq)]
struct Section {
    /// The name in lower case, without spaces; prefixes the form's field names.
    key: String,
    /// The template's name.
    name: String,
    fields: Vec<Field>,
}

#[derive(Debug, PartialEq)]
struct Field {
    /// The template parameter.
    key: String,
    /// Wiki text.
    description: String,
    required: bool,
}

#[derive(Debug, Default, PartialEq)]
struct Config {
    /// Texts of the page, in the configuration's language: `;key:text` lines.
    interface: HashMap<String, String>,
    sections: Vec<Section>,
}

/// The configuration: `;key:text` lines, then sections (`== Name ==`, any
/// level) with a table of `|*Parameter||Description` rows (`*`: required).
fn parse_config(wikitext: &str) -> Config {
    let wikitext = COMMENT.replace_all(wikitext, "");
    let mut config = Config::default();
    let mut in_table = false;
    for line in wikitext.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if config.sections.is_empty()
            && let Some((key, text)) = line.strip_prefix(';').and_then(|l| l.split_once(':'))
        {
            config.interface.insert(key.trim().to_lowercase(), text.trim().to_string());
            continue;
        }
        if line.starts_with("==") {
            let name = upper_first(line.replace('=', "").trim());
            let key = name.to_lowercase().replace(' ', "");
            config.sections.push(Section { key, name, fields: vec![] });
            in_table = false;
            continue;
        }
        let Some(section) = config.sections.last_mut() else { continue };
        if line.starts_with("{|") || line.starts_with("|}") {
            in_table = line.starts_with("{|");
            continue;
        }
        if !in_table || line.starts_with("|-") || line.starts_with('!') {
            continue;
        }
        match line.strip_prefix('|') {
            Some(row) => {
                let (key, description) = row.split_once("||").unwrap_or((row, ""));
                let (required, key) = match key.trim().strip_prefix('*') {
                    Some(key) => (true, key),
                    None => (false, key),
                };
                let key = wikimisc::escape::unescape_html(key.trim()).into_owned();
                if key.is_empty() {
                    continue;
                }
                let field = Field { key, description: description.trim().to_string(), required };
                match section.fields.iter_mut().find(|f| f.key.to_uppercase() == field.key.to_uppercase()) {
                    Some(existing) => *existing = field,
                    None => section.fields.push(field),
                }
            }
            // A description that goes on.
            None => {
                if let Some(field) = section.fields.last_mut() {
                    field.description.push('\n');
                    field.description.push_str(line);
                }
            }
        }
    }
    config
}

/// The template, with `values` (one per field), all parameters or only
/// those with a value, on one line or one per parameter.
fn wiki_code(section: &Section, values: &[String], all: bool, one_line: bool) -> String {
    let params = section.fields.iter().zip(values).filter(|(_, value)| all || !value.is_empty());
    let params = params.map(|(field, value)| {
        let value = mediawiki::template_value(value);
        match one_line {
            true => format!(" |{}={value}", field.key),
            false => format!(" |{} = {value}\n", field.key),
        }
    });
    let separator = if one_line { "" } else { "\n" };
    format!("{{{{{}{separator}{}}}}}", section.name, params.collect::<String>())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct Request(HashMap<String, String>);

impl Request {
    fn get(&self, key: &str) -> &str {
        self.0.get(key).map_or("", |v| v.trim())
    }
}

struct Link {
    name: String,
    url: String,
    current: bool,
}

/// Templates under a heading.
struct Group {
    heading: Option<String>,
    links: Vec<Link>,
}

struct Input {
    name: String,
    key: String,
    required: bool,
    missing: bool,
    /// Rendered by the wiki, or escaped text.
    description_html: String,
    value: String,
}

#[derive(Template)]
#[template(path = "tools/templator.html")]
struct Page {
    title: &'static str,
    language: String,
    config_url: String,
    config_title: String,
    groups: Vec<Group>,
    template_key: String,
    template_name: String,
    inputs: Vec<Input>,
    button: String,
    needed: String,
    missing: Option<String>,
    results: Vec<(&'static str, String)>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = match request.get("language").to_lowercase() {
        l if l.is_empty() => DEFAULT_LANGUAGE.to_string(),
        l if is_language_code(&l) => l,
        l => return Err(AppError::BadRequest(format!("“{l}” is not a language code."))),
    };
    let meta = Wiki::new("meta", "wikimedia")?;
    let config_title = format!("{CONFIG_PREFIX}{}", upper_first(&language));
    let wikitext = mediawiki::page_wikitext(&state.http, &meta, &config_title)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("There is no {config_title} on Meta for this language yet.")))?;
    let config = parse_config(&wikitext);
    let mut templates = config.sections.iter().filter(|s| !s.fields.is_empty());
    let section = match request.get("templatekey") {
        "" => templates.next(),
        key => templates.rfind(|s| s.key == key),
    }
    .ok_or_else(|| AppError::BadRequest(format!("{config_title} on Meta has no such template.")))?;
    let text = |key: &str, default: &str| config.interface.get(key).cloned().unwrap_or_else(|| default.to_string());

    let submitted = !request.get("doit").is_empty();
    let values: Vec<String> = section.fields.iter().map(|f| request.get(&form_name(section, f)).to_string()).collect();
    let any_missing = submitted && section.fields.iter().zip(&values).any(|(f, v)| f.required && v.is_empty());
    let results = match submitted && !any_missing {
        false => vec![],
        true => vec![
            ("Filled-in parameters", wiki_code(section, &values, false, false)),
            ("Filled-in parameters, on one line", wiki_code(section, &values, false, true)),
            ("All parameters", wiki_code(section, &values, true, false)),
            ("All parameters, on one line", wiki_code(section, &values, true, true)),
        ],
    };
    let descriptions = descriptions_html(&state.http, &language, &section.fields).await;
    let inputs = section
        .fields
        .iter()
        .zip(values)
        .zip(descriptions)
        .map(|((field, value), description_html)| Input {
            name: form_name(section, field),
            key: field.key.clone(),
            required: field.required,
            missing: submitted && field.required && value.is_empty(),
            description_html,
            value,
        })
        .collect();

    render(&Page {
        title: "Templator",
        groups: groups(&config.sections, &language, &section.key),
        config_url: meta.page_url(&config_title),
        config_title,
        language,
        template_key: section.key.clone(),
        template_name: section.name.clone(),
        inputs,
        button: text("get", "Make wiki text"),
        needed: text("needed", "(the highlighted fields must be filled in; please fill in as many others as you can)"),
        missing: any_missing.then(|| text("neededmissing", "One or more fields must be filled in!")),
        results,
    })
}

/// As the old tool named them, so that links keep working.
fn form_name(section: &Section, field: &Field) -> String {
    format!("{}_{}", section.key, field.key.to_uppercase())
}

/// The templates, under the headings without fields before them.
fn groups(sections: &[Section], language: &str, current: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = vec![];
    for section in sections {
        if section.fields.is_empty() {
            groups.push(Group { heading: Some(section.name.clone()), links: vec![] });
            continue;
        }
        let url = format!(
            "/templator/?language={}&templatekey={}",
            encode_component(language),
            encode_component(&section.key)
        );
        let link = Link { name: section.name.clone(), url, current: section.key == current };
        match groups.last_mut() {
            Some(group) => group.links.push(link),
            None => groups.push(Group { heading: None, links: vec![link] }),
        }
    }
    groups.retain(|g| !g.links.is_empty());
    groups
}

/// The descriptions as HTML, rendered by that language's Wikipedia in one
/// request; as plain text if that fails.
async fn descriptions_html(http: &reqwest::Client, language: &str, fields: &[Field]) -> Vec<String> {
    let plain = || fields.iter().map(|f| wikimisc::escape::html(&f.description).into_owned()).collect();
    let Ok(wiki) = Wiki::new(language, "wikipedia") else { return plain() };
    let wikitext = fields.iter().map(|f| f.description.as_str()).collect::<Vec<_>>().join(&format!("\n\n{SPLIT}\n\n"));
    match mediawiki::parse_to_html(http, &wiki, &wikitext).await {
        Ok(html) => {
            let parts: Vec<String> = SPLIT_HTML.split(&html).map(|p| p.trim().to_string()).collect();
            if parts.len() == fields.len() { parts } else { plain() }
        }
        Err(e) => {
            tracing::warn!("templator: {e:#}");
            plain()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    const CONFIG: &str = "Intro [[Templator]].\n;title:Vorlagen-Generator\n;get:Wikitext erzeugen\n\n== Geographie ==\n\n=== Infobox See ===\nSiehe [[Vorlage:Infobox See]].\n{| class=\"wikitable\"\n !Parameter !! Kommentar\n |-\n |BILD||Something.jpg\n |-\n |*LAGE||[[Nicaragua]]\n (Mittelamerika)\n |-\n |Afir&#238;ener<!-- x -->||\n | Taxon_Rang\n |}\n;notinterface:x\n== Leer ==\n";

    #[test]
    fn configuration() {
        let config = parse_config(CONFIG);
        assert_eq!(config.interface["title"], "Vorlagen-Generator");
        assert!(!config.interface.contains_key("notinterface"));
        let names: Vec<&str> = config.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Geographie", "Infobox See", "Leer"]);
        let see = &config.sections[1];
        assert_eq!(see.key, "infoboxsee");
        let keys: Vec<&str> = see.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["BILD", "LAGE", "Afirîener", "Taxon_Rang"]);
        assert!(see.fields[1].required && !see.fields[0].required);
        assert_eq!(see.fields[1].description, "[[Nicaragua]]\n(Mittelamerika)");
        assert_eq!(form_name(see, &see.fields[2]), "infoboxsee_AFIRÎENER");

        let groups = groups(&config.sections, "de", "infoboxsee");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].heading.as_deref(), Some("Geographie"));
        assert!(groups[0].links[0].current);
        assert_eq!(groups[0].links[0].url, "/templator/?language=de&templatekey=infoboxsee");
    }

    #[test]
    fn code() {
        let config = parse_config(CONFIG);
        let see = &config.sections[1];
        let values = ["a.jpg".to_string(), "[[A|B]] | C".to_string(), String::new(), String::new()];
        assert_eq!(wiki_code(see, &values, false, true), "{{Infobox See |BILD=a.jpg |LAGE=[[A|B]] {{!}} C}}");
        assert_eq!(
            wiki_code(see, &values, true, false),
            "{{Infobox See\n |BILD = a.jpg\n |LAGE = [[A|B]] {{!}} C\n |Afirîener = \n |Taxon_Rang = \n}}"
        );
    }

    #[tokio::test]
    async fn bad_language() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/templator/?language=de%2F..&doit=1").await;
        assert_eq!(page.status.as_u16(), 400);
        assert!(page.body.contains("is not a language code"));
    }
}

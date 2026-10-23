//! `prepbio.php`: wikitext for a new English Wikipedia biography (intro,
//! infobox, categories, authority control), optionally pre-filled from a
//! Wikidata item.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Label, hint. The parameter name is the label in lowercase without spaces.
const FIELDS: &[(&str, &str)] = &[
    ("Last name", "Required"),
    ("First name(s)", ""),
    ("Alternative name(s)", ""),
    ("Occupation", "XYZ gives a stub template (XYZ-stub) and a category (XYZs)"),
    ("Nationality", "If given here, it can be left out of the description"),
    ("Description", "e.g. \"historian specializing in polar bears\""),
    ("Day of birth", "Month and day; the month may be abbreviated"),
    ("Year of birth", ""),
    ("Place of birth", ""),
    ("Day of death", "Month and day; the month may be abbreviated"),
    ("Year of death", ""),
    ("Place of death", ""),
    ("Image", ""),
    ("Authority control", "VIAF=xxx|ORCID=xxx|…"),
];

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const MONTH_ABBREVIATIONS: &[(&str, &str)] = &[
    ("Jan.", "January"),
    ("Feb.", "February"),
    ("Mar.", "March"),
    ("Apr.", "April"),
    ("Jun.", "June"),
    ("Jul.", "July"),
    ("Aug.", "August"),
    ("Sep.", "September"),
    ("Sept.", "September"),
    ("Oct.", "October"),
    ("Nov.", "November"),
    ("Dec.", "December"),
];

const PERSON_INFOBOX: &[&str] = &[
    "name",
    "image",
    "image_size",
    "caption",
    "birth_name",
    "birth_date",
    "birth_place",
    "death_date",
    "death_place",
    "death_cause",
    "other_names",
    "known_for",
    "education",
    "employer",
    "occupation",
    "title",
    "networth",
    "height",
    "term",
    "predecessor",
    "successor",
    "party",
    "boards",
    "spouse",
    "partner",
    "children",
    "parents",
    "relatives",
    "signature",
    "website",
    "footnotes",
];

const WRITER_INFOBOX: &[&str] = &[
    "name",
    "image",
    "imagesize",
    "caption",
    "pseudonym",
    "birth_date",
    "birth_place",
    "death_date",
    "death_place",
    "occupation",
    "period",
    "genre",
    "subject",
    "movement",
    "debut_works",
    "influences",
    "influenced",
    "signature",
    "website",
    "footnotes",
];

/// Wikidata properties of identifiers, as `{{Authority control}}` names them.
const AUTHORITY_CONTROL: &[(&str, &str)] = &[
    ("VIAF", "P214"),
    ("LCCN", "P244"),
    ("ISNI", "P213"),
    ("ORCID", "P496"),
    ("GND", "P227"),
    ("SELIBR", "P906"),
    ("BNF", "P268"),
    ("BPN", "P651"),
    ("RID", "P1053"),
    ("BIBSYS", "P1015"),
    ("ULAN", "P245"),
    ("MBA", "P434"),
    ("NLA", "P409"),
    ("NDL", "P349"),
];

/// Label languages tried, in order, when pre-filling from Wikidata;
/// `mul` holds names that are the same in all languages.
const LANGUAGES: &[&str] = &["en", "mul", "de", "fr", "es", "it", "nl"];

fn key(label: &str) -> String {
    label.replace(' ', "").to_lowercase()
}

struct Field {
    label: &'static str,
    hint: &'static str,
    name: String,
    value: String,
}

#[derive(Template)]
#[template(path = "tools/prepbio.html")]
struct Page {
    title: &'static str,
    q: String,
    fields: Vec<Field>,
    categories: String,
    external_links: String,
    stub: bool,
    infobox: bool,
    brief_infobox: bool,
    wikitext: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<HashMap<String, String>>,
) -> Result<Html<String>, AppError> {
    let q = request.get("q").map(|q| q.chars().filter(char::is_ascii_digit).collect::<String>()).unwrap_or_default();
    let prefill = match q.is_empty() {
        true => HashMap::new(),
        false => from_wikidata(&state.http, &format!("Q{q}")).await?,
    };
    let value = |name: &str| request.get(name).or_else(|| prefill.get(name)).cloned().unwrap_or_default();
    let submitted = request.contains_key("doit");
    let checkbox = |name: &str, default: bool| if submitted { request.contains_key(name) } else { default };
    let fields =
        FIELDS.iter().map(|(label, hint)| Field { label, hint, name: key(label), value: value(&key(label)) }).collect();
    let page = Page {
        title: "PrepBio",
        q: if q.is_empty() { q } else { format!("Q{q}") },
        fields,
        categories: value("categories"),
        external_links: value("externallinks"),
        stub: checkbox("stub", true),
        infobox: checkbox("create_infobox", true),
        brief_infobox: checkbox("brief_infobox", false),
        wikitext: submitted.then(|| Biography(&request).wikitext()),
    };
    render(&page)
}

/// The submitted form.
struct Biography<'a>(&'a HashMap<String, String>);

impl Biography<'_> {
    fn get(&self, label: &str) -> String {
        self.0.get(&key(label)).map(|v| v.trim().to_string()).unwrap_or_default()
    }

    fn link(&self, label: &str) -> String {
        let v = self.get(label);
        if v.is_empty() { v } else { format!("[[{}]]", ucfirst(&v)) }
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    fn lines(&self, name: &str) -> Vec<String> {
        let text = self.0.get(name).map(String::as_str).unwrap_or_default();
        text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    fn date(&self, day: &str, year: &str) -> String {
        let day = self.get(day);
        let day = if day.is_empty() { day } else { format!("{}, ", expand_month(&day)) };
        format!("{day}{}", self.get(year))
    }

    fn wikitext(&self) -> String {
        let birth = self.date("Day of birth", "Year of birth");
        let death = self.date("Day of death", "Year of death");
        let nation = self.get("Nationality");
        let occupation = ucfirst(&self.get("Occupation"));
        let last_name = self.get("Last name");
        let full_name = format!("{} {last_name}", self.get("First name(s)"));
        let died = !self.get("Year of death").is_empty();

        let mut text = format!("'''{full_name}'''");
        let alternative_names = self.get("Alternative name(s)");
        if !alternative_names.is_empty() {
            text += &format!(" or '''{alternative_names}'''");
        }
        text += &if died { format!(" ({birth} &ndash; {death})") } else { format!(" (born {birth})") };

        let mut description = self.get("Description");
        if description.is_empty() {
            text += ".\n\n";
        } else {
            if !description.ends_with('.') {
                description.push('.');
            }
            let first = if nation.is_empty() { &description } else { &nation };
            let article = if first.to_lowercase().starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
            let verb = if died { "was" } else { "is" };
            text += &format!(" {verb} {article} {} {description}\n\n", self.link("Nationality"));
        }

        let birth_place = self.link("Place of birth");
        if !birth_place.is_empty() {
            text += &format!("{last_name} was born in {birth_place}.\n\n");
        }
        let death_place = self.link("Place of death");
        if !death_place.is_empty() {
            text += &format!("{last_name} died in {death_place}.\n\n");
        }
        let links: Vec<String> = self.lines("externallinks").iter().map(|l| format!("* [{l}]")).collect();
        if !links.is_empty() {
            text += &format!("== External links ==\n{}\n\n", links.join("\n"));
        }

        if self.has("stub") {
            let nation = ucfirst(nation.replace('.', "").trim());
            if !nation.is_empty() {
                text += &format!("{{{{{nation}-bio-stub}}}}\n");
            }
            if !occupation.is_empty() {
                text += &format!("{{{{{occupation}-stub}}}}\n");
            }
        }
        let authority_control = self.get("Authority control");
        if !authority_control.is_empty() {
            text += &format!("\n{{{{Authority control|{authority_control}}}}}\n");
        }
        text += &format!("\n{{{{DEFAULTSORT:{}}}}}\n", format!("{last_name}, {}", self.get("First name(s)")).trim());
        let year_of_birth = self.get("Year of birth");
        if !year_of_birth.is_empty() {
            text += &format!("[[Category:{year_of_birth} births]]\n");
        }
        if died {
            text += &format!("[[Category:{} deaths]]\n", self.get("Year of death"));
        } else if self.get("Day of death").is_empty() {
            text += "[[Category:Living people]]\n";
        }
        let place_of_birth = self.get("Place of birth");
        if !place_of_birth.is_empty() {
            text += &format!("[[Category:People from {}]]\n", ucfirst(&place_of_birth));
        }
        if !occupation.is_empty() {
            text += &format!("[[Category:{occupation}s]]\n");
        }
        for category in self.lines("categories") {
            text += &format!("[[Category:{}]]\n", ucfirst(&category));
        }
        let text = text.replace("  ", " ");

        match self.has("create_infobox") {
            true => self.infobox(&occupation, full_name.trim(), &birth, &death) + &text,
            false => text,
        }
    }

    fn infobox(&self, occupation: &str, name: &str, birth: &str, death: &str) -> String {
        let (kind, keys) = match occupation.to_lowercase().as_str() {
            "writer" => ("writer", WRITER_INFOBOX),
            _ => ("person", PERSON_INFOBOX),
        };
        let mut values: Vec<(&str, String)> = keys.iter().map(|k| (*k, String::new())).collect();
        let mut set = |key: &'static str, value: String| match values.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => values.push((key, value)),
        };
        set("name", name.to_string());
        set("birth_date", birth.to_string());
        set("birth_place", self.link("Place of birth"));
        set("death_date", death.to_string());
        set("death_place", self.link("Place of death"));
        set("nationality", self.link("Nationality"));
        let image = self.get("Image");
        if !image.is_empty() {
            set("image", image);
        }
        let width = keys.iter().map(|k| k.len()).max().unwrap_or_default();
        let brief = self.has("brief_infobox");
        let mut infobox = format!("{{{{Infobox {kind}\n");
        for (key, value) in &values {
            let value = value.trim();
            if value.is_empty() && brief {
                continue;
            }
            let value = if value.is_empty() { String::new() } else { format!(" {value}") };
            infobox += &format!("| {key:<width$} ={value}\n");
        }
        infobox + "}}\n"
    }
}

fn ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// `Sept. 5` → `September 5`, `jan 3` → `January 3`.
fn expand_month(day: &str) -> String {
    let day = ucfirst(day.trim_start());
    for (abbreviation, month) in MONTH_ABBREVIATIONS {
        if day.contains(abbreviation) {
            return day.replacen(abbreviation, month, 1);
        }
        let spaced = abbreviation.replace('.', " ");
        if day.contains(&spaced) {
            return day.replacen(&spaced, &format!("{month} "), 1);
        }
    }
    day
}

/// Form values from a Wikidata item, by parameter name.
async fn from_wikidata(http: &reqwest::Client, q: &str) -> Result<HashMap<String, String>, AppError> {
    let wikidata = Wiki::wikidata();
    let params = [("action", "wbgetentities"), ("ids", q), ("props", "labels|aliases|descriptions|claims")];
    let json = mediawiki::api(http, &wikidata, &params).await?;
    let item = &json["entities"][q];
    if item.get("missing").is_some() || item.is_null() {
        return Err(AppError::BadRequest(format!("There is no Wikidata item {q}.")));
    }
    let linked: BTreeSet<String> = ["P106", "P27", "P19", "P20"].iter().flat_map(|p| item_values(item, p)).collect();
    let labels = match linked.is_empty() {
        true => Value::Null,
        false => {
            let ids = linked.iter().take(50).cloned().collect::<Vec<_>>().join("|");
            let params =
                [("action", "wbgetentities"), ("ids", &ids), ("props", "labels"), ("languages", &LANGUAGES.join("|"))];
            mediawiki::api(http, &wikidata, &params).await?["entities"].take()
        }
    };
    Ok(prefill(item, &labels))
}

fn prefill(item: &Value, labels: &Value) -> HashMap<String, String> {
    let mut wd = HashMap::new();
    if let Some((day, year)) = date(item, "P569") {
        wd.insert("yearofbirth".to_string(), year);
        day.map(|d| wd.insert("dayofbirth".to_string(), d));
    }
    if let Some((day, year)) = date(item, "P570") {
        wd.insert("yearofdeath".to_string(), year);
        day.map(|d| wd.insert("dayofdeath".to_string(), d));
    }
    let ids: Vec<String> = AUTHORITY_CONTROL
        .iter()
        .filter_map(|(name, p)| string_values(item, p).first().map(|v| format!("{name}={v}")))
        .collect();
    if !ids.is_empty() {
        wd.insert("authoritycontrol".to_string(), ids.join("|"));
    }
    if let Some(image) = string_values(item, "P18").first() {
        wd.insert("image".to_string(), image.clone());
    }
    let urls = reference_urls(item);
    if !urls.is_empty() {
        wd.insert("externallinks".to_string(), urls.join("\n"));
    }

    let label_of =
        |q: &str| LANGUAGES.iter().find_map(|l| labels[q]["labels"][*l]["value"].as_str().map(str::to_string));
    let labels_of = |p: &str| item_values(item, p).iter().filter_map(|q| label_of(q)).collect::<Vec<_>>();
    for language in LANGUAGES {
        if !wd.contains_key("lastname")
            && let Some((first, last)) =
                item["labels"][language]["value"].as_str().and_then(|l| l.trim().rsplit_once(' '))
        {
            wd.insert("firstname(s)".to_string(), first.trim().to_string());
            wd.insert("lastname".to_string(), last.to_string());
        }
        if !wd.contains_key("alternativename(s)")
            && let Some(aliases) = item["aliases"][language].as_array()
        {
            let aliases: Vec<&str> = aliases.iter().filter_map(|a| a["value"].as_str()).collect();
            wd.insert("alternativename(s)".to_string(), aliases.join("; "));
        }
        if !wd.contains_key("description")
            && let Some(description) = item["descriptions"][language]["value"].as_str()
        {
            wd.insert("description".to_string(), description.to_string());
        }
    }
    let occupations = labels_of("P106");
    let nationality = labels_of("P27").into_iter().next();
    if let Some(occupation) = occupations.first() {
        wd.insert("occupation".to_string(), occupation.clone());
        if !wd.contains_key("description") {
            let from = nationality.as_ref().map_or_else(String::new, |n| format!(" from {n}"));
            wd.insert("description".to_string(), format!("{}{from}", occupations.join(", ")));
        }
    }
    nationality.map(|n| wd.insert("nationality".to_string(), n));
    labels_of("P19").into_iter().next().map(|p| wd.insert("placeofbirth".to_string(), p));
    labels_of("P20").into_iter().next().map(|p| wd.insert("placeofdeath".to_string(), p));
    wd
}

fn main_values<'a>(item: &'a Value, property: &str) -> impl Iterator<Item = &'a Value> {
    item["claims"][property].as_array().into_iter().flatten().map(|c| &c["mainsnak"]["datavalue"]["value"])
}

fn item_values(item: &Value, property: &str) -> Vec<String> {
    main_values(item, property).filter_map(|v| v["id"].as_str().map(str::to_string)).collect()
}

fn string_values(item: &Value, property: &str) -> Vec<String> {
    main_values(item, property).filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// `(Some("March 11"), "1952")` from a Wikidata date; the day only at day precision.
fn date(item: &Value, property: &str) -> Option<(Option<String>, String)> {
    let value = main_values(item, property).next()?;
    let time = value["time"].as_str()?;
    let (sign, rest) = time.split_at_checked(1)?;
    let mut parts = rest.split(['-', 'T']);
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    let year: i64 = format!("{}{year}", if sign == "-" { "-" } else { "" }).parse().ok()?;
    let day = (value["precision"].as_i64() == Some(11))
        .then(|| {
            let month = MONTHS.get(month.parse::<usize>().ok()?.checked_sub(1)?)?;
            Some(format!("{month} {}", day.parse::<u32>().ok()?))
        })
        .flatten();
    Some((day, year.to_string()))
}

/// The URLs (P854) of all references, without repeats.
fn reference_urls(item: &Value) -> Vec<String> {
    let mut urls: Vec<String> = vec![];
    let claims = item["claims"].as_object().into_iter().flat_map(|c| c.values());
    let statements = claims.filter_map(Value::as_array).flatten();
    let references = statements.filter_map(|s| s["references"].as_array()).flatten();
    for reference in references {
        for snak in reference["snaks"]["P854"].as_array().into_iter().flatten() {
            if let Some(url) = snak["datavalue"]["value"].as_str()
                && !urls.iter().any(|u| u == url)
            {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn months() {
        assert_eq!(expand_month("Sept. 5"), "September 5");
        assert_eq!(expand_month(" jan 3"), "January 3");
        assert_eq!(expand_month("May 1"), "May 1");
    }

    #[test]
    fn biography_text() {
        let form = form(&[
            ("lastname", "Adams"),
            ("firstname(s)", "Douglas"),
            ("occupation", "writer"),
            ("nationality", "English"),
            ("description", "humorist"),
            ("dayofbirth", "Mar. 11"),
            ("yearofbirth", "1952"),
            ("placeofbirth", "Cambridge"),
            ("dayofdeath", "May 11"),
            ("yearofdeath", "2001"),
            ("externallinks", "https://example.org\r\n\r\n"),
            ("categories", "english humorists\n"),
            ("stub", "1"),
            ("authoritycontrol", "VIAF=113230702"),
            ("doit", "1"),
        ]);
        assert_eq!(
            Biography(&form).wikitext(),
            "'''Douglas Adams''' (March 11, 1952 &ndash; May 11, 2001) was an [[English]] humorist.\n\n\
             Adams was born in [[Cambridge]].\n\n\
             == External links ==\n* [https://example.org]\n\n\
             {{English-bio-stub}}\n{{Writer-stub}}\n\n\
             {{Authority control|VIAF=113230702}}\n\n\
             {{DEFAULTSORT:Adams, Douglas}}\n\
             [[Category:1952 births]]\n[[Category:2001 deaths]]\n[[Category:People from Cambridge]]\n\
             [[Category:Writers]]\n[[Category:English humorists]]\n"
        );
    }

    #[test]
    fn brief_infobox() {
        let form =
            form(&[("lastname", "Doe"), ("firstname(s)", "Jane"), ("create_infobox", "1"), ("brief_infobox", "1")]);
        let text = Biography(&form).wikitext();
        assert!(text.starts_with("{{Infobox person\n| name        = Jane Doe\n}}\n'''Jane Doe''' (born )"), "{text}");
        assert!(text.contains("[[Category:Living people]]"));
    }

    #[test]
    fn prefill_from_an_item() {
        let item = json!({
            "labels": {"mul": {"value": "Douglas Adams"}, "de": {"value": "Douglas Noël Adams"}},
            "aliases": {"en": [{"value": "Douglas Noel Adams"}]},
            "claims": {
                "P569": [{"mainsnak": {"datavalue": {"value": {"time": "+1952-03-11T00:00:00Z", "precision": 11}}}}],
                "P570": [{"mainsnak": {"datavalue": {"value": {"time": "+2001-00-00T00:00:00Z", "precision": 9}}}}],
                "P214": [{"mainsnak": {"datavalue": {"value": "113230702"}}}],
                "P106": [{"mainsnak": {"datavalue": {"value": {"id": "Q36180"}}},
                          "references": [{"snaks": {"P854": [{"datavalue": {"value": "https://example.org/a"}}]}}]}],
                "P27": [{"mainsnak": {"datavalue": {"value": {"id": "Q145"}}}}],
            }
        });
        let labels = json!({"Q36180": {"labels": {"en": {"value": "writer"}}}, "Q145": {"labels": {"de": {"value": "Vereinigtes Königreich"}}}});
        let wd = prefill(&item, &labels);
        let get = |k: &str| wd.get(k).map(String::as_str);
        assert_eq!(get("lastname"), Some("Adams"));
        assert_eq!(get("firstname(s)"), Some("Douglas"));
        assert_eq!(get("alternativename(s)"), Some("Douglas Noel Adams"));
        assert_eq!(get("dayofbirth"), Some("March 11"));
        assert_eq!(get("yearofbirth"), Some("1952"));
        assert_eq!(get("dayofdeath"), None);
        assert_eq!(get("yearofdeath"), Some("2001"));
        assert_eq!(get("authoritycontrol"), Some("VIAF=113230702"));
        assert_eq!(get("externallinks"), Some("https://example.org/a"));
        assert_eq!(get("occupation"), Some("writer"));
        assert_eq!(get("description"), Some("writer from Vereinigtes Königreich"));
    }
}

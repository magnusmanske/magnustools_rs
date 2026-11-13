//! SPARQL for the Wikidata Terminator: a candidate search seeded by the
//! user's own query, and the presence check for precomputed candidates.

use super::{SearchType, SortField};

/// Languages whose Wikipedia lives under another subdomain.
const WIKIPEDIA_SUBDOMAINS: &[(&str, &str)] = &[
    ("nb", "no"),
    ("lzh", "zh-classical"),
    ("yue", "zh-yue"),
    ("nan", "zh-min-nan"),
    ("rup", "roa-rup"),
    ("sgs", "bat-smg"),
    ("vro", "fiu-vro"),
];

/// Wikipedia site IDs that do not follow the subdomain.
const WIKIPEDIA_SITE_IDS: &[(&str, &str)] = &[("be-tarask", "be_x_oldwiki")];

/// `^[a-z][a-z0-9-]{1,34}$`
pub fn valid_language(language: &str) -> bool {
    let mut chars = language.chars();
    matches!(chars.next(), Some('a'..='z'))
        && (1..=34).contains(&chars.clone().count())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn wikipedia_subdomain(language: &str) -> &str {
    WIKIPEDIA_SUBDOMAINS.iter().find(|(code, _)| *code == language).map_or(language, |(_, subdomain)| subdomain)
}

pub fn wikipedia_site_id(language: &str) -> String {
    if let Some((_, site)) = WIKIPEDIA_SITE_IDS.iter().find(|(code, _)| *code == language) {
        return (*site).to_string();
    }
    format!("{}wiki", wikipedia_subdomain(language).replace('-', "_"))
}

/// The numeric id of a Wikidata item (`Q42` or its entity URI), if any.
pub fn item_number(value: &str) -> Option<u32> {
    let value = value.trim().trim_end_matches('/');
    let last = value.rsplit('/').next().unwrap_or(value);
    let number = last.strip_prefix('Q')?.parse::<u32>().ok()?;
    (number > 0).then_some(number)
}

/// Splits a user query into its PREFIX declarations and its `{ … }` body.
pub fn extract_seed(user_sparql: &str) -> Result<(String, String), &'static str> {
    let Some(open) = user_sparql.find('{') else {
        return Err("SPARQL restriction must contain a { … } graph pattern");
    };
    let Some(close) = user_sparql.rfind('}') else {
        return Err("SPARQL restriction must contain a { … } graph pattern");
    };
    if close <= open {
        return Err("SPARQL restriction must contain a { … } graph pattern");
    }
    let mut prefixes = user_sparql[..open].trim().to_string();
    if let Some(select) = find_keyword(&prefixes, "select") {
        prefixes.truncate(select);
        prefixes = prefixes.trim().to_string();
    }
    let body = user_sparql[open + 1..close].trim().to_string();
    if !body.contains("?q") {
        return Err("SPARQL restriction must select matching items into ?q");
    }
    Ok((prefixes, body))
}

fn find_keyword(text: &str, keyword: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let keyword_bytes = keyword.as_bytes();
    let mut index = 0;
    while let Some(found) = lower[index..].find(keyword) {
        let start = index + found;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let end = start + keyword_bytes.len();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return Some(start);
        }
        index = start + keyword_bytes.len();
    }
    None
}

fn presence(search_type: SearchType, language: &str, tag: &str) -> String {
    match search_type {
        SearchType::Labels => {
            format!("{{ ?q rdfs:label ?_l{tag} . FILTER(LANG(?_l{tag})=\"{language}\") }}")
        }
        SearchType::Descriptions => {
            format!("{{ ?q schema:description ?_d{tag} . FILTER(LANG(?_d{tag})=\"{language}\") }}")
        }
        SearchType::Sites => {
            format!(
                "{{ ?_a{tag} schema:about ?q ; schema:isPartOf <https://{}.wikipedia.org/> }}",
                wikipedia_subdomain(language)
            )
        }
    }
}

fn missing_filter(search_type: SearchType, languages: &[String]) -> String {
    filter_any(search_type, languages, "m", "NOT EXISTS ")
}

fn present_filter(search_type: SearchType, languages: &[String]) -> String {
    filter_any(search_type, languages, "p", "EXISTS ")
}

fn filter_any(search_type: SearchType, languages: &[String], tag: &str, prefix: &str) -> String {
    let clauses: Vec<String> = languages
        .iter()
        .enumerate()
        .map(|(index, language)| format!("{prefix}{}", presence(search_type, language, &format!("{tag}{index}"))))
        .collect();
    if clauses.is_empty() { String::new() } else { format!("FILTER( {} )", clauses.join(" || ")) }
}

fn exclude_p31(p31: &[u32]) -> String {
    if p31.is_empty() {
        return String::new();
    }
    let values: Vec<String> = p31.iter().map(|q| format!("wd:Q{q}")).collect();
    format!("FILTER NOT EXISTS {{ ?q wdt:P31 ?_mp . VALUES ?_mp {{ {} }} }}", values.join(" "))
}

/// Nine parameters, each part of the query, are clearer than a wrapper.
#[allow(clippy::too_many_arguments)]
pub fn build_live_query(
    search_type: SearchType,
    missing_languages: &[String],
    present_languages: &[String],
    user_sparql: &str,
    exclude_p31_ids: Option<&[u32]>,
    min_score: i64,
    sort: SortField,
    limit: usize,
    offset: usize,
) -> Result<String, &'static str> {
    let (prefixes, mut seed) = extract_seed(user_sparql)?;
    if !seed.ends_with(['.', '}']) {
        seed.push_str(" .");
    }
    let mut blocks = vec![
        seed,
        "FILTER( STRSTARTS( STR(?q) , \"http://www.wikidata.org/entity/Q\" ) )".to_string(),
        "?q wikibase:statements ?claims ; wikibase:sitelinks ?sitelinks ; wikibase:identifiers ?identifiers ."
            .to_string(),
        "BIND( (?claims + ?sitelinks - ?identifiers) AS ?score )".to_string(),
        format!("FILTER( ?score >= {min_score} )"),
        "OPTIONAL { ?q wdt:P31 ?p31 }".to_string(),
        missing_filter(search_type, missing_languages),
        present_filter(search_type, present_languages),
    ];
    if let Some(p31) = exclude_p31_ids {
        blocks.push(exclude_p31(p31));
    }
    let body = blocks.into_iter().filter(|block| !block.is_empty()).collect::<Vec<_>>().join("\n  ");
    let head = if prefixes.is_empty() { String::new() } else { format!("{prefixes}\n") };
    Ok(format!(
        "{head}SELECT ?q ?claims ?sitelinks ?identifiers ?score (SAMPLE(?p31) AS ?p31s) WHERE {{\n  {body}\n}}\
         \nGROUP BY ?q ?claims ?sitelinks ?identifiers ?score\
         \nORDER BY DESC(?{}) ?q\
         \nLIMIT {limit} OFFSET {offset}",
        sort.as_str()
    ))
}

pub fn build_presence_query(numeric_ids: &[u32], search_type: SearchType, present_languages: &[String]) -> String {
    let values: Vec<String> = numeric_ids.iter().map(|q| format!("wd:Q{q}")).collect();
    let filter = present_filter(search_type, present_languages);
    format!("SELECT ?q WHERE {{\n  VALUES ?q {{ {} }}\n  {filter}\n}}", values.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_and_items() {
        assert!(valid_language("en"));
        assert!(valid_language("zh-yue"));
        assert!(!valid_language("EN"));
        assert!(!valid_language("e"));
        assert_eq!(wikipedia_subdomain("nb"), "no");
        assert_eq!(wikipedia_site_id("nb"), "nowiki");
        assert_eq!(wikipedia_site_id("be-tarask"), "be_x_oldwiki");
        assert_eq!(item_number("http://www.wikidata.org/entity/Q42"), Some(42));
        assert_eq!(item_number("Q0"), None);
        assert_eq!(item_number("L1"), None);
    }

    #[test]
    fn seeds_keep_prefixes_and_require_q() {
        let (prefixes, body) = extract_seed("PREFIX x: <http://x/> SELECT ?q WHERE { ?q wdt:P31 wd:Q1 }").unwrap();
        assert_eq!(prefixes, "PREFIX x: <http://x/>");
        assert_eq!(body, "?q wdt:P31 wd:Q1");
        assert!(extract_seed("SELECT ?x { ?x wdt:P31 wd:Q1 }").is_err());
        assert!(extract_seed("no braces").is_err());
    }

    #[test]
    fn live_query_has_the_expected_shape() {
        let query = build_live_query(
            SearchType::Labels,
            &["cy".to_string()],
            &[],
            "SELECT ?q { ?q wdt:P31 wd:Q4915012 }",
            Some(&[5]),
            15,
            SortField::Score,
            50,
            0,
        )
        .unwrap();
        assert!(query.contains("rdfs:label"));
        assert!(query.contains("wd:Q5"));
        assert!(query.contains("LIMIT 50 OFFSET 0"));
        assert!(!query.contains("NOT EXISTS {  }"));
    }

    #[test]
    fn presence_query_lists_the_items() {
        let query = build_presence_query(&[1, 2, 3], SearchType::Descriptions, &["en".to_string()]);
        assert!(query.contains("wd:Q1 wd:Q2 wd:Q3"));
        assert!(query.contains("schema:description"));
    }
}

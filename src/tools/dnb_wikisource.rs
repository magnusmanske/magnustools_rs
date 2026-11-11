//! `dnb_wikisource.php`: inconsistencies in the Wikisource DNB structure:
//! transclusions, edited Page pages, and author/biography links that do not
//! point both ways.

use super::dnb;
use crate::replicas::display_title;
use crate::state::AppState;
use crate::web::{AppError, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const TITLE: &str = "DNB on Wikisource";
const CATEGORY: &str = "DNB_biographies";
const PAGE_PREFIX: &str = "Dictionary_of_National_Biography_volume_";
/// Entries in one of the long lists; the page stays usable.
const MAX_LIST: usize = 2_000;

#[derive(Template)]
#[template(path = "tools/dnb_wikisource.html")]
struct Page {
    title: &'static str,
    total: usize,
    dnb_coded: usize,
    other: Vec<Link>,
    other_truncated: bool,
    transclusions: usize,
    edited_pages: usize,
    not_transcluded: Vec<Link>,
    not_transcluded_truncated: bool,
    author_to_bios: Vec<Group>,
    bio_to_authors: Vec<Group>,
}

struct Link {
    title: String,
    url: String,
}

struct Group {
    title: String,
    url: String,
    links: Vec<Link>,
}

struct Results {
    total: usize,
    dnb_coded: usize,
    other: Vec<Link>,
    other_truncated: bool,
    transclusions: usize,
    edited_pages: usize,
    not_transcluded: Vec<Link>,
    not_transcluded_truncated: bool,
    author_to_bios: Vec<Group>,
    bio_to_authors: Vec<Group>,
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let wiki = Wiki::new("en", "wikisource")?;
    let results = results(&state, &wiki).await?;
    render(&Page {
        title: TITLE,
        total: results.total,
        dnb_coded: results.dnb_coded,
        other: results.other,
        other_truncated: results.other_truncated,
        transclusions: results.transclusions,
        edited_pages: results.edited_pages,
        not_transcluded: results.not_transcluded,
        not_transcluded_truncated: results.not_transcluded_truncated,
        author_to_bios: results.author_to_bios,
        bio_to_authors: results.bio_to_authors,
    })
}

async fn results(state: &AppState, wiki: &Wiki) -> Result<Results, AppError> {
    let mut conn =
        state.replicas.conn(wiki, &["page", "categorylinks", "linktarget", "templatelinks", "pagelinks"]).await?;
    let pages: Vec<(u64, String)> = conn
        .exec(
            "SELECT p.page_id, p.page_title FROM page p
             JOIN categorylinks cl ON cl_from=p.page_id JOIN linktarget lt ON cl_target_id=lt_id
             WHERE p.page_namespace=0 AND lt.lt_namespace=14 AND lt.lt_title=?",
            (CATEGORY,),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let total = pages.len();
    let bio_ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
    let bio_titles: Vec<String> = pages.iter().map(|(_, title)| title.clone()).collect();
    let coded_templates: Vec<(u64, String)> = dnb::select_small(
        &mut conn,
        "SELECT tl_from, lt.lt_title FROM templatelinks tl JOIN linktarget lt ON lt.lt_id=tl.tl_target_id
         WHERE lt.lt_namespace=10 AND lt.lt_title REGEXP '^DNB[01][0-9]$' AND tl.tl_from IN",
        &bio_ids,
    )
    .await?;
    let coded_ids: HashSet<u64> =
        coded_templates.into_iter().filter(|(_, template)| is_dnb_template(template)).map(|(id, _)| id).collect();

    let mut dnb_coded = 0;
    let mut other = Vec::new();
    for (id, title) in bio_ids.iter().zip(bio_titles.iter()) {
        if coded_ids.contains(id) || is_dnb_coded(title) {
            dnb_coded += 1;
        } else {
            other.push(link(wiki, title, ""));
        }
    }
    other.sort_by(|a, b| a.title.cmp(&b.title));
    let other_truncated = other.len() > MAX_LIST;
    other.truncate(MAX_LIST);

    let transclusions: Vec<String> = conn
        .exec(
            "SELECT DISTINCT lt.lt_title FROM page p
             JOIN categorylinks cl ON cl_from=p.page_id JOIN linktarget clt ON cl.cl_target_id=clt.lt_id
             JOIN templatelinks tl ON tl_from=p.page_id JOIN linktarget lt ON lt.lt_id=tl.tl_target_id
             WHERE p.page_namespace=0 AND clt.lt_namespace=14 AND clt.lt_title=?
               AND lt.lt_namespace=104 AND lt.lt_title LIKE ?",
            (CATEGORY, format!("{PAGE_PREFIX}%")),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let edited_pages: Vec<String> = conn
        .exec(
            "SELECT DISTINCT p.page_title FROM page p
             JOIN categorylinks cl ON cl_from=p.page_id JOIN linktarget lt ON cl_target_id=lt_id
             WHERE p.page_namespace=104 AND lt.lt_namespace=14 AND lt.lt_title IN ('Proofread','Validated')
               AND p.page_title LIKE ?",
            (format!("{PAGE_PREFIX}%"),),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let transclusions_set: HashSet<&str> = transclusions.iter().map(String::as_str).collect();
    let mut not_transcluded: Vec<Link> = edited_pages
        .iter()
        .filter(|page| !transclusions_set.contains(page.as_str()))
        .map(|page| link(wiki, page, "Page"))
        .collect();
    not_transcluded.sort_by(|a, b| a.title.cmp(&b.title));
    let not_transcluded_truncated = not_transcluded.len() > MAX_LIST;
    not_transcluded.truncate(MAX_LIST);

    let mut bio_to_authors: HashMap<String, HashSet<String>> = HashMap::new();
    let links: Vec<(u64, String)> = dnb::select_small(
        &mut conn,
        "SELECT pl_from, lt.lt_title FROM pagelinks pl JOIN linktarget lt ON lt.lt_id=pl.pl_target_id
         WHERE lt.lt_namespace=102 AND pl.pl_from IN",
        &bio_ids,
    )
    .await?;
    let titles: HashMap<u64, &str> =
        bio_ids.iter().zip(bio_titles.iter()).map(|(id, title)| (*id, title.as_str())).collect();
    for (id, author) in links {
        if let Some(bio) = titles.get(&id) {
            bio_to_authors.entry(bio.to_string()).or_default().insert(author);
        }
    }

    let author_links: Vec<(String, String)> = conn
        .exec(
            "SELECT a.page_title, b.page_title FROM page a
             JOIN pagelinks pl ON pl.pl_from=a.page_id
             JOIN linktarget lt ON lt.lt_id=pl.pl_target_id
             JOIN page b ON b.page_title=lt.lt_title AND b.page_namespace=lt.lt_namespace
             JOIN categorylinks cl ON cl.cl_from=b.page_id
             JOIN linktarget clt ON clt.lt_id=cl.cl_target_id
             WHERE a.page_namespace=102 AND lt.lt_namespace=0
               AND clt.lt_namespace=14 AND clt.lt_title=?",
            (CATEGORY,),
        )
        .await
        .map_err(anyhow::Error::from)?;
    let mut author_to_bios: HashMap<String, HashSet<String>> = HashMap::new();
    for (author, bio) in author_links {
        author_to_bios.entry(author).or_default().insert(bio);
    }

    // A link in both directions is consistent; only the one-way ones remain.
    let mutual: Vec<(String, String)> = bio_to_authors
        .iter()
        .flat_map(|(bio, authors)| authors.iter().map(move |author| (bio.clone(), author.clone())))
        .filter(|(bio, author)| author_to_bios.get(author).is_some_and(|bios| bios.contains(bio)))
        .collect();
    for (bio, author) in mutual {
        if let Some(authors) = bio_to_authors.get_mut(&bio) {
            authors.remove(&author);
        }
        if let Some(bios) = author_to_bios.get_mut(&author) {
            bios.remove(&bio);
        }
    }
    bio_to_authors.retain(|_, authors| !authors.is_empty());
    author_to_bios.retain(|_, bios| !bios.is_empty());

    Ok(Results {
        total,
        dnb_coded,
        other,
        other_truncated,
        transclusions: transclusions.len(),
        edited_pages: edited_pages.len(),
        not_transcluded,
        not_transcluded_truncated,
        author_to_bios: groups(author_to_bios, wiki, "Author", ""),
        bio_to_authors: groups(bio_to_authors, wiki, "", "Author"),
    })
}

fn groups(map: HashMap<String, HashSet<String>>, wiki: &Wiki, subject_prefix: &str, link_prefix: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = map
        .into_iter()
        .map(|(subject, links)| {
            let mut links: Vec<Link> = links.iter().map(|link_title| link(wiki, link_title, link_prefix)).collect();
            links.sort_by(|a, b| a.title.cmp(&b.title));
            let subject = link(wiki, &subject, subject_prefix);
            Group { title: subject.title, url: subject.url, links }
        })
        .collect();
    groups.sort_by(|a, b| a.title.cmp(&b.title));
    groups
}

fn link(wiki: &Wiki, title: &str, prefix: &str) -> Link {
    let display = display_title(title);
    let full = if prefix.is_empty() { display.clone() } else { format!("{prefix}:{display}") };
    Link { title: display, url: wiki.page_url(&full) }
}

/// The old DNB00/DNB01/DNB12 titles, whose code starts with DNB0 or DNB1.
fn is_dnb_coded(title: &str) -> bool {
    title
        .strip_suffix(')')
        .and_then(|title| title.rsplit_once("_(DNB"))
        .is_some_and(|(_, code)| code.starts_with('0') || code.starts_with('1'))
}

/// A `DNB00`, `DNB01`, … template; those identify the older DNB editions.
fn is_dnb_template(title: &str) -> bool {
    title.strip_prefix("DNB").is_some_and(|code| {
        code.len() == 2 && code.chars().all(|c| c.is_ascii_digit()) && (code.starts_with('0') || code.starts_with('1'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnb_codes() {
        assert!(is_dnb_coded("Abbot,_George_(DNB00)"));
        assert!(is_dnb_coded("Smith,_John_(DNB12)"));
        assert!(!is_dnb_coded("Smith,_John_(DNB20)"));
        assert!(!is_dnb_coded("Smith,_John"));
    }
}

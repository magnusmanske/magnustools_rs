//! The wiki replica databases, and the queries several tools share.
//!
//! Some wikis keep tables on a separate server (Commons its link tables), so
//! every connection is for a set of tables, and a query must not mix tables
//! from both sides: such joins happen here, in code.

use crate::wiki::Wiki;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use wikimisc::mysql_async::prelude::{FromRow, Queryable};
use wikimisc::mysql_async::{Conn, Params, Value};
use wikimisc::toolforge::db::Cluster;
use wikimisc::toolforge::pool::WikiPool;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STATEMENT_TIME: Duration = Duration::from_secs(600);
const POOL_MAX: usize = 8;
/// Values per `IN (…)` list.
const CHUNK: usize = 5000;
pub const CATEGORY_TABLES: &[&str] = &["page", "categorylinks", "linktarget"];
const NS_CATEGORY: i64 = 14;
/// Category trees can be enormous; stop growing one at this size.
const MAX_CATEGORIES: usize = 50_000;

#[derive(Debug)]
pub struct Replicas {
    pool: WikiPool,
}

impl Replicas {
    pub fn new(credentials: &Path) -> Result<Self> {
        let pool = WikiPool::builder(Cluster::Analytics)
            .my_cnf_path(credentials)
            .pool_max(POOL_MAX)
            .connect_timeout(CONNECT_TIMEOUT)
            .max_statement_time(MAX_STATEMENT_TIME)
            .build()
            .with_context(|| format!("cannot read the database credentials from {}", credentials.display()))?;
        Ok(Self { pool })
    }

    /// A connection to the database of `wiki` that has `tables`.
    pub async fn conn(&self, wiki: &Wiki, tables: &[&str]) -> Result<Conn> {
        self.pool
            .connect_for_tables(wiki.dbname(), tables)
            .await
            .with_context(|| format!("cannot connect to the {} replica", wiki.dbname()))
    }

    /// `category` and its subcategories down to `depth` (0: just the
    /// category), as database titles.
    pub async fn category_tree(&self, wiki: &Wiki, category: &str, depth: u32) -> Result<Vec<String>> {
        let mut conn = self.conn(wiki, CATEGORY_TABLES).await?;
        let root = db_title(category.trim().trim_start_matches("Category:"));
        let mut tree = vec![root.clone()];
        let mut seen = HashSet::from([root.clone()]);
        let mut level = vec![root];
        for _ in 0..depth {
            let sql = "SELECT DISTINCT page_title FROM page
                JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
                WHERE lt_namespace=14 AND cl_type='subcat' AND lt_title IN";
            let subcategories: Vec<String> = select_in(&mut conn, sql, "", &[], &level).await?;
            level = subcategories.into_iter().filter(|c| seen.insert(c.clone())).collect();
            tree.extend(level.iter().cloned());
            if level.is_empty() || tree.len() >= MAX_CATEGORIES {
                break;
            }
        }
        Ok(tree)
    }

    /// Pages of `namespace` in a category tree, as database titles.
    pub async fn pages_in_tree(&self, wiki: &Wiki, category: &str, depth: u32, namespace: i64) -> Result<Vec<String>> {
        let categories = self.category_tree(wiki, category, depth).await?;
        if namespace == NS_CATEGORY {
            return Ok(categories);
        }
        let mut conn = self.conn(wiki, CATEGORY_TABLES).await?;
        let sql = "SELECT DISTINCT page_title FROM page
            JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
            WHERE page_namespace=? AND lt_namespace=14 AND lt_title IN";
        let pages: Vec<String> = select_in(&mut conn, sql, "", &[namespace.into()], &categories).await?;
        let mut seen = HashSet::new();
        Ok(pages.into_iter().filter(|p| seen.insert(p.clone())).collect())
    }

    /// A random non-redirect page of `namespace` in a category tree (of a
    /// huge tree, only in its first few thousand categories).
    pub async fn random_page_in_tree(
        &self,
        wiki: &Wiki,
        category: &str,
        depth: u32,
        namespace: i64,
    ) -> Result<Option<String>> {
        let categories = self.category_tree(wiki, category, depth).await?;
        let mut conn = self.conn(wiki, CATEGORY_TABLES).await?;
        let sql = format!(
            "SELECT page_title FROM page
            JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
            WHERE page_namespace=? AND page_is_redirect=0 AND lt_namespace=14 AND lt_title IN ({})
            ORDER BY RAND() LIMIT 1",
            placeholders(categories.len().min(CHUNK))
        );
        let mut params = vec![Value::from(namespace)];
        params.extend(categories.into_iter().take(CHUNK).map(Value::from));
        let title: Option<Vec<u8>> = conn.exec_first(sql, Params::Positional(params)).await?;
        Ok(title.map(|t| String::from_utf8_lossy(&t).into_owned()))
    }
}

/// `sql` ends in `… IN`; runs it for `values` in chunks, with `params`
/// before and `suffix` (e.g. `GROUP BY …`) after the list, and collects all rows.
pub async fn select_in<T: FromRow + Send + 'static, V: Into<Value> + Clone>(
    conn: &mut Conn,
    sql: &str,
    suffix: &str,
    params: &[Value],
    values: &[V],
) -> Result<Vec<T>> {
    let mut rows = vec![];
    for chunk in values.chunks(CHUNK) {
        let statement = format!("{sql} ({}) {suffix}", placeholders(chunk.len()));
        let mut all = params.to_vec();
        all.extend(chunk.iter().cloned().map(Into::into));
        rows.extend(conn.exec::<T, _, _>(statement, Params::Positional(all)).await?);
    }
    Ok(rows)
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// A title as the database has it: underscores for spaces, first letter upper case.
pub fn db_title(title: &str) -> String {
    let title = title.trim().replace(' ', "_");
    let mut chars = title.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// A database title for display.
pub fn display_title(title: &str) -> String {
    title.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(db_title(" belgrade Aviation museum "), "Belgrade_Aviation_museum");
        assert_eq!(display_title("A_b"), "A b");
        assert_eq!(placeholders(3), "?,?,?");
        assert_eq!(placeholders(0), "");
    }
}

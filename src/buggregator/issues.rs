//! Issues and tools, as the buggregator front-end lists and edits them.
//! Rows go out with every value as a string, as PHP's mysqli gave them.

use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params, Row, Value as Sql};

pub const STATUSES: &[&str] = &["OPEN", "CLOSED"];
/// BITBUCKET is kept for browsing legacy data, though imports are retired.
pub const SITES: &[&str] = &["WIKI", "WIKIDATA", "GITHUB", "CODEBERG", "BITBUCKET"];
pub const PRIORITIES: &[&str] = &["HIGH", "NORMAL", "LOW"];
pub const SORT_BY: &[&str] = &["label", "status", "date_created", "date_last", "site", "tool", "priority"];
pub const MAX_LIMIT: u64 = 500;

const ISSUE_COLUMNS: &str =
    "id, label, status, date_created, date_last, site, url, description_text_id, tool, priority,
    (SELECT text FROM text WHERE text.id=issue.description_text_id) AS description";

/// A filtered, sorted page of issues.
#[derive(Debug, Default, PartialEq)]
pub struct IssueQuery {
    pub tool: Option<u64>,
    pub statuses: Vec<&'static str>,
    pub sites: Vec<&'static str>,
    pub priorities: Vec<&'static str>,
    pub sort_by: &'static str,
    pub descending: bool,
    pub limit: u64,
    pub offset: u64,
}

impl IssueQuery {
    /// From the request; anything not on the lists is ignored.
    pub fn new(get: impl Fn(&str) -> Option<String>) -> Self {
        let pick = |name: &str, allowed: &'static [&'static str]| -> Vec<&'static str> {
            let wanted = get(name).unwrap_or_default();
            allowed.iter().copied().filter(|a| wanted.split(',').any(|w| w.trim() == *a)).collect()
        };
        let number = |name: &str| get(name).and_then(|v| v.trim().parse::<u64>().ok());
        Self {
            tool: number("tool"),
            statuses: pick("status", STATUSES),
            sites: pick("site", SITES),
            priorities: pick("priority", PRIORITIES),
            sort_by: get("sort_by").and_then(|s| SORT_BY.iter().copied().find(|c| *c == s)).unwrap_or("date_last"),
            descending: get("sort_order").is_none_or(|o| o == "descending"),
            limit: number("limit").filter(|l| *l >= 1).unwrap_or(25).min(MAX_LIMIT),
            offset: number("offset").unwrap_or(0),
        }
    }

    /// `WHERE …` and its parameters.
    fn condition(&self) -> (String, Vec<Sql>) {
        let mut parts = vec![];
        let mut params: Vec<Sql> = vec![];
        if let Some(tool) = self.tool {
            parts.push("tool=?".to_string());
            params.push(tool.into());
        }
        for (column, values) in [("status", &self.statuses), ("site", &self.sites), ("priority", &self.priorities)] {
            if !values.is_empty() {
                parts.push(format!("{column} IN ({})", vec!["?"; values.len()].join(",")));
                params.extend(values.iter().map(|v| Sql::from(*v)));
            }
        }
        let condition = if parts.is_empty() { String::new() } else { format!("WHERE {}", parts.join(" AND ")) };
        (condition, params)
    }
}

pub async fn get_issues(conn: &mut Conn, query: &IssueQuery) -> Result<Value> {
    let (condition, params) = query.condition();
    let direction = if query.descending { "DESC" } else { "ASC" };
    // sort_by is one of SORT_BY, so safe as an identifier.
    let sql = format!(
        "SELECT {ISSUE_COLUMNS} FROM issue {condition} ORDER BY `{}` {direction} LIMIT {} OFFSET {}",
        query.sort_by, query.limit, query.offset
    );
    let rows: Vec<Row> = conn.exec(sql, Params::Positional(params.clone())).await?;
    let this_query: Option<u64> =
        conn.exec_first(format!("SELECT COUNT(*) FROM issue {condition}"), Params::Positional(params)).await?;
    let total_open: Option<u64> = conn.query_first("SELECT COUNT(*) FROM issue WHERE status='OPEN'").await?;
    Ok(json!({
        "results": rows.iter().map(row_as_strings).collect::<Vec<_>>(),
        "stats": {
            "this_query": this_query.unwrap_or_default().to_string(),
            "total_open": total_open.unwrap_or_default().to_string(),
        },
    }))
}

/// Tools by id, each with a `nice_name`.
pub async fn tools_by_id(conn: &mut Conn) -> Result<Map<String, Value>> {
    let rows: Vec<Row> = conn.query("SELECT * FROM tool ORDER BY name, subdomain").await?;
    let mut tools = Map::new();
    for row in &rows {
        let mut tool = row_as_strings(row);
        let text = |key: &str| tool.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let (id, name, subdomain) = (text("id"), text("name"), text("subdomain"));
        let nice_name = match subdomain.is_empty() || subdomain.eq_ignore_ascii_case(&name) {
            true => name,
            false => format!("{subdomain}/{name}"),
        };
        tool.insert("nice_name".to_string(), nice_name.into());
        tools.insert(id, Value::Object(tool));
    }
    Ok(tools)
}

/// Tools with their open and closed tickets and key/values. (The PHP version
/// read a view that broke when columns were dropped from `tool`.)
pub async fn get_tools(conn: &mut Conn) -> Result<Value> {
    let kv: Vec<(u64, String, String)> = conn.query("SELECT tool_id, `key`, value FROM tool_kv").await?;
    let mut key_values: BTreeMap<u64, Map<String, Value>> = BTreeMap::new();
    for (tool_id, key, value) in kv {
        key_values.entry(tool_id).or_default().insert(key, value.into());
    }
    let rows: Vec<Row> = conn
        .query(
            "SELECT id, name, subdomain, webserver_type, own_oauth, using_logging, note, url, toolhub,
             (SELECT COUNT(*) FROM issue WHERE issue.tool=tool.id AND issue.status='OPEN') AS open_tickets,
             (SELECT COUNT(*) FROM issue WHERE issue.tool=tool.id AND issue.status='CLOSED') AS closed_tickets
             FROM tool",
        )
        .await?;
    let mut tools = vec![];
    for row in &rows {
        let mut tool = row_as_strings(row);
        let id: Option<u64> = row.get_opt(0).and_then(Result::ok);
        if let Some(kv) = id.and_then(|id| key_values.remove(&id)) {
            tool.insert("key_values".to_string(), Value::Object(kv));
        }
        tools.push(Value::Object(tool));
    }
    Ok(Value::Array(tools))
}

pub async fn set_status(conn: &mut Conn, issue_id: u64, status: &str, user: &str) -> Result<()> {
    log(conn, issue_id, "STATUS", status, user).await?;
    conn.exec_drop("UPDATE issue SET status=? WHERE id=?", (status, issue_id)).await?;
    Ok(())
}

pub async fn set_tool(conn: &mut Conn, issue_id: u64, tool_id: u64, user: &str) -> Result<()> {
    log(conn, issue_id, "TOOL_ID", &tool_id.to_string(), user).await?;
    conn.exec_drop("UPDATE issue SET tool=? WHERE id=?", (tool_id, issue_id)).await?;
    Ok(())
}

/// Records a change, and marks the issue as touched.
async fn log(conn: &mut Conn, issue_id: u64, event: &str, text: &str, user: &str) -> Result<()> {
    let text_id = text_id(conn, text).await?;
    let user_id = user_id(conn, user).await?;
    conn.exec_drop(
        "INSERT INTO log (issue_id, event, text_id, user_id) VALUES (?, ?, ?, ?)",
        (issue_id, event, text_id, user_id),
    )
    .await?;
    conn.exec_drop("UPDATE issue SET date_last=CURRENT_TIMESTAMP WHERE id=?", (issue_id,)).await?;
    Ok(())
}

/// Texts are stored once.
pub async fn text_id(conn: &mut Conn, text: &str) -> Result<u64> {
    if let Some(id) = conn.exec_first("SELECT id FROM text WHERE text=? LIMIT 1", (text,)).await? {
        return Ok(id);
    }
    conn.exec_drop("INSERT INTO text (text) VALUES (?)", (text,)).await?;
    conn.last_insert_id().ok_or_else(|| anyhow::anyhow!("no text id"))
}

/// A wiki user; created if new (the PHP version failed then).
async fn user_id(conn: &mut Conn, name: &str) -> Result<u64> {
    if let Some(id) = conn.exec_first("SELECT id FROM user WHERE name=? AND site='WIKI' LIMIT 1", (name,)).await? {
        return Ok(id);
    }
    conn.exec_drop("INSERT INTO user (name, site) VALUES (?, 'WIKI')", (name,)).await?;
    conn.last_insert_id().ok_or_else(|| anyhow::anyhow!("no user id"))
}

/// A row as column => string (or null).
pub fn row_as_strings(row: &Row) -> Map<String, Value> {
    row.columns_ref()
        .iter()
        .enumerate()
        .map(|(i, column)| (column.name_str().into_owned(), row.as_ref(i).map_or(Value::Null, sql_string)))
        .collect()
}

fn sql_string(value: &Sql) -> Value {
    match value {
        Sql::NULL => Value::Null,
        Sql::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned().into(),
        Sql::Int(n) => n.to_string().into(),
        Sql::UInt(n) => n.to_string().into(),
        Sql::Float(n) => n.to_string().into(),
        Sql::Double(n) => n.to_string().into(),
        Sql::Date(y, m, d, h, mi, s, _) => format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}").into(),
        Sql::Time(negative, days, h, m, s, _) => {
            let hours = u32::from(*h) + days * 24;
            format!("{}{hours:02}:{m:02}:{s:02}", if *negative { "-" } else { "" }).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn query(pairs: &[(&str, &str)]) -> IssueQuery {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        IssueQuery::new(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults() {
        let q = query(&[]);
        assert_eq!((q.sort_by, q.descending, q.limit, q.offset), ("date_last", true, 25, 0));
        assert_eq!(q.condition(), (String::new(), vec![]));
    }

    #[test]
    fn filters_are_allowlisted() {
        let q = query(&[
            ("status", "OPEN,'; DROP TABLE issue"),
            ("site", "GITHUB,WIKI"),
            ("sort_by", "id; DELETE"),
            ("sort_order", "ascending"),
            ("limit", "9999"),
            ("tool", "12"),
        ]);
        assert_eq!(q.statuses, vec!["OPEN"]);
        assert_eq!(q.sites, vec!["WIKI", "GITHUB"]);
        assert_eq!((q.sort_by, q.descending, q.limit), ("date_last", false, 500));
        let (condition, params) = q.condition();
        assert_eq!(condition, "WHERE tool=? AND status IN (?) AND site IN (?,?)");
        assert_eq!(params.len(), 4);
    }

    #[test]
    fn values_as_strings() {
        assert_eq!(sql_string(&Sql::Int(-3)), json!("-3"));
        assert_eq!(sql_string(&Sql::Bytes(b"x".to_vec())), json!("x"));
        assert_eq!(sql_string(&Sql::Date(2024, 1, 2, 3, 4, 5, 0)), json!("2024-01-02 03:04:05"));
        assert_eq!(sql_string(&Sql::NULL), Value::Null);
    }
}

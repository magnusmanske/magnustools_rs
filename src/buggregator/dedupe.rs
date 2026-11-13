//! One-off cleanup of duplicate rows that accumulated while the importers
//! could not recognise an existing issue. A dry run reports what would go;
//! `--apply` collapses `issue`, `user`, `text`, `url_issue` and `user_issue`
//! and adds the natural keys that prevent a recurrence.
//!
//! Run it while the web service and the update jobs are stopped: the cleanup
//! and the unique keys are not safe against concurrent imports.

use anyhow::{Result, bail};
use std::fmt;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// The sites whose issues carry a `git_issue` mapping.
const FORGE_SITES: &str = "'GITHUB','BITBUCKET','GITLAB','CODEBERG','GITLAB_WMF'";

/// Rows that a cleanup would remove, and other loose ends worth reporting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub duplicate_issues: u64,
    pub duplicate_users: u64,
    pub duplicate_urls: u64,
    pub duplicate_user_issues: u64,
    pub duplicate_texts: u64,
    pub empty_site_users: u64,
    pub forge_issues_without_mapping: u64,
}

impl Counts {
    pub fn is_clean(&self) -> bool {
        self.duplicate_issues == 0
            && self.duplicate_users == 0
            && self.duplicate_urls == 0
            && self.duplicate_user_issues == 0
            && self.duplicate_texts == 0
    }
}

impl fmt::Display for Counts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "  duplicate issues:       {}", self.duplicate_issues)?;
        writeln!(f, "  duplicate users:        {}", self.duplicate_users)?;
        writeln!(f, "  duplicate issue URLs:   {}", self.duplicate_urls)?;
        writeln!(f, "  duplicate user links:   {}", self.duplicate_user_issues)?;
        writeln!(f, "  duplicate texts:        {}", self.duplicate_texts)?;
        writeln!(f, "  users with no site:     {}", self.empty_site_users)?;
        write!(f, "  forge issues unmapped:  {}", self.forge_issues_without_mapping)
    }
}

/// Reports what would be removed; with `apply`, removes it and installs the
/// unique keys.
pub async fn run(conn: &mut Conn, apply: bool) -> Result<()> {
    let before = counts(conn).await?;
    println!("before:\n{before}");
    if !apply {
        println!("dry run; pass --apply to clean up");
        return Ok(());
    }
    prepare(conn).await?;
    let mut tx = conn.start_transaction(Default::default()).await?;
    dedupe_issues(&mut tx).await?;
    dedupe_users(&mut tx).await?;
    dedupe_texts(&mut tx).await?;
    dedupe_links(&mut tx).await?;
    drop_temporary_tables(&mut tx).await?;
    tx.commit().await?;
    let after = counts(conn).await?;
    if !after.is_clean() {
        bail!("duplicates remain; do not add the unique keys until they are gone");
    }
    finish(conn).await?;
    println!("after:\n{after}");
    Ok(())
}

async fn counts(conn: &mut impl Queryable) -> Result<Counts> {
    Ok(Counts {
        duplicate_issues: duplicates(conn, "`issue`", "`site`, `url`").await?,
        duplicate_users: duplicates(conn, "`user`", "`name`, `site`").await?,
        duplicate_urls: duplicates(conn, "`url_issue`", "`issue_id`, `url`").await?,
        duplicate_user_issues: duplicates(conn, "`user_issue`", "`user_id`, `issue_id`").await?,
        // The hash column may not exist before `prepare`; compute it on the fly.
        duplicate_texts: duplicates(conn, "`text`", "LOWER(SHA2(`text`,256))").await?,
        empty_site_users: count(conn, "SELECT COUNT(*) FROM `user` WHERE `site`=''").await?,
        forge_issues_without_mapping: count(
            conn,
            &format!(
                "SELECT COUNT(*) FROM `issue` i LEFT JOIN `git_issue` g ON g.`issue_id`=i.`id`
                 WHERE i.`site` IN ({FORGE_SITES}) AND g.`id` IS NULL"
            ),
        )
        .await?,
    })
}

/// Rows beyond the first in each group of `columns`.
async fn duplicates(conn: &mut impl Queryable, table: &str, columns: &str) -> Result<u64> {
    count(
        conn,
        &format!("SELECT COALESCE(SUM(c-1),0) FROM (SELECT COUNT(*) c FROM {table} GROUP BY {columns} HAVING c>1) x"),
    )
    .await
}

async fn count(conn: &mut impl Queryable, sql: &str) -> Result<u64> {
    Ok(conn.query_first::<u64, _>(sql).await?.unwrap_or_default())
}

// ── Schema ────────────────────────────────────────────────────────────────

/// The columns and indexes the cleanup needs. Safe to run twice.
async fn prepare(conn: &mut Conn) -> Result<()> {
    // `git_issue.site`: the forge-specific half of an issue's identity.
    if !has_column(conn, "git_issue", "site").await? {
        conn.query_drop(
            "ALTER TABLE `git_issue` ADD COLUMN `site`
             ENUM('GITHUB','BITBUCKET','GITLAB','CODEBERG','GITLAB_WMF')
             NOT NULL DEFAULT 'GITHUB' AFTER `issue_id`",
        )
        .await?;
    }
    conn.query_drop("UPDATE `git_issue` g JOIN `issue` i ON i.`id`=g.`issue_id` SET g.`site`=i.`site`").await?;
    if index(conn, "git_issue", "site_git_id").await?.is_none() {
        conn.query_drop(
            "ALTER TABLE `git_issue`
               DROP INDEX `git_id`,
               ADD UNIQUE KEY `site_git_id` (`site`,`git_id`,`git_repo_id`)",
        )
        .await?;
    }

    // Text hashes: the lookup key for `text_id` and the identity for the cleanup.
    if !has_column(conn, "text", "hash").await? {
        conn.query_drop(
            "ALTER TABLE `text` ADD COLUMN `hash` char(64) NOT NULL DEFAULT ''
             COMMENT 'SHA-256 of text, so equal texts are stored once' AFTER `id`",
        )
        .await?;
    }
    conn.query_drop("UPDATE `text` SET `hash`=LOWER(SHA2(`text`,256)) WHERE `hash`=''").await?;
    conn.query_drop("ALTER TABLE `text` MODIFY `hash` char(64) NOT NULL").await?;

    for (table, name, ddl) in [
        ("issue", "tool_status", "ALTER TABLE `issue` ADD KEY `tool_status` (`tool`,`status`)"),
        ("issue", "date_last", "ALTER TABLE `issue` ADD KEY `date_last` (`date_last`)"),
        ("logging_uses", "date", "ALTER TABLE `logging_uses` ADD KEY `date` (`date`)"),
        ("dash_instance", "ended_seen", "ALTER TABLE `dash_instance` ADD KEY `ended_seen` (`ended_at`,`last_seen`)"),
    ] {
        if index(conn, table, name).await?.is_none() {
            conn.query_drop(ddl).await?;
        }
    }

    // Forge users are attributed from now on; the enums must know their sites.
    conn.query_drop(
        "ALTER TABLE `issue` MODIFY `site`
         ENUM('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF')
         NOT NULL DEFAULT 'WIKI'",
    )
    .await?;
    conn.query_drop(
        "ALTER TABLE `user` MODIFY `site`
         ENUM('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF')
         NOT NULL DEFAULT 'WIKI'",
    )
    .await?;
    Ok(())
}

/// The natural keys; only after the data is clean.
async fn finish(conn: &mut Conn) -> Result<()> {
    let unique = [
        ("issue", "site_url", "ALTER TABLE `issue` ADD UNIQUE KEY `site_url` (`site`,`url`)"),
        ("url_issue", "issue_url", "ALTER TABLE `url_issue` ADD UNIQUE KEY `issue_url` (`issue_id`,`url`)"),
        ("user_issue", "user_issue", "ALTER TABLE `user_issue` ADD UNIQUE KEY `user_issue` (`user_id`,`issue_id`)"),
        ("text", "hash", "ALTER TABLE `text` ADD UNIQUE KEY `hash` (`hash`)"),
    ];
    for (table, name, ddl) in unique {
        if index(conn, table, name).await?.is_none() {
            conn.query_drop(ddl).await?;
        }
    }
    if index(conn, "user", "name_site").await?.is_none() {
        let drop_old = match index(conn, "user", "name").await? {
            Some(_) => "DROP INDEX `name`,",
            None => "",
        };
        conn.query_drop(format!("ALTER TABLE `user` {drop_old} ADD UNIQUE KEY `name_site` (`name`,`site`)")).await?;
    }
    Ok(())
}

async fn has_column(conn: &mut Conn, table: &str, column: &str) -> Result<bool> {
    let n: Option<u64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM information_schema.columns
             WHERE table_schema=DATABASE() AND table_name=? AND column_name=?",
            (table, column),
        )
        .await?;
    Ok(n.unwrap_or_default() > 0)
}

/// Whether `index` exists, and whether it is unique.
async fn index(conn: &mut Conn, table: &str, name: &str) -> Result<Option<bool>> {
    let non_unique: Option<u8> = conn
        .exec_first(
            "SELECT non_unique FROM information_schema.statistics
             WHERE table_schema=DATABASE() AND table_name=? AND index_name=? LIMIT 1",
            (table, name),
        )
        .await?;
    Ok(non_unique.map(|n| n == 0))
}

// ── Data ──────────────────────────────────────────────────────────────────

/// One issue per `(site, url)`; the keeper is the row with a forge/wiki
/// mapping, then one with a log, then a closed one, then the oldest.
async fn dedupe_issues(conn: &mut impl Queryable) -> Result<()> {
    conn.query_drop(
        "CREATE TEMPORARY TABLE `issue_keep` AS
         SELECT i.`site`, i.`url`,
           COALESCE(
             MIN(CASE WHEN (i.`site`='WIKI' AND w.`issue_id` IS NOT NULL)
                        OR (i.`site`<>'WIKI' AND g.`issue_id` IS NOT NULL) THEN i.`id` END),
             MIN(CASE WHEN w.`issue_id` IS NOT NULL THEN i.`id` END),
             MIN(CASE WHEN g.`issue_id` IS NOT NULL THEN i.`id` END),
             MIN(CASE WHEN l.`issue_id` IS NOT NULL THEN i.`id` END),
             MIN(CASE WHEN i.`status`='CLOSED' THEN i.`id` END),
             MIN(i.`id`)
           ) AS `keep_id`
         FROM `issue` i
         LEFT JOIN (SELECT DISTINCT `issue_id` FROM `wiki_issue`) w ON w.`issue_id`=i.`id`
         LEFT JOIN (SELECT DISTINCT `issue_id` FROM `git_issue`) g ON g.`issue_id`=i.`id`
         LEFT JOIN (SELECT DISTINCT `issue_id` FROM `log`) l ON l.`issue_id`=i.`id`
         GROUP BY i.`site`, i.`url`",
    )
    .await?;
    conn.query_drop(
        "CREATE TEMPORARY TABLE `issue_dupes` AS
         SELECT i.`id` AS `dup_id`, k.`keep_id`
         FROM `issue` i JOIN `issue_keep` k ON k.`site`=i.`site` AND k.`url`=i.`url`
         WHERE i.`id`<>k.`keep_id`",
    )
    .await?;
    // Re-point everything that survives on the keeper, then remove the rest.
    for table in ["url_issue", "user_issue"] {
        conn.query_drop(format!(
            "UPDATE IGNORE `{table}` x JOIN `issue_dupes` d ON x.`issue_id`=d.`dup_id`
             SET x.`issue_id`=d.`keep_id`"
        ))
        .await?;
        conn.query_drop(format!("DELETE x FROM `{table}` x JOIN `issue_dupes` d ON x.`issue_id`=d.`dup_id`")).await?;
    }
    conn.query_drop("UPDATE `log` l JOIN `issue_dupes` d ON l.`issue_id`=d.`dup_id` SET l.`issue_id`=d.`keep_id`")
        .await?;
    conn.query_drop("DELETE l FROM `log` l JOIN `issue_dupes` d ON l.`issue_id`=d.`dup_id`").await?;
    // The keeper was chosen to have the mapping/wikitext when any copy had one.
    conn.query_drop("DELETE g FROM `git_issue` g JOIN `issue_dupes` d ON g.`issue_id`=d.`dup_id`").await?;
    conn.query_drop("DELETE w FROM `wiki_issue` w JOIN `issue_dupes` d ON w.`issue_id`=d.`dup_id`").await?;
    conn.query_drop("DELETE i FROM `issue` i JOIN `issue_dupes` d ON i.`id`=d.`dup_id`").await?;
    Ok(())
}

/// One user per `(name, site)`, after mapping the old empty-site rows to the
/// single site of the issues they are linked to.
async fn dedupe_users(conn: &mut impl Queryable) -> Result<()> {
    conn.query_drop(
        "UPDATE `user` u
         JOIN (
           SELECT ui.`user_id`, MIN(i.`site`) AS `site`
           FROM `user_issue` ui
           JOIN `issue` i ON i.`id`=ui.`issue_id`
           JOIN `user` x ON x.`id`=ui.`user_id` AND x.`site`=''
           GROUP BY ui.`user_id`
           HAVING COUNT(DISTINCT i.`site`)=1
         ) s ON s.`user_id`=u.`id`
         SET u.`site`=s.`site`",
    )
    .await?;
    conn.query_drop(
        "CREATE TEMPORARY TABLE `user_keep` AS
         SELECT `name`, `site`, MIN(`id`) AS `keep_id` FROM `user` GROUP BY `name`, `site`",
    )
    .await?;
    conn.query_drop(
        "CREATE TEMPORARY TABLE `user_dupes` AS
         SELECT u.`id` AS `dup_id`, k.`keep_id`
         FROM `user` u JOIN `user_keep` k ON k.`name`=u.`name` AND k.`site`=u.`site`
         WHERE u.`id`<>k.`keep_id`",
    )
    .await?;
    conn.query_drop(
        "UPDATE IGNORE `user_issue` ui JOIN `user_dupes` d ON ui.`user_id`=d.`dup_id`
         SET ui.`user_id`=d.`keep_id`",
    )
    .await?;
    conn.query_drop("DELETE ui FROM `user_issue` ui JOIN `user_dupes` d ON ui.`user_id`=d.`dup_id`").await?;
    conn.query_drop("UPDATE `log` l JOIN `user_dupes` d ON l.`user_id`=d.`dup_id` SET l.`user_id`=d.`keep_id`").await?;
    conn.query_drop("DELETE l FROM `log` l JOIN `user_dupes` d ON l.`user_id`=d.`dup_id`").await?;
    conn.query_drop("DELETE u FROM `user` u JOIN `user_dupes` d ON u.`id`=d.`dup_id`").await?;
    Ok(())
}

/// One `text` row per SHA-256, re-pointing issue descriptions and log values.
async fn dedupe_texts(conn: &mut impl Queryable) -> Result<()> {
    conn.query_drop("UPDATE `text` SET `hash`=LOWER(SHA2(`text`,256)) WHERE `hash`=''").await?;
    conn.query_drop(
        "CREATE TEMPORARY TABLE `text_keep` AS
         SELECT `hash`, MIN(`id`) AS `keep_id` FROM `text` GROUP BY `hash`",
    )
    .await?;
    conn.query_drop(
        "UPDATE `issue` i JOIN `text` t ON t.`id`=i.`description_text_id`
         JOIN `text_keep` k ON k.`hash`=t.`hash`
         SET i.`description_text_id`=k.`keep_id`
         WHERE i.`description_text_id`<>k.`keep_id`",
    )
    .await?;
    conn.query_drop(
        "UPDATE `log` l JOIN `text` t ON t.`id`=l.`text_id`
         JOIN `text_keep` k ON k.`hash`=t.`hash`
         SET l.`text_id`=k.`keep_id`
         WHERE l.`text_id`<>k.`keep_id`",
    )
    .await?;
    conn.query_drop("DELETE t FROM `text` t JOIN `text_keep` k ON k.`hash`=t.`hash` WHERE t.`id`<>k.`keep_id`").await?;
    Ok(())
}

/// Duplicate join rows left by the re-pointing.
async fn dedupe_links(conn: &mut impl Queryable) -> Result<()> {
    conn.query_drop(
        "DELETE u1 FROM `url_issue` u1 JOIN `url_issue` u2
         ON u1.`issue_id`=u2.`issue_id` AND u1.`url`=u2.`url` AND u1.`id`>u2.`id`",
    )
    .await?;
    // Keep a CREATOR role over an UNKNOWN one before collapsing the rest.
    conn.query_drop(
        "DELETE u1 FROM `user_issue` u1 JOIN `user_issue` u2
         ON u1.`user_id`=u2.`user_id` AND u1.`issue_id`=u2.`issue_id`
         AND u1.`role`='UNKNOWN' AND u2.`role`='CREATOR'",
    )
    .await?;
    conn.query_drop(
        "DELETE u1 FROM `user_issue` u1 JOIN `user_issue` u2
         ON u1.`user_id`=u2.`user_id` AND u1.`issue_id`=u2.`issue_id` AND u1.`id`>u2.`id`",
    )
    .await?;
    Ok(())
}

async fn drop_temporary_tables(conn: &mut impl Queryable) -> Result<()> {
    for table in ["issue_dupes", "issue_keep", "user_dupes", "user_keep", "text_keep"] {
        conn.query_drop(format!("DROP TEMPORARY TABLE IF EXISTS `{table}`")).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_duplicates_make_a_report_dirty() {
        let mut counts = Counts::default();
        assert!(counts.is_clean());
        counts.empty_site_users = 5;
        counts.forge_issues_without_mapping = 2;
        assert!(counts.is_clean(), "loose ends are reported but do not block the unique keys");
        counts.duplicate_texts = 1;
        assert!(!counts.is_clean());
        let report = counts.to_string();
        assert!(report.lines().any(|line| line.split_whitespace().collect::<Vec<_>>() == ["duplicate", "texts:", "1"]));
    }
}

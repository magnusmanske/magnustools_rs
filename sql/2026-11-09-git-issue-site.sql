-- One-off migration, 2026-11-09.
--
-- Forge issue numbers are unique per repository *per forge*. When a repository
-- moved from Bitbucket to Codeberg, `git_repo` kept its id and the two forges
-- reused the same numbers, so `git_issue (git_id, git_repo_id)` could not tell
-- the old and new issues apart. The updater then inserted a new Codeberg issue
-- row on every run, because its site-filtered lookup never found the mapping.
--
-- This makes the mapping site-aware and collapses the duplicate Codeberg rows,
-- keeping one row per issue URL and mapping it to its repository.
--
-- Run once against the tool database. Back up `issue`, `git_issue`,
-- `user_issue`, `url_issue` and `log` first.

-- ── Schema ────────────────────────────────────────────────────────────────
ALTER TABLE git_issue
  ADD COLUMN site ENUM('GITHUB','BITBUCKET','GITLAB','CODEBERG','GITLAB_WMF')
    NOT NULL DEFAULT 'GITHUB' AFTER issue_id;

UPDATE git_issue g JOIN issue i ON i.id=g.issue_id SET g.site=i.site;

ALTER TABLE git_issue
  DROP INDEX git_id,
  ADD UNIQUE KEY site_git_id (site, git_id, git_repo_id);

-- ── Data ──────────────────────────────────────────────────────────────────
START TRANSACTION;

-- The row to keep for each Codeberg issue URL: an already mapped row first,
-- then an open row, then the most recently active, then the newest insert.
CREATE TEMPORARY TABLE cb_keep (
  keep_id INT UNSIGNED NOT NULL PRIMARY KEY,
  url VARCHAR(255) NOT NULL,
  repo_url VARCHAR(255) NOT NULL,
  num INT UNSIGNED NOT NULL
);
INSERT INTO cb_keep
SELECT keep_id, url, repo_url, num FROM (
  SELECT i.id AS keep_id, i.url,
         SUBSTRING_INDEX(i.url, '/issues/', 1) AS repo_url,
         CAST(SUBSTRING_INDEX(i.url, '/', -1) AS UNSIGNED) AS num,
         ROW_NUMBER() OVER (
           PARTITION BY i.url
           ORDER BY (m.issue_id IS NOT NULL) DESC,
                    (i.status='OPEN') DESC, i.date_last DESC, i.id DESC
         ) AS rn
  FROM issue i
  LEFT JOIN (SELECT DISTINCT issue_id FROM git_issue) m ON m.issue_id=i.id
  WHERE i.site='CODEBERG'
) ranked
WHERE rn=1;

-- Map surviving rows that had no mapping. For repositories with several
-- `git_repo` rows, the lowest id is the canonical one; the updater matches by
-- URL, so the other rows no longer create copies.
INSERT IGNORE INTO git_issue (issue_id, site, git_id, git_repo_id)
SELECT k.keep_id, 'CODEBERG', k.num, r.id
FROM cb_keep k
JOIN (
  SELECT html_url, MIN(id) AS id
  FROM git_repo
  WHERE repo_type='CODEBERG'
  GROUP BY html_url
) r ON r.html_url=k.repo_url
WHERE NOT EXISTS (SELECT 1 FROM git_issue g WHERE g.issue_id=k.keep_id);

CREATE TEMPORARY TABLE cb_delete (
  id INT UNSIGNED NOT NULL PRIMARY KEY
);
INSERT INTO cb_delete
SELECT i.id
FROM issue i
WHERE i.site='CODEBERG'
  AND NOT EXISTS (SELECT 1 FROM cb_keep k WHERE k.keep_id=i.id);

DELETE u FROM user_issue u JOIN cb_delete d ON d.id=u.issue_id;
DELETE u FROM url_issue u JOIN cb_delete d ON d.id=u.issue_id;
DELETE l FROM log l JOIN cb_delete d ON d.id=l.issue_id;
DELETE w FROM wiki_issue w JOIN cb_delete d ON d.id=w.issue_id;
DELETE g FROM git_issue g JOIN cb_delete d ON d.id=g.issue_id;
DELETE i FROM issue i JOIN cb_delete d ON d.id=i.id;

COMMIT;

DROP TEMPORARY TABLE cb_delete;
DROP TEMPORARY TABLE cb_keep;

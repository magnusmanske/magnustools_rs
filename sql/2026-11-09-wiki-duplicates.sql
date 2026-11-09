-- One-off cleanup, 2026-11-09.
--
-- The wiki importer looked issues up by their raw heading. Emoji were
-- stripped and headings over 255 characters truncated when stored, so those
-- headings never matched and a new issue row was added on every run. The
-- importer now normalises the heading the same way. This collapses the rows
-- that already accumulated, keeping one per (talk page, heading) and moving
-- the user, URL and log references to it.
--
-- Run once against the tool database. Back up `issue`, `wiki_issue`,
-- `user_issue`, `url_issue` and `log` first.

START TRANSACTION;

CREATE TEMPORARY TABLE wiki_keep (
  keep_id INT UNSIGNED NOT NULL PRIMARY KEY,
  wiki_page_id INT UNSIGNED NOT NULL,
  label VARCHAR(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  KEY page_label (wiki_page_id, label)
);
INSERT INTO wiki_keep
SELECT keep_id, wiki_page_id, label FROM (
  SELECT i.id AS keep_id, wi.wiki_page_id, i.label,
         ROW_NUMBER() OVER (
           PARTITION BY wi.wiki_page_id, i.label
           ORDER BY (l.issue_id IS NOT NULL) DESC,
                    (i.status='CLOSED') DESC, i.date_last DESC, i.id DESC
         ) AS rn
  FROM issue i
  JOIN wiki_issue wi ON wi.issue_id=i.id
  LEFT JOIN (SELECT DISTINCT issue_id FROM log) l ON l.issue_id=i.id
  WHERE i.site='WIKI'
) ranked
WHERE rn=1;

CREATE TEMPORARY TABLE wiki_dupes (
  id INT UNSIGNED NOT NULL PRIMARY KEY,
  keep_id INT UNSIGNED NOT NULL,
  KEY keep_id (keep_id)
);
INSERT INTO wiki_dupes
SELECT i.id, k.keep_id
FROM issue i
JOIN wiki_issue wi ON wi.issue_id=i.id
JOIN wiki_keep k ON k.wiki_page_id=wi.wiki_page_id AND k.label=i.label
WHERE i.site='WIKI' AND i.id<>k.keep_id;

-- Keep the associations of the rows that go away.
UPDATE url_issue u JOIN wiki_dupes d ON d.id=u.issue_id SET u.issue_id=d.keep_id;
UPDATE user_issue u JOIN wiki_dupes d ON d.id=u.issue_id SET u.issue_id=d.keep_id;
UPDATE log l JOIN wiki_dupes d ON d.id=l.issue_id SET l.issue_id=d.keep_id;

DELETE w FROM wiki_issue w JOIN wiki_dupes d ON d.id=w.issue_id;
DELETE g FROM git_issue g JOIN wiki_dupes d ON d.id=g.issue_id;
DELETE i FROM issue i JOIN wiki_dupes d ON d.id=i.id;

COMMIT;

DROP TEMPORARY TABLE wiki_dupes;
DROP TEMPORARY TABLE wiki_keep;

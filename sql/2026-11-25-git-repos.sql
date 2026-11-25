-- One-off `git_repo` cleanup, 2026-11-25.
--
-- The Buggregator updater imports issues per `git_repo` row. Several tools have
-- moved since their row was added: the old repository is gone (HTTP 404),
-- archived, or a snapshot of the pre-move code. These statements point those
-- rows at the maintained repository and add the repositories that were missing.
--
-- A repository shared by several tools is listed once, under the umbrella tool
-- (369, Magnus Tools): buggregator identifies an issue by (site, url), so
-- several rows for one repository would all map to the same issue, and only
-- the first row's tool would be kept.

START TRANSACTION;

-- The Codeberg repositories are gone; autodesc and geohack are Rust on GitHub.
UPDATE git_repo SET html_url='https://github.com/magnusmanske/autodesc', repo_type='GITHUB'
 WHERE id=32 AND html_url='https://codeberg.org/magnusmanske/autodesc';
UPDATE git_repo SET html_url='https://github.com/magnusmanske/geohack', repo_type='GITHUB'
 WHERE id=38 AND html_url='https://codeberg.org/magnusmanske/geohack';

-- The Codeberg repository is archived; wikidata-todo continues on GitHub.
UPDATE git_repo SET html_url='https://github.com/magnusmanske/wikidata_todo_rs', repo_type='GITHUB'
 WHERE id=27 AND html_url='https://codeberg.org/magnusmanske/wikidata-todo';

-- add-information's repository is gone and its code is part of the magnustools
-- Rust codebase. This row becomes the `magnustools_rs` entry for the umbrella
-- tool; add-information's old issues keep their `issue.tool`.
UPDATE git_repo SET html_url='https://github.com/magnusmanske/magnustools_rs', repo_type='GITHUB', tool_id=369
 WHERE id=55 AND html_url='https://codeberg.org/magnusmanske/add_information';

-- Repositories that were not listed yet.
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://codeberg.org/magnusmanske/tabernacle', 'CODEBERG', 221
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://codeberg.org/magnusmanske/tabernacle' AND repo_type='CODEBERG' AND tool_id=221);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/baglama2_rs', 'GITHUB', 33
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://github.com/magnusmanske/baglama2_rs' AND repo_type='GITHUB' AND tool_id=33);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/wdrc_rs', 'GITHUB', 388
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://github.com/magnusmanske/wdrc_rs' AND repo_type='GITHUB' AND tool_id=388);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/harvesttemplates', 'GITHUB', 409
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://github.com/magnusmanske/harvesttemplates' AND repo_type='GITHUB' AND tool_id=409);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://codeberg.org/magnusmanske/wd-reconcile', 'CODEBERG', 411
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://codeberg.org/magnusmanske/wd-reconcile' AND repo_type='CODEBERG' AND tool_id=411);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/faike', 'GITHUB', 414
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://github.com/magnusmanske/faike' AND repo_type='GITHUB' AND tool_id=414);

-- crossref-rs is the async Crossref crate the papers suite uses.
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/crossref-rs', 'GITHUB', 366
 WHERE NOT EXISTS (SELECT 1 FROM git_repo
                   WHERE html_url='https://github.com/magnusmanske/crossref-rs' AND repo_type='GITHUB' AND tool_id=366);

COMMIT;

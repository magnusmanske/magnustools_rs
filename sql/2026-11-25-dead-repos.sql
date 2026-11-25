-- One-off removal of dead `git_repo` rows, 2026-11-25.
--
-- These repositories are gone (HTTP 404) or archived, and their tools have
-- either moved to a repository already tracked (`listeria_rs`, `magpie`,
-- `tabernacle`, `wd-infernal`) or have no repository to track any more.
-- The issue rows themselves stay; only the repository association goes.
-- `git_issue` maps to `git_repo` with a foreign key, so its rows go first.
--
-- Deliberately kept: the archived `quickstatements` repository (row 2) is
-- explicitly retired in `RETIRED_REPOS`; `sourcemd` (row 49) still exists and
-- is retired there too.

START TRANSACTION;

DELETE FROM git_issue WHERE git_repo_id IN (29, 35, 39, 45, 46, 52, 59, 65);
DELETE FROM git_repo WHERE id IN (29, 35, 39, 45, 46, 52, 59, 65);

COMMIT;

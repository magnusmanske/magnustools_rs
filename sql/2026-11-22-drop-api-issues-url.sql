-- One-off migration, 2026-11-22.
--
-- `git_repo.api_issues_url` was derived data: every correct value was built
-- from `html_url` and `repo_type`, and the wrong ones were web-page links that
-- made the updater skip the repository. The updater now builds the endpoint
-- itself, so the column has no readers left.
--
-- Deploy the code that no longer selects this column before running this.

ALTER TABLE `git_repo` DROP COLUMN IF EXISTS `api_issues_url`;

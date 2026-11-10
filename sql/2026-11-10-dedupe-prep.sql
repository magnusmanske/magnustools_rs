-- One-off preparation for the 2026-11-10 issue/user/text dedupe.
--
-- Run this first, on a copy, then:
--   magnustools_rs --config <copy> buggregator-dedupe            # dry run
--   magnustools_rs --config <copy> buggregator-dedupe --apply    # clean up
--
-- `buggregator-dedupe --apply` can also add these columns and indexes itself,
-- so this file is mainly for review and for deployments that prefer plain SQL.
-- Nothing here is destructive; the unique keys are in the finish migration.

-- Text is deduplicated by SHA-256 from now on. Existing rows get their hash
-- filled in; `text` is mediumtext, so this is the only practical index.
ALTER TABLE `text`
  ADD COLUMN `hash` char(64) NOT NULL DEFAULT '' COMMENT 'SHA-256 of text, so equal texts are stored once';
UPDATE `text` SET `hash`=LOWER(SHA2(`text`,256)) WHERE `hash`='';
ALTER TABLE `text` MODIFY `hash` char(64) NOT NULL;

-- The issue list filters and default sort; `text_id` no longer scans `text`.
ALTER TABLE `issue`
  ADD KEY `tool_status` (`tool`,`status`),
  ADD KEY `date_last` (`date_last`);

-- The usage dashboard asks for each tool's last used day.
ALTER TABLE `logging_uses`
  ADD KEY `date` (`date`);

-- The hourly maintenance looks for stale, not-yet-ended instances.
ALTER TABLE `dash_instance`
  ADD KEY `ended_seen` (`ended_at`,`last_seen`);

-- Forge users are attributed from now on, so the enum must know their sites.
-- `issue.site` also needs GITLAB_WMF to match `git_issue.site`/`git_repo.repo_type`.
ALTER TABLE `issue`
  MODIFY `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF')
    NOT NULL DEFAULT 'WIKI';

ALTER TABLE `user`
  MODIFY `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF')
    NOT NULL DEFAULT 'WIKI';

-- One-off finish for the 2026-11-10 dedupe.
--
-- Run this only after `magnustools_rs buggregator-dedupe --apply` reports no
-- duplicates. These keys make the identity of an issue a natural one
-- (`site` + `url`), and keep `user`, `user_issue`, `url_issue` and `text`
-- from accumulating duplicate rows again.
--
-- `buggregator-dedupe --apply` adds these keys itself once the data is clean;
-- this file is the explicit/manual equivalent.

ALTER TABLE `issue`
  ADD UNIQUE KEY `site_url` (`site`,`url`);

ALTER TABLE `user`
  DROP INDEX `name`,
  ADD UNIQUE KEY `name_site` (`name`,`site`);

ALTER TABLE `url_issue`
  ADD UNIQUE KEY `issue_url` (`issue_id`,`url`);

ALTER TABLE `user_issue`
  ADD UNIQUE KEY `user_issue` (`user_id`,`issue_id`);

ALTER TABLE `text`
  ADD UNIQUE KEY `hash` (`hash`);

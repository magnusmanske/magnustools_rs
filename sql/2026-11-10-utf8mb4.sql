-- One-off migration, 2026-11-10.
--
-- The issue tracker stores talk-page headings and forge titles, so it must
-- accept the full Unicode range. Moving the old utf8mb3 tables to utf8mb4 also
-- removes the emoji stripping the importer needed.
--
-- Several of these tables are COMPACT, which limits an indexed column to 767
-- bytes; utf8mb4 needs DYNAMIC for their 255-character indexed columns.

ALTER TABLE `issue` ROW_FORMAT=DYNAMIC;
ALTER TABLE `text` ROW_FORMAT=DYNAMIC;
ALTER TABLE `git_issue` ROW_FORMAT=DYNAMIC;
ALTER TABLE `git_repo` ROW_FORMAT=DYNAMIC;
ALTER TABLE `keywords` ROW_FORMAT=DYNAMIC;
ALTER TABLE `log` ROW_FORMAT=DYNAMIC;
ALTER TABLE `url_issue` ROW_FORMAT=DYNAMIC;
ALTER TABLE `user` ROW_FORMAT=DYNAMIC;
ALTER TABLE `wiki_issue` ROW_FORMAT=DYNAMIC;
ALTER TABLE `wiki_page` ROW_FORMAT=DYNAMIC;

ALTER TABLE `issue` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `text` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `git_issue` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `git_repo` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `keywords` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `log` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `url_issue` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `user` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `wiki_issue` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE `wiki_page` CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;

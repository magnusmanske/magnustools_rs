-- One-off cleanup, 2026-11-10.
--
-- No Rust code reads these views, and `vw_tools_tickets` refers to `tool`
-- columns that no longer exist. Keeping stale SQL SECURITY DEFINER views is a
-- maintenance and privilege hazard; the backend owns the queries now.

DROP VIEW IF EXISTS `vw_git_issue`;
DROP VIEW IF EXISTS `vw_issue`;
DROP VIEW IF EXISTS `vw_log`;
DROP VIEW IF EXISTS `vw_status_overview`;
DROP VIEW IF EXISTS `vw_tool_logs`;
DROP VIEW IF EXISTS `vw_tool_logs_main_tool_last_month`;
DROP VIEW IF EXISTS `vw_tool_logs_summary`;
DROP VIEW IF EXISTS `vw_tool_logs_summary_collapsed`;
DROP VIEW IF EXISTS `vw_tool_tag`;
DROP VIEW IF EXISTS `vw_tools_tickets`;
DROP VIEW IF EXISTS `vw_wiki_issue`;

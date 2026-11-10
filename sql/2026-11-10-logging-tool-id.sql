-- The column held a `logging_tools.id`, not a `tool.id`, which the comment
-- already warned about. The new name says so; queries outside this repository
-- that used `logging_uses.tool_id` must be updated.
ALTER TABLE `logging_uses`
  CHANGE `tool_id` `logging_tool_id` int(11) unsigned NOT NULL
    COMMENT 'The tool ID in the logging_tools table (NOT the tool table!)',
  RENAME INDEX `tool` TO `tool_date`;

-- The live service.manifest is the source of truth: every active PHP web
-- service now runs php8.4. These rows still carried the old php8.2/php5.6
-- values (checked 2026-11-23). upload-stats-bot keeps has_webservice = 0.
UPDATE `tool` SET
  `webserver_type` = 'php8.4'
WHERE `is_active` = 1
  AND `subdomain` IN ('reasonator', 'toolflow', 'wikiflix', 'wikishootme', 'upload-stats-bot');

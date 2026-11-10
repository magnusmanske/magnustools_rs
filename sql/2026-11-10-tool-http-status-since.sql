-- Since when a tool's web address answers as it does, for the alerts;
-- `http_status` is now also filled for tools without a Toolforge subdomain,
-- from their URL or their dashboard service's URL.
ALTER TABLE `tool`
  ADD COLUMN `http_status_since` int(10) unsigned DEFAULT NULL AFTER `http_status`;

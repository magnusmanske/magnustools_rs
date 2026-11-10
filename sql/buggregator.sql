SET FOREIGN_KEY_CHECKS=0;

CREATE TABLE `dash_ack` (
  `service_id` int(10) unsigned NOT NULL,
  `component` varchar(32) NOT NULL,
  `kind` enum('status','restarts','memory') NOT NULL,
  `acked_at` int(10) unsigned NOT NULL,
  `acked_by` varchar(255) NOT NULL,
  PRIMARY KEY (`service_id`,`component`,`kind`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_counter_state` (
  `instance_id` int(10) unsigned NOT NULL,
  `series_id` int(10) unsigned NOT NULL,
  `value` double NOT NULL,
  PRIMARY KEY (`instance_id`,`series_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_event` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `service_id` int(10) unsigned NOT NULL,
  `instance_id` int(10) unsigned NOT NULL,
  `ts` int(10) unsigned NOT NULL,
  `level` enum('info','warn','error') NOT NULL,
  `message` varchar(1000) NOT NULL,
  PRIMARY KEY (`id`),
  KEY `service_ts` (`service_id`,`ts`),
  KEY `ts` (`ts`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_instance` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `service_id` int(10) unsigned NOT NULL,
  `component` varchar(32) NOT NULL,
  `uid` varchar(64) NOT NULL,
  `host` varchar(128) NOT NULL DEFAULT '',
  `version` varchar(64) NOT NULL DEFAULT '',
  `started_at` int(10) unsigned NOT NULL,
  `last_seen` int(10) unsigned NOT NULL,
  `ended_at` int(10) unsigned DEFAULT NULL,
  `end_reason` enum('shutdown','exit','panic','watchdog','memory','other','oom','crash','vanished') DEFAULT NULL,
  `end_detail` varchar(1000) NOT NULL DEFAULT '',
  `rss_bytes` bigint(20) unsigned DEFAULT NULL COMMENT 'from the latest push',
  `mem_limit_bytes` bigint(20) unsigned DEFAULT NULL COMMENT 'from the latest push',
  PRIMARY KEY (`id`),
  UNIQUE KEY `service_uid` (`service_id`,`uid`),
  KEY `component_started` (`service_id`,`component`,`started_at`),
  KEY `ended_at` (`ended_at`),
  KEY `ended_seen` (`ended_at`,`last_seen`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_point` (
  `series_id` int(10) unsigned NOT NULL,
  `instance_id` int(10) unsigned NOT NULL,
  `ts` int(10) unsigned NOT NULL,
  `value` double NOT NULL,
  `delta` double DEFAULT NULL,
  PRIMARY KEY (`series_id`,`ts`,`instance_id`),
  KEY `instance_ts` (`instance_id`,`ts`),
  KEY `ts` (`ts`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_point_hourly` (
  `series_id` int(10) unsigned NOT NULL,
  `hour` int(10) unsigned NOT NULL COMMENT 'unix seconds, start of hour',
  `delta_sum` double DEFAULT NULL,
  `value_avg` double NOT NULL,
  `value_max` double NOT NULL,
  PRIMARY KEY (`series_id`,`hour`),
  KEY `hour` (`hour`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `dash_series` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `service_id` int(10) unsigned NOT NULL,
  `component` varchar(32) NOT NULL,
  `name` varchar(100) NOT NULL,
  `labels` varchar(255) NOT NULL DEFAULT '{}',
  `kind` enum('counter','gauge') NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `series` (`service_id`,`component`,`name`,`labels`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
;
CREATE TABLE `dash_service` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(64) NOT NULL,
  `label` varchar(255) NOT NULL DEFAULT '',
  `url` varchar(255) NOT NULL DEFAULT '',
  `tool_id` int(10) unsigned DEFAULT NULL COMMENT 'buggregator tool.id, for issue counts',
  `token_hash` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `is_active` tinyint(1) NOT NULL DEFAULT 1,
  `created_at` int(10) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `name` (`name`),
  UNIQUE KEY `token_hash` (`token_hash`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `git_issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `issue_id` int(11) unsigned NOT NULL DEFAULT 0,
  `site` enum('GITHUB','BITBUCKET','GITLAB','CODEBERG','GITLAB_WMF') NOT NULL DEFAULT 'GITHUB',
  `git_id` int(11) unsigned NOT NULL,
  `git_repo_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `issue_id` (`issue_id`),
  UNIQUE KEY `site_git_id` (`site`,`git_id`,`git_repo_id`),
  KEY `git_repo_id` (`git_repo_id`),
  CONSTRAINT `git_issue_ibfk_1` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`),
  CONSTRAINT `git_issue_ibfk_2` FOREIGN KEY (`git_repo_id`) REFERENCES `git_repo` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `git_repo` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `html_url` varchar(255) NOT NULL DEFAULT '',
  `api_issues_url` varchar(255) NOT NULL DEFAULT '',
  `repo_type` enum('GITHUB','BITBUCKET','GITLAB','CODEBERG','GITLAB_WMF') NOT NULL DEFAULT 'GITHUB',
  `tool_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  KEY `tool_id` (`tool_id`),
  CONSTRAINT `git_repo_ibfk_1` FOREIGN KEY (`tool_id`) REFERENCES `tool` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `label` varchar(255) NOT NULL DEFAULT '',
  `status` enum('OPEN','CLOSED') NOT NULL DEFAULT 'OPEN',
  `date_created` timestamp NULL DEFAULT NULL,
  `date_last` timestamp NULL DEFAULT NULL,
  `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF') NOT NULL DEFAULT 'WIKI',
  `url` varchar(255) NOT NULL,
  `description_text_id` int(11) unsigned DEFAULT NULL,
  `tool` int(11) unsigned DEFAULT NULL,
  `priority` enum('HIGH','NORMAL','LOW') NOT NULL DEFAULT 'NORMAL',
  PRIMARY KEY (`id`),
  UNIQUE KEY `site_url` (`site`,`url`),
  KEY `status` (`status`,`priority`,`date_last`),
  KEY `tool_status` (`tool`,`status`),
  KEY `date_last` (`date_last`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `keywords` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `tool_id` int(11) unsigned NOT NULL,
  `keyword` varchar(32) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool_id` (`tool_id`,`keyword`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `log` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `issue_id` int(11) unsigned NOT NULL,
  `event` enum('STATUS','TOOL_ID') NOT NULL DEFAULT 'STATUS',
  `user_id` int(11) unsigned NOT NULL,
  `timestamp` datetime NOT NULL DEFAULT current_timestamp(),
  `text_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  KEY `issue_id` (`issue_id`),
  KEY `user_id` (`user_id`),
  KEY `text_id` (`text_id`),
  CONSTRAINT `log_ibfk_1` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`),
  CONSTRAINT `log_ibfk_2` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`),
  CONSTRAINT `log_ibfk_3` FOREIGN KEY (`text_id`) REFERENCES `text` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `logging_tools` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(128) NOT NULL DEFAULT '',
  `method` varchar(128) NOT NULL DEFAULT '',
  `url` varchar(255) DEFAULT NULL,
  `start_log` int(11) NOT NULL,
  `main_tool_id` int(10) unsigned DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `name` (`name`,`method`),
  KEY `main_tool_id` (`main_tool_id`),
  CONSTRAINT `logging_tools_ibfk_1` FOREIGN KEY (`main_tool_id`) REFERENCES `tool` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `logging_uses` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `logging_tool_id` int(11) unsigned NOT NULL COMMENT 'The tool ID in the logging_tools table (NOT the tool table!)',
  `date` int(11) unsigned NOT NULL COMMENT 'Day, as integer',
  `used` int(11) unsigned NOT NULL DEFAULT 0 COMMENT 'counter how often this tool/method was used on that day',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool_date` (`logging_tool_id`,`date`),
  KEY `date` (`date`),
  CONSTRAINT `logging_uses_ibfk_1` FOREIGN KEY (`logging_tool_id`) REFERENCES `logging_tools` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `tag` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `tag` varchar(64) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tab` (`tag`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `tag2tool` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `tool_id` int(11) unsigned NOT NULL,
  `tag_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool_id` (`tool_id`,`tag_id`),
  KEY `tag_id` (`tag_id`),
  CONSTRAINT `tag2tool_ibfk_1` FOREIGN KEY (`tool_id`) REFERENCES `tool` (`id`),
  CONSTRAINT `tag2tool_ibfk_2` FOREIGN KEY (`tag_id`) REFERENCES `tag` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `text` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `hash` char(64) NOT NULL COMMENT 'SHA-256 of text, so equal texts are stored once',
  `text` mediumtext CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `hash` (`hash`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `tool` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(64) NOT NULL DEFAULT '',
  `subdomain` varchar(32) NOT NULL DEFAULT '',
  `description` varchar(255) NOT NULL,
  `webserver_type` varchar(64) NOT NULL DEFAULT '',
  `own_oauth` enum('OK','N/A','') NOT NULL DEFAULT '',
  `using_logging` varchar(16) NOT NULL DEFAULT '',
  `note` varchar(255) NOT NULL DEFAULT '',
  `url` varchar(255) NOT NULL DEFAULT '',
  `toolhub` varchar(64) NOT NULL DEFAULT '',
  `has_webservice` tinyint(1) NOT NULL DEFAULT 0,
  `is_active` tinyint(1) NOT NULL DEFAULT 1,
  `http_status` int(11) DEFAULT NULL,
  `http_status_since` int(10) unsigned DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `name` (`name`,`subdomain`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `tool_kv` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `tool_id` int(11) unsigned NOT NULL,
  `key` varchar(16) NOT NULL DEFAULT '',
  `value` varchar(127) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool_id` (`tool_id`,`key`,`value`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `url_issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `url` varchar(255) NOT NULL DEFAULT '',
  `issue_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `issue_url` (`issue_id`,`url`),
  KEY `issue_id` (`issue_id`),
  CONSTRAINT `url_issue_ibfk_1` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `user` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL DEFAULT '',
  `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG','GITLAB_WMF') NOT NULL DEFAULT 'WIKI',
  PRIMARY KEY (`id`),
  UNIQUE KEY `name_site` (`name`,`site`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `user_issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `user_id` int(11) unsigned NOT NULL,
  `issue_id` int(11) unsigned NOT NULL,
  `role` enum('UNKNOWN','CREATOR') NOT NULL DEFAULT 'UNKNOWN',
  PRIMARY KEY (`id`),
  UNIQUE KEY `user_issue` (`user_id`,`issue_id`),
  KEY `user_id` (`user_id`),
  KEY `issue_id` (`issue_id`),
  CONSTRAINT `user_issue_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`),
  CONSTRAINT `user_issue_ibfk_2` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `wiki_issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `wiki_page_id` int(11) unsigned NOT NULL,
  `issue_id` int(11) unsigned NOT NULL,
  `wikitext` mediumtext NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `issue_id` (`issue_id`),
  KEY `wiki_page_id` (`wiki_page_id`),
  CONSTRAINT `wiki_issue_ibfk_1` FOREIGN KEY (`wiki_page_id`) REFERENCES `wiki_page` (`id`),
  CONSTRAINT `wiki_issue_ibfk_2` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;
CREATE TABLE `wiki_page` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `wiki` varchar(16) NOT NULL DEFAULT '',
  `page` varchar(255) NOT NULL DEFAULT '',
  `tool_hint` int(11) unsigned DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `wiki` (`wiki`,`page`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
;

-- Foreign keys whose referenced tables are created later in the dump.
ALTER TABLE `issue`
  ADD CONSTRAINT `issue_tool_fk` FOREIGN KEY (`tool`) REFERENCES `tool` (`id`) ON DELETE SET NULL,
  ADD CONSTRAINT `issue_description_fk` FOREIGN KEY (`description_text_id`) REFERENCES `text` (`id`);

ALTER TABLE `wiki_page`
  ADD CONSTRAINT `wiki_page_tool_fk` FOREIGN KEY (`tool_hint`) REFERENCES `tool` (`id`) ON DELETE SET NULL;

ALTER TABLE `dash_service`
  ADD CONSTRAINT `dash_service_tool_fk` FOREIGN KEY (`tool_id`) REFERENCES `tool` (`id`) ON DELETE SET NULL;

ALTER TABLE `dash_ack`
  ADD CONSTRAINT `dash_ack_service_fk` FOREIGN KEY (`service_id`) REFERENCES `dash_service` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_instance`
  ADD CONSTRAINT `dash_instance_service_fk` FOREIGN KEY (`service_id`) REFERENCES `dash_service` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_series`
  ADD CONSTRAINT `dash_series_service_fk` FOREIGN KEY (`service_id`) REFERENCES `dash_service` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_point`
  ADD CONSTRAINT `dash_point_series_fk` FOREIGN KEY (`series_id`) REFERENCES `dash_series` (`id`) ON DELETE CASCADE,
  ADD CONSTRAINT `dash_point_instance_fk` FOREIGN KEY (`instance_id`) REFERENCES `dash_instance` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_point_hourly`
  ADD CONSTRAINT `dash_point_hourly_series_fk` FOREIGN KEY (`series_id`) REFERENCES `dash_series` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_counter_state`
  ADD CONSTRAINT `dash_counter_state_instance_fk` FOREIGN KEY (`instance_id`) REFERENCES `dash_instance` (`id`) ON DELETE CASCADE,
  ADD CONSTRAINT `dash_counter_state_series_fk` FOREIGN KEY (`series_id`) REFERENCES `dash_series` (`id`) ON DELETE CASCADE;

ALTER TABLE `dash_event`
  ADD CONSTRAINT `dash_event_service_fk` FOREIGN KEY (`service_id`) REFERENCES `dash_service` (`id`) ON DELETE CASCADE,
  ADD CONSTRAINT `dash_event_instance_fk` FOREIGN KEY (`instance_id`) REFERENCES `dash_instance` (`id`) ON DELETE CASCADE;

SET FOREIGN_KEY_CHECKS=1;

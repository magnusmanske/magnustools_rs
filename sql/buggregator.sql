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
  KEY `ended_at` (`ended_at`)
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
  `token_hash` char(64) NOT NULL,
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
  `git_id` int(11) unsigned NOT NULL,
  `git_repo_id` int(11) unsigned NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `issue_id` (`issue_id`),
  UNIQUE KEY `git_id` (`git_id`,`git_repo_id`),
  KEY `git_repo_id` (`git_repo_id`),
  CONSTRAINT `git_issue_ibfk_1` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`),
  CONSTRAINT `git_issue_ibfk_2` FOREIGN KEY (`git_repo_id`) REFERENCES `git_repo` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE TABLE `issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `label` varchar(255) NOT NULL DEFAULT '',
  `status` enum('OPEN','CLOSED') NOT NULL DEFAULT 'OPEN',
  `date_created` timestamp NOT NULL DEFAULT '0000-00-00 00:00:00',
  `date_last` timestamp NOT NULL DEFAULT '0000-00-00 00:00:00',
  `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER','WIKIDATA','GITLAB','CODEBERG') NOT NULL DEFAULT 'WIKI',
  `url` varchar(255) NOT NULL,
  `description_text_id` int(11) NOT NULL DEFAULT 0,
  `tool` int(11) unsigned NOT NULL DEFAULT 0,
  `priority` enum('HIGH','NORMAL','LOW') NOT NULL DEFAULT 'NORMAL',
  PRIMARY KEY (`id`),
  KEY `status` (`status`,`priority`,`date_last`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE TABLE `keywords` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `tool_id` int(11) unsigned NOT NULL,
  `keyword` varchar(32) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool_id` (`tool_id`,`keyword`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
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
  `tool_id` int(11) unsigned NOT NULL COMMENT 'The tool ID in the logging_tools table (NOT the tool table!)',
  `date` int(11) unsigned NOT NULL COMMENT 'Day, as integer',
  `used` int(11) unsigned NOT NULL DEFAULT 0 COMMENT 'counter how often this tool/method was used on that day',
  PRIMARY KEY (`id`),
  UNIQUE KEY `tool` (`tool_id`,`date`),
  CONSTRAINT `logging_uses_ibfk_1` FOREIGN KEY (`tool_id`) REFERENCES `logging_tools` (`id`)
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
  `text` mediumtext CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
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
  KEY `issue_id` (`issue_id`),
  CONSTRAINT `url_issue_ibfk_1` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE TABLE `user` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL DEFAULT '',
  `site` enum('WIKI','GITHUB','BITBUCKET','TWITTER') NOT NULL DEFAULT 'WIKI',
  PRIMARY KEY (`id`),
  KEY `name` (`name`,`site`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE TABLE `user_issue` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `user_id` int(11) unsigned NOT NULL,
  `issue_id` int(11) unsigned NOT NULL,
  `role` enum('UNKNOWN','CREATOR') NOT NULL DEFAULT 'UNKNOWN',
  PRIMARY KEY (`id`),
  KEY `user_id` (`user_id`),
  KEY `issue_id` (`issue_id`),
  CONSTRAINT `user_issue_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`),
  CONSTRAINT `user_issue_ibfk_2` FOREIGN KEY (`issue_id`) REFERENCES `issue` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_git_issue` AS select `issue`.`id` AS `id`,`issue`.`label` AS `label`,`issue`.`status` AS `status`,`issue`.`date_created` AS `date_created`,`issue`.`date_last` AS `date_last`,`issue`.`site` AS `site`,`issue`.`url` AS `url`,`text`.`text` AS `description`,`issue`.`tool` AS `tool`,`issue`.`priority` AS `priority`,`git_issue`.`git_id` AS `git_id`,`git_issue`.`git_repo_id` AS `git_repo_id`,`git_repo`.`html_url` AS `html_url`,`git_repo`.`api_issues_url` AS `api_issues_url`,`git_repo`.`repo_type` AS `repo_type`,`git_repo`.`tool_id` AS `tool_id` from (((`issue` join `git_issue`) join `git_repo`) join `text`) where `issue`.`site` in ('GITHUB','BITBUCKET','GITLAB','CODEBERG') and `git_issue`.`issue_id` = `issue`.`id` and `git_issue`.`git_repo_id` = `git_repo`.`id` and `issue`.`description_text_id` = `text`.`id`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_issue` AS select `issue`.`id` AS `id`,`issue`.`label` AS `label`,`issue`.`status` AS `status`,`issue`.`date_created` AS `date_created`,`issue`.`date_last` AS `date_last`,`issue`.`site` AS `site`,`issue`.`url` AS `url`,`issue`.`description_text_id` AS `description_text_id`,`issue`.`tool` AS `tool`,`issue`.`priority` AS `priority`,(select `text`.`text` from `text` where `issue`.`description_text_id` = `text`.`id`) AS `description` from `issue`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_log` AS select `log`.`id` AS `id`,`log`.`issue_id` AS `issue_id`,`log`.`event` AS `event`,`log`.`user_id` AS `user_id`,`log`.`timestamp` AS `timestamp`,`log`.`text_id` AS `text_id`,`issue`.`label` AS `issue_label`,`user`.`name` AS `user_name`,`text`.`text` AS `value` from (((`log` join `issue`) join `user`) join `text`) where `log`.`issue_id` = `issue`.`id` and `log`.`user_id` = `user`.`id` and `log`.`text_id` = `text`.`id`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_status_overview` AS select `issue`.`status` AS `status`,`issue`.`priority` AS `priority`,count(0) AS `cnt` from `issue` group by `issue`.`status`,`issue`.`priority`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tool_logs` AS select `logging_uses`.`tool_id` AS `tool_id`,`logging_uses`.`date` AS `date`,`logging_uses`.`used` AS `used`,`logging_tools`.`name` AS `name`,`logging_tools`.`method` AS `method`,`logging_tools`.`url` AS `url` from (`logging_uses` join `logging_tools`) where `logging_tools`.`id` = `logging_uses`.`tool_id` order by `logging_uses`.`date` desc,`logging_tools`.`name`,`logging_tools`.`method`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tool_logs_main_tool_last_month` AS select `logging_tools`.`main_tool_id` AS `main_tool_id`,sum(`vw_tool_logs`.`used`) AS `monthly_usage` from (`vw_tool_logs` join `logging_tools`) where `vw_tool_logs`.`date` like concat(year(current_timestamp() - interval 1 month),right(concat('00',month(current_timestamp() - interval 1 month)),2),'%') and `logging_tools`.`id` = `vw_tool_logs`.`tool_id` and `logging_tools`.`main_tool_id` is not null group by `logging_tools`.`main_tool_id` order by sum(`vw_tool_logs`.`used`) desc
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tool_logs_summary` AS select `logging_uses`.`tool_id` AS `tool_id`,sum(`logging_uses`.`used`) AS `total_usage`,count(`logging_uses`.`date`) AS `days_used`,round(if(count(`logging_uses`.`date`) = 0,0,sum(`logging_uses`.`used`) / count(`logging_uses`.`date`)),2) AS `uses_per_day`,`logging_tools`.`name` AS `name`,`logging_tools`.`method` AS `method`,`logging_tools`.`url` AS `url` from (`logging_uses` join `logging_tools`) where `logging_tools`.`id` = `logging_uses`.`tool_id` group by `logging_uses`.`tool_id` order by round(if(count(`logging_uses`.`date`) = 0,0,sum(`logging_uses`.`used`) / count(`logging_uses`.`date`)),2) desc
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tool_logs_summary_collapsed` AS select `logging_uses`.`tool_id` AS `tool_id`,sum(`logging_uses`.`used`) AS `total_usage`,count(distinct `logging_uses`.`date`) AS `days_used`,round(if(count(`logging_uses`.`date`) = 0,0,sum(`logging_uses`.`used`) / count(`logging_uses`.`date`)),2) AS `uses_per_day`,`logging_tools`.`name` AS `name` from (`logging_uses` join `logging_tools`) where `logging_tools`.`id` = `logging_uses`.`tool_id` group by `logging_tools`.`name` order by round(if(count(`logging_uses`.`date`) = 0,0,sum(`logging_uses`.`used`) / count(`logging_uses`.`date`)),2) desc
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tool_tag` AS select `tag`.`tag` AS `tag`,`tool`.`id` AS `id`,`tool`.`name` AS `name`,`tool`.`subdomain` AS `subdomain`,`tool`.`description` AS `description`,`tool`.`webserver_type` AS `webserver_type`,`tool`.`own_oauth` AS `own_oauth`,`tool`.`using_logging` AS `using_logging`,`tool`.`note` AS `note`,`tool`.`url` AS `url`,`tool`.`toolhub` AS `toolhub` from ((`tool` join `tag`) join `tag2tool`) where `tag2tool`.`tag_id` = `tag`.`id` and `tag2tool`.`tool_id` = `tool`.`id`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_tools_tickets` AS select `tool`.`id` AS `id`,`tool`.`name` AS `name`,`tool`.`subdomain` AS `subdomain`,`tool`.`webserver_type` AS `webserver_type`,`tool`.`own_oauth` AS `own_oauth`,`s51079__buggregator`.`tool`.`using_subdomain` AS `using_subdomain`,`s51079__buggregator`.`tool`.`using_kubernetes` AS `using_kubernetes`,`s51079__buggregator`.`tool`.`using_new_wbterms` AS `using_new_wbterms`,`s51079__buggregator`.`tool`.`using_composer` AS `using_composer`,`s51079__buggregator`.`tool`.`using_logging` AS `using_logging`,`s51079__buggregator`.`tool`.`note` AS `note`,`s51079__buggregator`.`tool`.`url` AS `url`,`s51079__buggregator`.`tool`.`toolhub` AS `toolhub`,(select count(0) from `issue` where `s51079__buggregator`.`issue`.`tool` = `s51079__buggregator`.`tool`.`id` and `s51079__buggregator`.`issue`.`status` = 'OPEN') AS `open_tickets`,(select count(0) from `issue` where `s51079__buggregator`.`issue`.`tool` = `s51079__buggregator`.`tool`.`id` and `s51079__buggregator`.`issue`.`status` = 'CLOSED') AS `closed_tickets` from `tool`
;
CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `vw_wiki_issue` AS select `issue`.`id` AS `id`,`issue`.`label` AS `label`,`issue`.`status` AS `status`,`issue`.`date_created` AS `date_created`,`issue`.`date_last` AS `date_last`,`issue`.`site` AS `site`,`issue`.`url` AS `url`,`issue`.`description_text_id` AS `description_text_id`,`issue`.`tool` AS `tool`,`issue`.`priority` AS `priority`,`text`.`text` AS `description`,`wiki_issue`.`wiki_page_id` AS `wiki_page_id`,`wiki_page`.`wiki` AS `wiki`,`wiki_page`.`page` AS `page`,`wiki_page`.`tool_hint` AS `tool_hint`,`wiki_issue`.`wikitext` AS `wikitext` from (((`issue` join `wiki_issue`) join `wiki_page`) join `text`) where `wiki_issue`.`issue_id` = `issue`.`id` and `issue`.`site` = 'WIKI' and `wiki_issue`.`wiki_page_id` = `wiki_page`.`id` and `issue`.`description_text_id` = `text`.`id`
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;
CREATE TABLE `wiki_page` (
  `id` int(11) unsigned NOT NULL AUTO_INCREMENT,
  `wiki` varchar(16) NOT NULL DEFAULT '',
  `page` varchar(255) NOT NULL DEFAULT '',
  `tool_hint` int(11) unsigned NOT NULL DEFAULT 0,
  PRIMARY KEY (`id`),
  UNIQUE KEY `wiki` (`wiki`,`page`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_general_ci
;

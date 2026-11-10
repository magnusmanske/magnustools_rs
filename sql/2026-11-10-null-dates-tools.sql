-- One-off migration, 2026-11-10.
--
-- Replaces the "no value" sentinels (the zero date and id 0) with NULL and
-- adds the foreign keys that the sentinels had made impossible. Run after the
-- dedupe and the utf8mb4 migration.

ALTER TABLE `issue`
  MODIFY `date_created` timestamp NULL DEFAULT NULL,
  MODIFY `date_last` timestamp NULL DEFAULT NULL,
  MODIFY `description_text_id` int(11) unsigned DEFAULT NULL,
  MODIFY `tool` int(11) unsigned DEFAULT NULL;

ALTER TABLE `wiki_page`
  MODIFY `tool_hint` int(11) unsigned DEFAULT NULL;

UPDATE `issue` SET `date_created`=NULL WHERE `date_created`='0000-00-00 00:00:00';
UPDATE `issue` SET `date_last`=NULL WHERE `date_last`='0000-00-00 00:00:00';

UPDATE `issue` SET `description_text_id`=NULL WHERE `description_text_id`=0;
UPDATE `issue` SET `tool`=NULL WHERE `tool`=0 OR NOT EXISTS (SELECT 1 FROM `tool` t WHERE t.`id`=`issue`.`tool`);
UPDATE `wiki_page` SET `tool_hint`=NULL
  WHERE `tool_hint`=0 OR NOT EXISTS (SELECT 1 FROM `tool` t WHERE t.`id`=`wiki_page`.`tool_hint`);
UPDATE `dash_service` d SET d.`tool_id`=NULL
  WHERE d.`tool_id` IS NOT NULL AND NOT EXISTS (SELECT 1 FROM `tool` t WHERE t.`id`=d.`tool_id`);

-- Remove orphaned dashboard rows before the foreign keys can reject them.
DELETE a FROM `dash_ack` a LEFT JOIN `dash_service` s ON s.`id`=a.`service_id` WHERE s.`id` IS NULL;
DELETE i FROM `dash_instance` i LEFT JOIN `dash_service` s ON s.`id`=i.`service_id` WHERE s.`id` IS NULL;
DELETE s FROM `dash_series` s LEFT JOIN `dash_service` v ON v.`id`=s.`service_id` WHERE v.`id` IS NULL;
DELETE p FROM `dash_point` p
  LEFT JOIN `dash_series` s ON s.`id`=p.`series_id`
  LEFT JOIN `dash_instance` i ON i.`id`=p.`instance_id`
  WHERE s.`id` IS NULL OR i.`id` IS NULL;
DELETE h FROM `dash_point_hourly` h LEFT JOIN `dash_series` s ON s.`id`=h.`series_id` WHERE s.`id` IS NULL;
DELETE c FROM `dash_counter_state` c
  LEFT JOIN `dash_instance` i ON i.`id`=c.`instance_id`
  LEFT JOIN `dash_series` s ON s.`id`=c.`series_id`
  WHERE i.`id` IS NULL OR s.`id` IS NULL;
DELETE e FROM `dash_event` e
  LEFT JOIN `dash_service` s ON s.`id`=e.`service_id`
  LEFT JOIN `dash_instance` i ON i.`id`=e.`instance_id`
  WHERE s.`id` IS NULL OR i.`id` IS NULL;

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

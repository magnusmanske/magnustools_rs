-- Wikidata Terminator candidate index, on ToolsDB in `<tool user>__terminator_p`.
-- The daily `terminator-refresh` job creates these tables and seeds `main_p31`
-- itself; this file is the reference schema and the migration for a new tool.

CREATE TABLE IF NOT EXISTS `main_p31` (
  `q` INT UNSIGNED NOT NULL,
  PRIMARY KEY (`q`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `meta` (
  `name` VARCHAR(64) NOT NULL,
  `value` VARCHAR(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `top_candidates` (
  `type` ENUM('labels','descriptions','sites') NOT NULL,
  `language` VARCHAR(35) NOT NULL,
  `q` INT UNSIGNED NOT NULL,
  `score` INT NOT NULL,
  `claims` INT NOT NULL,
  `identifiers` INT NOT NULL,
  `sitelinks` INT NOT NULL,
  `p31` INT UNSIGNED NOT NULL DEFAULT 0,
  `rnd` FLOAT NOT NULL,
  PRIMARY KEY (`type`,`language`,`q`),
  KEY `by_score` (`type`,`language`,`score`),
  KEY `by_claims` (`type`,`language`,`claims`),
  KEY `by_sitelinks` (`type`,`language`,`sitelinks`),
  KEY `by_rnd` (`type`,`language`,`rnd`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- P31 values considered "main" and not worth translating. The refresh job
-- keeps this list as well.
INSERT IGNORE INTO `main_p31` (`q`) VALUES
  (5), (4167410), (4167836), (4663903), (6964088), (11266439), (11753321),
  (13406463), (13442814), (14204246), (15184295), (15647814), (17362920),
  (19842659), (19887878), (20010800), (20769160), (23838648), (23894233),
  (24046192), (30432511), (35250433), (36330215);

-- Move the six PHP tools to the magnustools Rust service (2026-11-13).
-- The old subdomains redirect there with Toolforge's redirect container.
UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/catnap/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly catnap.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'catnap';

UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/file-siblings/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly file-siblings.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'file-siblings';

UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/filedupes/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly filedupes.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'filedupes';

UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/multidesc/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly multidesc.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'multidesc';

UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/new-page-linker/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly new-page-linker.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'new-page-linker';

UPDATE `tool` SET
  `subdomain` = 'magnustools',
  `url` = 'https://magnustools.toolforge.org/wikidata-terminator/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly wikidata-terminator.toolforge.org; that subdomain redirects here with its parameters, including /api.php.'
WHERE `name` = 'wikidata-terminator';

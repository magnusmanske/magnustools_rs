-- The four upload tools moved to the magpie Rust service (2026-11-20); their
-- old subdomains are Toolforge redirect containers. The `tool` rows still named
-- the old subdomains, so the monitor did not see them as covered by magpie's
-- dashboard service and mailed "non-200 status" for their unchecked (NULL)
-- `http_status`. Point them at the service's subdomain, as for the tools moved
-- to magnustools on 2026-11-13.
UPDATE `tool` SET
  `subdomain` = 'magpie',
  `url` = 'https://magpie.toolforge.org/commonshelper/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly commonshelper.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'commonshelper';

UPDATE `tool` SET
  `subdomain` = 'magpie',
  `url` = 'https://magpie.toolforge.org/flickr2commons/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly flickr2commons.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'flickr2commons';

UPDATE `tool` SET
  `subdomain` = 'magpie',
  `url` = 'https://magpie.toolforge.org/geograph2commons/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly geograph2commons.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'geograph2commons';

UPDATE `tool` SET
  `subdomain` = 'magpie',
  `url` = 'https://magpie.toolforge.org/url2commons/',
  `webserver_type` = 'rust/axum',
  `has_webservice` = 1,
  `is_active` = 1,
  `note` = 'Formerly url2commons.toolforge.org; that subdomain redirects here with its parameters.'
WHERE `name` = 'url2commons';

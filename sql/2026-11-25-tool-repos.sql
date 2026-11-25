-- One-off `tool`/`git_repo` additions for code-only projects, 2026-11-25.
--
-- These repositories have no tool row yet: published Rust crates, and the
-- shared PHP source of WikiFlix and WikiVibes. They have no webservice of their
-- own (`has_webservice=0`), so the URL is informational only.

START TRANSACTION;

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'bookmeta', '', 'Book metadata by ISBN from several catalogues', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/bookmeta', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='bookmeta' AND subdomain='' AND note='code only');
SET @bookmeta = (SELECT id FROM tool WHERE name='bookmeta' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'wikimisc', '', 'A library with MediaWiki-related functionality', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/wikimisc', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='wikimisc' AND subdomain='' AND note='code only');
SET @wikimisc = (SELECT id FROM tool WHERE name='wikimisc' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'mediawiki_rest_api', '', 'A Rust client for the MediaWiki REST API', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/mediawiki_rest_api', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='mediawiki_rest_api' AND subdomain='' AND note='code only');
SET @mediawiki_rest_api = (SELECT id FROM tool WHERE name='mediawiki_rest_api' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'wikibase_rest_api', '', 'A Rust client for the Wikibase REST API', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/wikibase_rest_api', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='wikibase_rest_api' AND subdomain='' AND note='code only');
SET @wikibase_rest_api = (SELECT id FROM tool WHERE name='wikibase_rest_api' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'tools_interface', '', 'Abstractions of several Wikipedia/Wikimedia/Wikidata-related tools', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/tools_interface', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='tools_interface' AND subdomain='' AND note='code only');
SET @tools_interface = (SELECT id FROM tool WHERE name='tools_interface' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'goodreads-metadata-scraper', '', 'A library to scrape book metadata from Goodreads', '', 'N/A', 'N/A', 'code only', 'https://crates.io/crates/goodreads-metadata-scraper', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='goodreads-metadata-scraper' AND subdomain='' AND note='code only');
SET @goodreads = (SELECT id FROM tool WHERE name='goodreads-metadata-scraper' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO tool (name, subdomain, description, webserver_type, own_oauth, using_logging, note, url, toolhub, has_webservice, is_active)
SELECT 'wikistream', '', 'Shared source of WikiFlix and WikiVibes', '', 'N/A', 'N/A', 'code only', '', '', 0, 1
 WHERE NOT EXISTS (SELECT 1 FROM tool WHERE name='wikistream' AND subdomain='' AND note='code only');
SET @wikistream = (SELECT id FROM tool WHERE name='wikistream' AND subdomain='' AND note='code only' ORDER BY id LIMIT 1);

INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/bookmeta', 'GITHUB', @bookmeta
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/bookmeta' AND repo_type='GITHUB' AND tool_id=@bookmeta);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/wikimisc', 'GITHUB', @wikimisc
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/wikimisc' AND repo_type='GITHUB' AND tool_id=@wikimisc);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/mediawiki_rest_api', 'GITHUB', @mediawiki_rest_api
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/mediawiki_rest_api' AND repo_type='GITHUB' AND tool_id=@mediawiki_rest_api);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/wikibase_rest_api', 'GITHUB', @wikibase_rest_api
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/wikibase_rest_api' AND repo_type='GITHUB' AND tool_id=@wikibase_rest_api);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/tools_interface', 'GITHUB', @tools_interface
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/tools_interface' AND repo_type='GITHUB' AND tool_id=@tools_interface);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/goodreads-metadata-scraper', 'GITHUB', @goodreads
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/goodreads-metadata-scraper' AND repo_type='GITHUB' AND tool_id=@goodreads);
INSERT INTO git_repo (html_url, repo_type, tool_id)
SELECT 'https://github.com/magnusmanske/wikistream', 'GITHUB', @wikistream
 WHERE NOT EXISTS (SELECT 1 FROM git_repo WHERE html_url='https://github.com/magnusmanske/wikistream' AND repo_type='GITHUB' AND tool_id=@wikistream);

COMMIT;

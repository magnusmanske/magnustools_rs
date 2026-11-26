-- User talk:Reinheitsgebot is a shared bot, not the tool that issues belong to.
-- Sections that name an invoking tool are classified by the importer; the rest
-- stay untooled for manual triage.
UPDATE `wiki_page` SET `tool_hint`=NULL
  WHERE `wiki`='wikidatawiki' AND `page`='User talk:Reinheitsgebot';

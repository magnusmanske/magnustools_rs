# Buggregator issue triage

Repeatable process for the open issues on
[buggregator](https://magnustools.toolforge.org/buggregator/): decide which
can be closed, close them in Buggregator **and** on their git tracker, and set
priorities.

Issues come from wiki talk pages (`WIKI`) and from GitHub/Codeberg
(`site` + `git_issue`). The sources are re-imported by the daily
`buggregator-update` job, so a close in Buggregator alone is not durable for a
tracker issue that is still open upstream.

## Hard rules

- **Check every issue individually.** Do not close on age alone; the
  2026-11-11 bulk close of 301 old wiki sections was reverted for that reason.
- **Close tracker issues upstream too.** GitHub/Codeberg issues still open
  upstream must be closed there as well, or the daily sync reopens them.
- **Never edit wiki pages.** WIKI issues are closed in Buggregator only; the
  talk-page section stays as it is.
- **Verify data corrections against Wikidata.** "The bot made this edit" is
  only resolved if the wrong statement is gone. Use the *correct* property ID
  (SIKART is `P781`, IDFA film is `P7741`; `P1362`/`P6385` are something else).
- **Keep a rollback and a pre-close snapshot.**
- **Prefer evidence:** an upstream `state=closed`, an explicit "fixed"/"thanks"
  reply, a current value that no longer shows the problem, or a bot's actual
  edit history. A 10-year-old unanswered feature request is not "resolved".

## Setup

- `replica.my.cnf` with the tool database credentials (gitignored).
- `gh auth login` with `repo` scope, for GitHub.
- `FORGEJO_API_TOKEN` in the environment for Codeberg (reading works without
  it; closing does not).
- A tunnel to ToolsDB. Either:

  ```sh
  TOOL_DATA_DIR=. toolforge-tunnel toolsdb
  # or
  ssh -N -o ExitOnForwardFailure=yes dev.toolforge.org \
      -L 3398:tools.db.svc.wikimedia.cloud:3306
  ```

  The scripts default to `127.0.0.1:3398` and database
  `s51079__buggregator`. Override with `BUGGREGATOR_DB`,
  `BUGGREGATOR_DB_HOST`, `BUGGREGATOR_DB_PORT`, `BUGGREGATOR_CNF`.

## 1. Fetch

```sh
python3 triage/fetch.py /tmp/bt
```

Writes to `/tmp/bt`:

| File | Content |
|---|---|
| `open.json` | every open issue: id, site, tracker repo/number, tool, priority, URL |
| `states.json` | live upstream state (open/closed, labels, comments, dates) |
| `details.json` | body and comments of each issue still open upstream |
| `wiki.json` | the current wikitext of the section behind each WIKI issue |
| `review.txt` | a digest grouped by site and tool/repo |

The tracker repo is derived from the **issue URL**, not from
`git_repo.html_url`: after a repository move the row points at the new
repository while old issues still live on the old tracker.

## 2. Decide

Build a list of closable issues with a reason each.

**Tracker (`states.json`)**
- `state: closed` → close in Buggregator only (`"upstream": false`).
- `state: open` and the issue is resolved, a duplicate, withdrawn, or not a
  real issue → close upstream and in Buggregator. Use `"completed"` or
  `"not planned"` for GitHub.
- If the code moved and the old repository is gone, the issue URL may 404;
  check the new repository for the same title/behaviour before closing.

**WIKI (`wiki.json`)**
- automated notifications, greetings, barnstars → not an issue;
- an explicit "fixed", "works again", "thanks", "nevermind", or the reporter
  handled it → resolved;
- a question that has an answer → resolved;
- the section no longer exists on the page → resolved/gone;
- a reported Wikidata error whose statement is gone (check the item and the
  correct property) → resolved.

Do not close a thread that is only old, or where the diagnosis is still
unaddressed. Do not edit the page to resolve it.

Duplicates: close the duplicate and keep a canonical issue open.

## 3. Write the plan

```json
{
  "close": {
    "2202": {"reason": "resolved; coordinate editing no longer hits the wrong item", "upstream": false},
    "8492": {"reason": "duplicate of #116", "upstream": "not planned"},
    "6473": {"reason": "copyright issue fixed", "upstream": "completed"}
  },
  "priorities": {
    "6798": "HIGH",
    "6947": "LOW"
  },
  "tools": {
    "2211": 195,
    "6359": 220
  }
}
```

- `reason` is only for the report and rollback file; it is not written to the
  tracker or the wiki.
- `upstream` is optional. `false` leaves the tracker alone (WIKI issues or an
  already-closed tracker issue). For GitHub it is `"completed"` or
  `"not planned"`; Codeberg only gets `state=closed`. The default is
  `"completed"`.
- Priorities are `HIGH`, `NORMAL`, `LOW`.
- `tools` maps an issue id to the id of the tool that actually caused it
  (`tool.id`). Use it when the linked account is only an editing bot, e.g.
  Reinheitsgebot issues that belong to Mix'n'match (195) or SourceMD (220).
  The id is validated against `tool`; the change is logged as `TOOL_ID` and
  does not bump `date_last`. The rollback restores the previous tool (or
  `NULL`).

## 4. Apply

```sh
python3 triage/apply.py /tmp/bt/plan.json --data /tmp/bt          # dry run
python3 triage/apply.py /tmp/bt/plan.json --data /tmp/bt --apply  # for real
```

The script:

1. validates the ids against the open set, the priorities, and the tool ids;
2. closes git issues upstream first (skipping ones already closed);
3. writes the Buggregator changes (close, priority, tool) in one transaction,
   logging `STATUS` → `CLOSED` and `TOOL_ID` as `Magnus Manske`;
4. writes `/tmp/bt/rollback.sql` (reopen the ids, delete the new log rows,
   restore changed priorities and tools);
5. prints the final status and priority counts.

It is idempotent: issues no longer open, or already closed upstream, are
skipped.

## 5. Priorities

- **HIGH** — ongoing wrong edits/data corruption on Wikimedia projects,
  copyright/licensing problems, or a core tool broken for many users.
- **LOW** — cosmetic or single-user requests, minor enhancements, stale
  suggestions, documentation, niche features.
- **NORMAL** — the rest: valid bugs and feature requests with moderate impact.

Priorities are updated directly in `issue.priority`; the log event enum has no
priority event, and `date_last` is not bumped.

## 6. Verify

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3398 -u "$USER" s51079__buggregator -e \
  "SELECT status, COUNT(*) FROM issue GROUP BY status;
   SELECT priority, COUNT(*) FROM issue WHERE status='OPEN' GROUP BY priority;"
```

For every git issue closed in this pass, confirm its tracker state is
`closed`. Keep `rollback.sql` and the pre-close snapshot with the other
triage records.

## Rollback

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3398 -u "$USER" s51079__buggregator \
  < /tmp/bt/rollback.sql
```

Upstream GitHub/Codeberg closures must be reopened through their API/UI; the
SQL cannot do that.

## Notes from the 2026-11-26/27 run

- 327 → 222 → 221 → 220 open: 50 tracker issues were already closed upstream,
  14 were closed upstream in the pass, 42 WIKI issues were resolved/answered/
  notifications. Two more HIGH tracker issues (`529`, `547`) were then closed
  separately, leaving 218.
- Moved repositories matter: `commonshelper`, `flickr2commons`,
  `geograph2commons` and `url2commons` now live in `magnusmanske/magpie`, but
  their old Codeberg issue URLs still resolve; query the URL's repository.
- WIKI closes do not stick if a new reply appears: `reopen_on_reply` reopens a
  thread closed by the maintainer when a newer signature arrives.
- 17 open issues on `User_talk:Reinheitsgebot` were reassigned from the bot
  tool (375): 13 to Mix'n'match (195), 4 to SourceMD (220). The edit summaries
  of the linked diffs name the invoking tool (`invoked by mixnmatch:…`,
  `SourceMD author name string resolver script`). The page's `tool_hint` was
  cleared; the importer recognises Mix'n'match and SourceMD invocations, but
  other bot-page sections stay untooled and need the same edit-summary check.
- The property check for `6377` initially used `P1362`; SIKART is `P781`, and
  the wrong SIKART ID was still on the item, so the close was reverted.
- Priority split after the pass: 21 HIGH, 108 NORMAL, 89 LOW.

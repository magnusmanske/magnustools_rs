#!/usr/bin/env python3
"""Apply a triage plan to the trackers and the Buggregator database.

Plan format (JSON):

  {
    "close": {
      "2202": {"reason": "resolved; upstream fixed", "upstream": false},
      "8492": {"reason": "duplicate of #116", "upstream": "not planned"}
    },
    "priorities": {"6473": "HIGH", "6947": "LOW"}
  }

- `reason` is for the report and the rollback file, not written anywhere else.
- `upstream` is optional. For a GitHub issue it is "completed" or
  "not planned"; for Codeberg only the closed state is set. `false` leaves the
  tracker alone (already closed, or a WIKI issue). The default for a git issue
  is "completed".
- Priorities are HIGH, NORMAL or LOW.

The script is a dry run unless `--apply` is given. It is idempotent: issues
that are no longer open in Buggregator, or already closed upstream, are skipped.
"""
import argparse
import json
import os
import subprocess
import urllib.request

from common import ids as sql_ids
from common import rows, sql

VALID_PRIORITIES = {"HIGH", "NORMAL", "LOW"}
VALID_REASONS = {"completed", "not planned"}


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def close_upstream(issue, reason):
    repo, number = issue["repo"], issue["number"]
    if issue["forge"] == "GITHUB":
        command = ["gh", "issue", "close", str(number), "-R", repo, "--reason", reason]
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError((result.stderr or result.stdout).strip()[:300])
        return f"GitHub {repo}#{number} ({reason})"
    request = urllib.request.Request(
        f"https://codeberg.org/api/v1/repos/{repo}/issues/{number}",
        data=json.dumps({"state": "closed"}).encode(), method="PATCH",
        headers={"Authorization": f"token {os.environ['FORGEJO_API_TOKEN']}",
                 "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=40) as response:
        if response.status not in (200, 201):
            raise RuntimeError(f"HTTP {response.status}")
    return f"Codeberg {repo}#{number}"


def triage_user_sql():
    """Make `@triage_user` the log user for the other statements."""
    return """
INSERT INTO user (name, site) VALUES ('Magnus Manske', 'WIKI')
  ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id);
SET @triage_user = LAST_INSERT_ID();
"""


def close_sql(close_ids):
    joined = sql_ids(close_ids)
    return f"""
INSERT INTO text (hash, text) VALUES (SHA2('CLOSED', 256), 'CLOSED')
  ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id);
SET @closed_text = LAST_INSERT_ID();
INSERT INTO log (issue_id, event, text_id, user_id)
  SELECT id, 'STATUS', @closed_text, @triage_user
  FROM issue WHERE status='OPEN' AND id IN ({joined});
UPDATE issue SET status='CLOSED', date_last=CURRENT_TIMESTAMP
  WHERE status='OPEN' AND id IN ({joined});
"""


def tool_sql(by_tool):
    statements = []
    for tool_id, tool_ids in sorted(by_tool.items()):
        joined = sql_ids(tool_ids)
        statements.append(f"""
INSERT INTO text (hash, text) VALUES (SHA2('{tool_id}', 256), '{tool_id}')
  ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id);
SET @tool_text = LAST_INSERT_ID();
INSERT INTO log (issue_id, event, text_id, user_id)
  SELECT id, 'TOOL_ID', @tool_text, @triage_user
  FROM issue WHERE id IN ({joined});
UPDATE issue SET tool={tool_id} WHERE id IN ({joined});
""")
    return "".join(statements)


def priority_sql(by_level):
    statements = []
    for level, level_ids in by_level.items():
        statements.append(f"UPDATE issue SET priority='{level}' WHERE status='OPEN' "
                          f"AND id IN ({sql_ids(level_ids)});")
    return "\n".join(statements)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("plan")
    parser.add_argument("--data", default="/tmp/buggregator-triage")
    parser.add_argument("--apply", action="store_true", help="write the changes")
    args = parser.parse_args()

    plan = load(args.plan)
    close_plan = plan.get("close", {})
    priority_plan = plan.get("priorities", {})
    tool_plan = {int(i): int(t) for i, t in plan.get("tools", {}).items()}

    issues = {i["id"]: i for i in load(os.path.join(args.data, "open.json"))} if close_plan else {}
    states = load(os.path.join(args.data, "states.json")) if close_plan else {}

    open_rows = rows("SELECT id, priority, COALESCE(tool,0) FROM issue WHERE status='OPEN'")
    open_now = {int(i): priority for i, priority, _ in open_rows}
    current_tools = {int(i): int(tool) for i, _, tool in open_rows}

    unknown = (set(map(int, close_plan)) | set(map(int, priority_plan)) | set(tool_plan)) - set(open_now)
    if unknown:
        raise SystemExit(f"not open: {sorted(unknown)}")
    bad = {p for p in priority_plan.values() if p not in VALID_PRIORITIES}
    if bad:
        raise SystemExit(f"bad priorities: {sorted(bad)}")
    if tool_plan:
        known_tools = {int(i) for (i,) in rows("SELECT id FROM tool")}
        bad_tools = set(tool_plan.values()) - known_tools
        if bad_tools:
            raise SystemExit(f"bad tools: {sorted(bad_tools)}")

    close_ids = set(map(int, close_plan))
    for issue_id, entry in close_plan.items():
        upstream = entry.get("upstream", True)
        if upstream not in (True, False) and upstream not in VALID_REASONS:
            raise SystemExit(f"{issue_id}: bad upstream reason {upstream!r}")

    # Upstream first, so the daily sync cannot reopen a closed issue.
    needs_codeberg = any(
        issues[i]["site"] != "WIKI" and close_plan[str(i)].get("upstream", True) is not False
        and states.get(str(i), {}).get("state") == "open"
        and issues[i]["forge"] == "CODEBERG"
        for i in close_ids
    )
    if needs_codeberg and not os.environ.get("FORGEJO_API_TOKEN"):
        raise SystemExit("FORGEJO_API_TOKEN is required for Codeberg closures")

    for issue_id in sorted(close_ids):
        entry = close_plan[str(issue_id)]
        issue = issues[issue_id]
        upstream = entry.get("upstream", True)
        if issue["site"] == "WIKI" or upstream is False:
            continue
        state = states.get(str(issue_id), {}).get("state")
        if state != "open":
            print(f"skip  {issue_id}: upstream is {state or 'unknown'}")
            continue
        reason = upstream if isinstance(upstream, str) else "completed"
        if args.apply:
            action = close_upstream(issue, reason)
            print(f"CLOSE {issue_id}: {action}")
        else:
            print(f"DRY   {issue_id}: would close {issue['forge']} "
                  f"{issue['repo']}#{issue['number']} ({reason})")

    old_priorities = {i: open_now[i] for i in map(int, priority_plan)}
    changed = {i: p for i, p in ((int(i), p) for i, p in priority_plan.items())
               if old_priorities[i] != p}
    by_level = {}
    for issue_id, level in changed.items():
        by_level.setdefault(level, set()).add(issue_id)

    changed_tools = {i: t for i, t in tool_plan.items() if current_tools[i] != t}
    by_tool = {}
    for issue_id, tool_id in changed_tools.items():
        by_tool.setdefault(tool_id, set()).add(issue_id)

    close_ids = {i for i in close_ids if i in open_now}
    max_log = int(rows("SELECT COALESCE(MAX(id),0) FROM log")[0][0])

    changes = []
    if close_ids or by_tool:
        changes.append(triage_user_sql())
    if close_ids:
        changes.append(close_sql(close_ids))
    if by_tool:
        changes.append(tool_sql(by_tool))
    if by_level:
        changes.append(priority_sql(by_level))

    if args.apply and changes:
        sql("START TRANSACTION;\n" + "\n".join(changes) + "\nCOMMIT;\n")
        print(f"closed {len(close_ids)} in Buggregator; changed {len(changed)} priorities; "
              f"changed {len(changed_tools)} tools")
    elif not args.apply:
        print(f"dry run: would close {len(close_ids)}, change {len(changed)} priorities, "
              f"change {len(changed_tools)} tools")

    rollback = ["-- Rollback for this triage. Run after the apply step.", "",
                "START TRANSACTION;"]
    if close_ids:
        rollback.append(f"UPDATE issue SET status='OPEN' WHERE id IN ({sql_ids(close_ids)});")
        rollback.append(f"DELETE FROM log WHERE id > {max_log} AND issue_id IN ({sql_ids(close_ids)})\n"
                        "  AND event='STATUS' AND text_id=(SELECT id FROM text WHERE text='CLOSED');")
    for issue_id, level in sorted(old_priorities.items()):
        if issue_id in changed:
            rollback.append(f"UPDATE issue SET priority='{level}' WHERE id={issue_id};")
    if changed_tools:
        rollback.append(f"DELETE FROM log WHERE id > {max_log} AND event='TOOL_ID'\n"
                        f"  AND issue_id IN ({sql_ids(changed_tools)});")
    for issue_id in sorted(changed_tools):
        old = current_tools[issue_id]
        rollback.append(f"UPDATE issue SET tool={old or 'NULL'} WHERE id={issue_id};")
    rollback += ["COMMIT;", ""]
    path = os.path.join(args.data, "rollback.sql")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(rollback))
    print(f"rollback: {path}")

    if args.apply:
        for line in rows("SELECT status, COUNT(*) FROM issue GROUP BY status"):
            print("  ", *line)
        for line in rows("SELECT priority, COUNT(*) FROM issue WHERE status='OPEN' GROUP BY priority"):
            print("  ", *line)


if __name__ == "__main__":
    main()

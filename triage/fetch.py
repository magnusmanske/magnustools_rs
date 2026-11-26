#!/usr/bin/env python3
"""Fetch every open Buggregator issue with the context needed to triage it.

Writes to the output directory (default /tmp/buggregator-triage):

  open.json     the open issues, with tool, priority, tracker URL and repo
  states.json   live state of each linked GitHub/Codeberg issue
  details.json  body and comments of each issue still open upstream
  wiki.json     the current wikitext of the section behind each WIKI issue
  review.txt    a human-readable digest, grouped by site and tool/repo

Requires: the local database tunnel, an authenticated `gh`, and
FORGEJO_API_TOKEN for Codeberg. See README.md.
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from common import sql

OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/buggregator-triage"
UA = "BuggregatorTriage/1.0 (https://magnustools.toolforge.org/)"
TOKEN = os.environ.get("FORGEJO_API_TOKEN")
SLEEP = 0.15

OPEN_SQL = """
SELECT CONCAT_WS(' § ', i.id, i.site, COALESCE(gi.site,''), COALESCE(gi.git_id,0),
       COALESCE(r.html_url,''),
       REPLACE(REPLACE(REPLACE(i.url,'\\t',' '),'\\n',' '),'\\r',' '),
       REPLACE(REPLACE(REPLACE(i.label,'\\t',' '),'\\n',' '),'\\r',' '),
       COALESCE(t.name,''), i.priority,
       REPLACE(REPLACE(REPLACE(COALESCE(tx.text,''),'\\t',' '),'\\n',' '),'\\r',' '),
       COALESCE(DATE(i.date_created),''), COALESCE(DATE(i.date_last),''))
FROM issue i
LEFT JOIN git_issue gi ON gi.issue_id=i.id
LEFT JOIN git_repo r ON r.id=gi.git_repo_id
LEFT JOIN tool t ON t.id=i.tool
LEFT JOIN text tx ON tx.id=i.description_text_id
WHERE i.status='OPEN'
ORDER BY i.id
"""


def open_issues():
    issues = []
    for line in sql(OPEN_SQL).splitlines():
        p = line.split(" \u00a7 ")
        if len(p) != 12 or not p[0].isdigit():
            continue
        url = p[5]
        repo, number = tracker(url)
        issues.append(dict(
            id=int(p[0]), site=p[1], forge=p[2], git_id=p[3], git_repo=p[4],
            url=url, label=p[6], tool=p[7], priority=p[8], description=p[9],
            created=p[10], last=p[11], repo=repo, number=number,
        ))
    return issues


def tracker(url):
    """owner/repo and issue number from a tracker URL.

    The `git_repo` row can point at the repository the code moved to, so the
    issue URL is the authority on which tracker holds the issue.
    """
    parts = url.rstrip("/").split("/")
    for i, part in enumerate(parts):
        if part == "issues" and i >= 2:
            repo = "/".join(parts[i - 2:i])
            try:
                return repo, int(parts[i + 1])
            except (IndexError, ValueError):
                break
    return "", 0


def github_get(path):
    proc = subprocess.run(["gh", "api", path], capture_output=True, text=True)
    if proc.returncode:
        raise RuntimeError((proc.stderr or proc.stdout).strip()[:300])
    return json.loads(proc.stdout)


def codeberg_get(path):
    headers = {"User-Agent": UA}
    if TOKEN:
        headers["Authorization"] = f"token {TOKEN}"
    request = urllib.request.Request(f"https://codeberg.org/api/v1/{path}", headers=headers)
    with urllib.request.urlopen(request, timeout=40) as response:
        return json.load(response)


def forge_state(issue):
    try:
        if issue["forge"] == "GITHUB":
            data = github_get(f"repos/{issue['repo']}/issues/{issue['number']}")
        else:
            data = codeberg_get(f"repos/{issue['repo']}/issues/{issue['number']}")
        return dict(
            state=data.get("state"), title=data.get("title"),
            comments=data.get("comments"), updated_at=data.get("updated_at"),
            created_at=data.get("created_at"), html_url=data.get("html_url"),
            labels=[l["name"] for l in data.get("labels") or []],
        )
    except Exception as error:  # noqa: BLE001
        return {"error": str(error)[:300]}


def forge_detail(issue):
    if issue["forge"] == "GITHUB":
        data = github_get(f"repos/{issue['repo']}/issues/{issue['number']}")
        comments = github_get(f"repos/{issue['repo']}/issues/{issue['number']}/comments?per_page=100")
    else:
        data = codeberg_get(f"repos/{issue['repo']}/issues/{issue['number']}")
        comments = codeberg_get(f"repos/{issue['repo']}/issues/{issue['number']}/comments?limit=50")
    return dict(
        body=data.get("body") or "",
        comments=[dict(author=c["user"]["login"], body=c.get("body") or "", created=c.get("created_at"))
                  for c in comments],
    )


def wiki_sections(issues):
    pages = {}
    result = {}
    for issue in issues:
        if issue["site"] != "WIKI":
            continue
        split = urllib.parse.urlsplit(issue["url"])
        page = urllib.parse.unquote(split.path.split("/wiki/", 1)[-1])
        key = (split.netloc, page)
        if key not in pages:
            pages[key] = fetch_wikitext(*key)
            time.sleep(0.2)
        info = pages[key]
        if "error" in info:
            result[issue["id"]] = {"host": split.netloc, "page": page, "error": info["error"]}
            continue
        wanted = normalize(issue["label"])
        fragment = normalize(urllib.parse.unquote(split.fragment))
        section = None
        for heading, text in sections(info["wikitext"]):
            if normalize(heading) in (wanted, fragment) or (wanted and normalize(heading).startswith(wanted[:40])):
                section = {"heading": heading, "text": text[:60000]}
                break
        result[issue["id"]] = {"host": split.netloc, "page": page, "fragment": split.fragment,
                               "section": section}
    return result


def fetch_wikitext(host, page):
    query = urllib.parse.urlencode({
        "action": "parse", "page": page, "prop": "wikitext", "redirects": "1",
        "format": "json", "formatversion": "2",
    })
    request = urllib.request.Request(f"https://{host}/w/api.php?{query}",
                                     headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(request, timeout=40) as response:
            return {"wikitext": json.load(response)["parse"]["wikitext"]}
    except Exception as error:  # noqa: BLE001
        return {"error": str(error)[:300]}


HEADING = re.compile(r"^(={2,})\s*(.+?)\s*\1\s*$")


def sections(wikitext):
    found = []
    current = None
    for line in wikitext.splitlines():
        match = HEADING.match(line)
        if match:
            if current:
                found.append(current)
            current = [match.group(2).strip(), line]
        elif current:
            current[1] += "\n" + line
    if current:
        found.append(current)
    return [(heading, text) for heading, text in found]


def normalize(text):
    return re.sub(r"\s+", " ", text.replace("_", " ")).strip().lower()


def excerpt(text, limit=400):
    return re.sub(r"\s+", " ", text)[:limit]


def review(issues, states, details, wiki):
    lines = ["# Open issues", ""]
    for issue in sorted(issues, key=lambda i: (i["site"], i["repo"] or i["tool"], i["id"])):
        state = states.get(issue["id"], {}).get("state", "")
        lines.append(f"## {issue['id']} | {issue['site']} | {issue['priority']} | "
                     f"{issue['repo'] or issue['tool']} {('#' + str(issue['number'])) if issue['number'] else ''} "
                     f"{('[' + state + ']') if state else ''}")
        lines.append(f"{issue['label']}")
        lines.append(f"{issue['url']}")
        body = details.get(issue["id"], {}).get("body") or issue["description"]
        if issue["id"] in wiki:
            section = wiki[issue["id"]].get("section")
            body = section["text"] if section else "(section no longer on the page)"
        if body:
            lines.append(excerpt(body))
        comments = details.get(issue["id"], {}).get("comments") or []
        if comments:
            lines.append(f"last: {comments[-1]['author']}: {excerpt(comments[-1]['body'], 220)}")
        lines.append("")
    return "\n".join(lines)


def main():
    os.makedirs(OUT, exist_ok=True)
    issues = open_issues()
    print(f"{len(issues)} open issues")

    states = {}
    details = {}
    for issue in issues:
        if not issue["repo"] or not issue["number"]:
            continue
        state = forge_state(issue)
        states[issue["id"]] = state
        if state.get("state") == "open":
            try:
                details[issue["id"]] = forge_detail(issue)
            except Exception as error:  # noqa: BLE001
                details[issue["id"]] = {"error": str(error)[:300]}
            time.sleep(SLEEP)
        time.sleep(SLEEP)

    wiki = wiki_sections(issues)

    for name, data in [("open", issues), ("states", states), ("details", details), ("wiki", wiki)]:
        with open(os.path.join(OUT, f"{name}.json"), "w", encoding="utf-8") as f:
            json.dump(data, f, indent=1)
    with open(os.path.join(OUT, "review.txt"), "w", encoding="utf-8") as f:
        f.write(review(issues, states, details, wiki))

    open_upstream = sum(1 for s in states.values() if s.get("state") == "open")
    print(f"{len(states)} tracker issues ({open_upstream} still open upstream), "
          f"{len(wiki)} wiki issues")
    for name in ("open", "states", "details", "wiki", "review.txt"):
        print(" ", os.path.join(OUT, name))


if __name__ == "__main__":
    main()

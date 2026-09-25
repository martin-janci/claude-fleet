#!/usr/bin/env python3
"""A loopback fake of the Jira Cloud REST API, for scripts/hub-e2e.sh.

Work graph M10.2. A hub built with `--features e2e` and started with
FLEET_E2E_TRACKER_URL=http://127.0.0.1:<port> sends every tracker request
here, in plain HTTP, with the site it was meant for in `X-Fleet-E2E-Site`
(crates/fleet-core/src/net/e2e_tracker.rs). One process plays every site in
SITES, each with its own project and issues.

Only what crates/fleet-core/src/service/trackers/jira.rs calls is answered:

  GET  /rest/api/3/myself, /_edge/tenant_info, /rest/api/3/project/search,
       /rest/api/3/field, /rest/api/3/filter/favourite
  POST /rest/api/3/search/jql      (views; `updated >= -Nm` windows; paged)
  POST /rest/api/3/issue/bulkfetch (by id or key)

Every tracker call needs HTTP Basic auth; anything else is a 401, which the
adapter maps to `auth_failed`. Control endpoints (no auth, never reached by
a hub, which only ever sends /rest/... and /_edge/... paths):

  POST /_fake/issue     {"site":..., "key":..., "status": "todo|in_progress|done",
                         "resolution"?:..., "title"?:...}  -> the issue
  GET  /_fake/requests  -> [{"site","method","path"}] every tracker request seen

Usage: fake_jira.py PORT_FILE   (binds 127.0.0.1:0, writes the port, serves)
"""

import base64
import json
import os
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ME = {"accountId": "e2e-me", "displayName": "E2E Dev", "timeZone": "UTC"}
STATUS = {
    "todo": ("To Do", "new"),
    "in_progress": ("In Progress", "indeterminate"),
    "done": ("Done", "done"),
}


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%S.000+0000", time.gmtime(t))


def issue(site_id, project, n, title, status="todo", mine=True):
    return {
        "id": str(site_id * 1000 + n),
        "key": f"{project}-{n}",
        "title": title,
        "status": status,
        "resolution": None,
        "mine": mine,
        "updated": time.time() - 3600,
        "description": f"Acceptance criteria for {project}-{n}: it works.",
    }


# Two sites: two trackers, two orgs in the e2e's isolation step.
SITES = {
    "acme.atlassian.net": {
        "project": "ABC",
        "issues": [
            issue(1, "ABC", 1, "Fix the login redirect"),
            issue(1, "ABC", 2, "Add a health endpoint"),
            issue(1, "ABC", 3, "Cache the ticket card", status="in_progress"),
            issue(1, "ABC", 4, "Retire the old importer"),
            issue(1, "ABC", 5, "Nobody's story", mine=False),
        ],
    },
    "beta.atlassian.net": {
        "project": "XYZ",
        "issues": [
            issue(2, "XYZ", 1, "Beta org secret work"),
            issue(2, "XYZ", 2, "Another beta story"),
        ],
    },
}
LOCK = threading.Lock()
SEEN = []


def as_jira(site, it):
    name, cat = STATUS[it["status"]]
    return {
        "id": it["id"],
        "key": it["key"],
        "fields": {
            "summary": it["title"],
            "status": {"name": name, "statusCategory": {"key": cat}},
            "resolution": {"name": it["resolution"]} if it["resolution"] else None,
            "issuetype": {"name": "Story", "hierarchyLevel": 0},
            "parent": None,
            "assignee": (
                {"displayName": ME["displayName"], "accountId": ME["accountId"]}
                if it["mine"]
                else None
            ),
            "updated": iso(it["updated"]),
            "project": {"key": site["project"]},
            "description": it["description"],
        },
    }


def view_filter(jql):
    """The adapter's built-in views, closely enough for a fake."""
    q = jql.lower()
    if "sprint in opensprints()" in q:
        return lambda it: False
    since = None
    m = re.search(r"updated >= -(\d+)m", q)
    if m:
        since = time.time() - int(m.group(1)) * 60
    mine = "currentuser()" in q
    not_done = "statuscategory != done" in q

    def keep(it):
        if mine and not it["mine"]:
            return False
        if not_done and it["status"] == "done":
            return False
        if since is not None and it["updated"] < since:
            return False
        return True

    return keep


class Handler(BaseHTTPRequestHandler):
    server_version = "fake-jira/1"

    def log_message(self, fmt, *args):  # quiet: the e2e prints its own lines
        sys.stderr.write("fake-jira: " + (fmt % args) + "\n")

    def reply(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(raw)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}") if n else {}

    def tracker_site(self):
        """The site this request is for, or None after answering an error."""
        site_name = self.headers.get("X-Fleet-E2E-Site", "")
        with LOCK:
            SEEN.append({"site": site_name, "method": self.command, "path": self.path})
        site = SITES.get(site_name)
        if site is None:
            self.reply(404, {"errorMessages": [f"no fake site {site_name!r}"]})
            return None
        auth = self.headers.get("Authorization", "")
        ok = False
        if auth.startswith("Basic "):
            try:
                user, _, secret = base64.b64decode(auth[6:]).decode().partition(":")
                ok = bool(user) and bool(secret)
            except Exception:  # noqa: BLE001 - any garbage is a 401
                ok = False
        if not ok:
            self.reply(401, {"errorMessages": ["unauthorized"]})
            return None
        return site

    def do_GET(self):
        if self.path == "/_fake/requests":
            with LOCK:
                return self.reply(200, list(SEEN))
        site = self.tracker_site()
        if site is None:
            return
        path = self.path.split("?", 1)[0]
        if path == "/rest/api/3/myself":
            return self.reply(200, ME)
        if path == "/_edge/tenant_info":
            return self.reply(200, {"cloudId": "cloud-" + site["project"].lower()})
        if path == "/rest/api/3/project/search":
            return self.reply(200, {"values": [{"key": site["project"]}], "isLast": True})
        if path in ("/rest/api/3/field", "/rest/api/3/filter/favourite"):
            return self.reply(200, [])
        self.reply(404, {"errorMessages": [f"not faked: GET {path}"]})

    def do_POST(self):
        if self.path == "/_fake/issue":
            return self.fake_issue()
        site = self.tracker_site()
        if site is None:
            return
        body = self.body()
        with LOCK:
            if self.path == "/rest/api/3/search/jql":
                keep = view_filter(body.get("jql", ""))
                hits = sorted(
                    (it for it in site["issues"] if keep(it)),
                    key=lambda it: -it["updated"],
                )
                start = int(body.get("nextPageToken") or 0)
                size = int(body.get("maxResults") or 50)
                page = hits[start : start + size]
                nxt = start + len(page)
                return self.reply(
                    200,
                    {
                        "issues": [as_jira(site, it) for it in page],
                        "isLast": nxt >= len(hits),
                        "nextPageToken": str(nxt),
                    },
                )
            if self.path == "/rest/api/3/issue/bulkfetch":
                asked = {str(r).upper() for r in body.get("issueIdsOrKeys", [])}
                found = [
                    as_jira(site, it)
                    for it in site["issues"]
                    if it["id"] in asked or it["key"] in asked
                ]
                return self.reply(200, {"issues": found})
        self.reply(404, {"errorMessages": [f"not faked: POST {self.path}"]})

    def fake_issue(self):
        b = self.body()
        site = SITES.get(b.get("site", ""))
        with LOCK:
            it = site and next((i for i in site["issues"] if i["key"] == b.get("key")), None)
            if not it:
                return self.reply(404, {"error": "no such issue"})
            if "status" in b:
                if b["status"] not in STATUS:
                    return self.reply(400, {"error": "status is todo|in_progress|done"})
                it["status"] = b["status"]
            if "resolution" in b:
                it["resolution"] = b["resolution"]
            if "title" in b:
                it["title"] = b["title"]
            it["updated"] = time.time()
            return self.reply(200, as_jira(site, it))


def main():
    port_file = sys.argv[1]
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    with open(port_file + ".tmp", "w") as f:
        f.write(str(srv.server_address[1]))
    # Renamed into place so a reader never sees a half-written port.
    os.replace(port_file + ".tmp", port_file)
    srv.serve_forever()


if __name__ == "__main__":
    main()

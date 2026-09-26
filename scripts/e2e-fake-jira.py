#!/usr/bin/env python3
"""A fake Jira Cloud for scripts/hub-e2e.sh's work-graph leg (work graph M10.2).

Stdlib only. Listens on 127.0.0.1 (never anything else), plain HTTP: the hub
under test reaches it only through the test-only `e2e` cargo feature's
loopback override (FLEET_E2E_TRACKER_PORT), which keeps the Jira Cloud host
fence and the https:// URL and changes only where the bytes go.

It serves the Jira-Cloud-shaped JSON the provider reads
(crates/fleet-core/src/service/trackers/jira.rs):

  GET  /rest/api/3/myself                    who am I (the probe)
  GET  /_edge/tenant_info                    the cloud id
  GET  /rest/api/3/project/search            key prefixes (one project: E2E)
  GET  /rest/api/3/field                     no sprint field
  GET  /rest/api/3/filter/favourite          no favourite filters
  POST /rest/api/3/search/jql                search (JQL; the views)
  POST /rest/api/3/issue/bulkfetch           by id or key (linked items)
  GET  /rest/api/3/issue/<key>               one issue
  GET  /rest/api/3/issue/<key>/changelog     its status history

and a control surface for the script, not Jira's:

  POST /_e2e/status   {"key": "E2E-4", "status": "Done"}   move a ticket
  GET  /_e2e/log                                            request lines seen

Every Jira route needs an Authorization header (401 without one), so a sync
that forgot the credential fails here as it would on a real site. The JQL is
not parsed: `statusCategory != Done` hides done tickets, everything else is
answered with every ticket (the provider dedupes, and a relative `updated >=`
window over a handful of tickets changes nothing).
"""

import argparse
import json
import os
import sys
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ME = {
    "accountId": "557058:e2e00000-0000-0000-0000-000000000001",
    "displayName": "Erin Twoe",
    "timeZone": "UTC",
    "active": True,
}
CATEGORY = {"To Do": "new", "In Progress": "indeterminate", "Done": "done"}
TICKETS = [
    ("10001", "E2E-1", "Fix the login redirect"),
    ("10002", "E2E-2", "Add CSV export"),
    ("10003", "E2E-3", "Refactor the ledger"),
    ("10004", "E2E-4", "Tidy the clean session"),
    ("10005", "E2E-5", "Tidy the dirty session"),
]
LOCK = threading.Lock()
STATE = {}  # key -> {"id", "key", "summary", "status", "updated", "history"}
LOG = []


def now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000+0000")


def seed():
    t = now_iso()
    for iid, key, summary in TICKETS:
        STATE[key] = {
            "id": iid,
            "key": key,
            "summary": summary,
            "status": "To Do",
            "updated": t,
            "history": [],
        }


def issue_json(t):
    status = t["status"]
    return {
        "id": t["id"],
        "key": t["key"],
        "fields": {
            "summary": t["summary"],
            "status": {"name": status, "statusCategory": {"key": CATEGORY[status]}},
            "resolution": {"name": "Done"} if status == "Done" else None,
            "issuetype": {"name": "Story", "hierarchyLevel": 0},
            "assignee": {"accountId": ME["accountId"], "displayName": ME["displayName"]},
            "updated": t["updated"],
            "project": {"key": "E2E"},
            "description": {
                "type": "doc",
                "version": 1,
                "content": [
                    {
                        "type": "paragraph",
                        "content": [
                            {"type": "text", "text": f"Acceptance: {t['summary']} works end to end."}
                        ],
                    }
                ],
            },
        },
    }


def find(ref):
    ref = str(ref).upper()
    for t in STATE.values():
        if t["key"] == ref or t["id"] == ref:
            return t
    return None


class Handler(BaseHTTPRequestHandler):
    server_version = "e2e-fake-jira/1"
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quiet: the script reads /_e2e/log
        pass

    def reply(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            return {}

    def route(self, method):
        path = self.path.split("?", 1)[0]
        with LOCK:
            LOG.append(f"{method} {path}")
        if path.startswith("/_e2e/"):
            return self.control(method, path)
        if not self.headers.get("Authorization"):
            return self.reply(401, {"errorMessages": ["no credential"]})
        with LOCK:
            if method == "GET" and path == "/rest/api/3/myself":
                return self.reply(200, ME)
            if method == "GET" and path == "/_edge/tenant_info":
                return self.reply(200, {"cloudId": "e2e00000-1111-2222-3333-444444444444"})
            if method == "GET" and path == "/rest/api/3/project/search":
                return self.reply(200, {"isLast": True, "values": [{"id": "10000", "key": "E2E"}]})
            if method == "GET" and path == "/rest/api/3/field":
                return self.reply(200, [])
            if method == "GET" and path == "/rest/api/3/filter/favourite":
                return self.reply(200, [])
            if method == "POST" and path == "/rest/api/3/search/jql":
                jql = str(self.body().get("jql", ""))
                hide_done = "statusCategory != Done" in jql
                issues = [
                    issue_json(t)
                    for t in STATE.values()
                    if not (hide_done and t["status"] == "Done")
                ]
                return self.reply(200, {"issues": issues, "isLast": True})
            if method == "POST" and path == "/rest/api/3/issue/bulkfetch":
                refs = self.body().get("issueIdsOrKeys", [])
                found = [issue_json(t) for t in (find(r) for r in refs) if t]
                return self.reply(200, {"issues": found})
            parts = path.strip("/").split("/")
            # rest/api/3/issue/<key>[/changelog]
            if method == "GET" and parts[:4] == ["rest", "api", "3", "issue"] and len(parts) in (5, 6):
                t = find(parts[4])
                if not t:
                    return self.reply(404, {"errorMessages": ["Issue does not exist"]})
                if len(parts) == 5:
                    return self.reply(200, issue_json(t))
                if parts[5] == "changelog":
                    return self.reply(200, {"isLast": True, "values": t["history"]})
        return self.reply(404, {"errorMessages": [f"no fake route for {method} {path}"]})

    def control(self, method, path):
        with LOCK:
            if method == "GET" and path == "/_e2e/log":
                return self.reply(200, LOG)
            if method == "POST" and path == "/_e2e/status":
                b = self.body()
                t = find(b.get("key", ""))
                status = b.get("status")
                if not t or status not in CATEGORY:
                    return self.reply(400, {"error": "need a known key and a status of " + ", ".join(CATEGORY)})
                if t["status"] != status:
                    at = now_iso()
                    t["history"].append(
                        {
                            "created": at,
                            "items": [{"field": "status", "fromString": t["status"], "toString": status}],
                        }
                    )
                    t["status"] = status
                    t["updated"] = at
                return self.reply(200, issue_json(t))
        return self.reply(404, {"error": "no such control route"})

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port-file", required=True, help="where to write the port it listens on")
    args = ap.parse_args()
    seed()
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    port = srv.server_address[1]
    with open(args.port_file + ".tmp", "w") as f:
        f.write(f"{port}\n")
    # Renamed into place, so a reader never sees a half-written port.

    os.replace(args.port_file + ".tmp", args.port_file)
    print(f"e2e-fake-jira: listening on 127.0.0.1:{port}", file=sys.stderr, flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()

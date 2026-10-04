# File downloads: a file made on a host, on your phone and desktop

Status: building (2026-10-03). Owner's answers: Claude sends a file AND a
person picks one; the copy lives on the hub; 100 MB per file, 7 days;
hub, desktop and fleet-mobile in one go.

## Problem

A Claude session on a remote host makes a file the user asked for (a PDF,
a CSV, a build, an image). The host is often one the user cannot reach
directly — it sits behind the hub. Today the only way to see the file is
`repo_file`, which returns text up to 512 KiB and nothing for a binary.

## Shape

1. **Ask.** `send_file { session_id, path, note? }` — Claude calls it with
   its own session id (from `whoami`) when it has made something the user
   should have; a person calls the same tool from the desktop or the phone
   (a file's *Send to downloads*). `path` is absolute, or relative to the
   session's worktree root (the git top level of the pane's directory, as
   `repo_file` reads it; the pane's directory outside git).
2. **Copy.** The hub `stat`s the file over SSH (or the agent), refuses what
   breaks the budget, inserts a `downloads` row in state `fetching`, and
   returns it at once. A background task pulls the bytes in 8 MiB chunks
   (the move carry's `chunk_script` / `payload`) into
   `<data dir>/downloads/<id>`, hashing as it writes, and flips the row to
   `ready` (or `failed` with the reason). Every change emits
   `download:changed { id }`.
3. **Fetch.** A client lists rows (`list_downloads`) and pulls the bytes
   with `GET /downloads/<id>` — a plain HTTP body, not a tool result, so
   neither the 2 MB axum default nor the phone's 8 MiB RPC cap applies.
4. **Forget.** The GC sweep deletes rows (and their files) older than
   `downloads.keep_secs`; a new file that would pass
   `downloads.max_total_mb` evicts the oldest ready files first. A person
   removes one with `remove_download { id }`.

On a standalone desktop (no hub) the same service runs in-process: the
desktop's own data dir holds the copy and *Save* copies it out.

## Who sees what

| caller | `send_file` | `list_downloads` / `GET` / `remove_download` |
|---|---|---|
| master, unbound client (`full`) | any visible session | every row |
| client bound to an org | sessions of its org | rows whose session's org is its org |
| per-host token (a host's Claude) | **its own host's** sessions only | rows of its own host |
| `readonly` client | refused | list and GET only |

A host's Claude can already read every file on its host, so sending one of
them to its owner widens nothing; it can never pull a file from another
host. `download:changed` is in `HOST_BOUND_HIDDEN_KINDS`: a scoped stream
never receives it, and a scoped client re-reads `list_downloads`.

## Wire (contract revision 7)

### `send_file` (Access::Client, write)

```json
{ "session_id": 12, "path": "out/report.pdf", "note": "the Q3 report" }
```

Answers the row (below) in state `fetching`. Errors: `E_NOTFOUND` (no such
session / file is not a regular readable file), `E_INVALID` (empty path,
NUL, a directory), `E_LIMIT` (over `max_file_mb`, or over
`max_total_mb` even after evicting every ready file).

### `list_downloads` (Access::Client, read)

```json
{ "session_id": 12, "limit": 100 }   // both optional
```

```json
{ "downloads": [Download…], "total_bytes": 123, "max_total_bytes": 2147483648,
  "max_file_bytes": 104857600 }
```

Newest first.

### `remove_download` (Access::Client, write)

`{ "id": 7 }` → `{ "removed": true }` (false when it was already gone).

### `Download`

```json
{
  "id": 7,
  "at": 1790000000,                // asked (unix seconds)
  "host_alias": "gpu-1",
  "session_id": 12,                // null once the session row is gone
  "session_name": "fleet-report",  // tmux name, kept after the session
  "path": "/home/u/proj/out/report.pdf",
  "name": "report.pdf",
  "size": 48213,
  "state": "fetching" | "ready" | "failed",
  "error": "…",                    // failed only
  "sha256": "…",                   // ready only
  "source": "agent" | "person",
  "note": "the Q3 report",
  "ready_at": 1790000004,
  "downloaded_at": 1790000100,     // last GET, null if never
  "expires_at": 1790604804         // null when kept forever / not ready
}
```

Every field but `id`, `at`, `host_alias`, `path`, `name`, `size`, `state`,
`source` may be absent.

### `GET /downloads/<id>`

Behind `authorize` like `/events`. `200` with the bytes,
`Content-Type` from the extension (`application/octet-stream` otherwise),
`Content-Length`, `Content-Disposition: attachment; filename="<ascii>";
filename*=UTF-8''<percent-encoded name>`, `X-Fleet-Sha256`. `404` when the
row is not visible to the caller, not `ready`, or gone. Stamps
`downloaded_at`.

### Event

`download:changed` `{ "id": 7 }` — kind `download`. Ids only: re-read.

## Settings

| key | default | |
|---|---|---|
| `downloads.max_file_mb` | 100 | one file's ceiling (1…4096) |
| `downloads.max_total_mb` | 2048 | everything kept together (1…1048576) |
| `downloads.keep_secs` | 604800 (7 days) | `0` keeps them until removed |

Home: Settings → Limits, section *Downloads*.

## Desktop

Commands `send_file`, `list_downloads`, `remove_download` (routed to the
hub tools of the same name when paired), `save_download { id, dest }`
(standalone: copy out of the data dir; paired: `GET /downloads/<id>` from
the hub, streamed to `dest`). UI: a *Downloads* sheet (footer button with a
count of unseen ready files), *Send to downloads* in the file viewer, and
a toast when a file becomes ready.

## Phone (fleet-mobile)

A *Files* screen listing `list_downloads`, refreshed on
`download:changed` and on open. A row downloads through `GET
/downloads/<id>` streamed to the app's cache, then **Save** (Android:
MediaStore `Downloads`; iOS: the share sheet's *Save to Files*) or
**Share** / **Open**. A session's file browser (where it has one) offers
*Send to downloads*.

## Not in this cut

Folders (zip them first: Claude can), resumable range requests, sending a
file *to* a host from the phone (the desktop's attachments already do),
detecting written files automatically from hooks (too noisy; the owner
chose explicit sending).

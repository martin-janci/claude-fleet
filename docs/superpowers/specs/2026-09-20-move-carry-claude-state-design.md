# Move carry, slice 2: the Claude-side state travels too

**Date:** 2026-09-20
**Status:** Implemented
**Builds on:** `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`
(slice 1, landed in PR #163) and `docs/adr/0002-move-carries-work-as-is.md`
**Slice:** 2 of 3. Slice 3 (Transfer button, preflight sheet, progress,
wait-for-idle, retry / return trip) builds on the report fields added here.

## Goal

After a move the resumed session is as complete on the target as it was on
the source: its subagents' transcripts, its out-of-line tool results and its
title arrive with it, and what it learned — the project's Claude memory — is
available there, without ever overwriting anything the target already has.

Today a move carries the main transcript (`<id>.jsonl`) and nothing else of
Claude's state, so on the target subagents cannot be continued, the fleet's
conversation view has no agent rows, and the session has none of the
project's memory unless that host happened to build its own.

## What the state is (verified on macOS and on a Linux fleet host)

Under `~/.claude/projects/`:

```
<encoded cwd>/                       # keyed by the session's cwd — the WORKTREE
  <id>.jsonl                         # main transcript        (carried since ADR 0001)
  <id>/                              # per-session directory  (NOT carried today)
    subagents/agent-<hex>.jsonl      #   one transcript per subagent, append-only
    subagents/agent-<hex>.meta.json
    tool-results/<name>.txt          #   large tool outputs kept out of line
    custom-title.json
    workflows/…                      #   sometimes
<encoded REPO ROOT>/                 # keyed by the main checkout, not the worktree
  memory/MEMORY.md                   # index, one line per memory
  memory/<slug>.md                   # one fact per file
```

Two facts shape the design:

- **The per-session directory is often bigger than the transcript**
  (observed: 16 MB of session directory, 15 MB of it subagents, next to a
  4.4 MB transcript).
- **Memory is per repo per host, shared by every worktree and session of
  that repo there** (observed: 14 memory dirs on a fleet host, none under a
  worktree-keyed project dir). A move therefore *merges into* a directory the
  target may already have filled on its own; it never simply places files.

Every encoded directory name **begins with `-`**. Each path operand in every
script is therefore passed after `--` or with a `./` prefix; a bare
`ls "$d"` reads the name as options.

## One new step, and it can only warn

The step runs after the ignored files and before the transcript `put` —
before the target tmux session starts, like every carry step. It has two
independent halves, **A. session directory** and **B. project memory**. A
failure in either becomes a report warning (`"session state was not carried:
…"`, `"project memory was not carried: …"`) and leaves the other half and the
move untouched: the main transcript is all `--resume` needs, so nothing here
may abort a move. Both halves use the slice-1 machinery unchanged: scripts
under `bash -lc` with the `__CF_OUT__` marker, the id and `$HOME` guards,
the chunked relay through a `0600` temp file, the transfer dir
`~/.cache/claude-fleet/transfer/<id>/`, and its cleanup.

### A. Session directory

Source directory: the parent of the located transcript, plus `<id>/`.
Target directory: the parent of the prep script's transcript path, plus
`<id>/`. Absent on the source → nothing to do, no warning.

1. **List (source):** every regular file under `<id>/` as
   `<bytes>\t<relative path>\0` (`find … -type f`, sized with `wc -c` — byte
   exact, unlike `du -k`, which reports allocated blocks). Symlinks and
   special files are not listed and do not travel.
2. **Select (Rust, pure):** paths outside `[A-Za-z0-9._/-]`, or containing
   `..`, are left behind (`unsupported_name`). If the total is within
   `move.max_session_state_mb` (default **200**, 1–4096) everything
   travels. Otherwise the **largest** files are excluded one by one until
   the rest fits; more than 200 exclusions → the half is skipped with a
   warning. Excluded files are reported as left behind (`over_cap`).
3. **Pack (source):** one `tar -czf` of **the whole `./<id>`** from the
   project dir **minus** the excludes — not "the selected files". Each
   excluded path contributes both member-name spellings
   (`--exclude=./<id>/<p>` and `--exclude=<id>/<p>`) so GNU and BSD tar
   agree. A path with a character outside the safe charset is excluded by a
   pattern in which every such character becomes the `[!/]` token, a bracket
   expression matching exactly one NON-slash character; a plain `?` is not
   safe, since both tars' `fnmatch` let it match `/` too and a pattern built
   for one odd file could then reach across a directory boundary. The common
   case has no excludes, so argv stays tiny; the bound of 200 keeps it small
   in the worst case.
4. **Relay**, then **extract into a staging dir** inside the target's
   transfer dir — never in place.
5. **Merge (target):** for each staged regular file, move it into place iff
   the target has no such file or a **strictly smaller** one. Equal or
   larger on the target → kept, reported as `kept_target`. Directories are
   created `0700`, files keep the `0600` they were packed with. The script
   prints one `carried\t<bytes>\t<path>`, `kept\t<path>` or
   `failed\t<path>` line per file after the marker.

   The merge **treats every file as append-only**. That is exact for the
   transcripts (`subagents/*.jsonl`), which is the rule the main transcript
   already follows ("a larger existing target transcript may hold turns
   taken there"). It is an approximation for the small files Claude Code
   REWRITES rather than appends to — `custom-title.json`, `*.meta.json`,
   `workflows/…`: on a return trip, such a file changed on B to an equal or
   smaller size is `kept` on A, so A keeps its stale copy. That is the
   deliberate price of one rule that can never lose data; a per-file-type
   policy belongs to slice 3, with the return trip.

   Because the pack is "the whole `./<id>` minus the excludes", a path the
   LISTING dropped (a TAB or newline in the name, a non-UTF-8 name) still
   reaches the staging dir, as does a file created after the listing. The
   merge therefore re-checks the charset itself and never moves such a file,
   reporting it `failed\t(unsupported name)` **without** its raw name — a
   newline in a name would otherwise forge an extra report line. The flow
   then reconciles: every path the selection chose must come back in
   `carried ∪ kept ∪ failed`, or the half warns.

### B. Project memory

Source directory: `~/.claude/projects/<enc(repo root)>/memory`, where the
repo root is the parent of `git rev-parse --path-format=absolute
--git-common-dir` in the source worktree, physical (`pwd -P`), encoded as
Claude Code does (`[^A-Za-z0-9]` → `-`); if that has no `memory/`, the
worktree's own encoded dir is tried. Target directory:
`~/.claude/projects/<enc(pwd -P of the target project root)>/memory`. No
memory on the source → nothing to do, no warning.

1. **List both sides:** `<git hash-object --no-filters>\t<bytes>\t<name>\0`
   for every regular `*.md` directly in `memory/` (the directory is flat;
   subdirectories are ignored). `git hash-object` gives one content hash on
   every host without depending on `sha256sum` vs `shasum`.
2. **Decide (Rust, pure):** names outside `[A-Za-z0-9._-]+\.md` are skipped
   (`unsupported_name`). For every other source file except `MEMORY.md`:
   - not on the target → **carry**, within fixed bounds of 1 MiB per file,
     8 MiB and 300 files in total (constants, not settings: memory is small
     text, and hitting a bound is a warning, not a tuning problem);
   - on the target with the same hash → `identical`;
   - on the target with another hash → `kept_target`. The target's file is
     never overwritten.
3. **Carry the files:** pack the chosen names, relay, extract in the target
   memory dir (created `0700` if absent) with the keep-existing extraction —
   so even a file that appeared on the target since the listing survives.
   This is the one extract that does not stage into a scratch directory
   first: it writes straight into the user's own notes, so it does not trust
   the archive the way slice 1's generic extract does. Every member must be
   `<name>` or `./<name>` with `<name>` matching `[A-Za-z0-9._-]+\.md`, must
   not be `MEMORY.md` in any ASCII case, and must be a regular file; one bad
   member refuses the whole archive with nothing extracted.
4. **Merge the index:** the source's and the target's `MEMORY.md` are read
   (each at most 256 KiB; larger → the index is left alone, with a
   warning). A source line is appended to the target's index iff its first
   markdown link target (`](name.md)`) names a file **carried in this move**.
   Lines of files the target already had, lines without a link, and headings
   do not travel: an index line describes a file, and the target's own line
   already describes the target's own version. The target's existing lines
   are never rewritten or reordered; with no `MEMORY.md` on the target one
   is created as `# Memory Index` + a blank line + the appended lines.
   Appended text is bounded at 32 KiB and reaches the script as a quoted
   heredoc; text with a line equal to the heredoc delimiter, or containing a
   NUL (which bash silently discards while reading a script, so such a line
   would still close the heredoc), is refused outright. The missing trailing
   newline before the appended text is added by the append **script**, not
   by the pure `merge_index`: only the script can see the real file's last
   byte, since the read `merge_index` gets is bounded and can be empty for a
   file that exists but could not be read.

Host-specific notes travel like any other (a note about a macOS-only path
arrives on a Linux host). The move does not try to judge portability;
keep-existing guarantees it can only add, never replace.

## Report, API, docs

`CarryReport` (slice 1) gains two fields. Every new type derives
`Serialize + Deserialize` with **no** `#[serde(default)]`: the report is read
back from a hub in remote mode, under the wire rule in
`service::repo_read` — the rule whose absence broke slice 1's merge.

```rust
pub struct CarryReport {
    // … slice-1 fields …
    pub session_state: SessionStateReport,
    pub memory: MemoryReport,
}
pub struct SessionStateReport {
    pub carried: Vec<IgnoredEntry>,       // { path, bytes } — merged into place
    pub kept_target: Vec<String>,         // target had an equal or larger copy
    pub left_behind: Vec<LeftBehind>,     // over_cap | unsupported_name
}
pub struct MemoryReport {
    pub carried: Vec<IgnoredEntry>,
    pub kept_target: Vec<String>,         // same name on both hosts, contents differ
    pub identical: u32,
    pub index_lines_added: u32,
    pub left_behind: Vec<LeftBehind>,
}
```

**Mixed versions: upgrade the hub before the desktops.** The report gained
two REQUIRED wire fields, so a NEW desktop routed through an OLD hub gets
`E_PARSE` when it reads the reply — and it gets it *after* the hub has
already completed the move, so the move happened and only its report is
lost. An old desktop talking to a new hub is fine (it ignores fields it does
not know). This is the `repo_read` wire rule working as designed, exactly as
in slice 1; it is stated here because the ordering it implies is not
otherwise written down anywhere.

No new move argument, so the hub route's hand-built argument JSON
(`src-tauri/src/backend/remote.rs`) is unchanged; the routed report payload
in `src-tauri/src/backend/tests_routing.rs` gains the two fields. The TS
mirror in `src/lib/moveSession.ts` follows field for field. The
`move_session` tool description says what now travels (→ regenerate
`docs/control-api-reference.md`); ADR 0002 gains a "what travels" paragraph
listing the Claude-side state and the two merge rules. New setting
`move.max_session_state_mb` gets its spec entry, its mirrored `*_MAX`
constant and its Settings row (the parity test requires the row).

## Errors

None new. Both halves only warn. The id / `$HOME` guards and the marker
contract are slice 1's; a script that fails prints the `__CF_CARRY_FAILED__`
sentinel and exits non-zero, which the flow turns into the half's warning.

## Testing

1. **Pure policy (Rust):** session selection — under the cap everything
   travels; over it the largest files go first; the 200-exclusion bound;
   charset and `..` rejection. Memory decision — carry / identical /
   kept_target, the three bounds, `MEMORY.md` never in the carry set. Index
   merge — a line travels only with its carried file; lines of existing
   files, link-less lines and headings stay; first link wins on a line with
   several; target index with and without a trailing newline; no target
   index; the 32 KiB bound.
2. **Real filesystem, real tar, real bash** (both CI runners, so BSD and GNU
   userland): a session dir with `subagents/`, `tool-results/`, a title and
   a nested `workflows/` file → pack with and without `--exclude` → staging
   → merge over a target that has a smaller, an equal and a larger copy and
   one file of its own; modes `0700`/`0600`; project dir names that begin
   with `-`. Memory: keep-existing over a differing target file; the index
   append on real files; a target with no memory dir.
3. **`FakeSsh` flow:** the step's reports are populated on the happy path;
   each half failing (list, pack, relay, extract/merge) is a warning and the
   move still succeeds; a source with neither directory sends no pack
   script; cleanup still runs; the routed payload round-trips.
4. **`carry_e2e` (cross-host harness):** the source gets a fake session dir
   and a memory dir, the target a pre-existing memory file and index; the
   run asserts the merged result on the real target. Re-run macOS → Linux
   before the PR.

## Out of scope

- **Stale absolute paths inside the transcript.** It refers to
  `tool-results` files by the source's absolute path. The files travel (the
  UI and subagents use them); transcript content is never rewritten.
- Memory sync outside a move (that stays `claude-handoff`), two-way merging,
  deletion propagation, and any judgement about which notes are portable.
- `~/.claude` state that is not per-project: settings, skills, credentials,
  todos, shell snapshots.
- Everything slice 3 owns, including showing these report fields.

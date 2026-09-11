# ADR 0001: Descope Freeze, replace Handoff with `move_session`

- Status: accepted
- Date: 2026-09-11
- Finding: PROD-8 (plan `docs/plans/2026-09-10-fleet-improvement-plan.md`, Wave 5 G2)
- Supersedes: design spec `docs/specs/2026-05-19-claude-fleet-design.md` §8.3 (Handoff) and §8.4 (Freeze)

## Context

The original spec defined two session-transfer features. Neither was built:

- **Handoff (§8.3)**: a "Send to…" dialog with three modes. *Live mirror*
  attached a second client to the source tmux. *Scrollback snapshot* stored
  `tmux capture-pane` output in `sessions.frozen_scrollback` and replayed it
  on the destination before `cl --continue`. *State only* ran
  `claude-handoff push` and then `cl --continue`. Every transfer appended a
  row to `handoffs`.
- **Freeze (§8.4)**: capture the scrollback into `frozen_scrollback`, show a
  snowflake in the sidebar, and open the capture read-only on attach.

The schema for both shipped in migration 001 (`handoffs`, `frozen_scrollback`),
but no code ever wrote or read either. Meanwhile the fleet changed under
them:

- Every work session gets an app-minted `claude_session_id`, and recreate and
  restart resume that exact conversation with `cl --resume <id>`. The
  conversation now survives the process. Freeze was a workaround for losing
  it.
- `capture_session`, `session_history` and `session_transcript` already give
  you the screen, the timeline and the reply text of any session, live or
  ghost. A frozen read-only buffer adds nothing.
- `cl --continue` resumes "the most recent conversation in this cwd". On a
  host that runs several sessions in one project it picks the wrong one, so
  the spec's Handoff modes would have resumed the wrong conversation.
  Resuming by id needs the transcript file itself on the destination host.

## Decision

1. **Freeze is descoped.** It will not be built. "Keep this for later" is
   covered by leaving the session alone (resumable by id) or by recreating
   it. "Show me what it said" is covered by the transcript and capture
   tools.
2. **Handoff is replaced by `move_session`**, one operation available as a
   service, an MCP tool (`move_session`, confirm-gated) and the "Move to
   host…" action in session details:
   - Preflight. The source must be a work session with a Claude session id
     and a worktree branch, and both hosts must be reachable. The target must
     be provisioned. The worktree must be clean (`E_MOVE_DIRTY`) and the
     branch pushed with nothing unpushed (`E_MOVE_UNPUSHED`). The move never
     pushes, stashes or carries uncommitted work.
   - The transcript JSONL is copied through the desktop: it is read over ssh
     and written with `upload_file` stdin to
     `~/.claude/projects/<encoded target cwd>/<id>.jsonl` on the target. Copies
     are capped by `move.max_transcript_mb` (default 200, `E_MOVE_TOO_LARGE`).
   - The target worktree is created through the create-only automatic repair
     path (`repair::ensure_for_new_session`), from the same branch, and
     fast-forwarded to the source HEAD. The session then starts with the
     recreate pane command (`cl --resume <id>`).
   - Nothing on the source changes until the target is confirmed running with
     its transcript in place. After that, both rows get a `session_moved`
     event, and the new row's `parent_session_id` points at the source. The
     source is then killed through the normal kill path, unless
     `keep_source` is set.
   - If a step fails after the target session started, the move returns
     `E_MOVE_PARTIAL` and leaves both sessions running.
   - The MCP tool requires a caller allowed on both hosts, so per-host tokens
     cannot move sessions. In practice that means the master token.
3. **Live mirror is dropped.** Several clients can already attach to one tmux
   session, and the app attaches exactly one PTY at a time by design.
4. Migration 022 drops the dead `handoffs` table and the
   `sessions.frozen_scrollback` column. Migration 001 no longer creates
   `handoffs`, because it re-runs on every launch.

## Consequences

- Moving a session is copy-then-start, never live. Turns the source takes
  after the transcript copy stay on the source. The source keeps running
  until the target is confirmed, and `keep_source` makes that explicit.
- A move needs the branch on origin. Local-only work has to be pushed first,
  on purpose: the move never makes a git decision for the user.
- Claude Code shortens encoded project directories longer than 200
  characters with a hash that cannot be reproduced. A target cwd that long is
  refused before anything is copied.
- The scrollback-replay idea is gone. Anyone who needs the old screen can
  still get it from `capture_session` on the source before it is killed.

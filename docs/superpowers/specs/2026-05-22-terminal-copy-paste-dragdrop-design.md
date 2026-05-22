# Terminal copy / paste / drag-drop

Date: 2026-05-22
Status: Approved — ready for implementation plan

## Goal

Make the hand-rolled terminal (`TerminalView.svelte` + `ansi.ts`) support the
clipboard and file workflows people expect from a real terminal, with the
primary use being **working with Claude Code**:

1. **Copy** text out of the terminal (e.g. Claude's output) — auto-copy on
   selection.
2. **Paste** text into the terminal (e.g. into Claude's prompt) — Cmd+V, using
   bracketed paste when the remote app supports it.
3. **Drag-and-drop** files/images onto the terminal — upload to the session's
   host and paste the resulting remote path.

Non-goals: scrollback selection, inline image rendering (Sixel/iTerm protocol),
clipboard-image paste into remote Claude, and cleanup of uploaded files.

## Context / current state

- Input path: keyboard → `keyToBytes()` → `invoke('pty_write', { args: { data }})`
  → `pty.rs` writer → PTY master fd → tmux/shell.
- Output path: PTY reader thread → buffer → `pty_drain` poll → `screen.write()`
  → render. Rows render as real text `<span>`s inside `.row` divs; only the
  visible screen is in the DOM (`overflow:hidden`, no DOM scrollback).
- Mouse forwarding (`onMousedown`/`onWheel`/window move+up) only fires when
  `screen.mouseEnabled` — i.e. when the remote app (tmux, Claude Code) turned
  mouse reporting on. Otherwise mouse events are inert today.
- No copy, paste, bracketed-paste, drag-drop, or file handling exists.
- Sessions run on a host identified by an alias; `"local"` is the marker for
  the local machine (`service/sessions.rs::exec_for`). Remote commands go
  through `SshClient::run(host, args)` over a per-host ControlMaster;
  `SshClient::remote_home(host)` resolves `~`.
- Shell-quoting: any value interpolated into an SSH/tmux command string MUST be
  quoted (`shell_quote` in `tmux.rs`, `shq` in `service/sessions.rs`).

## 1. Copy — auto-copy on select

**Behavior:** Mouse-drag selects text; on mouse-up, a non-empty selection inside
the grid is written to the clipboard automatically (iTerm "copy on select").
The selection stays highlighted.

**Implementation:**

- Add `user-select: text` to `.grid` so native selection works. The cursor
  overlay already has `pointer-events: none`, so it won't interfere.
- Add a grid-scoped `mouseup` handler. On mouse-up, read
  `window.getSelection()`. If the selection is non-empty and anchored within the
  grid, extract its text, normalize it, and call
  `navigator.clipboard.writeText()`.
- **Trailing-whitespace trim:** rows are space-padded to full width. Before
  copying, trim trailing whitespace from each line of the selected text (split
  on `\n`, `trimEnd()` each line, re-join). This is the unit-testable seam —
  put it in a pure helper, e.g. `trimSelectionText(raw: string): string` in
  `ansi.ts` (or a small `clipboard.ts`).

**Coexistence with mouse reporting (Option bypass):**

- When `screen.mouseEnabled`, mouse events forward to the remote app exactly as
  today — UNLESS the **Option (Alt)** key is held.
- In `onMousedown`, `onWheel`, and the window `mousemove`/`mouseup` handlers:
  if `e.altKey` is set, skip the mouse-forwarding (`sendMouse`) path and let the
  browser perform native selection instead.
- When `screen.mouseEnabled` is false, plain drag already selects; no bypass
  needed.
- The auto-copy `mouseup` handler runs regardless of `altKey` (selection can
  exist either because mouse reporting was off, or because Option bypassed it).

## 2. Paste — Cmd+V, bracketed when supported

**Behavior:** Cmd+V sends clipboard text to the PTY. If the remote app enabled
bracketed-paste mode (Claude Code does), the text is wrapped in
`ESC[200~` … `ESC[201~` so multi-line pastes don't auto-submit or mis-indent.

**Track bracketed-paste mode in `ansi.ts`:**

- Add a `bracketedPaste: boolean` field to `Screen` (default `false`).
- In the CSI handler, parse private mode 2004: `ESC[?2004h` sets it `true`,
  `ESC[?2004l` sets it `false`. Mirror exactly how the existing mouse private
  modes (`?1000`/`?1002`/`?1003`/`?1006`) are parsed.

**Paste flow (`TerminalView.svelte`):**

- Handle Cmd+V. Two layers (cover both since WKWebView is finicky):
  - A `paste` event handler on the grid reading `e.clipboardData`.
  - A Cmd+V branch that calls `navigator.clipboard.readText()`.
  Use whichever fires; guard against double-send.
- Sanitize: strip any embedded `ESC[201~` from the pasted text so it can't
  prematurely close the bracket. (Pure helper, e.g.
  `sanitizePaste(text: string): string`.)
- Wrap: if `screen.bracketedPaste`, send `\x1b[200~` + sanitized + `\x1b[201~`;
  else send sanitized raw. (Pure helper
  `framePaste(text: string, bracketed: boolean): string`.)
- Send via the existing `pty_write` invoke + `bumpDrain()`.
- Update `keyToBytes` so Cmd+V is not swallowed/forwarded as a stray key (it
  currently returns `null` for `metaKey`, which is fine — paste is handled
  separately and should `preventDefault`).

## 3. Drag-and-drop — upload over SSH, paste remote path

**Behavior:** Dropping files/images onto the terminal uploads them to the
session's host into a per-session staging dir, then pastes the remote path(s)
into the prompt (using the paste path from §2). When on the Mac and the session
is remote, the upload goes over SSH; when the session is local, it's a local
file copy.

**Detecting the drop (frontend):**

- Use Tauri's window drag-drop event (`getCurrentWebview().onDragDropEvent`),
  which provides real local filesystem paths. (HTML5 drop in WKWebView does not
  expose real paths.) Ensure the window's drag-drop is enabled (Tauri v2 default
  is enabled; confirm `tauri.conf.json` doesn't disable it).
- Filter the event to drops landing over the terminal grid (hit-test the drop
  position against the grid's bounding rect), so drops elsewhere in the app
  aren't hijacked.
- Only act when a terminal is attached (`ptyOpen`) and a current session is
  known.

**Backend upload command:**

- New Tauri command + `service/` function:
  `upload_to_session(session_key, local_paths: Vec<String>) -> Result<Vec<String>, IpcError>`
  returning the remote absolute paths in input order.
- Resolve the session's host alias and a stable per-session subdir name from the
  session identifier already used by the PTY/session row.
- Staging dir: `<remote_home>/.claude-fleet/uploads/<session>/`.
  - Remote host: `SshClient::run(host, ["mkdir","-p", <quoted dir>])` then upload.
  - Local host (`"local"`): `std::fs::create_dir_all` + `std::fs::copy`.
- **Remote upload mechanism:** stream the local file's bytes into
  `ssh <host> 'cat > <quoted remote path>'` via `tokio::process::Command` with
  the local file piped to stdin (NOT base64 — avoids arg-size and bloat). Reuse
  the ControlMaster ssh options. This likely needs a new `SshClient::upload`
  (or `run_with_stdin`) method, since the existing `run` captures output and
  doesn't pipe a file to stdin.
- Filenames: preserve the basename. On collision in the staging dir, de-dupe
  (e.g. append `-1`, `-2`). All interpolated paths shell-quoted.

**After upload (frontend):**

- Join the returned remote paths with spaces, quoting any path containing
  whitespace, and paste the result through the §2 paste path (bracketed if the
  app supports it). Trailing space after the paths so the user can keep typing.

## 4. Drop overlay UI

- An absolutely-positioned overlay over `.grid`, shown on drag-enter:
  "Drop files to upload to `<host>`". Hidden on drag-leave / drop.
- During the upload, show an "Uploading…" state (count / spinner).
- On upload error, surface it via the existing error affordance (the `.err`
  banner style) rather than a blocking dialog.

## 5. Error handling

- Clipboard read/write failures: log and no-op; never throw into the event loop.
- Upload failure: reject the command with an `IpcError` (`E_*` code consistent
  with the SSH/transport errors in the codebase); the frontend shows the error
  overlay and pastes nothing.
- Oversized files: out of scope for now (no size cap); revisit if it bites.

## 6. Testing

**Frontend (Vitest), pure helpers — no DOM needed:**

- `trimSelectionText` — trailing-whitespace trim per line, including
  multi-line selections and selections that are entirely blank.
- `framePaste` — wraps with `ESC[200~`/`ESC[201~` when bracketed, raw when not.
- `sanitizePaste` — strips embedded `ESC[201~`.
- `ansi.ts` parsing — `ESC[?2004h` / `ESC[?2004l` toggles
  `screen.bracketedPaste`; unrelated CSI sequences leave it unchanged.

**Backend (cargo):**

- `upload_to_session` staging-path construction and shell-quoting.
- Local-host branch performs a real `fs::copy` into the staging dir (tmpdir).
- De-dupe on filename collision.

## Open items intentionally deferred (YAGNI)

- Cleanup / GC of uploaded files (per decision: skip).
- Scrollback selection and inline image rendering.
- Clipboard-image (non-file) paste into remote Claude.
- File-size limits / progress for large uploads.

## Touch list

- `src/lib/ansi.ts` — `bracketedPaste` field + `?2004h/l` parse; pure paste/copy
  helpers (or a new `clipboard.ts`).
- `src/lib/TerminalView.svelte` — auto-copy on mouseup, Option bypass in mouse
  handlers, Cmd+V/paste handling, drag-drop wiring, drop overlay.
- `src-tauri/src/ssh.rs` — `upload` / `run_with_stdin` method.
- `src-tauri/src/service/` — `upload_to_session` logic.
- `src-tauri/src/commands/` + `lib.rs` — register the `upload_to_session`
  command.
- `src-tauri/tauri.conf.json` — confirm window drag-drop enabled.

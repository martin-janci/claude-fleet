# Terminal Copy / Paste / Drag-drop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the hand-rolled terminal real clipboard support — auto-copy on selection, Cmd+V paste (bracketed when supported), and drag-and-drop of files/images that upload to the session's host and paste the remote path.

**Architecture:** Pure string helpers live in a new `src/lib/clipboard.ts` (unit-tested without a DOM). `Screen` (`src/lib/ansi.ts`) gains a `bracketedPaste` flag parsed from DECSET `?2004`. `TerminalView.svelte` wires copy (native selection + auto-copy), paste (Cmd+V), and drag-drop (Tauri `onDragDropEvent`) onto the grid, with an Option-key bypass so selection works while mouse-reporting is active. The backend gains an `upload_to_session` command (new `commands/upload.rs`) that stages files into `~/.claude-fleet/uploads/<session>/` on the session's host — local `fs::copy` or, for remote hosts, a new `SshClient::upload_file` that pipes bytes over the existing ControlMaster.

**Tech Stack:** Svelte 5 runes, TypeScript, Vitest (frontend); Rust, Tauri 2, tokio (backend).

---

## File Structure

- `src/lib/clipboard.ts` — **new.** Pure helpers: `trimSelectionText`, `sanitizePaste`, `framePaste`. No DOM, no Tauri.
- `src/lib/clipboard.test.ts` — **new.** Vitest for the three helpers.
- `src/lib/ansi.ts` — **modify.** Add `bracketedPaste` field + `?2004h/l` parsing in `applyDecPrivate`.
- `src/lib/ansi.test.ts` — **modify.** Add `?2004` parsing test.
- `src/lib/TerminalView.svelte` — **modify.** Copy-on-select + Option bypass, Cmd+V paste, drag-drop wiring + drop overlay, `user-select` CSS.
- `src-tauri/src/ssh.rs` — **modify.** Add `upload_file` (stdin-pipe over ControlMaster).
- `src-tauri/src/commands/upload.rs` — **new.** `dedupe_names` helper + `upload_to_session` command.
- `src-tauri/src/commands/mod.rs` — **modify.** `pub mod upload;`
- `src-tauri/src/lib.rs` — **modify.** Register `commands::upload::upload_to_session` in the invoke handler.
- `src-tauri/tauri.conf.json` — **modify.** Make `dragDropEnabled: true` explicit on the window.

---

## Task 1: Pure clipboard helpers

**Files:**
- Create: `src/lib/clipboard.ts`
- Test: `src/lib/clipboard.test.ts`

- [ ] **Step 1: Write the failing tests**

Create `src/lib/clipboard.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { trimSelectionText, sanitizePaste, framePaste } from './clipboard';

describe('trimSelectionText', () => {
  it('trims trailing whitespace per line', () => {
    expect(trimSelectionText('foo   \nbar\t\n')).toBe('foo\nbar\n');
  });
  it('leaves interior and leading whitespace alone', () => {
    expect(trimSelectionText('  foo bar  ')).toBe('  foo bar');
  });
  it('handles an all-blank selection', () => {
    expect(trimSelectionText('   \n   ')).toBe('\n');
  });
});

describe('sanitizePaste', () => {
  it('strips an embedded paste-end marker', () => {
    expect(sanitizePaste('a\x1b[201~b')).toBe('ab');
  });
  it('leaves ordinary text untouched', () => {
    expect(sanitizePaste('hello\nworld')).toBe('hello\nworld');
  });
});

describe('framePaste', () => {
  it('wraps in bracketed-paste markers when enabled', () => {
    expect(framePaste('hi', true)).toBe('\x1b[200~hi\x1b[201~');
  });
  it('returns raw text when disabled', () => {
    expect(framePaste('hi', false)).toBe('hi');
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm test -- src/lib/clipboard.test.ts`
Expected: FAIL — cannot resolve `./clipboard`.

- [ ] **Step 3: Implement the helpers**

Create `src/lib/clipboard.ts`:

```ts
// Pure helpers for terminal copy/paste. No DOM or Tauri deps so they can be
// unit-tested directly and reused by both the Cmd+V and drag-drop paths.

/** Bracketed-paste markers (DEC mode 2004). */
const PASTE_START = '\x1b[200~';
const PASTE_END = '\x1b[201~';

/** Trim trailing whitespace from each line of a selection. Terminal rows are
 *  space-padded to the full width, so a raw selection carries a block of
 *  trailing spaces; real terminals strip them on copy. Leading/interior
 *  whitespace is preserved. */
export function trimSelectionText(raw: string): string {
  return raw
    .split('\n')
    .map((line) => line.replace(/[ \t]+$/, ''))
    .join('\n');
}

/** Remove any embedded paste-end marker from text about to be pasted, so a
 *  malicious/odd clipboard payload can't prematurely close the bracket. */
export function sanitizePaste(text: string): string {
  return text.split(PASTE_END).join('');
}

/** Wrap text in bracketed-paste markers when the remote app requested mode
 *  2004; otherwise return it unchanged. */
export function framePaste(text: string, bracketed: boolean): string {
  return bracketed ? `${PASTE_START}${text}${PASTE_END}` : text;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm test -- src/lib/clipboard.test.ts`
Expected: PASS (7 assertions).

- [ ] **Step 5: Commit**

```bash
git add src/lib/clipboard.ts src/lib/clipboard.test.ts
git commit -m "feat(terminal): pure copy/paste string helpers"
```

---

## Task 2: Track bracketed-paste mode in the screen buffer

**Files:**
- Modify: `src/lib/ansi.ts` (field near line 161; parsing in `applyDecPrivate` ~line 656)
- Test: `src/lib/ansi.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/ansi.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { Screen } from './ansi';

describe('bracketed paste mode (DECSET ?2004)', () => {
  it('defaults to off', () => {
    expect(new Screen(24, 80).bracketedPaste).toBe(false);
  });
  it('?2004h enables and ?2004l disables', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?2004h');
    expect(s.bracketedPaste).toBe(true);
    s.write('\x1b[?2004l');
    expect(s.bracketedPaste).toBe(false);
  });
  it('an unrelated private mode leaves it unchanged', () => {
    const s = new Screen(24, 80);
    s.write('\x1b[?2004h');
    s.write('\x1b[?25l'); // hide cursor — must not touch bracketedPaste
    expect(s.bracketedPaste).toBe(true);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- src/lib/ansi.test.ts -t "bracketed paste"`
Expected: FAIL — `bracketedPaste` is `undefined`.

- [ ] **Step 3: Add the field**

In `src/lib/ansi.ts`, after the `cursorVisible = true;` field (~line 161), add:

```ts
  /** Bracketed-paste mode (DECSET ?2004). When on, the host app (e.g. Claude
   *  Code) wants pasted text wrapped in ESC[200~ … ESC[201~ so multi-line
   *  pastes aren't treated as typed input. The component reads this to decide
   *  whether to frame a paste. */
  bracketedPaste = false;
```

- [ ] **Step 4: Parse the mode**

In `applyDecPrivate` (`src/lib/ansi.ts`, the `else if` chain ~line 656), add a branch alongside the mouse modes — e.g. after the `p === 1006` branch:

```ts
      } else if (p === 2004) {
        this.bracketedPaste = set;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm test -- src/lib/ansi.test.ts -t "bracketed paste"`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/lib/ansi.ts src/lib/ansi.test.ts
git commit -m "feat(terminal): track DECSET ?2004 bracketed-paste mode"
```

---

## Task 3: Copy — auto-copy on select with Option bypass

**Files:**
- Modify: `src/lib/TerminalView.svelte`

No automated test (DOM selection + clipboard is environment-bound); verified via `pnpm check` and the manual check at the end of the task.

- [ ] **Step 1: Add the auto-copy handler**

In `src/lib/TerminalView.svelte`, add this function near the other mouse handlers (after `onWindowMouseup`, ~line 167). It imports `trimSelectionText`:

```ts
  /** After a mouse-up, if the user selected text inside the grid, copy it to
   *  the clipboard automatically (iTerm "copy on select"). The selection is
   *  left highlighted. Trailing whitespace per line is trimmed because rows
   *  are space-padded to the full width. */
  function onGridMouseup() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !container) return;
    if (!container.contains(sel.anchorNode) && !container.contains(sel.focusNode)) return;
    const text = trimSelectionText(sel.toString());
    if (text.trim() === '') return;
    void navigator.clipboard.writeText(text).catch(() => {});
  }
```

- [ ] **Step 2: Update the import**

Change the `./clipboard` consumers — add the import near the top of `<script>` (after the `./ansi` import, ~line 5):

```ts
  import { trimSelectionText, sanitizePaste, framePaste } from './clipboard';
```

(`sanitizePaste`/`framePaste` are used in Task 4; importing them now keeps the import line stable.)

- [ ] **Step 3: Add the Option bypass to mouse forwarding**

In `onMousedown` (~line 113), add an Option/Alt bypass as the FIRST line of the body, before the `mouseEnabled` guard:

```ts
  function onMousedown(e: MouseEvent) {
    // Option (Alt) held → let the browser do a native text selection instead
    // of forwarding the click to the app, so the user can copy while mouse
    // reporting is on.
    if (e.altKey) return;
    if (!ptyOpen || !screen || !screen.mouseEnabled) return;
```

Also add the same bypass as the first line of `onWheel` (~line 86) so an Option-scroll doesn't forward wheel reports during a selection drag:

```ts
  function onWheel(e: WheelEvent) {
    if (e.altKey) return;
    if (!ptyOpen || !screen || !screen.mouseEnabled) return;
```

- [ ] **Step 4: Wire the handler and enable selection**

In the grid markup (~line 585), add the `onmouseup` handler next to `onmousedown`:

```svelte
      onmousedown={onMousedown}
      onmouseup={onGridMouseup}
```

In the `.grid` CSS block (~line 693), add a `user-select` rule so native selection is allowed (it is forwarded-away during mouse mode via `preventDefault`):

```css
  .grid {
    position: relative;
    flex: 1 1 auto;
    user-select: text;
    -webkit-user-select: text;
```

- [ ] **Step 5: Verify type-check passes**

Run: `pnpm check`
Expected: no new errors in `TerminalView.svelte`.

- [ ] **Step 6: Manual check**

Run the app (`pnpm tauri dev`), attach a session, drag-select some output. With mouse reporting off it copies on release; in Claude (mouse mode on) hold Option and drag — it selects and copies. Paste elsewhere to confirm trailing spaces are trimmed.

- [ ] **Step 7: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): auto-copy on select with Option bypass"
```

---

## Task 4: Paste — Cmd+V into the terminal

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Add the shared paste function**

In `src/lib/TerminalView.svelte`, add near the other PTY-writing helpers (after `sendMouse`, ~line 84):

```ts
  /** Send text to the PTY as a paste: strip any embedded paste-end marker,
   *  then frame in bracketed-paste markers if the app requested mode 2004.
   *  Shared by Cmd+V and the drag-drop path. */
  function sendPaste(text: string) {
    if (!ptyOpen || text === '') return;
    const framed = framePaste(sanitizePaste(text), screen?.bracketedPaste ?? false);
    void invoke('pty_write', { args: { data: framed } }).catch(() => {});
    bumpDrain();
  }
```

- [ ] **Step 2: Handle Cmd+V in onKeydown**

In `onKeydown` (~line 410), add a paste branch BEFORE the `keyToBytes` call. Non-editable `<div>`s don't fire a `paste` event, so we read the clipboard explicitly:

```ts
  function onKeydown(e: KeyboardEvent) {
    if (!ptyOpen) return;
    if (e.isComposing) return;
    // Cmd+V / Ctrl+V → read the clipboard and paste into the PTY.
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'v') {
      e.preventDefault();
      void navigator.clipboard.readText().then((t) => sendPaste(t)).catch(() => {});
      return;
    }
    const bytes = keyToBytes(e);
```

- [ ] **Step 3: Verify type-check passes**

Run: `pnpm check`
Expected: no new errors.

- [ ] **Step 4: Manual check**

In the app, copy multi-line text elsewhere, focus the terminal in a Claude prompt, press Cmd+V. The text arrives as one paste (no per-line submit) because Claude Code enables `?2004`. In a plain shell prompt (no `?2004`) it pastes raw.

- [ ] **Step 5: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): Cmd+V paste with bracketed-paste framing"
```

---

## Task 5: Backend — filename de-dup helper

**Files:**
- Create: `src-tauri/src/commands/upload.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: Register the module**

In `src-tauri/src/commands/mod.rs`, add (keep alphabetical):

```rust
pub mod upload;
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/commands/upload.rs` with just the helper + tests for now:

```rust
//! `upload_to_session` — stage dropped files on the session's host so their
//! remote path can be pasted into the prompt. Local sessions copy with
//! `std::fs`; remote sessions stream bytes over the ControlMaster
//! (`SshClient::upload_file`). No cleanup (per the design — files accumulate
//! under ~/.claude-fleet/uploads/<session>/).

/// Make a batch of basenames collision-free, preserving order. The first
/// occurrence keeps its name; a later duplicate gets `-1`, `-2`, … inserted
/// before its extension (`a.png` → `a-1.png`; `notes` → `notes-1`).
pub fn dedupe_names(names: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = name.clone();
        let mut n = 1;
        while seen.contains(&candidate) {
            candidate = suffix_name(name, n);
            n += 1;
        }
        seen.insert(candidate.clone());
        out.push(candidate);
    }
    out
}

/// Insert `-{n}` before the final extension (if any). `file_stem`/`extension`
/// semantics: a leading-dot name like `.bashrc` has no extension, so the
/// suffix goes at the end.
fn suffix_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{n}.{ext}"),
        _ => format!("{name}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::dedupe_names;

    #[test]
    fn keeps_unique_names() {
        let got = dedupe_names(&["a.png".into(), "b.png".into()]);
        assert_eq!(got, vec!["a.png", "b.png"]);
    }

    #[test]
    fn suffixes_collisions_before_extension() {
        let got = dedupe_names(&["a.png".into(), "a.png".into(), "a.png".into()]);
        assert_eq!(got, vec!["a.png", "a-1.png", "a-2.png"]);
    }

    #[test]
    fn handles_names_without_extension() {
        let got = dedupe_names(&["notes".into(), "notes".into()]);
        assert_eq!(got, vec!["notes", "notes-1"]);
    }

    #[test]
    fn leading_dot_name_has_no_extension() {
        let got = dedupe_names(&[".env".into(), ".env".into()]);
        assert_eq!(got, vec![".env", ".env-1"]);
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib commands::upload`
Expected: PASS (4 tests). (If the Tauri system-lib build gap blocks `cargo test`, note it — this is the pre-existing environment caveat in CLAUDE.md.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/upload.rs src-tauri/src/commands/mod.rs
git commit -m "feat(upload): filename de-dup helper for staged drops"
```

---

## Task 6: Backend — SshClient::upload_file

**Files:**
- Modify: `src-tauri/src/ssh.rs`

- [ ] **Step 1: Add the import**

In `src-tauri/src/ssh.rs`, add to the `use std::path` line (currently `use std::path::PathBuf;` at line 25):

```rust
use std::path::{Path, PathBuf};
```

- [ ] **Step 2: Add the method**

In the `impl SshClient` block, after `run_cancellable` (~line 218), add:

```rust
    /// Upload a local file to `remote_path` on `host` by piping its bytes into
    /// `cat > <quoted path>` over the ControlMaster. The remote parent
    /// directory must already exist (caller `mkdir -p`s it). Returns Err on a
    /// non-zero ssh/cat exit. Uses the same `-o` muxing as `run`.
    pub async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        timeout: Duration,
    ) -> Result<(), IpcError> {
        self.inner.seen.insert(host.to_string(), ());
        let file = std::fs::File::open(local_path).map_err(|e| {
            IpcError::new("E_UPLOAD", format!("open {}: {e}", local_path.display()))
        })?;
        let mut cmd = tokio::process::Command::new("ssh");
        for opt in self.mux_opts(host, timeout) {
            cmd.arg(opt);
        }
        // Single remote word: the remote login shell runs `cat > 'path'`,
        // reading the piped file from stdin. Path is single-quoted.
        let remote_cmd = format!("cat > {}", crate::shell::quote(remote_path));
        cmd.arg("--").arg(host).arg(&remote_cmd);
        cmd.stdin(std::process::Stdio::from(file));
        let out = cmd
            .output()
            .await
            .map_err(|e| IpcError::new("E_UPLOAD", format!("ssh {host}: {e}")))?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "upload to {host} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(())
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cd src-tauri && cargo check`
Expected: compiles (modulo the Tauri system-lib build gap from CLAUDE.md, which surfaces in a build script, not in this code).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/ssh.rs
git commit -m "feat(ssh): upload_file streams bytes over the ControlMaster"
```

---

## Task 7: Backend — upload_to_session command

**Files:**
- Modify: `src-tauri/src/commands/upload.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the command imports and body**

At the TOP of `src-tauri/src/commands/upload.rs`, above `dedupe_names`, add:

```rust
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

/// Generous timeout — covers an image upload over a slow link.
const UPLOAD_TIMEOUT_SECS: u64 = 60;

#[derive(Deserialize)]
pub struct UploadArgs {
    pub host_alias: String,
    /// The session's tmux name — used as the per-session staging subdir.
    pub session_name: String,
    /// Absolute local paths of the dropped files.
    pub local_paths: Vec<String>,
}

/// Stage `local_paths` under `~/.claude-fleet/uploads/<session>/` on the
/// session's host and return the resulting absolute remote paths, in order.
#[tauri::command]
pub async fn upload_to_session(
    args: UploadArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<String>, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }

    // Collision-free destination basenames.
    let basenames: Vec<String> = args
        .local_paths
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        })
        .collect();
    let names = dedupe_names(&basenames);

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let is_local = args.host_alias == "local";

    // Resolve the staging dir (absolute, so the returned paths are pasteable).
    let home = if is_local {
        std::env::var("HOME").map_err(|_| IpcError::new("E_UPLOAD", "HOME not set"))?
    } else {
        ssh.remote_home(&args.host_alias).await?
    };
    let dir = format!("{home}/.claude-fleet/uploads/{}", args.session_name);

    let mut remote_paths = Vec::with_capacity(names.len());

    if is_local {
        std::fs::create_dir_all(&dir)
            .map_err(|e| IpcError::new("E_UPLOAD", format!("mkdir {dir}: {e}")))?;
        for (src, name) in args.local_paths.iter().zip(&names) {
            let dest = format!("{dir}/{name}");
            std::fs::copy(src, &dest)
                .map_err(|e| IpcError::new("E_UPLOAD", format!("copy {src}: {e}")))?;
            remote_paths.push(dest);
        }
    } else {
        let mkdir = ssh
            .run(&args.host_alias, &["mkdir", "-p", &shq(&dir)], timeout)
            .await?;
        if !mkdir.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "mkdir on {} failed: {}",
                    args.host_alias,
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            ));
        }
        for (src, name) in args.local_paths.iter().zip(&names) {
            let dest = format!("{dir}/{name}");
            ssh.upload_file(&args.host_alias, Path::new(src), &dest, timeout)
                .await?;
            remote_paths.push(dest);
        }
    }

    Ok(remote_paths)
}
```

- [ ] **Step 2: Register the command**

In `src-tauri/src/lib.rs`, in the `tauri::generate_handler![ … ]` list (near the `commands::files::*` entries ~line 305), add:

```rust
            commands::upload::upload_to_session,
```

- [ ] **Step 3: Verify it compiles and tests still pass**

Run: `cd src-tauri && cargo check && cargo test --lib commands::upload`
Expected: compiles; the 4 `dedupe_names` tests still PASS. (Tauri build-gap caveat applies.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/upload.rs src-tauri/src/lib.rs
git commit -m "feat(upload): upload_to_session command (local + remote staging)"
```

---

## Task 8: Frontend — drag-drop wiring + overlay

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Track the current host and drop state**

In `src/lib/TerminalView.svelte`, add the import (top of `<script>`, after the `invoke` import ~line 3):

```ts
  import { getCurrentWebview } from '@tauri-apps/api/webview';
```

Add state vars near `currentSession` (~line 40):

```ts
  let currentHost: string | null = $state(null);
  /** Drop-overlay state: shown while a drag is over the grid, switched to a
   *  spinner during the upload. */
  let dragOver = $state(false);
  let uploading = $state(false);
```

In `openTerm`, where `currentSession = sess.tmux_name;` is set (~line 237), also capture the host:

```ts
      currentSession = sess.tmux_name;
      currentHost = sess.host_alias;
      ptyOpen = true;
```

- [ ] **Step 2: Add the drop hit-test + handler**

Add these helpers near `sendPaste` (~line 84). The Tauri drag-drop position is in physical pixels; convert to CSS px before comparing with the grid rect:

```ts
  /** Is a physical-pixel point inside the terminal grid? */
  function pointOverGrid(px: number, py: number): boolean {
    if (!container) return false;
    const dpr = window.devicePixelRatio || 1;
    const r = container.getBoundingClientRect();
    const x = px / dpr;
    const y = py / dpr;
    return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }

  /** Build the prompt text for a set of uploaded remote paths: space-joined,
   *  single-quoted if a path contains whitespace, trailing space so the user
   *  can keep typing. */
  function pathsToPasteText(paths: string[]): string {
    return paths.map((p) => (/\s/.test(p) ? `'${p}'` : p)).join(' ') + ' ';
  }

  async function handleDrop(paths: string[]) {
    if (!ptyOpen || !currentSession || !currentHost || paths.length === 0) return;
    uploading = true;
    try {
      const remote = await invoke<string[]>('upload_to_session', {
        args: { host_alias: currentHost, session_name: currentSession, local_paths: paths },
      });
      if (remote.length > 0) sendPaste(pathsToPasteText(remote));
    } catch (e) {
      openError = describeError(e);
    } finally {
      uploading = false;
    }
  }
```

- [ ] **Step 3: Subscribe to the Tauri drag-drop event**

Add an `$effect` that registers the listener once and cleans it up (place after the existing `$effect`s, ~line 187). The unlisten promise is awaited in the cleanup:

```ts
  // Native (OS-level) drag-drop. HTML5 drop in WKWebView can't expose real
  // file paths, so we use Tauri's window event, which does. We only act on
  // drops that land over the grid.
  $effect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === 'enter' || p.type === 'over') {
          dragOver = pointOverGrid(p.position.x, p.position.y);
        } else if (p.type === 'leave') {
          dragOver = false;
        } else if (p.type === 'drop') {
          const over = pointOverGrid(p.position.x, p.position.y);
          dragOver = false;
          if (over) void handleDrop(p.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  });
```

- [ ] **Step 4: Add the overlay markup**

Inside the grid `<div class="grid" …>` block, after the cursor `{#if cursor}` block (~line 606) and before the closing `</div>`, add:

```svelte
      {#if dragOver || uploading}
        <div class="drop-overlay" data-testid="terminal-drop-overlay">
          {uploading ? 'Uploading…' : `Drop files to upload to ${currentHost ?? 'host'}`}
        </div>
      {/if}
```

- [ ] **Step 5: Add the overlay CSS**

In the `<style>` block (after the `.cursor` rules ~line 733), add:

```css
  .drop-overlay {
    position: absolute;
    inset: 0;
    z-index: 4;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(20, 30, 50, 0.55);
    border: 2px dashed var(--accent, #4f8fff);
    color: #e8e8e8;
    font-size: 0.95rem;
    pointer-events: none;
  }
```

- [ ] **Step 6: Verify type-check passes**

Run: `pnpm check`
Expected: no new errors. (If `onDragDropEvent`'s payload type needs it, the `event` param is typed by `@tauri-apps/api`; no manual cast should be required.)

- [ ] **Step 7: Manual check**

Run the app, attach a remote session, drag an image file onto the terminal. The overlay shows "Drop files to upload to <host>", then "Uploading…", then the remote path `~/.claude-fleet/uploads/<session>/<file>` is pasted into the prompt. Confirm on the host that the file landed. Repeat with a local session.

- [ ] **Step 8: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): drag-drop files — upload to host, paste remote path"
```

---

## Task 9: Config + final verification

**Files:**
- Modify: `src-tauri/tauri.conf.json`

- [ ] **Step 1: Make drag-drop explicit**

In `src-tauri/tauri.conf.json`, add `"dragDropEnabled": true` to the single window object (it defaults to true in Tauri v2, but make it explicit so a future edit doesn't silently break drops):

```json
      {
        "title": "claude-fleet",
        "width": 1280,
        "height": 800,
        "minWidth": 800,
        "minHeight": 500,
        "dragDropEnabled": true
      }
```

- [ ] **Step 2: Run the full frontend suite**

Run: `pnpm test`
Expected: the new `clipboard.test.ts` and `ansi.test.ts` tests PASS. Pre-existing `localStorage is undefined` failures (`session_ui.test.ts`, `App.test.ts`, …) are unrelated — verify they also fail on `main` per CLAUDE.md before attributing.

- [ ] **Step 3: Type-check**

Run: `pnpm check`
Expected: clean (no new errors).

- [ ] **Step 4: Backend checks**

Run: `cd src-tauri && cargo test --lib commands::upload && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: tests pass, no clippy warnings, formatting clean. (If the Tauri system-lib build gap blocks these, note it — pre-existing environment caveat.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/tauri.conf.json
git commit -m "chore(tauri): make window dragDropEnabled explicit"
```

---

## Self-Review notes

- **Spec coverage:** §1 Copy → Task 3. §2 Paste + `?2004` tracking → Tasks 1, 2, 4. §3 Drag-drop upload (local + remote, per-session dir, dedupe, shell-quoting, SSH stdin pipe) → Tasks 5, 6, 7, 8. §4 Overlay → Task 8. §5 Error handling → `E_UPLOAD` in Tasks 6/7, clipboard no-op catches in Tasks 3/4, overlay error surfacing in Task 8. §6 Testing → Tasks 1, 2, 5. Deferred items (cleanup, scrollback, image-paste, size limits) intentionally omitted.
- **Type/name consistency:** `trimSelectionText`/`sanitizePaste`/`framePaste` (Task 1) used in Tasks 3/4; `bracketedPaste` (Task 2) read in Task 4; `dedupe_names` (Task 5) used in Task 7; `SshClient::upload_file` (Task 6) called in Task 7; `upload_to_session` + `UploadArgs { host_alias, session_name, local_paths }` (Task 7) invoked identically in Task 8; `currentHost`/`currentSession` set in `openTerm` and read in `handleDrop`.
- **No placeholders:** every code step shows full content.
```

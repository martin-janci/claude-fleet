# Terminal Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every defect found in the 2026-09-14 terminal analysis: parser crashes that freeze the pane, wide/combining glyph misplacement, main-thread blocking in PTY commands, lossy buffer overflow, missing keys, IME input, OSC 52 mojibake, false disconnect banners, and the smaller parser/renderer gaps.

**Architecture:** Three layers change. (1) `src/lib/ansi.ts` (the hand-rolled screen model) learns tmux 3.6a's width rules, stops throwing, and handles private/string sequences; a recorded tmux attach becomes a golden regression test. (2) `src-tauri/src/pty.rs` moves its byte buffer and its input writer into two Tauri-free modules (`pty_buffer.rs`, `pty_writer.rs`) so no PTY command blocks on I/O, overflow and EOF become explicit flags, and the ssh attach reuses `SshClient::mux_opts` (now with keepalive). (3) `src/lib/TerminalView.svelte` delegates to small pure modules (`terminal_drain.ts`, `terminal_style.ts`, `keys.ts`, `wcwidth.ts`) that are unit-tested; the component itself is verified by `svelte-check`, build, and the manual checklist in Task 15.

**Tech Stack:** Svelte 5 (runes), TypeScript, Vitest 4 (jsdom), Tauri 2.11.2, Rust (portable-pty 0.9), tmux 3.6a.

**Analysis evidence:** probe scripts `probe.ts`, `diff.ts`, `drift.ts`, `fixture/` in the 2026-09-14 session scratchpad. Every "confirmed" behaviour below was reproduced there.

## Global Constraints

- Branch: create a fresh worktree/branch off `origin/main` (`/usr/bin/git fetch origin && /usr/bin/git worktree add .worktrees/terminal-hardening -b fix/terminal-hardening origin/main`). Do not build on `feat/sm-q2-marker-strip`.
- pnpm: `corepack pnpm` resolves pnpm 9 on mefistos and fails ("packages field missing or empty"). Use `npx -y pnpm@10 <cmd>`. Run `npx -y pnpm@10 install` once per worktree.
- git inside agent worktrees: call `/usr/bin/git`, not `git` (the RTK hook rewrite is refused by the worktree guard). Never hide a refused command in a script, `bash -c`, or eval.
- cargo: run in the FOREGROUND, one build at a time, as a plain command without `&&` chains or redirects: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml <filter>`.
- Commits: Conventional Commits (release-please owns versions and CHANGELOG). Never bump versions.
- No `#[tool(...)]` or `generate_handler!` edits, so `docs/control-api-reference.md` needs no regeneration.
- PTY rule introduced by this plan: never perform blocking I/O (write, wait, kill) while holding the `Mutex<PtyState>` guard.
- Width model: tmux 3.6a as measured (`#{cursor_x}`): `⏺ ✻ ⎿ ─ █ ❤ á` = 1, `中 🤖 ☕ ✅` = 2, combining marks / ZWJ / VS16 = 0, and a narrow glyph followed by VS16 = 2.
- Pre-existing frontend failures (`localStorage is undefined` in `session_ui.test.ts`, `App.test.ts`) are not caused by this work. Compare against `main` before attributing a failure.
- Existing coordinate convention: `eventToCell` is 1-based; `Screen.cells[row][col]`, selection, and overlays are 0-based.

## Decisions taken (flag in the PR descriptions)

1. Shift+Enter and Option/Alt+Enter send `ESC CR`, which Claude Code treats as "insert newline". Plain Enter still sends `CR`.
2. On buffer overflow the backend drops everything and flags `reset`; the frontend re-attaches (tmux repaints and resends all modes) instead of trying to resynchronise.
3. The in-band `[cf] …` status lines stay visible in the terminal; disconnect detection moves to an explicit `eof` flag.
4. The header "ticks" counter (a reactive write on every poll) is removed; the byte counter stays.
5. Not planned: re-measuring cell metrics on zoom (the app has no zoom), DECCKM application cursor keys (tmux accepts both `CSI A` and `SS3 A`), SGR 53 overline, DECSC (`ESC 7`) saving colours/charset (tmux 3.6a never sent `ESC 7` in the recorded attaches).

## Delivery: four PRs, in order

| PR | Branch commits | Title |
|---|---|---|
| A | Tasks 1–3 | `fix(terminal): parser can't freeze the pane; private/string sequences; SGR gaps` |
| B | Tasks 4–7 | `fix(pty): non-blocking writes, overflow/EOF flags, ssh keepalive, size floor` |
| C | Tasks 8–11 | `fix(terminal): drain lifecycle, tmux-accurate wide glyphs, fixed-width rendering` |
| D | Tasks 12–15 | `feat(terminal): full key encoding, IME input, resize debounce` |

Task 8 needs PR B merged (it consumes `eof`/`reset`). Everything else in a PR only depends on earlier tasks in the same or earlier PRs.

## File map

| File | Status | Responsibility |
|---|---|---|
| `src/lib/ansi.ts` | modify | Screen model: never throws; private/string sequences; SGR; wide/combining cells; `rowToRuns` with cell counts |
| `src/lib/wcwidth.ts` | create | `charWidth(cp)` matching tmux 3.6a |
| `src/lib/terminal_drain.ts` | create | `applyDrain(screen, result)` — feeds one drain result into a Screen, never throws |
| `src/lib/terminal_style.ts` | create | `runStyle(run)` — CSS for a styled run (moved out of the component) |
| `src/lib/keys.ts` | create | `keyToBytes(e)` — KeyboardEvent → terminal bytes (moved out of the component) |
| `src/lib/TerminalView.svelte` | modify | Uses the modules above; drain lifecycle; IME proxy; resize debounce |
| `src/lib/__fixtures__/tmux-attach.raw.txt` + `.pane.txt` | create | Recorded tmux 3.6a attach + `capture-pane` ground truth |
| `src/lib/ansi.tmux-fixture.test.ts` | create | Golden test: Screen vs capture-pane |
| `scripts/capture-tmux-fixture.sh` | create | Regenerates the fixture (Linux only) |
| `.gitattributes` | create | Keep fixture bytes verbatim |
| `src-tauri/src/pty_buffer.rs` | create | Reader→drain byte buffer: cap, overflow reset flag, sticky EOF, UTF-8 tail |
| `src-tauri/src/pty_writer.rs` | create | Channel + thread PTY input writer with a queue cap |
| `src-tauri/src/pty.rs` | modify | Uses both modules; `attach_command`; `pty_size`; detach/shutdown outside the lock; async open/close |
| `src-tauri/src/ssh.rs` | modify | `mux_opts` gains keepalive and becomes `pub(crate)` |
| `src-tauri/src/lib.rs` | modify | `mod pty_buffer; mod pty_writer;` |
| `CLAUDE.md` | modify | Document the PTY no-blocking rule and the width model |

---

### Task 1: The parser can't throw, and the drain loop survives errors

Confirmed crashes: `CSI 1K` / `CSI 1J` while the cursor is in the pending-wrap column (`cursorCol === cols`) index past the row; restoring a cursor saved before a shrink (`ESC 8`, `CSI u`, `CSI ?1048l`) leaves `cursorRow` off-screen and the next printed char throws. Any throw inside `screen.write` escapes `drainOnce` and `runDrain` never reschedules, freezing the pane until a manual reconnect.

**Files:**
- Modify: `src/lib/ansi.ts` (`resize` ~261-283, `parseEscape` ESC 8 ~445-449, `applyCsi` case `'u'` ~651-654, `applyDecPrivate` 1048 ~681-689, `eraseInDisplay` ~770-786, `eraseInLine` ~788-796)
- Create: `src/lib/terminal_drain.ts`
- Modify: `src/lib/TerminalView.svelte` (`runDrain` ~552-559, `drainOnce` ~573-599)
- Test: `src/lib/ansi.test.ts`, `src/lib/terminal_drain.test.ts`

**Interfaces:**
- Produces: `applyDrain(screen: Screen, result: DrainResult): DrainOutcome` with
  `interface DrainResult { data: string; bytes: number; eof?: boolean; reset?: boolean }` and
  `interface DrainOutcome { wrote: boolean; disconnected: boolean; reset: boolean; error: string | null }`. Task 8 relies on these exact names.

- [ ] **Step 1: Write the failing parser tests**

Append to `src/lib/ansi.test.ts`:

```ts
describe('ansi.Screen — never throws on edge cursor state', () => {
  it('EL1 in the pending-wrap column clears the whole row', () => {
    const s = new Screen(3, 5);
    s.write('abcde\x1b[1K');
    expect(rowText(s, 0)).toBe('     ');
  });

  it('ED1 in the pending-wrap column clears through the last column', () => {
    const s = new Screen(3, 5);
    s.write('\x1b[2;1Habcde\x1b[1J');
    expect(rowText(s, 0)).toBe('     ');
    expect(rowText(s, 1)).toBe('     ');
  });

  it('ESC 8 clamps a cursor saved below a later shrink', () => {
    const s = new Screen(10, 10);
    s.write('\x1b[10;1H\x1b7');
    s.resize(5, 10);
    s.write('\x1b8X');
    expect(s.cursorRow).toBe(4);
    expect(rowText(s, 4)).toBe('X         ');
  });

  it('CSI u and ?1048l clamp a saved cursor after a shrink', () => {
    const s = new Screen(10, 10);
    s.write('\x1b[10;10H\x1b[s');
    s.resize(4, 4);
    s.write('\x1b[u');
    expect([s.cursorRow, s.cursorCol]).toEqual([3, 3]);

    const t = new Screen(10, 10);
    t.write('\x1b[10;10H\x1b[?1048h');
    t.resize(4, 4);
    t.write('\x1b[?1048l');
    expect([t.cursorRow, t.cursorCol]).toEqual([3, 3]);
  });

  it('a cursor saved on the primary screen survives alt-screen + shrink', () => {
    const s = new Screen(10, 10);
    s.write('\x1b[10;1H\x1b7\x1b[?1049h');
    s.resize(5, 10);
    s.write('\x1b[?1049l\x1b8X');
    expect(s.cursorRow).toBe(4);
  });
});
```

- [ ] **Step 2: Write the failing drain test**

Create `src/lib/terminal_drain.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { Screen } from './ansi';
import { applyDrain } from './terminal_drain';

describe('applyDrain', () => {
  it('writes data into the screen', () => {
    const s = new Screen(2, 10);
    const out = applyDrain(s, { data: 'hi', bytes: 2 });
    expect(out).toEqual({ wrote: true, disconnected: false, reset: false, error: null });
    expect(s.cells[0][0].ch).toBe('h');
  });

  it('reports nothing written for an empty drain', () => {
    const s = new Screen(2, 10);
    expect(applyDrain(s, { data: '', bytes: 0 })).toEqual({
      wrote: false, disconnected: false, reset: false, error: null,
    });
  });

  it('passes the eof and reset flags through', () => {
    const s = new Screen(2, 10);
    expect(applyDrain(s, { data: '', bytes: 0, eof: true, reset: true })).toEqual({
      wrote: false, disconnected: true, reset: true, error: null,
    });
  });

  it('catches a throwing write instead of propagating it', () => {
    const s = new Screen(2, 10);
    s.write = () => {
      throw new Error('boom');
    };
    expect(applyDrain(s, { data: 'x', bytes: 1 })).toEqual({
      wrote: true, disconnected: false, reset: false, error: 'boom',
    });
  });
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/terminal_drain.test.ts`
Expected: FAIL — the new ansi tests throw `Cannot set properties of undefined (setting 'ch')` / `Cannot read properties of undefined (reading '0')`; the drain test fails to import `./terminal_drain`.

- [ ] **Step 4: Clamp every cursor restore in `ansi.ts`**

Add this private method to `Screen` (next to `putChar`):

```ts
  /** Apply the saved cursor (DECRC / CSI u / ?1048l), clamped to the screen.
   *  The save can predate a shrink, so the stored row/col may be off-screen
   *  now; an unclamped row made the next printed char throw. */
  private restoreCursor(): void {
    this.cursorRow = clamp(this.savedRow, 0, this.rows - 1);
    this.cursorCol = clamp(this.savedCol, 0, this.cols - 1);
  }
```

Replace the three restore sites:

```ts
    if (intro === '8') {
      this.restoreCursor();
      return 2;
    }
```

```ts
      case 'u': // restore cursor
        this.restoreCursor();
        return;
```

```ts
        } else {
          this.restoreCursor();
        }
```

In `resize`, after `if (this.cursorCol >= cols) this.cursorCol = cols - 1;`, add:

```ts
    this.savedRow = Math.min(this.savedRow, rows - 1);
    this.savedCol = Math.min(this.savedCol, cols - 1);
```

and inside the existing `if (this.savedScreen !== null) { … }` block add:

```ts
      saved.savedRow = Math.min(saved.savedRow, rows - 1);
      saved.savedCol = Math.min(saved.savedCol, cols - 1);
```

- [ ] **Step 5: Clamp erase-to-cursor loops in `ansi.ts`**

In `eraseInDisplay`, replace the `mode === 1` branch:

```ts
    } else if (mode === 1) {
      // From start of screen to cursor. In the pending-wrap state cursorCol
      // equals cols, so clamp to the last real column.
      const last = Math.min(this.cursorCol, this.cols - 1);
      for (let r = 0; r < this.cursorRow; r++)
        for (let c = 0; c < this.cols; c++) this.clearCell(r, c);
      for (let c = 0; c <= last; c++) this.clearCell(this.cursorRow, c);
```

In `eraseInLine`, replace the `mode === 1` branch:

```ts
    } else if (mode === 1) {
      const last = Math.min(this.cursorCol, this.cols - 1);
      for (let c = 0; c <= last; c++) this.clearCell(this.cursorRow, c);
```

- [ ] **Step 6: Create `src/lib/terminal_drain.ts`**

```ts
import type { Screen } from './ansi';

/** Shape returned by the `pty_drain` command. `eof` and `reset` are optional
 *  so older backends (without the flags) still type-check. */
export interface DrainResult {
  data: string;
  bytes: number;
  eof?: boolean;
  reset?: boolean;
}

export interface DrainOutcome {
  /** Bytes were fed to the screen (even if the parser threw part-way). */
  wrote: boolean;
  /** The PTY reached EOF — show the reconnect banner. */
  disconnected: boolean;
  /** The backend dropped output past its cap — the screen must be rebuilt. */
  reset: boolean;
  /** Parser error message, if `screen.write` threw. */
  error: string | null;
}

/** Feed one drain result into a Screen. Never throws: a parser bug on one
 *  chunk used to escape the drain loop and freeze the pane for good. */
export function applyDrain(screen: Screen, result: DrainResult): DrainOutcome {
  const out: DrainOutcome = {
    wrote: false,
    disconnected: result.eof === true,
    reset: result.reset === true,
    error: null,
  };
  if (result.bytes === 0 && result.data === '') return out;
  out.wrote = true;
  try {
    screen.write(result.data);
  } catch (e) {
    out.error = e instanceof Error ? e.message : String(e);
  }
  return out;
}
```

- [ ] **Step 7: Use it in `TerminalView.svelte` and make the loop unkillable**

Add to the imports:

```ts
  import { applyDrain, type DrainResult } from './terminal_drain';
```

Replace `runDrain`:

```ts
  /** One drain tick, then reschedule itself. The delay halves to the floor on
   *  any output and doubles toward DRAIN_MAX_MS when idle. The reschedule is in
   *  `finally` so no error can stop the loop. */
  async function runDrain() {
    drainTimer = null;
    let got = false;
    try {
      got = await drainOnce();
    } catch (e) {
      console.error('[terminal] drain tick failed:', e);
    } finally {
      drainDelay = got ? DRAIN_MIN_MS : Math.min(DRAIN_MAX_MS, drainDelay * 2);
      // Reschedule only if still attached and no newer loop has taken over
      // (a concurrent openTerm would have set its own drainTimer).
      if (screen && ptyOpen && drainTimer === null) scheduleDrain();
    }
  }
```

In `drainOnce`, change the result type and replace the two lines `totalBytes += result.bytes;` / `screen.write(result.data);`:

```ts
    let result: DrainResult;
    try {
      result = await invoke<DrainResult>('pty_drain');
    } catch {
      return false;
    }
    if (screen !== drainingInto) return false;
    drainTicks += 1;
    if (result.bytes === 0) return false;
    totalBytes += result.bytes;
    const outcome = applyDrain(screen, result);
    if (outcome.error) console.error('[terminal] parser error, chunk skipped:', outcome.error);
    renderVersion++;
```

(Leave the existing `[cf] PTY EOF` marker check below it unchanged; Task 8 replaces it.)

- [ ] **Step 8: Run the tests to verify they pass**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/terminal_drain.test.ts`
Expected: PASS, no failures.

Run: `npx -y pnpm@10 run check`
Expected: `svelte-check found 0 errors`.

- [ ] **Step 9: Commit**

```bash
/usr/bin/git add src/lib/ansi.ts src/lib/ansi.test.ts src/lib/terminal_drain.ts src/lib/terminal_drain.test.ts src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): clamp cursor restores and erase-to-cursor; drain loop survives parser errors"
```

---

### Task 2: Private CSI, string sequences, full reset, pending cap, OSC 52 UTF-8

Confirmed: `CSI >4;1m` sets bold+underline (attrs=9); `CSI >1u` restores the cursor; DCS payloads print as text (`+q544e`); OSC 52 decodes `čšá` as `ÄÅ¡Ã¡`. Also: `ESC c` leaves mouse/bracketed-paste/cursor modes set, `<` and `=` are not recognised as private markers, and an unterminated string grows `pending` forever.

**Files:**
- Modify: `src/lib/ansi.ts` (`write` ~286-300, `parseEscape` OSC branch ~480-498, `applyOsc` ~522-537, `fullReset` ~539-555, `applyCsi` marker strip ~561-565 and cases `'s'`, `'u'`, `'m'`)
- Test: `src/lib/ansi.test.ts`

**Interfaces:**
- Produces: module-level `findStringTerminator(s: string, from: number): { end: number; next: number } | null` and `const MAX_PENDING = 1 << 20` in `ansi.ts` (internal, not exported).

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/ansi.test.ts`:

```ts
describe('ansi.Screen — private and string sequences', () => {
  it('ignores CSI > 4;1 m (modifyOtherKeys) instead of applying SGR', () => {
    const s = new Screen(2, 10);
    s.write('\x1b[>4;1mA');
    expect(s.cells[0][0].attrs).toBe(0);
  });

  it('ignores kitty keyboard push/pop/query instead of restoring the cursor', () => {
    const s = new Screen(5, 10);
    s.write('\x1b[3;3H\x1b[>1u\x1b[<u\x1b[?u');
    expect([s.cursorRow, s.cursorCol]).toEqual([2, 2]);
  });

  it('still honours plain CSI s / CSI u', () => {
    const s = new Screen(5, 10);
    s.write('\x1b[3;3H\x1b[s\x1b[1;1H\x1b[u');
    expect([s.cursorRow, s.cursorCol]).toEqual([2, 2]);
  });

  it('consumes a DCS string without printing its payload', () => {
    const s = new Screen(2, 20);
    s.write('\x1bP+q544e\x1b\\ok');
    expect(rowText(s, 0).trimEnd()).toBe('ok');
  });

  it('consumes APC, PM and SOS strings', () => {
    const s = new Screen(2, 20);
    s.write('\x1b_Gf=24\x1b\\\x1b^pm\x1b\\\x1bXsos\x1b\\ok');
    expect(rowText(s, 0).trimEnd()).toBe('ok');
  });

  it('waits for the terminator of a DCS split across writes', () => {
    const s = new Screen(2, 20);
    s.write('\x1bPtmux;pay');
    s.write('load\x1b\\ok');
    expect(rowText(s, 0).trimEnd()).toBe('ok');
  });

  it('drops an unterminated sequence once it exceeds the pending cap', () => {
    const s = new Screen(2, 10);
    s.write('\x1b]0;' + 'x'.repeat((1 << 20) + 1));
    s.write('ok');
    expect(rowText(s, 0).trimEnd()).toBe('ok');
  });

  it('ESC c resets mouse, bracketed paste and cursor visibility', () => {
    const s = new Screen(2, 10);
    s.write('\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1bc');
    expect(s.mouseEnabled).toBe(false);
    expect(s.mouseSgr).toBe(false);
    expect(s.bracketedPaste).toBe(false);
    expect(s.cursorVisible).toBe(true);
  });

  it('decodes OSC 52 clipboard payloads as UTF-8', () => {
    const s = new Screen(2, 10);
    let got = '';
    s.onClipboard = (t) => {
      got = t;
    };
    const b64 = btoa(String.fromCharCode(...new TextEncoder().encode('čšá')));
    s.write(`\x1b]52;c;${b64}\x07`);
    expect(got).toBe('čšá');
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts`
Expected: FAIL on the modifyOtherKeys (attrs 9), kitty (cursor 0,0), DCS/APC (`+q544e`), pending-cap (empty row), ESC c, and OSC 52 (`ÄÅ¡Ã¡`) tests. The plain `CSI s/u` test passes.

- [ ] **Step 3: Add the string-terminator helper and pending cap**

Near `clamp` at the bottom of `ansi.ts`:

```ts
/** Longest escape sequence we keep waiting for across writes (1 MiB). An
 *  unterminated OSC/DCS would otherwise grow `pending` forever and be
 *  rescanned on every chunk. */
const MAX_PENDING = 1 << 20;

/** Scan for a string terminator (BEL or ST = ESC \) starting at `from`.
 *  Returns the payload end and the index just past the terminator, or null
 *  if the string is still incomplete. */
function findStringTerminator(s: string, from: number): { end: number; next: number } | null {
  for (let i = from; i < s.length; i++) {
    const ch = s.charCodeAt(i);
    if (ch === 0x07) return { end: i, next: i + 1 };
    if (ch === 0x1b && i + 1 < s.length && s.charCodeAt(i + 1) === 0x5c) {
      return { end: i, next: i + 2 };
    }
  }
  return null;
}
```

In `write`, replace the incomplete-escape branch:

```ts
        if (consumed < 0) {
          // Incomplete — stash everything from here and wait for more, unless
          // it has grown past the cap (then drop the broken sequence).
          const rest = s.slice(i);
          this.pending = rest.length > MAX_PENDING ? '' : rest;
          return;
        }
```

- [ ] **Step 4: Parse OSC and DCS/SOS/PM/APC with the helper**

Replace the whole `if (intro === ']') { … }` branch in `parseEscape`:

```ts
    if (intro === ']') {
      // OSC: ESC ] ... BEL  or  ESC ] ... ESC \
      const t = findStringTerminator(s, start + 2);
      if (t === null) return -1; // incomplete OSC
      this.applyOsc(s.slice(start + 2, t.end));
      return t.next - start;
    }
    if (intro === 'P' || intro === 'X' || intro === '^' || intro === '_') {
      // DCS / SOS / PM / APC: string payloads we never act on (tmux
      // passthrough, XTGETTCAP, sixel, kitty graphics). Consume them so the
      // payload is not printed as text.
      const t = findStringTerminator(s, start + 2);
      if (t === null) return -1;
      return t.next - start;
    }
```

- [ ] **Step 5: Decode OSC 52 as UTF-8**

In `applyOsc`, replace the `try` block:

```ts
    try {
      // atob yields one char per byte; decode those bytes as UTF-8 so
      // non-ASCII text (e.g. Slovak diacritics) survives the copy.
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const text = new TextDecoder().decode(bytes);
      if (this.onClipboard) this.onClipboard(text);
    } catch {
      // Invalid base64 — ignore
    }
```

- [ ] **Step 6: Recognise all private markers and ignore private `m`/`s`/`u`**

In `applyCsi`, replace the marker strip:

```ts
    // Leading private-mode markers ('?', '>', '<', '=', '!'). Private forms of
    // m/s/u/L/M/… are different commands (modifyOtherKeys, kitty keyboard
    // push/pop, XTSMGRAPHICS…) and must not run as their public counterparts.
    let isPrivate = false;
    if (body.length > 0 && '?><=!'.includes(body[0])) {
      isPrivate = true;
      body = body.slice(1);
    }
```

Replace cases `'s'`, `'u'`, `'m'`:

```ts
      case 's': // save cursor (ANSI.SYS variant)
        if (isPrivate) return;
        this.savedRow = this.cursorRow;
        this.savedCol = this.cursorCol;
        return;
      case 'u': // restore cursor (private forms are kitty keyboard push/pop/query)
        if (isPrivate) return;
        this.restoreCursor();
        return;
      case 'm': // SGR (private `>…m` is modifyOtherKeys)
        if (isPrivate) return;
        this.applySgr(params);
        return;
```

- [ ] **Step 7: Reset modes on `ESC c`**

At the end of `fullReset`, after `this.savedScreen = null;`:

```ts
    this._mouse1000 = false;
    this._mouse1002 = false;
    this._mouse1003 = false;
    this._mouse1006 = false;
    this.bracketedPaste = false;
    this.cursorVisible = true;
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
/usr/bin/git add src/lib/ansi.ts src/lib/ansi.test.ts
/usr/bin/git commit -m "fix(terminal): ignore private CSI m/s/u, consume DCS/APC strings, reset modes on RIS, UTF-8 OSC 52"
```

---

### Task 3: SGR completeness and `runStyle` extraction

Gaps: strikethrough (9/29) and hidden (8/28) are dropped; ITU colon sub-parameters (`38:2::R:G:B`, `4:3`) lose their colour or get misread; `58;2;R;G;B` (underline colour) is not consumed, so its `2` is applied as DIM. `runStyle` lives inside the component and is untested.

**Files:**
- Modify: `src/lib/ansi.ts` (attribute constants ~28-32, `applyCsi` case `'m'`, `applySgr` ~859-925)
- Create: `src/lib/terminal_style.ts`
- Modify: `src/lib/TerminalView.svelte` (remove `styleCache` + `runStyle` ~786-839, import the module)
- Test: `src/lib/ansi.test.ts`, `src/lib/terminal_style.test.ts`

**Interfaces:**
- Consumes: `Run` from `ansi.ts` (Task 10 adds `cells: number` to it; `runStyle` ignores that field).
- Produces: exported `ATTR_HIDDEN = 1 << 5`, `ATTR_STRIKE = 1 << 6`, `sgrParams(body: string): number[]` in `ansi.ts`; `runStyle(run: Run): string` in `terminal_style.ts`.

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/ansi.test.ts` (add `ATTR_HIDDEN`, `ATTR_STRIKE`, `sgrParams` to the import list at the top):

```ts
describe('ansi SGR completeness', () => {
  it('sets and clears strikethrough and hidden', () => {
    const s = new Screen(1, 10);
    s.write('\x1b[9;8mA\x1b[29;28mB');
    expect(s.cells[0][0].attrs).toBe(ATTR_STRIKE | ATTR_HIDDEN);
    expect(s.cells[0][1].attrs).toBe(0);
  });

  it('accepts ITU colon truecolor with and without a colour-space id', () => {
    const s = new Screen(1, 10);
    s.write('\x1b[38:2::255:0:0mA\x1b[48:2:0:255:0mB');
    expect(s.cells[0][0].fg).toBe(rgb(255, 0, 0));
    expect(s.cells[0][1].bg).toBe(rgb(0, 255, 0));
  });

  it('consumes underline colour (58) without applying its sub-params', () => {
    const s = new Screen(1, 10);
    s.write('\x1b[58;2;10;20;30mA\x1b[58:5:9mB');
    expect(s.cells[0][0].attrs).toBe(0);
    expect(s.cells[0][1].attrs).toBe(0);
  });

  it('maps colon underline styles: 4:3 underlines, 4:0 clears', () => {
    const s = new Screen(1, 10);
    s.write('\x1b[4:3mA\x1b[4:0mB');
    expect(s.cells[0][0].attrs).toBe(ATTR_UNDERLINE);
    expect(s.cells[0][1].attrs).toBe(0);
  });

  it('sgrParams normalises colon groups to the semicolon form', () => {
    expect(sgrParams('')).toEqual([]);
    expect(sgrParams('1;38:5:196')).toEqual([1, 38, 5, 196]);
    expect(sgrParams('38:2::1:2:3')).toEqual([38, 2, 1, 2, 3]);
  });
});
```

Create `src/lib/terminal_style.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { runStyle } from './terminal_style';
import {
  ATTR_BOLD, ATTR_REVERSE, ATTR_UNDERLINE, ATTR_STRIKE, ATTR_HIDDEN, COLOR_DEFAULT, type Run,
} from './ansi';

const run = (over: Partial<Run>): Run =>
  ({ text: 'x', fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, attrs: 0, cells: 1, ...over }) as Run;

describe('runStyle', () => {
  it('is empty for a default run', () => {
    expect(runStyle(run({}))).toBe('');
  });

  it('swaps default colours for reverse video', () => {
    expect(runStyle(run({ attrs: ATTR_REVERSE }))).toBe('color:#0a0a0a;background:#e8e8e8');
  });

  it('combines underline and strikethrough in one text-decoration', () => {
    expect(runStyle(run({ attrs: ATTR_UNDERLINE | ATTR_STRIKE }))).toBe(
      'text-decoration:underline line-through',
    );
  });

  it('hides text but keeps the background', () => {
    expect(runStyle(run({ bg: 1, attrs: ATTR_HIDDEN | ATTR_BOLD }))).toBe(
      'background:#cd3131;font-weight:600;color:transparent',
    );
  });
});
```

(The `cells: 1` field and the cast keep this test valid before and after Task 10 adds `cells` to `Run`.)

- [ ] **Step 2: Run to verify they fail**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/terminal_style.test.ts`
Expected: FAIL — `ATTR_STRIKE`/`sgrParams` are not exported; `./terminal_style` does not exist.

- [ ] **Step 3: Add the attributes and `sgrParams` to `ansi.ts`**

After `export const ATTR_REVERSE = 1 << 4;`:

```ts
export const ATTR_HIDDEN = 1 << 5;
export const ATTR_STRIKE = 1 << 6;
```

Before `function makeRow`:

```ts
/** Normalise ITU colon sub-parameters to the semicolon form `applySgr`
 *  understands: `38:2::R:G:B` / `38:2:R:G:B` → 38;2;R;G;B, `38:5:N` →
 *  38;5;N (same for 48 and 58), `4:0` → 24, `4:N` → 4. */
export function sgrParams(body: string): number[] {
  if (body === '') return [];
  const out: number[] = [];
  for (const group of body.split(';')) {
    const parts = group.split(':').map((x) => (x === '' ? 0 : parseInt(x, 10) || 0));
    if (parts.length === 1) {
      out.push(parts[0]);
      continue;
    }
    const [p, mode] = parts;
    const isColor = p === 38 || p === 48 || p === 58;
    if (isColor && mode === 2) {
      // 6 parts carry a colour-space id before R:G:B; 5 parts do not.
      out.push(p, 2, ...(parts.length >= 6 ? parts.slice(3, 6) : parts.slice(2, 5)));
    } else if (isColor && mode === 5) {
      out.push(p, 5, parts[2] ?? 0);
    } else if (p === 4) {
      out.push(mode === 0 ? 24 : 4);
    } else {
      out.push(p);
    }
  }
  return out;
}
```

In `applyCsi`, the `'m'` case (from Task 2) becomes:

```ts
      case 'm': // SGR (private `>…m` is modifyOtherKeys)
        if (isPrivate) return;
        this.applySgr(sgrParams(body));
        return;
```

- [ ] **Step 4: Extend `applySgr`**

Insert these branches in `applySgr`, directly after the `p === 7` branch:

```ts
      } else if (p === 8) {
        this.curAttrs |= ATTR_HIDDEN;
      } else if (p === 9) {
        this.curAttrs |= ATTR_STRIKE;
```

directly after the `p === 27` branch:

```ts
      } else if (p === 28) {
        this.curAttrs &= ~ATTR_HIDDEN;
      } else if (p === 29) {
        this.curAttrs &= ~ATTR_STRIKE;
```

and directly after the `p === 49` branch:

```ts
      } else if (p === 58) {
        // Underline colour: not rendered, but its sub-params must be consumed
        // so `2`/`5` are not re-read as DIM / blink.
        if (params[i + 1] === 5 && i + 2 < params.length) i += 2;
        else if (params[i + 1] === 2 && i + 4 < params.length) i += 4;
        else i = params.length;
      } else if (p === 59) {
        // Default underline colour — nothing to do.
```

- [ ] **Step 5: Create `src/lib/terminal_style.ts`**

```ts
import {
  ATTR_BOLD, ATTR_DIM, ATTR_ITALIC, ATTR_UNDERLINE, ATTR_REVERSE, ATTR_HIDDEN, ATTR_STRIKE,
  colorToCss, type Run,
} from './ansi';

/** Grid defaults — must match `.grid` color/background in TerminalView.svelte. */
const DEFAULT_FG = '#e8e8e8';
const DEFAULT_BG = '#0a0a0a';

// A screen uses only a handful of (fg, bg, attrs) combos, but runStyle runs for
// every run on every render — caching collapses it to a Map lookup.
const cache = new Map<string, string>();

/** Inline CSS for one styled run. */
export function runStyle(run: Run): string {
  const key = `${run.fg}|${run.bg}|${run.attrs}`;
  const hit = cache.get(key);
  if (hit !== undefined) return hit;
  const parts: string[] = [];
  let fg = colorToCss(run.fg);
  let bg = colorToCss(run.bg);
  // Reverse video: swap fg/bg, substituting the grid defaults. This is how
  // claude/tmux draw the input caret and selections.
  if (run.attrs & ATTR_REVERSE) {
    const f = fg ?? DEFAULT_FG;
    const b = bg ?? DEFAULT_BG;
    fg = b;
    bg = f;
  }
  if (fg) parts.push(`color:${fg}`);
  if (bg) parts.push(`background:${bg}`);
  if (run.attrs & ATTR_BOLD) parts.push('font-weight:600');
  if (run.attrs & ATTR_DIM) parts.push('opacity:0.75');
  if (run.attrs & ATTR_ITALIC) parts.push('font-style:italic');
  const deco: string[] = [];
  if (run.attrs & ATTR_UNDERLINE) deco.push('underline');
  if (run.attrs & ATTR_STRIKE) deco.push('line-through');
  if (deco.length > 0) parts.push(`text-decoration:${deco.join(' ')}`);
  // Last, so it overrides any colour above.
  if (run.attrs & ATTR_HIDDEN) parts.push('color:transparent');
  const style = parts.join(';');
  cache.set(key, style);
  return style;
}
```

- [ ] **Step 6: Use it in the component**

In `TerminalView.svelte`, delete the `styleCache` declaration and the whole `function runStyle(run: Run): string { … }`, and add the import:

```ts
  import { runStyle } from './terminal_style';
```

The template call `style={runStyle(run)}` stays as is.

- [ ] **Step 7: Run the tests and type-check**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/terminal_style.test.ts`
Expected: PASS.

Run: `npx -y pnpm@10 run check`
Expected: 0 errors. (If `Run` has no `cells` yet, the cast in the test keeps it valid.)

- [ ] **Step 8: Commit**

```bash
/usr/bin/git add src/lib/ansi.ts src/lib/ansi.test.ts src/lib/terminal_style.ts src/lib/terminal_style.test.ts src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): SGR strikethrough/hidden, colon sub-params, underline colour; extract runStyle"
```

---

### Task 4: `PtyBuffer` — overflow reset flag, sticky EOF, race-free UTF-8 tail

Today the reader thread trims the oldest bytes when the 1 MiB cap is hit (`pty.rs:249-252`): that splits escape sequences and drops mode switches no redraw replays, and it memmoves 1 MiB per 4 KB read while pinned. `pty_drain` pushes an incomplete UTF-8 tail back in a second lock (a `pty_open` in between prepends it to the new session), and an invalid byte anywhere makes the trailing partial codepoint decode lossily. EOF is signalled only by in-band text the frontend string-searches.

**Files:**
- Create: `src-tauri/src/pty_buffer.rs`
- Modify: `src-tauri/src/lib.rs:10` (module declaration)
- Test: unit tests inside `src-tauri/src/pty_buffer.rs`

**Interfaces:**
- Produces (used by Task 7):
  - `pub const PTY_BUFFER_CAP: usize = 1 << 20;`
  - `pub struct PtyBuffer` with `pub fn new() -> Self`, `pub fn push(&mut self, chunk: &[u8])`, `pub fn finish(&mut self, note: &[u8])`, `pub fn take(&mut self) -> Taken`
  - `pub struct Taken { pub raw: Vec<u8>, pub eof: bool, pub reset: bool }`
  - `#[derive(Serialize)] pub struct Drained { pub data: String, pub bytes: usize, pub eof: bool, pub reset: bool }` and `pub fn decode(t: Taken) -> Drained`
  - `pub fn incomplete_utf8_tail(b: &[u8]) -> usize`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/lib.rs`, directly below `mod pty;` add:

```rust
mod pty_buffer;
```

- [ ] **Step 2: Write the module skeleton with failing tests**

Create `src-tauri/src/pty_buffer.rs`:

```rust
//! Byte buffer between the PTY reader thread and `pty_drain`.
//!
//! Free of Tauri types so it is unit-testable. One buffer exists per
//! `pty_open`, so bytes from a previous session can never reach a new screen.

use serde::Serialize;

/// Hard cap on un-drained bytes (1 MiB). Past it everything is discarded and a
/// reset is flagged: trimming at an arbitrary byte splits escape sequences and
/// can drop mode switches (alt screen, mouse, bracketed paste) that a redraw
/// never replays, so the frontend must re-attach instead.
pub const PTY_BUFFER_CAP: usize = 1 << 20;

#[derive(Debug, Default)]
pub struct PtyBuffer {
    bytes: Vec<u8>,
    overflowed: bool,
    eof: bool,
}

/// Raw bytes taken under the lock; decode them with [`decode`] after releasing it.
#[derive(Debug, PartialEq, Eq)]
pub struct Taken {
    pub raw: Vec<u8>,
    pub eof: bool,
    pub reset: bool,
}

/// What one `pty_drain` returns to the frontend.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Drained {
    pub data: String,
    pub bytes: usize,
    pub eof: bool,
    pub reset: bool,
}

impl PtyBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, _chunk: &[u8]) {
        unimplemented!()
    }

    pub fn finish(&mut self, _note: &[u8]) {
        unimplemented!()
    }

    pub fn take(&mut self) -> Taken {
        unimplemented!()
    }
}

pub fn decode(_t: Taken) -> Drained {
    unimplemented!()
}

pub fn incomplete_utf8_tail(_b: &[u8]) -> usize {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(b: &mut PtyBuffer) -> Drained {
        decode(b.take())
    }

    #[test]
    fn push_then_take_returns_all_bytes() {
        let mut b = PtyBuffer::new();
        b.push(b"hello ");
        b.push(b"world");
        assert_eq!(
            drain(&mut b),
            Drained { data: "hello world".into(), bytes: 11, eof: false, reset: false }
        );
        assert_eq!(drain(&mut b).bytes, 0);
    }

    #[test]
    fn incomplete_codepoint_waits_for_the_next_drain() {
        let mut b = PtyBuffer::new();
        b.push(b"a\xE2\x94"); // "a" + first two bytes of U+2500 '─'
        assert_eq!(drain(&mut b).data, "a");
        b.push(b"\x80");
        assert_eq!(drain(&mut b).data, "─");
    }

    #[test]
    fn invalid_byte_earlier_does_not_swallow_the_tail() {
        let mut b = PtyBuffer::new();
        b.push(b"\xFFx\xC3"); // invalid byte, 'x', first byte of 'é'
        assert_eq!(drain(&mut b).data, "\u{FFFD}x");
        b.push(b"\xA9");
        assert_eq!(drain(&mut b).data, "é");
    }

    #[test]
    fn overflow_discards_everything_and_flags_reset_once() {
        let mut b = PtyBuffer::new();
        b.push(&vec![b'a'; PTY_BUFFER_CAP]);
        b.push(b"b"); // one byte past the cap
        b.push(b"dropped too");
        let first = drain(&mut b);
        assert_eq!((first.bytes, first.reset), (0, true));
        b.push(b"after");
        assert_eq!(
            drain(&mut b),
            Drained { data: "after".into(), bytes: 5, eof: false, reset: false }
        );
    }

    #[test]
    fn finish_flushes_the_tail_and_eof_is_sticky() {
        let mut b = PtyBuffer::new();
        b.push(b"\xC3");
        b.finish(b"[eof]");
        let first = drain(&mut b);
        assert_eq!(first.data, "\u{FFFD}[eof]");
        assert!(first.eof);
        assert!(drain(&mut b).eof);
    }

    #[test]
    fn tail_lengths() {
        assert_eq!(incomplete_utf8_tail(b""), 0);
        assert_eq!(incomplete_utf8_tail(b"abc"), 0);
        assert_eq!(incomplete_utf8_tail("é".as_bytes()), 0);
        assert_eq!(incomplete_utf8_tail(b"\xC3"), 1);
        assert_eq!(incomplete_utf8_tail(b"\xE2\x94"), 2);
        assert_eq!(incomplete_utf8_tail(b"\xF0\x9F\xA4"), 3);
        assert_eq!(incomplete_utf8_tail("🤖".as_bytes()), 0);
        assert_eq!(incomplete_utf8_tail(b"\x80\x80\x80"), 0);
    }
}
```

- [ ] **Step 3: Run to verify the tests fail**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty_buffer`
Expected: compiles; every `pty_buffer::tests::*` test FAILS with `not implemented`.

- [ ] **Step 4: Implement**

Replace the `impl PtyBuffer`, `decode`, and `incomplete_utf8_tail` stubs:

```rust
impl PtyBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append bytes read from the PTY. After an overflow, everything is
    /// dropped until the next `take` reports the reset.
    pub fn push(&mut self, chunk: &[u8]) {
        if self.overflowed {
            return;
        }
        if self.bytes.len() + chunk.len() > PTY_BUFFER_CAP {
            self.bytes = Vec::new();
            self.overflowed = true;
            return;
        }
        self.bytes.extend_from_slice(chunk);
    }

    /// The reader hit EOF or an error. `note` is a human-readable line shown in
    /// the terminal; `eof` stays set for every later `take`.
    pub fn finish(&mut self, note: &[u8]) {
        if !self.overflowed {
            self.bytes.extend_from_slice(note);
        }
        self.eof = true;
    }

    /// Take everything except an incomplete trailing UTF-8 sequence, which
    /// stays in the buffer for the next drain (all in one lock hold, so no
    /// concurrent `pty_open` can receive it). The reset flag is one-shot.
    pub fn take(&mut self) -> Taken {
        let reset = std::mem::take(&mut self.overflowed);
        let keep = if self.eof { 0 } else { incomplete_utf8_tail(&self.bytes) };
        let raw = if keep == 0 {
            std::mem::take(&mut self.bytes)
        } else {
            let cut = self.bytes.len() - keep;
            self.bytes.drain(..cut).collect()
        };
        Taken { raw, eof: self.eof, reset }
    }
}

/// Decode taken bytes. Call after releasing the buffer lock — decoding is the
/// bulk of a drain and must not block the reader thread.
pub fn decode(t: Taken) -> Drained {
    Drained {
        bytes: t.raw.len(),
        data: String::from_utf8_lossy(&t.raw).into_owned(),
        eof: t.eof,
        reset: t.reset,
    }
}

/// Length (0..=3) of a trailing UTF-8 sequence cut off mid-codepoint. Only the
/// last three bytes are inspected, so an invalid byte earlier in the buffer no
/// longer forces the tail to be decoded lossily.
pub fn incomplete_utf8_tail(b: &[u8]) -> usize {
    let n = b.len();
    for back in 1..=n.min(3) {
        let byte = b[n - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue; // continuation byte: keep looking for the lead
        }
        let need = if byte & 0b1110_0000 == 0b1100_0000 {
            2
        } else if byte & 0b1111_0000 == 0b1110_0000 {
            3
        } else if byte & 0b1111_1000 == 0b1111_0000 {
            4
        } else {
            return 0; // ASCII or invalid lead: nothing pending
        };
        return if need > back { back } else { 0 };
    }
    0
}
```

- [ ] **Step 5: Run to verify the tests pass**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty_buffer`
Expected: `test result: ok. 6 passed`.

The module is not used yet, so clippy would warn `dead_code`. Add `#![allow(dead_code)] // wired in by Task 7` as the first line of `pty_buffer.rs`; Task 7 removes it.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add src-tauri/src/pty_buffer.rs src-tauri/src/lib.rs
/usr/bin/git commit -m "feat(pty): PtyBuffer with overflow reset flag, sticky EOF and race-free UTF-8 tail"
```

---

### Task 5: `PtyWriter` — PTY input that never blocks the caller

`pty_write` is a non-async Tauri command, so it runs on the main thread, and it calls a blocking `write_all` + `flush` while holding the PTY mutex (`pty.rs:340-355`). If the child stops reading (ssh stalled on a dead link, PTY input queue full), a paste freezes the whole app.

**Files:**
- Create: `src-tauri/src/pty_writer.rs`
- Modify: `src-tauri/src/lib.rs` (module declaration)
- Test: unit tests inside `src-tauri/src/pty_writer.rs`

**Interfaces:**
- Produces (used by Task 7):
  - `pub const PTY_WRITE_QUEUE_CAP: usize = 4 << 20;`
  - `pub enum SendError { Full, Closed }` (derives `Debug, PartialEq, Eq`)
  - `pub struct PtyWriter` with `pub fn spawn(writer: Box<dyn Write + Send>) -> Self` and `pub fn send(&self, bytes: &[u8]) -> Result<(), SendError>`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/lib.rs`, below `mod pty_buffer;` add:

```rust
mod pty_writer;
```

- [ ] **Step 2: Write the module skeleton with failing tests**

Create `src-tauri/src/pty_writer.rs`:

```rust
//! Non-blocking PTY input.
//!
//! `pty_write` runs on Tauri's main thread. A blocking `write_all` there (ssh
//! stalled on a dead link, PTY input queue full) froze the whole app, so bytes
//! go through a channel to a dedicated writer thread instead.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

/// Max bytes queued but not yet written (4 MiB). A paste into a session that
/// is not reading is refused past this instead of growing memory unbounded.
pub const PTY_WRITE_QUEUE_CAP: usize = 4 << 20;

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// The queue is over [`PTY_WRITE_QUEUE_CAP`].
    Full,
    /// The writer thread exited (write error / PTY closed).
    Closed,
}

pub struct PtyWriter {
    tx: Sender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
}

impl PtyWriter {
    pub fn spawn(_writer: Box<dyn Write + Send>) -> Self {
        unimplemented!()
    }

    pub fn send(&self, _bytes: &[u8]) -> Result<(), SendError> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Records everything written into a shared Vec.
    struct Recorder(Arc<Mutex<Vec<u8>>>);
    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Blocks every write until the test sends on the gate (or drops it).
    struct Gated(Mutex<Receiver<()>>);
    impl Write for Gated {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.lock().unwrap().recv();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Fails every write.
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("pty gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn eventually(mut check: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn bytes_arrive_in_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let w = PtyWriter::spawn(Box::new(Recorder(Arc::clone(&seen))));
        w.send(b"ab").unwrap();
        w.send(b"cd").unwrap();
        assert!(eventually(|| seen.lock().unwrap().as_slice() == b"abcd"));
    }

    #[test]
    fn send_returns_immediately_while_the_writer_is_blocked() {
        let (_gate, rx) = channel::<()>();
        let w = PtyWriter::spawn(Box::new(Gated(Mutex::new(rx))));
        let start = Instant::now();
        for _ in 0..3 {
            w.send(b"paste").unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn refuses_past_the_queue_cap() {
        let (_gate, rx) = channel::<()>();
        let w = PtyWriter::spawn(Box::new(Gated(Mutex::new(rx))));
        w.send(&vec![0u8; PTY_WRITE_QUEUE_CAP]).unwrap();
        assert_eq!(w.send(b"x"), Err(SendError::Full));
    }

    #[test]
    fn reports_closed_after_a_write_error() {
        let w = PtyWriter::spawn(Box::new(Broken));
        w.send(b"x").unwrap();
        assert!(eventually(|| w.send(b"y") == Err(SendError::Closed)));
    }
}
```

- [ ] **Step 3: Run to verify the tests fail**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty_writer`
Expected: every `pty_writer::tests::*` test FAILS with `not implemented`.

- [ ] **Step 4: Implement**

Replace the `impl PtyWriter` block:

```rust
impl PtyWriter {
    /// Move `writer` onto its own thread. The thread exits when the
    /// `PtyWriter` is dropped (channel closed) or a write fails.
    pub fn spawn(mut writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = channel::<Vec<u8>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let q = Arc::clone(&queued);
        std::thread::spawn(move || {
            for chunk in rx {
                let ok = writer.write_all(&chunk).and_then(|_| writer.flush()).is_ok();
                q.fetch_sub(chunk.len(), Ordering::SeqCst);
                if !ok {
                    break;
                }
            }
        });
        Self { tx, queued }
    }

    /// Queue bytes for the PTY without blocking.
    pub fn send(&self, bytes: &[u8]) -> Result<(), SendError> {
        let len = bytes.len();
        let before = self.queued.fetch_add(len, Ordering::SeqCst);
        if before + len > PTY_WRITE_QUEUE_CAP {
            self.queued.fetch_sub(len, Ordering::SeqCst);
            return Err(SendError::Full);
        }
        self.tx.send(bytes.to_vec()).map_err(|_| {
            self.queued.fetch_sub(len, Ordering::SeqCst);
            SendError::Closed
        })
    }
}
```

Add `#![allow(dead_code)] // wired in by Task 7` as the first line of `pty_writer.rs`.

- [ ] **Step 5: Run to verify the tests pass**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty_writer`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add src-tauri/src/pty_writer.rs src-tauri/src/lib.rs
/usr/bin/git commit -m "feat(pty): channel-backed PtyWriter so terminal input never blocks the main thread"
```

---

### Task 6: ssh keepalive and a testable `attach_command`

No ssh invocation except `service/tunnel.rs` sets `ServerAliveInterval` (the `ssh_config.rs` hit is a test fixture). On a dead link ssh never notices: the PTY reader never gets EOF, so no "Connection lost" banner ever shows. `pty_open` also hand-copies the ControlMaster options instead of sharing `SshClient::mux_opts`.

**Files:**
- Modify: `src-tauri/src/ssh.rs` (`mux_opts` ~110-123 and its test ~322-328)
- Modify: `src-tauri/src/pty.rs` (command construction ~110-178 moves into `attach_command`)
- Test: `src-tauri/src/ssh.rs` tests, new `#[cfg(test)] mod tests` in `src-tauri/src/pty.rs`

**Interfaces:**
- Produces: `pub(crate) fn mux_opts(&self, host: &str, timeout: Duration) -> Vec<String>` on `SshClient`; `pub(crate) fn attach_command(ssh: &SshClient, host_alias: &str, session_name: &str) -> CommandBuilder` in `pty.rs` (Task 7 calls it).

- [ ] **Step 1: Write the failing tests**

In `src-tauri/src/ssh.rs`, extend `mux_opts_carry_controlmaster_auto_and_persist`:

```rust
    #[test]
    fn mux_opts_carry_controlmaster_auto_and_persist() {
        let c = SshClient::new();
        let opts = c.mux_opts("h", Duration::from_secs(5));
        assert!(opts.iter().any(|o| o == "ControlMaster=auto"));
        assert!(opts.iter().any(|o| o == "ControlPersist=10m"));
        assert!(opts.iter().any(|o| o == "ConnectTimeout=5"));
        // A dead link must end the connection (and any attached PTY) within
        // ~45 s instead of hanging forever.
        assert!(opts.iter().any(|o| o == "ServerAliveInterval=15"));
        assert!(opts.iter().any(|o| o == "ServerAliveCountMax=3"));
    }
```

At the bottom of `src-tauri/src/pty.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn argv(c: &CommandBuilder) -> Vec<String> {
        c.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn remote_attach_uses_shared_mux_opts_and_ends_option_parsing() {
        let ssh = SshClient::new();
        let a = argv(&attach_command(&ssh, "box", "dev-x"));
        assert_eq!(&a[..2], ["ssh", "-tt"]);
        assert!(a.iter().any(|o| o == "ControlMaster=auto"));
        assert!(a.iter().any(|o| o == "ServerAliveInterval=15"));
        let dd = a.iter().position(|o| o == "--").expect("`--` before the host");
        assert_eq!(&a[dd + 1..dd + 4], ["box", "bash", "-lc"]);
        let script = a.last().unwrap();
        assert!(script.contains("tmux attach -t"), "{script}");
        assert!(script.contains("dev-x"), "{script}");
    }

    #[test]
    fn local_attach_runs_tmux_directly() {
        let ssh = SshClient::new();
        assert_eq!(argv(&attach_command(&ssh, "local", "dev-x")), ["tmux", "attach", "-t", "dev-x"]);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml attach`
Expected: compile error `cannot find function attach_command`.

- [ ] **Step 3: Add keepalive to `mux_opts` and widen its visibility**

In `src-tauri/src/ssh.rs`, change the signature to `pub(crate) fn mux_opts(&self, host: &str, timeout: Duration) -> Vec<String>` and append to the returned `vec![…]` after the `ConnectTimeout` entry:

```rust
            // Detect a dead link: 3 missed keepalives 15 s apart end the
            // connection, which EOFs every multiplexed client (incl. the PTY).
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
```

- [ ] **Step 4: Extract `attach_command` in `pty.rs`**

Add `use std::time::Duration;` to the imports and add, above `pty_open`:

```rust
/// ssh connect timeout for the attach (same value the PTY used before).
const ATTACH_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Build the `tmux attach` command: run locally for `"local"`, otherwise over
/// ssh through the shared ControlMaster options (which carry the keepalive, so
/// a dead link ends the PTY and the frontend shows "Connection lost").
pub(crate) fn attach_command(ssh: &SshClient, host_alias: &str, session_name: &str) -> CommandBuilder {
    let mut cmd = if host_alias == "local" {
        let mut c = CommandBuilder::new("tmux");
        c.args(["attach", "-t", session_name]);
        c
    } else {
        let mut c = CommandBuilder::new("ssh");
        c.arg("-tt");
        c.args(ssh.mux_opts(host_alias, ATTACH_CONNECT_TIMEOUT));
        // `--` ends ssh option parsing so a host alias can never be
        // interpreted as an option (defence-in-depth; the alias is also
        // validated by the caller).
        c.args(["--", host_alias, "bash", "-lc"]);
        // We re-export LANG/LC_ALL/COLORTERM/TERM inside the remote shell so
        // the embedded TUI gets proper Unicode glyph rendering even if the
        // remote sshd has AcceptEnv disabled.
        //
        // CRITICAL: `ssh <host> bash -lc <script>` joins all trailing argv
        // with spaces before sending to the remote sshd, which then
        // re-tokenizes. We MUST single-quote the whole script so it crosses
        // the ssh boundary as a single shell word; otherwise the remote bash
        // receives `LANG=...` as its -c argument and never runs tmux attach.
        // (Same fix shape as RemoteTmux::remote_bash in tmux.rs.)
        // `quote(session_name)` keeps its inner quoting; the outer wrap
        // escapes those single quotes via the canonical `'\''` dance.
        c.arg(quote(&format!(
            "LANG=${{LANG:-en_US.UTF-8}} LC_ALL=${{LC_ALL:-en_US.UTF-8}} COLORTERM=truecolor TERM=xterm-256color tmux attach -t {}",
            quote(session_name)
        )));
        c
    };
    // Inherit PATH that lib.rs already backfilled at startup so /opt/homebrew/bin
    // is visible to the spawned tmux.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TERM", "xterm-256color");
    // Inherit locale env (lib.rs imports/backfills these at startup so they're
    // populated even when launched from Finder). Without UTF-8 locale, claude
    // and other modern TUIs detect a degraded terminal and render ASCII
    // fallbacks (`_` instead of `└` / `↑` / `█` block glyphs).
    for var in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.env(var, val);
            }
        }
    }
    // COLORTERM=truecolor signals to apps (claude, vim, etc.) that they can
    // emit 24-bit SGR sequences. Our renderer supports them already.
    cmd.env("COLORTERM", "truecolor");
    cmd
}
```

In `pty_open`, replace everything from `let mut cmd = if args.host_alias == "local" {` through `cmd.env("COLORTERM", "truecolor");` with:

```rust
    let cmd = attach_command(&ssh, &args.host_alias, &args.session_name);
```

- [ ] **Step 5: Run the tests**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml attach`
Expected: `remote_attach_uses_shared_mux_opts_and_ends_option_parsing` and `local_attach_runs_tmux_directly` PASS.

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml mux_opts`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add src-tauri/src/ssh.rs src-tauri/src/pty.rs
/usr/bin/git commit -m "fix(pty): ssh keepalive via shared mux_opts; extract attach_command"
```

---

### Task 7: Wire `PtyBuffer` + `PtyWriter` into `pty.rs`; nothing blocks under the lock

**Files:**
- Modify: `src-tauri/src/pty.rs` (whole state/command section)
- Modify: `src-tauri/src/pty_buffer.rs`, `src-tauri/src/pty_writer.rs` (drop the `#![allow(dead_code)]` lines)
- Modify: `CLAUDE.md` (Conventions)
- Test: `src-tauri/src/pty.rs` tests module

**Interfaces:**
- Consumes: `PtyBuffer`, `Taken`, `Drained`, `decode` (Task 4); `PtyWriter`, `SendError` (Task 5); `attach_command` (Task 6).
- Produces: `pty_drain` returns `Drained` serialised as `{ data, bytes, eof, reset }` (Task 8 consumes it). New IPC error code `E_PTY_BUSY`. `pub(crate) const MIN_COLS: u16 = 10; pub(crate) const MIN_ROWS: u16 = 2; pub(crate) fn pty_size(cols: u16, rows: u16) -> PtySize`. `PtyState::close(&mut self)` keeps its signature (lib.rs exit handler).

- [ ] **Step 1: Write the failing size-floor test**

Add to the `tests` module in `pty.rs`:

```rust
    #[test]
    fn pty_size_floor_matches_the_frontend_grid_minimum() {
        // TerminalView.computeDimensions floors at 10 cols × 2 rows; a larger
        // backend floor made tmux draw for a bigger terminal than the grid.
        let s = pty_size(3, 1);
        assert_eq!((s.cols, s.rows), (10, 2));
        let s = pty_size(120, 40);
        assert_eq!((s.cols, s.rows), (120, 40));
    }
```

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty_size`
Expected: compile error `cannot find function pty_size`.

- [ ] **Step 2: Replace the state, close path and size helper**

In `pty.rs`, change the imports to:

```rust
use crate::ipc_error::IpcError;
use crate::pty_buffer::{decode, Drained, PtyBuffer};
use crate::pty_writer::{PtyWriter, SendError};
use crate::shell::quote;
use crate::ssh::SshClient;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;
```

Delete `const PTY_BUFFER_CAP` (it now lives in `pty_buffer.rs`). Replace the doc comment, `PtyState` struct and its `impl` block with:

```rust
/// One active PTY at a time (we render a single terminal pane). Opening a new
/// PTY closes the previous one.
///
/// Polling transport: the reader thread appends to `buffer`; the frontend
/// calls `pty_drain` every 30-250 ms. (Tauri 2 emits from the reader thread
/// were observed to silently never reach JS; polling has no missing-event
/// class of bugs.)
///
/// Rule: no blocking I/O (write, kill, wait) while holding the
/// `Mutex<PtyState>` guard — `pty_drain`/`pty_write` run on the main thread
/// and would freeze the app. Input goes through `PtyWriter`; teardown takes the
/// parts out with `detach` and shuts them down after the guard is dropped.
pub struct PtyState {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<PtyWriter>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    buffer: Arc<Mutex<PtyBuffer>>,
}

/// The parts of a PTY taken out of `PtyState`, to be shut down outside the lock.
pub(crate) struct Detached {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<PtyWriter>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl Detached {
    pub(crate) fn shutdown(mut self) {
        // Terminate the child with SIGKILL BEFORE tearing down the pty fds.
        //
        // The child is the local `tmux attach`, or — for a remote host — the
        // `ssh -tt … tmux attach` that runs it. portable-pty's Child::kill()
        // sends SIGHUP, not SIGKILL. At this point our reader thread still holds
        // a cloned master fd, so the pty is still LIVE when that SIGHUP lands —
        // which lets `ssh -tt` do a *graceful* shutdown and relay a trailing
        // newline down its pty to the remote tmux pane. That stray `\n` is
        // delivered into the attached app's (claude's) input on every detach /
        // session-switch / deselect — the long-standing "new line on switch"
        // bug. SIGKILL gives the child no chance to relay anything; the ssh
        // channel and remote tty then tear down on their own and tmux detaches
        // our client cleanly.
        if let Some(mut child) = self.child.take() {
            match child.process_id() {
                Some(pid) => {
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                }
                // No pid (already exited / unsupported): fall back to SIGHUP.
                None => {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
        // Child is gone: drop the input channel (its thread exits once its
        // current write errors out) and the master fd, so the reader sees EOF.
        self.writer.take();
        self.master.take();
    }
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            master: None,
            writer: None,
            child: None,
            buffer: Arc::new(Mutex::new(PtyBuffer::new())),
        }
    }

    /// Take every live part out and give the state a fresh, empty buffer.
    pub(crate) fn detach(&mut self) -> Detached {
        self.buffer = Arc::new(Mutex::new(PtyBuffer::new()));
        Detached {
            master: self.master.take(),
            writer: self.writer.take(),
            child: self.child.take(),
        }
    }

    /// Detach and shut down in place. Only for app exit (lib.rs), where
    /// blocking the event loop briefly is acceptable.
    pub(crate) fn close(&mut self) {
        self.detach().shutdown();
    }
}

/// Minimum PTY size. Must match `computeDimensions` in TerminalView.svelte
/// (cols ≥ 10, rows ≥ 2).
pub(crate) const MIN_COLS: u16 = 10;
pub(crate) const MIN_ROWS: u16 = 2;

pub(crate) fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(MIN_ROWS),
        cols: cols.max(MIN_COLS),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn lock_err() -> IpcError {
    IpcError::new("E_LOCK", "pty mutex poisoned")
}
```

- [ ] **Step 3: Replace `pty_open`**

Update the `PtyOpenArgs` doc comment on `cols`/`rows` to `/// Initial PTY size from the frontend's grid measurement.` and replace the whole `pty_open` function:

```rust
/// `async` so the spawn and the previous PTY's kill/wait run off the main thread.
#[tauri::command(async)]
pub fn pty_open(
    args: PtyOpenArgs,
    state: State<'_, Mutex<PtyState>>,
    ssh: State<'_, std::sync::Arc<SshClient>>,
) -> Result<(), IpcError> {
    // Validate untrusted IPC input before it reaches `ssh` / `tmux`.
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.session_name)?;

    let pair = native_pty_system()
        .openpty(pty_size(args.cols, args.rows))
        .map_err(|e| IpcError::new("E_PTY", format!("openpty: {e}")))?;
    let child = pair
        .slave
        .spawn_command(attach_command(&ssh, &args.host_alias, &args.session_name))
        .map_err(|e| IpcError::new("E_PTY", format!("spawn tmux attach: {e}")))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| IpcError::new("E_PTY", format!("clone reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| IpcError::new("E_PTY", format!("take writer: {e}")))?;

    // A FRESH buffer per open: a dying previous reader thread keeps its own
    // buffer, so stale bytes can never bleed into the new screen.
    let buffer = Arc::new(Mutex::new(PtyBuffer::new()));
    if let Ok(mut b) = buffer.lock() {
        b.push(
            format!(
                "\x1b[90m[cf] attached to {}@{} via polling buffer\x1b[0m\r\n",
                args.session_name, args.host_alias
            )
            .as_bytes(),
        );
    }

    let previous = {
        let mut s = state.lock().map_err(|_| lock_err())?;
        let previous = s.detach();
        s.master = Some(pair.master);
        s.writer = Some(PtyWriter::spawn(writer));
        s.child = Some(child);
        s.buffer = Arc::clone(&buffer);
        previous
    };
    previous.shutdown();

    std::thread::spawn(move || {
        let finish = |note: String| {
            if let Ok(mut b) = buffer.lock() {
                b.finish(note.as_bytes());
            }
        };
        let mut buf = [0u8; 4096];
        let mut total = 0usize;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    finish(format!(
                        "\r\n\x1b[33m[cf] PTY EOF after {total} bytes (tmux attach exited)\x1b[0m\r\n"
                    ));
                    break;
                }
                Ok(n) => {
                    total += n;
                    match buffer.lock() {
                        Ok(mut b) => b.push(&buf[..n]),
                        Err(_) => break,
                    }
                }
                Err(e) => {
                    finish(format!(
                        "\r\n\x1b[31m[cf] reader error after {total} bytes: {e}\x1b[0m\r\n"
                    ));
                    break;
                }
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 4: Replace `pty_drain`, `pty_write`, `pty_resize`, `pty_close`**

Delete the `PtyDrainResult` struct, then replace `pty_drain`, `pty_write`, `pty_resize` and `pty_close` (with their arg structs) with:

```rust
#[tauri::command]
pub fn pty_drain(state: State<'_, Mutex<PtyState>>) -> Result<Drained, IpcError> {
    // Hold the state lock only to clone the buffer handle, the buffer lock
    // only to take the bytes, and decode after releasing both.
    let buffer = Arc::clone(&state.lock().map_err(|_| lock_err())?.buffer);
    let taken = buffer
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "pty buffer poisoned"))?
        .take();
    Ok(decode(taken))
}

#[derive(Deserialize)]
pub struct PtyWriteArgs {
    pub data: String,
}

#[tauri::command]
pub fn pty_write(args: PtyWriteArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let s = state.lock().map_err(|_| lock_err())?;
    let writer = s
        .writer
        .as_ref()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    writer.send(args.data.as_bytes()).map_err(|e| match e {
        SendError::Full => IpcError::new(
            "E_PTY_BUSY",
            "terminal input queue is full (the session is not reading input)",
        ),
        SendError::Closed => IpcError::new("E_PTY_CLOSED", "PTY input closed"),
    })
}

#[derive(Deserialize)]
pub struct PtyResizeArgs {
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn pty_resize(args: PtyResizeArgs, state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let s = state.lock().map_err(|_| lock_err())?;
    let master = s
        .master
        .as_ref()
        .ok_or_else(|| IpcError::new("E_PTY_CLOSED", "no PTY open"))?;
    master
        .resize(pty_size(args.cols, args.rows))
        .map_err(|e| IpcError::new("E_PTY", format!("resize: {e}")))
}

/// `async`, and the kill/wait happens after the guard is dropped.
#[tauri::command(async)]
pub fn pty_close(state: State<'_, Mutex<PtyState>>) -> Result<(), IpcError> {
    let detached = state.lock().map_err(|_| lock_err())?.detach();
    detached.shutdown();
    Ok(())
}
```

Remove the `#![allow(dead_code)] // wired in by Task 7` first line from `pty_buffer.rs` and `pty_writer.rs`.

- [ ] **Step 5: Document the rule in `CLAUDE.md`**

Under `## Conventions`, after the SQLite bullet, add:

```markdown
- The PTY (`pty.rs`) must never do blocking I/O while holding the
  `Mutex<PtyState>` guard: `pty_drain`/`pty_write` run on the main thread.
  Input goes through `PtyWriter` (`pty_writer.rs`); teardown uses
  `detach()` + `shutdown()` outside the lock.
```

- [ ] **Step 6: Build, lint and test the backend**

Run: `env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml pty`
Expected: all `pty::tests`, `pty_buffer::tests`, `pty_writer::tests` PASS.

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
Expected: no output (if it prints a diff, run `cargo fmt --manifest-path src-tauri/Cargo.toml` and re-check).

Run: `env CARGO_BUILD_JOBS=6 cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
Expected: `Finished` with no warnings.

- [ ] **Step 7: Manual smoke test (desktop app)**

Run: `npx -y pnpm@10 tauri dev`, select a local session and a remote session in turn.
Expected: both attach and render; typing echoes; switching sessions shows no stale output and no stray newline in the previous session.

- [ ] **Step 8: Commit**

```bash
/usr/bin/git add src-tauri/src/pty.rs src-tauri/src/pty_buffer.rs src-tauri/src/pty_writer.rs CLAUDE.md
/usr/bin/git commit -m "fix(pty): non-blocking writes, EOF/reset flags, teardown outside the lock, 10x2 size floor"
```

---

### Task 8: Frontend drain lifecycle — EOF/reset flags, idle cost, stranded session switch

Requires PR B (Tasks 4–7) merged. Today disconnects are detected by searching output for `[cf] PTY EOF`, so printing that text (e.g. grepping this repo) raises a false banner. `drainTicks` is reactive and bumps every poll, re-rendering the header 4–33×/s while idle. If `pty_open` fails while the user has already clicked another session, the click (dropped by the `opening` guard) is never retried.

**Files:**
- Modify: `src/lib/TerminalView.svelte` (`drainTicks` ~66, `openTerm` ~448-520, `drainOnce` ~573-599, `closeTerm` ~607-640, header counters ~856-858)

**Interfaces:**
- Consumes: `applyDrain`, `DrainResult` (Task 1); backend `pty_drain` → `{ data, bytes, eof, reset }` (Task 7).

- [ ] **Step 1: Replace `drainOnce`**

```ts
  /** Drain the PTY buffer once. Returns true if any bytes were consumed. */
  async function drainOnce(): Promise<boolean> {
    if (!screen || !ptyOpen) return false;
    // Capture the screen we're draining into. If the session is switched
    // (openTerm builds a new Screen) while this pty_drain is in flight, the
    // resolved bytes belong to the old PTY — discard them.
    const drainingInto = screen;
    let result: DrainResult;
    try {
      result = await invoke<DrainResult>('pty_drain');
    } catch {
      return false;
    }
    if (screen !== drainingInto) return false;
    const outcome = applyDrain(screen, result);
    if (outcome.reset) {
      // The backend dropped output past its cap: our cells and mode flags
      // (alt screen, mouse, bracketed paste) can't be trusted. Re-attach so
      // tmux repaints and resends every mode.
      void reconnect();
      return false;
    }
    if (outcome.error) console.error('[terminal] parser error, chunk skipped:', outcome.error);
    if (outcome.wrote) {
      totalBytes += result.bytes;
      renderVersion++;
    }
    if (outcome.disconnected) disconnected = true;
    return outcome.wrote;
  }
```

Note the order: `applyDrain` runs first, but when `reset` is set the backend has already discarded the data, so nothing stale is written.

- [ ] **Step 2: Remove the per-tick reactive counter**

Delete `let drainTicks = $state(0);` and, in `closeTerm`, the line `drainTicks = 0;`. Replace the header counters span:

```svelte
      <span class="counters" data-testid="terminal-counters">{totalBytes}B</span>
```

- [ ] **Step 3: Retry a session switch that arrived during an open**

Add below `let opening = false;`:

```ts
  /** A selection change made while an open was in flight was dropped by the
   *  `opening` guard. If the user has since picked a different session, open
   *  it now (success or failure of the previous open). */
  function retryIfSelectionMoved(openedName: string) {
    const now = $selectedSession;
    if (now && now.tmux_name !== openedName) void openTerm();
  }
```

In `openTerm`, replace the `catch` block:

```ts
    } catch (e) {
      openError = `PTY error: ${describeError(e)}`;
      opening = false;
      retryIfSelectionMoved(sess.tmux_name);
      return;
    }
```

and replace the final `opening = false;` at the end of `openTerm`:

```ts
    opening = false;
    retryIfSelectionMoved(sess.tmux_name);
  }
```

- [ ] **Step 4: Verify**

Run: `npx -y pnpm@10 run check`
Expected: 0 errors (no remaining references to `drainTicks`).

Run: `npx -y pnpm@10 vitest run src/lib/terminal_drain.test.ts src/lib/ansi.test.ts`
Expected: PASS.

Manual (`npx -y pnpm@10 tauri dev`):
1. In an attached shell run `printf '[cf] PTY EOF\n'` → no "Connection lost" banner.
2. Run `tmux detach` inside the attached session → banner appears.
3. Select a session on an unreachable host, then immediately a local session → the local session attaches.

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): EOF/reset flags drive the banner and re-attach; retry dropped session switch; drop per-tick counter"
```

---

### Task 9: `charWidth` — tmux 3.6a display widths

**Files:**
- Create: `src/lib/wcwidth.ts`
- Test: `src/lib/wcwidth.test.ts`

**Interfaces:**
- Produces: `charWidth(cp: number): 0 | 1 | 2` and `const VS16 = 0xfe0f` (both exported). Task 10 uses both.

- [ ] **Step 1: Write the failing test**

Create `src/lib/wcwidth.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { charWidth, VS16 } from './wcwidth';

// Ground truth: tmux 3.6a `#{cursor_x}` after printing each glyph (2026-09-15).
describe('charWidth matches tmux 3.6a', () => {
  it.each([
    ['⏺', 1], ['✻', 1], ['⎿', 1], ['─', 1], ['█', 1], ['❤', 1],
    ['a', 1], ['á', 1], ['ž', 1],
    ['中', 2], ['文', 2], ['한', 2], ['🤖', 2], ['☕', 2], ['✅', 2],
    ['\u0301', 0], ['\u200d', 0], ['\ufe0f', 0],
  ] as const)('%s → %i', (ch, want) => {
    expect(charWidth(ch.codePointAt(0)!)).toBe(want);
  });

  it('exports the VS16 code point', () => {
    expect(VS16).toBe(0xfe0f);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx -y pnpm@10 vitest run src/lib/wcwidth.test.ts`
Expected: FAIL — cannot resolve `./wcwidth`.

- [ ] **Step 3: Implement**

Create `src/lib/wcwidth.ts`:

```ts
/**
 * Display width of one code point, as tmux 3.6a counts it (measured with
 * `#{cursor_x}`). The screen model must agree with tmux exactly: tmux
 * addresses later updates by column, so any disagreement misplaces them.
 *
 *   0 — combining marks (Mn/Me) and format chars (Cf: ZWJ, VS16 …)
 *   2 — East Asian Wide/Fullwidth, and emoji with default emoji presentation
 *   1 — everything else (incl. ⏺ ✻ ⎿ box drawing, ❤ without VS16)
 *
 * A narrow glyph FOLLOWED by VS16 is drawn two columns wide; the Screen
 * handles that when the VS16 arrives.
 */

export const VS16 = 0xfe0f;

const ZERO_WIDTH = /^[\p{Mn}\p{Me}\p{Cf}]$/u;
const EMOJI_PRESENTATION = /^\p{Emoji_Presentation}$/u;

/** East Asian Wide (W) and Fullwidth (F) blocks not covered by the emoji test. */
const WIDE_RANGES: ReadonlyArray<readonly [number, number]> = [
  [0x1100, 0x115f], // Hangul Jamo initial consonants
  [0x2e80, 0x303e], // CJK radicals, Kangxi, CJK symbols and punctuation
  [0x3041, 0x33ff], // Hiragana, Katakana, Bopomofo, CJK compatibility
  [0x3400, 0x4dbf], // CJK Extension A
  [0x4e00, 0x9fff], // CJK Unified Ideographs
  [0xa000, 0xa4cf], // Yi
  [0xa960, 0xa97f], // Hangul Jamo Extended-A
  [0xac00, 0xd7a3], // Hangul syllables
  [0xf900, 0xfaff], // CJK compatibility ideographs
  [0xfe10, 0xfe19], // Vertical forms
  [0xfe30, 0xfe6f], // CJK compatibility forms, small form variants
  [0xff00, 0xff60], // Fullwidth forms
  [0xffe0, 0xffe6], // Fullwidth signs
  [0x20000, 0x2fffd], // CJK Extensions B–F
  [0x30000, 0x3fffd], // CJK Extension G+
];

export function charWidth(cp: number): 0 | 1 | 2 {
  // Fast path: ASCII, Latin-1 and Latin Extended-A/B are all width 1 (the
  // parser handles C0 controls before calling this).
  if (cp < 0x0300) return 1;
  const ch = String.fromCodePoint(cp);
  if (ZERO_WIDTH.test(ch)) return 0;
  if (EMOJI_PRESENTATION.test(ch)) return 2;
  for (const [lo, hi] of WIDE_RANGES) {
    if (cp >= lo && cp <= hi) return 2;
  }
  return 1;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx -y pnpm@10 vitest run src/lib/wcwidth.test.ts`
Expected: PASS (19 tests).

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add src/lib/wcwidth.ts src/lib/wcwidth.test.ts
/usr/bin/git commit -m "feat(terminal): charWidth matching tmux 3.6a display widths"
```

---

### Task 10: tmux golden fixture, then tmux-accurate wide and combining cells

Confirmed today: the parser uses one cell per UTF-16 code unit. CJK takes 1 cell (tmux: 2), a combining accent takes its own cell (tmux: 0), an emoji is split into two lone-surrogate cells. tmux's attach paint still looks right, but its column-addressed updates land wrong. Against the recorded fixture the current parser gets `CJK:中文X Y` (want `CJK:中文Y`), `emoji:\ud83eZX` (want `emoji: ZX`), `accent:aYX` (want `accent:áY`). The code in this task was validated in the scratchpad (all assertions and the fixture pass, one write and 1–64-char random chunks).

**Files:**
- Create: `scripts/capture-tmux-fixture.sh`
- Create: `src/lib/__fixtures__/tmux-attach.raw.txt`, `src/lib/__fixtures__/tmux-attach.pane.txt` (generated)
- Create: `.gitattributes`
- Create: `src/lib/ansi.tmux-fixture.test.ts`
- Modify: `src/lib/ansi.ts` (`Cell` doc ~17-25, header comment ~1-15, `write` printable path ~338-346, `putChar` ~349-367, `resize` ~261-283, `deleteChars`/`insertChars` ~827-840, `clearCell` ~850-857, `selectionText` ~951)
- Modify: `src/lib/TerminalView.svelte` header comment ~22-24
- Test: `src/lib/ansi.test.ts`

**Interfaces:**
- Consumes: `charWidth`, `VS16` (Task 9).
- Produces: `export const WIDE_CONT = ''` in `ansi.ts` (the continuation cell of a wide glyph). Task 11 relies on it.

- [ ] **Step 1: Create the fixture script**

Create `scripts/capture-tmux-fixture.sh` and `chmod +x` it:

```bash
#!/usr/bin/env bash
# Regenerate the tmux attach fixture used by src/lib/ansi.tmux-fixture.test.ts.
# Linux only (util-linux `script`). Needs tmux; widths reflect THIS tmux version
# (the committed fixture was recorded with tmux 3.6a).
set -euo pipefail
out=${1:-src/lib/__fixtures__}
mkdir -p "$out"
sock="cf-fixture-$$"
work=$(mktemp -d)
trap 'tmux -L "$sock" kill-server 2>/dev/null || true; rm -rf "$work"' EXIT

cat > "$work/content.sh" <<'EOF'
printf '\e[38;2;255;120;0m⏺ truecolor\e[0m \e[1;4mbold-ul\e[0m \e[7mrev\e[0m\n'
printf '╭──────╮ ⎿ ✻ │ █\n'
printf 'CJK:中文X\n'
printf 'emoji:\xf0\x9f\xa4\x96X\n'
printf 'accent:a\xcc\x81X\n'
printf 'vs16:X\xef\xb8\x8fX\n'
printf 'long:%s\n' "$(printf 'abcdefghij%.0s' $(seq 1 12))"
# Block until a client is attached, so the updates below reach it as
# incremental, column-addressed redraws rather than part of the attach paint.
tmux -L "$SOCK" wait-for attached
printf '\e[3;9HY\e[4;8HZ\e[5;9HY\e[6;8HQ\e[12;1H@@done@@\n'
tmux -L "$SOCK" wait-for -S done
exec cat
EOF

tmux -L "$sock" -f /dev/null new-session -d -s fx -x 80 -y 24 "SOCK=$sock bash $work/content.sh"
tmux -L "$sock" set-hook -g client-attached "run-shell 'tmux -L $sock wait-for -S attached'"
# A FIFO opened read-write never reaches EOF, so `script` does not forward an
# end-of-input byte into the pane.
mkfifo "$work/stdin"
exec 3<>"$work/stdin"
script -qfc "stty rows 24 cols 80; TERM=xterm-256color tmux -L $sock attach -t fx" "$work/raw" <&3 >/dev/null 2>&1 &
spid=$!
timeout 10 tmux -L "$sock" wait-for done
tmux -L "$sock" capture-pane -p -t fx > "$out/tmux-attach.pane.txt"
tmux -L "$sock" detach-client -s fx
wait "$spid" || true
if ! grep -aq '@@done@@' "$work/raw"; then
  echo "capture missed the final update (tmux flush race); rerun" >&2
  exit 1
fi
cp "$work/raw" "$out/tmux-attach.raw.txt"
echo "wrote $out/tmux-attach.{raw.txt,pane.txt} with $(tmux -V)"
```

- [ ] **Step 2: Generate the fixture and protect its bytes**

Run: `./scripts/capture-tmux-fixture.sh`
Expected: `wrote src/lib/__fixtures__/tmux-attach.{raw.txt,pane.txt} with tmux 3.6a` (5 of 5 runs succeeded during analysis).

Check: `head -6 src/lib/__fixtures__/tmux-attach.pane.txt` shows rows `⏺ truecolor bold-ul rev`, `╭──────╮ ⎿ ✻ │ █`, `CJK:中文Y`, `emoji: ZX`, `accent:áY`, `vs16:X️Q`.

Create `.gitattributes`:

```gitattributes
# Recorded terminal byte streams: never normalise line endings or diff as text.
src/lib/__fixtures__/*.raw.txt -text -diff
```

- [ ] **Step 3: Write the failing golden test**

Create `src/lib/ansi.tmux-fixture.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { Screen } from './ansi';
// Vite `?raw` keeps the bytes as a string and needs no node:fs types.
import recorded from './__fixtures__/tmux-attach.raw.txt?raw';
import paneText from './__fixtures__/tmux-attach.pane.txt?raw';

// The recording ends with tmux's detach repaint, which wipes the screen; cut
// the stream right after the sentinel the fixture script prints last.
const SENTINEL = '@@done@@';
const raw = recorded.slice(0, recorded.indexOf(SENTINEL) + SENTINEL.length);
const pane = paneText.split('\n');
const ROWS = 23; // 24-row client minus tmux's status line
const want = Array.from({ length: ROWS }, (_, r) => (pane[r] ?? '').trimEnd());

function render(chunks: string[]): string[] {
  const s = new Screen(24, 80);
  for (const c of chunks) s.write(c);
  return Array.from({ length: ROWS }, (_, r) => s.cells[r].map((c) => c.ch).join('').trimEnd());
}

describe('ansi.Screen vs a recorded tmux 3.6a attach', () => {
  it('the fixture contains the sentinel', () => {
    expect(recorded.includes(SENTINEL)).toBe(true);
  });

  it('matches capture-pane row for row in one write', () => {
    expect(render([raw])).toEqual(want);
  });

  it('matches when the stream arrives in small random chunks', () => {
    let seed = 7;
    const rnd = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
    const chunks: string[] = [];
    for (let i = 0; i < raw.length; ) {
      const n = 1 + Math.floor(rnd() * 64);
      chunks.push(raw.slice(i, i + n));
      i += n;
    }
    expect(render(chunks)).toEqual(want);
  });
});
```

- [ ] **Step 4: Write the failing unit tests**

Append to `src/lib/ansi.test.ts`:

```ts
describe('ansi.Screen — wide and combining characters (tmux 3.6a widths)', () => {
  it('a CJK glyph occupies two cells, the second a continuation', () => {
    const s = new Screen(2, 10);
    s.write('中X');
    expect([s.cells[0][0].ch, s.cells[0][1].ch, s.cells[0][2].ch, s.cursorCol]).toEqual(['中', '', 'X', 3]);
  });

  it('an astral emoji stays whole (no lone surrogates)', () => {
    const s = new Screen(2, 10);
    s.write('\u{1F916}X');
    expect([s.cells[0][0].ch, s.cells[0][1].ch, s.cells[0][2].ch]).toEqual(['\u{1F916}', '', 'X']);
  });

  it('column-addressed updates after CJK land where tmux puts them', () => {
    const s = new Screen(2, 20);
    s.write('CJK:中文Y\x1b[1;9HZ');
    expect(rowText(s, 0).trimEnd()).toBe('CJK:中文Z');
  });

  it('a combining mark joins the previous cell', () => {
    const s = new Screen(2, 20);
    s.write('accent:a\u0301Y\x1b[1;9HZ');
    expect(rowText(s, 0).trimEnd()).toBe('accent:a\u0301Z');
  });

  it('overwriting the second half of a wide glyph blanks the first half', () => {
    const s = new Screen(2, 20);
    s.write('emoji:\u{1F916}Y\x1b[1;8HZ');
    expect(rowText(s, 0).trimEnd()).toBe('emoji: ZY');
  });

  it('VS16 widens the preceding narrow glyph to two columns', () => {
    const s = new Screen(2, 10);
    s.write('X\uFE0FY');
    expect([s.cells[0][0].ch, s.cells[0][1].ch, s.cells[0][2].ch]).toEqual(['X\uFE0F', '', 'Y']);
  });

  it('a wide glyph that would straddle the right edge wraps first', () => {
    const s = new Screen(2, 3);
    s.write('ab中');
    expect([rowText(s, 0), s.cells[1][0].ch]).toEqual(['ab ', '中']);
  });

  it('erasing the lead of a wide glyph blanks its continuation', () => {
    const s = new Screen(1, 5);
    s.write('中X\x1b[1;1H\x1b[X');
    expect(rowText(s, 0)).toBe('  X  ');
  });

  it('deleting a wide lead leaves no orphan continuation', () => {
    const s = new Screen(1, 6);
    s.write('a中b\x1b[1;2H\x1b[P');
    expect(rowText(s, 0)).toBe('a b   ');
  });

  it('selectionText returns a wide glyph once, without padding', () => {
    const s = new Screen(1, 10);
    s.write('中X');
    expect(s.selectionText({ row: 0, col: 0 }, { row: 0, col: 9 })).toBe('中X');
  });

  it('a surrogate pair split across writes is held until complete', () => {
    const s = new Screen(1, 10);
    s.write('a\uD83E');
    s.write('\uDD16b');
    expect([s.cells[0][1].ch, s.cells[0][2].ch, s.cells[0][3].ch]).toEqual(['\u{1F916}', '', 'b']);
  });
});
```

- [ ] **Step 5: Run to verify they fail**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/ansi.tmux-fixture.test.ts`
Expected: FAIL — the fixture's two render tests differ on rows 2–4; the wide/combining unit tests fail (e.g. `CJK:中文X Y`, lone surrogates). The sentinel test passes.

- [ ] **Step 6: Implement wide cells in `ansi.ts`**

Add the import at the top of `ansi.ts` (below the header comment):

```ts
import { charWidth, VS16 } from './wcwidth';
```

Replace the `Cell` interface doc and add the continuation constant:

```ts
export interface Cell {
  /** One grapheme: a code point plus any combining marks / variation
   *  selectors that joined it. `WIDE_CONT` marks the second cell of a
   *  two-column glyph. */
  ch: string;
  fg: number;
  bg: number;
  attrs: number;
}

/** `ch` of the right-hand cell covered by a two-column glyph. */
export const WIDE_CONT = '';
```

In the header comment, replace `mouse-tracking, no application keypad, no UTF-8 width fixups for wide` / `glyphs, no scrollback beyond the visible window.` with `application keypad and no scrollback beyond the visible window. Glyph widths` / `follow tmux 3.6a (see wcwidth.ts).`

In `write`, replace the final printable block (from `// Printable: write at cursor, advance.` through its `i++;`):

```ts
      // A high surrogate at the very end of a chunk: wait for its pair.
      if (code >= 0xd800 && code <= 0xdbff && i + 1 === s.length) {
        this.pending = s.slice(i);
        return;
      }
      // Printable: iterate by code point so astral chars stay whole.
      const cp = s.codePointAt(i)!;
      const ch = String.fromCodePoint(cp);
      const width = charWidth(cp);
      if (width === 0) this.appendToPrevious(cp, ch);
      else this.putChar(ch, width);
      i += ch.length;
```

Replace `putChar` and add the helpers after it:

```ts
  /** Write a glyph at the cursor with the current SGR state and advance by
   *  its width. If the active charset (G0/G1) is DEC Special Graphics, the
   *  char is mapped through `DEC_SPECIAL_GRAPHICS` first. */
  private putChar(ch: string, width: 1 | 2 = 1): void {
    if (width === 2 && this.cols < 2) width = 1;
    // Deferred wrap; a wide glyph that would straddle the edge wraps early.
    if (this.cursorCol >= this.cols || (width === 2 && this.cursorCol === this.cols - 1)) {
      this.cursorCol = 0;
      this.lineFeed();
    }
    const graphics = this.useG1 ? this.g1Graphics : this.g0Graphics;
    const mapped = graphics ? (DEC_SPECIAL_GRAPHICS[ch] ?? ch) : ch;
    const row = this.cells[this.cursorRow];
    this.breakWide(row, this.cursorCol);
    if (width === 2) this.breakWide(row, this.cursorCol + 1);
    this.setCell(row[this.cursorCol], mapped);
    if (width === 2) this.setCell(row[this.cursorCol + 1], WIDE_CONT);
    this.markRow(this.cursorRow);
    this.cursorCol += width;
  }

  private setCell(cell: Cell, ch: string): void {
    cell.ch = ch;
    cell.fg = this.curFg;
    cell.bg = this.curBg;
    cell.attrs = this.curAttrs;
  }

  /** Writing over or erasing either half of a wide glyph destroys the whole
   *  glyph (tmux/xterm behaviour): blank the orphaned other half. */
  private breakWide(row: Cell[], col: number): void {
    const cell = row[col];
    if (!cell) return;
    if (cell.ch === WIDE_CONT) {
      if (col > 0) row[col - 1].ch = ' ';
    } else if (col + 1 < row.length && row[col + 1].ch === WIDE_CONT) {
      row[col + 1].ch = ' ';
    }
  }

  /** Zero-width code points (combining marks, ZWJ, variation selectors) join
   *  the glyph before the cursor, as in tmux. VS16 also promotes a narrow
   *  glyph to emoji presentation, which tmux 3.6 draws two columns wide. */
  private appendToPrevious(cp: number, ch: string): void {
    const row = this.cells[this.cursorRow];
    let col = Math.min(this.cursorCol, this.cols) - 1;
    if (col >= 0 && row[col].ch === WIDE_CONT) col--;
    if (col < 0) return;
    const cell = row[col];
    const narrow = col + 1 >= this.cols || row[col + 1].ch !== WIDE_CONT;
    cell.ch += ch;
    if (cp === VS16 && narrow && this.cursorCol === col + 1 && col + 1 < this.cols) {
      this.breakWide(row, col + 1);
      const next = row[col + 1];
      next.ch = WIDE_CONT;
      next.fg = cell.fg;
      next.bg = cell.bg;
      next.attrs = cell.attrs;
      this.cursorCol = col + 2;
    }
    this.markRow(this.cursorRow);
  }

  /** Repair a row after cells were shifted or truncated (ICH/DCH/resize): a
   *  continuation without its lead, or a wide lead cut off at the right
   *  edge, becomes a blank. */
  private fixWideRow(row: Cell[]): void {
    for (let c = 0; c < row.length; c++) {
      if (row[c].ch === WIDE_CONT && (c === 0 || !isWideLead(row[c - 1].ch))) row[c].ch = ' ';
    }
    const last = row[row.length - 1];
    if (last.ch !== WIDE_CONT && charWidth(last.ch.codePointAt(0)!) === 2) last.ch = ' ';
  }
```

Add this module-level helper next to `makeRow`:

```ts
/** Does this cell's glyph span two columns (wide by width, or VS16-promoted)? */
function isWideLead(ch: string): boolean {
  return ch !== WIDE_CONT && (charWidth(ch.codePointAt(0)!) === 2 || ch.includes('\uFE0F'));
}
```

Replace `clearCell`:

```ts
  private clearCell(r: number, c: number): void {
    const row = this.cells[r];
    this.breakWide(row, c);
    const cell = row[c];
    cell.ch = ' ';
    cell.fg = COLOR_DEFAULT;
    cell.bg = COLOR_DEFAULT;
    cell.attrs = 0;
    this.markRow(r);
  }
```

In `deleteChars` and `insertChars`, add `this.fixWideRow(row);` directly before `this.markRow(this.cursorRow);`.

In `resize`, directly after `this.cells = resizeGrid(this.cells, this.rows, this.cols, rows, cols);` add:

```ts
    for (const row of this.cells) this.fixWideRow(row);
```

and inside the `savedScreen` block after `saved.cells = resizeGrid(…);`:

```ts
      for (const row of saved.cells) this.fixWideRow(row);
```

In `selectionText`, replace `for (let c = from; c <= to; c++) line += this.cells[r][c].ch || ' ';` with:

```ts
      // Continuation cells are '' so a wide glyph is copied once.
      for (let c = from; c <= to; c++) line += this.cells[r][c].ch;
```

In `TerminalView.svelte`'s header comment, replace the line `//   - No wide-glyph (CJK / emoji) width fixups.` with `//   - Glyph widths follow tmux 3.6a (wcwidth.ts); a different tmux may differ.`

- [ ] **Step 7: Run to verify they pass**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/ansi.tmux-fixture.test.ts src/lib/wcwidth.test.ts src/lib/terminal_selection.test.ts`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
/usr/bin/git add scripts/capture-tmux-fixture.sh src/lib/__fixtures__ .gitattributes src/lib/ansi.tmux-fixture.test.ts src/lib/ansi.ts src/lib/ansi.test.ts src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): tmux-accurate wide, combining and VS16 cells; golden tmux attach fixture"
```

---

### Task 11: Fixed-width rendering so glyph advance widths can't shift a row

Even with correct cells, the DOM lays text out by each glyph's real advance width: a wide glyph, or a symbol Menlo lacks (`⏺ ✻ ⎿` fall back to another font), shifts the rest of the row, while the cursor and selection overlays are positioned at `col × cellWidth`. Runs get an explicit cell count; any glyph that is not safe in Menlo gets its own run with a fixed pixel width.

**Files:**
- Modify: `src/lib/ansi.ts` (`Run` ~996-1001, `rowToRuns` ~1006-1023)
- Modify: `src/lib/TerminalView.svelte` (`visibleRows` key ~774-777, run `<span>` ~892, `.row span` CSS ~1066-1069)
- Test: `src/lib/ansi.test.ts` (`describe('ansi.rowToRuns')`)

**Interfaces:**
- Consumes: `WIDE_CONT` (Task 10).
- Produces: `Run.cells: number` — the number of grid columns the run covers.

- [ ] **Step 1: Write the failing tests**

Add inside `describe('ansi.rowToRuns', …)` in `src/lib/ansi.test.ts`:

```ts
  it('counts grid cells per run, including a wide glyph continuation', () => {
    const s = new Screen(1, 7);
    s.write('ab中c');
    expect(rowToRuns(s.cells[0]).map((r) => [r.text, r.cells])).toEqual([
      ['ab', 2], ['中', 2], ['c  ', 3],
    ]);
  });

  it('isolates fallback-font symbols into their own one-cell runs', () => {
    const s = new Screen(1, 5);
    s.write('a⏺b');
    expect(rowToRuns(s.cells[0]).map((r) => [r.text, r.cells])).toEqual([
      ['a', 1], ['⏺', 1], ['b  ', 3],
    ]);
  });

  it('keeps box drawing in one run', () => {
    const s = new Screen(1, 4);
    s.write('╭──╮');
    expect(rowToRuns(s.cells[0]).map((r) => [r.text, r.cells])).toEqual([['╭──╮', 4]]);
  });
```

- [ ] **Step 2: Run to verify they fail**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts -t rowToRuns`
Expected: FAIL — `cells` is `undefined`, and `⏺` merges into its neighbours.

- [ ] **Step 3: Implement**

Replace `Run` and `rowToRuns` in `ansi.ts`:

```ts
/** A horizontal run of same-styled cells on a single row. */
export interface Run {
  text: string;
  fg: number;
  bg: number;
  attrs: number;
  /** Grid columns covered (wide glyphs count 2). The renderer sizes the run
   *  to exactly `cells × cellWidth`. */
  cells: number;
}

/** Glyphs Menlo draws at exactly one cell width, so they may share a run:
 *  ASCII, Latin-1, Latin Extended-A/B, box drawing and block elements. */
function isGridSafe(ch: string): boolean {
  if (ch.length !== 1) return false;
  const c = ch.charCodeAt(0);
  return c < 0x0300 || (c >= 0x2500 && c <= 0x259f);
}

/** Group a row's cells into runs. Adjacent grid-safe cells with the same
 *  style merge; any other glyph (CJK, emoji, fallback-font symbols) gets its
 *  own run so a different advance width can't shift the rest of the row.
 *  Trailing default-styled blanks are kept so the grid stays aligned. */
export function rowToRuns(row: Cell[]): Run[] {
  const runs: Run[] = [];
  let cur: Run | null = null;
  let curSafe = false;
  for (const cell of row) {
    if (cell.ch === WIDE_CONT && cur !== null) {
      cur.cells++;
      continue;
    }
    const safe = isGridSafe(cell.ch);
    if (
      cur !== null &&
      curSafe &&
      safe &&
      cur.fg === cell.fg &&
      cur.bg === cell.bg &&
      cur.attrs === cell.attrs
    ) {
      cur.text += cell.ch;
      cur.cells++;
    } else {
      cur = { text: cell.ch, fg: cell.fg, bg: cell.bg, attrs: cell.attrs, cells: 1 };
      curSafe = safe;
      runs.push(cur);
    }
  }
  return runs;
}
```

- [ ] **Step 4: Render runs at fixed widths**

In `TerminalView.svelte`, in `visibleRows`, include the cell count in the key:

```ts
        key += `\u0001${run.fg}\u0002${run.bg}\u0003${run.attrs}\u0004${run.cells}\u0005${run.text}`;
```

(and update the comment above it: `joined with control bytes 0x01..0x05`).

Replace the run span:

```svelte
            <span style="{runStyle(run)};width:{run.cells * cellWidth}px">{run.text}</span>
```

Replace the `.row span` CSS:

```css
  .row span {
    /* Each run is sized to exactly cells × cellWidth, so a glyph whose advance
       width differs (CJK, emoji, fallback-font symbols) can't shift the row
       out of line with the cursor/selection overlays. */
    display: inline-block;
    height: 16px;
    vertical-align: top;
    overflow: hidden;
  }
```

- [ ] **Step 5: Verify**

Run: `npx -y pnpm@10 vitest run src/lib/ansi.test.ts src/lib/terminal_style.test.ts src/lib/ansi.tmux-fixture.test.ts`
Expected: PASS.

Run: `npx -y pnpm@10 run check`
Expected: 0 errors.

Manual (`npx -y pnpm@10 tauri dev`): in an attached shell run `printf '0123456789b\na中文🤖⏺✻⎿b\n'`. Expected: the second line's `b` sits exactly under the first line's `b` (a=1, 中文=4, 🤖=2, ⏺✻⎿=3 columns). Then type `中文` at the prompt and press ← → the cursor block covers `文`, not the gap beside it.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add src/lib/ansi.ts src/lib/ansi.test.ts src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): size each run to its grid cells so glyph widths can't shift rows"
```

---

### Task 12: `keyToBytes` in its own module, with full key encoding

Gaps in the component's `keyToBytes` (~646-673): forward Delete, Insert and F-keys send nothing; modifiers on arrows/Home/End/PageUp/PageDown/Delete are dropped (Alt/Ctrl+← send plain ←); Shift/Option+Enter send plain `\r`, so there is no newline key for Claude's prompt; Ctrl+`/` sends a literal `/`.

**Files:**
- Create: `src/lib/keys.ts`
- Modify: `src/lib/TerminalView.svelte` (delete local `keyToBytes` ~642-673, import the module)
- Test: `src/lib/keys.test.ts`

**Interfaces:**
- Produces: `type KeyLike = Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>` and `keyToBytes(e: KeyLike): string | null`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/keys.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { keyToBytes, type KeyLike } from './keys';

const k = (key: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods,
});

describe('keyToBytes', () => {
  it.each<[string, KeyLike, string | null]>([
    ['Enter', k('Enter'), '\r'],
    ['Shift+Enter inserts a newline', k('Enter', { shiftKey: true }), '\x1b\r'],
    ['Alt+Enter inserts a newline', k('Enter', { altKey: true }), '\x1b\r'],
    ['Backspace', k('Backspace'), '\x7f'],
    ['Alt+Backspace deletes a word', k('Backspace', { altKey: true }), '\x1b\x7f'],
    ['Ctrl+Backspace', k('Backspace', { ctrlKey: true }), '\x08'],
    ['Tab', k('Tab'), '\t'],
    ['Shift+Tab', k('Tab', { shiftKey: true }), '\x1b[Z'],
    ['Escape', k('Escape'), '\x1b'],
    ['Left', k('ArrowLeft'), '\x1b[D'],
    ['Alt+Left', k('ArrowLeft', { altKey: true }), '\x1b[1;3D'],
    ['Ctrl+Right', k('ArrowRight', { ctrlKey: true }), '\x1b[1;5C'],
    ['Home', k('Home'), '\x1b[H'],
    ['Shift+End', k('End', { shiftKey: true }), '\x1b[1;2F'],
    ['Delete', k('Delete'), '\x1b[3~'],
    ['Ctrl+Delete', k('Delete', { ctrlKey: true }), '\x1b[3;5~'],
    ['Insert', k('Insert'), '\x1b[2~'],
    ['PageUp', k('PageUp'), '\x1b[5~'],
    ['F1', k('F1'), '\x1bOP'],
    ['Shift+F1', k('F1', { shiftKey: true }), '\x1b[1;2P'],
    ['F5', k('F5'), '\x1b[15~'],
    ['F12', k('F12'), '\x1b[24~'],
    ['Ctrl+C', k('c', { ctrlKey: true }), '\x03'],
    ['Ctrl+Shift+C', k('C', { ctrlKey: true, shiftKey: true }), '\x03'],
    ['Ctrl+/', k('/', { ctrlKey: true }), '\x1f'],
    ['Ctrl+Space', k(' ', { ctrlKey: true }), '\x00'],
    ['Ctrl+1 is not a control char', k('1', { ctrlKey: true }), null],
    ['printable', k('a'), 'a'],
    ['Option-composed @ (Slovak layout)', k('@', { altKey: true }), '@'],
    ['Cmd chords stay with the app', k('v', { metaKey: true }), null],
    ['bare modifier', k('Shift'), null],
    ['dead key', k('Dead'), null],
  ])('%s', (_name, e, want) => {
    expect(keyToBytes(e)).toBe(want);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx -y pnpm@10 vitest run src/lib/keys.test.ts`
Expected: FAIL — cannot resolve `./keys`.

- [ ] **Step 3: Implement**

Create `src/lib/keys.ts`:

```ts
/** The KeyboardEvent fields keyToBytes reads (a plain object works in tests). */
export type KeyLike = Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>;

const CSI_CURSOR = new Map<string, string>([
  ['ArrowUp', 'A'], ['ArrowDown', 'B'], ['ArrowRight', 'C'], ['ArrowLeft', 'D'],
  ['Home', 'H'], ['End', 'F'],
]);
const SS3_FKEYS = new Map<string, string>([['F1', 'P'], ['F2', 'Q'], ['F3', 'R'], ['F4', 'S']]);
const TILDE_KEYS = new Map<string, number>([
  ['Insert', 2], ['Delete', 3], ['PageUp', 5], ['PageDown', 6],
  ['F5', 15], ['F6', 17], ['F7', 18], ['F8', 19], ['F9', 20], ['F10', 21], ['F11', 23], ['F12', 24],
]);
const CTRL_SYMBOLS = new Map<string, string>([
  [' ', '\x00'], ['@', '\x00'], ['2', '\x00'], ['[', '\x1b'], ['\\', '\x1c'], [']', '\x1d'],
  ['^', '\x1e'], ['6', '\x1e'], ['_', '\x1f'], ['/', '\x1f'], ['-', '\x1f'],
]);

/** xterm modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4). */
function modParam(e: KeyLike): number {
  return 1 + (e.shiftKey ? 1 : 0) + (e.altKey ? 2 : 0) + (e.ctrlKey ? 4 : 0);
}

/** Translate a key press into the bytes an xterm sends. Returns null for keys
 *  that should not reach the PTY (bare modifiers, dead keys, Cmd chords). */
export function keyToBytes(e: KeyLike): string | null {
  // ESC CR is Meta+Enter, which Claude Code treats as "insert newline".
  if (e.key === 'Enter') return e.shiftKey || e.altKey ? '\x1b\r' : '\r';
  if (e.key === 'Backspace') return e.altKey ? '\x1b\x7f' : e.ctrlKey ? '\x08' : '\x7f';
  if (e.key === 'Tab') return e.shiftKey ? '\x1b[Z' : '\t';
  if (e.key === 'Escape') return '\x1b';
  const mod = modParam(e);
  const cursor = CSI_CURSOR.get(e.key);
  if (cursor !== undefined) return mod === 1 ? `\x1b[${cursor}` : `\x1b[1;${mod}${cursor}`;
  const ss3 = SS3_FKEYS.get(e.key);
  if (ss3 !== undefined) return mod === 1 ? `\x1bO${ss3}` : `\x1b[1;${mod}${ss3}`;
  const tilde = TILDE_KEYS.get(e.key);
  if (tilde !== undefined) return mod === 1 ? `\x1b[${tilde}~` : `\x1b[${tilde};${mod}~`;
  // Cmd chords belong to the app (copy / paste / select all are handled first).
  if (e.metaKey) return null;
  if (e.ctrlKey && e.key.length === 1) {
    const lower = e.key.toLowerCase();
    if (lower >= 'a' && lower <= 'z') return String.fromCharCode(lower.charCodeAt(0) - 96);
    return CTRL_SYMBOLS.get(lower) ?? null;
  }
  // One printable character, including Option-composed ones (e.g. @ on a
  // Slovak layout) — so Option is NOT turned into an ESC prefix here.
  if ([...e.key].length === 1) return e.key;
  return null;
}
```

- [ ] **Step 4: Use it in the component**

In `TerminalView.svelte`, delete the whole local `function keyToBytes(e: KeyboardEvent): string | null { … }` with its doc comment, and add:

```ts
  import { keyToBytes } from './keys';
```

`onKeydown` keeps calling `keyToBytes(e)` after its Cmd+V / Cmd+C / Cmd+A handling.

- [ ] **Step 5: Verify**

Run: `npx -y pnpm@10 vitest run src/lib/keys.test.ts`
Expected: PASS (32 cases).

Run: `npx -y pnpm@10 run check`
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add src/lib/keys.ts src/lib/keys.test.ts src/lib/TerminalView.svelte
/usr/bin/git commit -m "feat(terminal): modifier-aware arrows, Delete/Insert/F-keys, newline on Shift/Option+Enter"
```

---

### Task 13: IME and dead-key input through a hidden textarea

`oncompositionend` sits on a focusable `div` that is not editable, and WebKit fires composition events only on editable elements, so dead keys (Slovak `´` + `a`), the accent-hold popup, the emoji picker and CJK IMEs most likely never reach the PTY. This was not reproducible headless; Step 3 is the acceptance test. xterm.js solves the same problem with a hidden textarea that owns focus.

**Files:**
- Modify: `src/lib/TerminalView.svelte` (state ~32-72, `onMousedown` focus calls ~255/292/342, `onKeydown` ~675-711, `onCompositionEnd` ~713-719, grid markup ~871-916, CSS `.grid:focus-visible` ~1056-1058)

**Interfaces:**
- Consumes: `cursor` derived (existing), `bumpDrain` (existing).

- [ ] **Step 1: Add the proxy and route focus to it**

Add to the component state:

```ts
  /** Hidden textarea that owns keyboard focus. WebKit only runs IME / dead-key
   *  composition on editable elements, so composed text lands here and is
   *  flushed to the PTY. Positioned at the cursor so IME popups appear there. */
  let imeInput: HTMLTextAreaElement | undefined = $state(undefined);

  function focusInput() {
    (imeInput ?? container)?.focus({ preventScroll: true });
  }

  /** Send whatever the proxy holds, then empty it. Called from both
   *  compositionend and input: WebKit and Chromium order those two events
   *  differently, so reading the textarea (not event.data) makes the second
   *  call a no-op instead of a duplicate. */
  function flushProxy() {
    if (!imeInput) return;
    const text = imeInput.value;
    imeInput.value = '';
    if (text === '' || !ptyOpen) return;
    void invoke('pty_write', { args: { data: text } }).catch(() => {});
    bumpDrain();
  }

  function onProxyInput(e: Event) {
    if ((e as InputEvent).isComposing) return;
    flushProxy();
  }
```

Replace the existing `onCompositionEnd` function with:

```ts
  function onCompositionEnd() {
    flushProxy();
  }
```

In `onMousedown`, replace each of the three `(e.currentTarget as HTMLElement | null)?.focus();` lines with:

```ts
      focusInput();
```

In `onKeydown`, replace `if (e.isComposing) return;` with:

```ts
    // keyCode 229 is the "IME is processing this key" placeholder WebKit sends
    // around composition; the text arrives through the proxy instead.
    if (e.isComposing || e.keyCode === 229) return;
```

In the grid `<div class="grid" …>` markup, remove `oncompositionend={onCompositionEnd}` and add `onfocus={focusInput}`. Inside the grid, directly after the `.measure` span, add:

```svelte
      <textarea
        class="ime-proxy"
        bind:this={imeInput}
        style={cursor ? `left:${cursor.left}px; top:${cursor.top}px` : ''}
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        tabindex="-1"
        aria-hidden="true"
        oncompositionend={onCompositionEnd}
        oninput={onProxyInput}
      ></textarea>
```

Keystrokes typed into the textarea still bubble to the grid's `onkeydown`, which calls `preventDefault()` for every key it forwards, so ordinary characters never enter the textarea.

Replace the `.grid:focus-visible` rule and add the proxy style:

```css
  .grid:focus-within {
    box-shadow: inset 0 0 0 1px var(--accent, #4f8fff);
  }
  .ime-proxy {
    position: absolute;
    width: 1px;
    height: 16px;
    padding: 0;
    border: 0;
    margin: 0;
    opacity: 0;
    resize: none;
    overflow: hidden;
    pointer-events: none;
    caret-color: transparent;
  }
```

- [ ] **Step 2: Type-check and run the frontend suite**

Run: `npx -y pnpm@10 run check`
Expected: 0 errors.

Run: `npx -y pnpm@10 run test`
Expected: all terminal tests pass; only the pre-existing `localStorage is undefined` failures (if present on `main`) remain.

- [ ] **Step 3: Manual acceptance on macOS (required)**

Run the app (`npx -y pnpm@10 tauri dev`), attach a shell session, click into the terminal, then:
1. Slovak keyboard: press `´` then `a` → `á` appears once. Press `ˇ` then `c` → `č`.
2. US keyboard: hold `e` and pick `é` from the accent popup → `é` appears once.
3. Ctrl+Cmd+Space, pick 🤖 → one emoji appears.
4. Japanese Romaji IME: type `nihon`, press Space, Enter → `日本` appears once.
5. Plain typing, Ctrl+C, arrows and Cmd+V still work; nothing is typed twice.

If step 1 already worked before this task on the target macOS version, keep the change anyway (steps 2–4 cover other paths) and note it in the PR.

- [ ] **Step 4: Commit**

```bash
/usr/bin/git add src/lib/TerminalView.svelte
/usr/bin/git commit -m "fix(terminal): route IME and dead-key input through a hidden textarea"
```

---

### Task 14: Debounce resize so a window drag sends one SIGWINCH

Every ResizeObserver callback resizes the Screen and calls `pty_resize`; dragging the window sends dozens of SIGWINCHs, each making tmux repaint the full client.

**Files:**
- Modify: `src/lib/TerminalView.svelte` (ResizeObserver in `openTerm` ~475-487, `closeTerm` ~607-640)

- [ ] **Step 1: Implement**

Add near the drain-loop constants:

```ts
  /** Coalesce a burst of ResizeObserver callbacks (window drag) into one
   *  screen resize + one SIGWINCH; each SIGWINCH makes tmux repaint. */
  const RESIZE_DEBOUNCE_MS = 60;
  let resizeTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleResize() {
    if (resizeTimer !== null) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(applyResize, RESIZE_DEBOUNCE_MS);
  }

  function applyResize() {
    resizeTimer = null;
    if (!screen) return;
    const next = computeDimensions();
    if (next.cols === lastCols && next.rows === lastRows) return;
    lastCols = next.cols;
    lastRows = next.rows;
    screen.resize(next.rows, next.cols);
    renderVersion++;
    if (ptyOpen) {
      void invoke('pty_resize', { args: { cols: next.cols, rows: next.rows } }).catch(() => {});
    }
  }
```

In `openTerm`, replace the `resizeObserver = new ResizeObserver(() => { … });` block with:

```ts
    resizeObserver = new ResizeObserver(scheduleResize);
```

In `closeTerm`, directly after `resizeObserver = null;`, add:

```ts
    if (resizeTimer !== null) {
      clearTimeout(resizeTimer);
      resizeTimer = null;
    }
```

- [ ] **Step 2: Verify**

Run: `npx -y pnpm@10 run check`
Expected: 0 errors.

Manual: attach a session running `htop` (or Claude), drag the window edge for ~2 s. Expected: the header size updates once the drag pauses; no flicker storm; `htop` fills the pane correctly at the end.

- [ ] **Step 3: Commit**

```bash
/usr/bin/git add src/lib/TerminalView.svelte
/usr/bin/git commit -m "perf(terminal): debounce resize into a single SIGWINCH"
```

---

### Task 15: End-to-end verification and delivery

**Files:** none new.

- [ ] **Step 1: Mirror CI locally (foreground, one command at a time)**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
env CARGO_BUILD_JOBS=6 cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
env CARGO_BUILD_JOBS=6 cargo test --manifest-path src-tauri/Cargo.toml
npx -y pnpm@10 install --frozen-lockfile
npx -y pnpm@10 run check
npx -y pnpm@10 run test
npx -y pnpm@10 run build
```

Expected: fmt prints nothing; clippy and cargo test finish without warnings/failures; svelte-check 0 errors; vitest passes apart from the pre-existing `localStorage` failures already present on `main`; vite build succeeds.

- [ ] **Step 2: Manual checklist in the desktop app (macOS)**

Run `npx -y pnpm@10 tauri dev`. For each item record pass/fail in the PR description:

1. **Freeze-proof:** in an attached shell run `printf 'abcdefghij%.0s' {1..30}; printf '\e[1K'` → the terminal keeps updating afterwards.
2. **Wide glyphs:** `printf 'CJK:中文X\nemoji:🤖X\n'`, then run Claude and type `中文` into its prompt → text, cursor block and selection line up; no leftover half-glyphs after editing.
3. **OSC 52:** inside tmux, `printf '\e]52;c;%s\a' "$(printf 'čšá' | base64)"`, then Cmd+V in another app → `čšá`.
4. **Private/string sequences:** `printf '\e[>4;1mplain\e[0m \eP+q544e\e\\ok\n'` → `plain ok`, not bold/underlined, no `+q544e`.
5. **False banner:** `printf '[cf] PTY EOF\n'` → no "Connection lost".
6. **Real disconnect:** attach a remote session, then turn off Wi-Fi → "Connection lost" appears within ~60 s; meanwhile paste a large text (Cmd+V) → the app stays responsive (sidebar clickable).
7. **Keys:** in a shell `Alt+←`/`Alt+→` jump words, Delete removes the char under the cursor, F-keys reach `htop`; in Claude, Shift+Enter inserts a newline and Enter submits.
8. **IME:** Task 13 Step 3 checklist.
9. **Narrow pane:** drag the terminal pane below ~320 px wide / ~170 px tall → tmux's status line fits the grid; no wrapped garbage.
10. **Resize storm:** Task 14 Step 2.
11. **Session switch during a failing open:** select a session on an unreachable host, then immediately a local one → the local one attaches.
12. **No stray newline:** switch between two Claude sessions several times → no blank line is submitted into either prompt.

- [ ] **Step 3: Open the PRs in order**

For each PR in the delivery table (A, B, C, D), from a branch containing exactly its tasks:

```bash
/usr/bin/git push -u origin HEAD
gh pr create --base main --title "<title from the delivery table>" --body "<summary, decisions affected, manual checklist results>"
```

Before merging, read the check conclusions (the `--watch | tail` form always exits 0):

```bash
gh pr view <N> --json statusCheckRollup --jq '.statusCheckRollup[]|"\(.name) \(.conclusion)"'
```

Expected: every `rust` and `frontend` matrix entry `SUCCESS`. Then `gh pr merge <N> --merge --delete-branch` (merge commits are the project style) and rebase the next branch onto `origin/main`.


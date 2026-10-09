import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  buttonsOf,
  indexDialogs,
  isStatusCopy,
  lintSvelte,
  seventhWords,
  pictographs,
  type CopyFinding,
  type DialogIndex,
} from './copy_lint';
import { STATE_WORD, STATUS_WORDS } from './kit/status';
import { claudeStatusLabel, stuckStatus } from './attention';
import { backgroundStatusWord, type BackgroundStatus } from './conversation';
import { CLAUDE_STATUSES, STUCK_KINDS } from './sessions';
import { ROW_ACTIONS } from './session_actions';
import { ATTENTION_STATES } from './attention';
import { STATE_LABELS } from './row_groups';
import { verdictLabel, type Verdict } from './evidence';
import { nodeLabel } from './missions';
import { outcomeLabel } from './mission_triage';

// Redesign step 7.8, the copy pass: the manual's content rules
// (docs/ux/2026-10-08-orbit-fleet-redesign/design-system/README.md,
// "Content fundamentals") held on the source. A status says one of six words;
// a label that opens a dialog ends with "…".

// The self-gated dialogs whose own `{#if}` reads a local or an alias, and the
// store that opens them. A new one must be named here, or the lint cannot see
// who opens it.
const SELF_GATED: Record<string, string | null> = {
  ShareSheet: 'shareSheetFor',
  TransferSheet: 'transferSheetFor',
  QuickSwitcher: 'switcherRequest',
  ShortcutSheet: 'shortcutSheetOpen',
  // Opened by an agent's MCP call, never by a button.
  McpConfirmDialog: null,
};

// Findings that wait, each with why. An entry that no longer matches a finding
// fails below, so the list only shrinks.
const WAITING: { file: string; text: string; why: string }[] = [
  // NewSessionDialog.svelte and Sidebar.svelte are serial files held by
  // another open PR (4.4); these get their "…" when it lands.
  { file: 'src/lib/NewSessionDialog.svelte', text: 'Resume', why: 'serial file held by 4.4' },
  // The live transfer chip is the move's state ("⇄ moving to x · 2/5"); it
  // opens the Transfer sheet the way a status badge opens its detail.
  { file: 'src/lib/TransferChip.svelte', text: '⇄ move failed', why: 'a live status chip, not a label' },
];

function sources() {
  const svelte: Record<string, string> = {};
  const ts: Record<string, string> = {};
  // Forward slashes on every OS: Windows lists `lib\Foo.svelte`, and the
  // checks below look files up as `src/lib/Foo.svelte`.
  for (const f of readdirSync('src', { recursive: true }).map((p) => p.replace(/\\/g, '/'))) {
    if (f.includes('.test.') || f.endsWith('.d.ts') || f.includes('copy_lint')) continue;
    if (f.endsWith('.svelte')) svelte[`src/${f}`] = readFileSync(`src/${f}`, 'utf8');
    else if (f.endsWith('.ts')) ts[`src/${f}`] = readFileSync(`src/${f}`, 'utf8');
  }
  return { svelte, ts };
}

describe('the status words', () => {
  it('are the manual six', () => {
    expect([...STATUS_WORDS]).toEqual(['Needs you', 'Working', 'Failed', 'Done', 'Paused', 'Idle']);
  });

  it('take a reason after " · " and nothing else', () => {
    expect(isStatusCopy('Failed · auth menu')).toBe(true);
    expect(isStatusCopy('Needs you')).toBe(true);
    expect(isStatusCopy('⚡ working')).toBe(false);
    expect(isStatusCopy('blocked')).toBe(false);
    expect(isStatusCopy('working')).toBe(false);
  });

  it('every status table says one of them', () => {
    const words = [
      ...Object.values(STATE_WORD),
      ...CLAUDE_STATUSES.map((s) => claudeStatusLabel(s)),
      ...STUCK_KINDS.map((k) => stuckStatus(k)),
      stuckStatus(null),
      ...(['running', 'done', 'failed', 'stopped', 'idle'] as BackgroundStatus[]).map(backgroundStatusWord),
      // Seven attention states, six words: Blocked reads Needs you.
      ...ATTENTION_STATES.map((s) => STATE_LABELS[s]),
      ...(['ready', 'waiting', 'unknown', 'blocked', 'merged', 'closed'] as Verdict[]).map(verdictLabel),
      ...['done', 'running', 'doing', 'verifying', 'failed', 'blocked', 'proposed', 'held', 'waiting', 'ready', 'rejected'].map(nodeLabel),
      ...['done', 'partial', 'blocked', 'failed'].map(outcomeLabel),
    ];
    for (const w of words) expect(isStatusCopy(w), w).toBe(true);
  });

  it('no source writes the old glyph-prefixed statuses', () => {
    const { svelte, ts } = sources();
    const old = /['">`]\s*[⚡⏸✓✗■⚠·]\s?(?:working|blocked|done|failed|stopped|idle|stuck)\b/i;
    const hits = Object.entries({ ...svelte, ...ts }).flatMap(([f, s]) =>
      s.split('\n').flatMap((l, i) => (old.test(l) ? [`${f}:${i + 1}: ${l.trim()}`] : [])),
    );
    expect(hits).toEqual([]);
  });
});

describe('no seventh status word', () => {
  it('flags a non-status word as a label or a status head, not in prose or a comment', () => {
    const src = [
      "const L = { blocked: 'Blocked' };",
      "return { label: 'Queued · Claude reads it after this turn' };",
      '<p class="why"><strong>Stuck</strong> · {why}</p>',
      '// "Blocked" in a comment is fine',
      "const s = 'Blocked on TASK-212 until it lands';",
      "return 'Thinking · reading hub/pair.rs';",
    ].join('\n');
    expect(seventhWords('src/X.ts', src).map((f) => `${f.line} ${f.text}`)).toEqual(['1 Blocked', '2 Queued', '3 Stuck']);
  });

  it('no source writes one', () => {
    const { svelte, ts } = sources();
    const hits = Object.entries({ ...svelte, ...ts }).flatMap(([f, s]) => seventhWords(f, s).map((h) => `${f}:${h.line}: ${h.text}`));
    expect(hits).toEqual([]);
  });
});

describe('icons, not emoji (manual: Iconography)', () => {
  it('flags a pictograph in markup, keeps the text markers and ignores script', () => {
    const src = `<script>const g = '⚠';</script>
<span class="warn">⚠</span>
<span>🔗{n}</span>
<span>✓ passed</span><span>✦ Proposed</span>
<!-- 🤖 in a comment -->`;
    expect(pictographs('src/X.svelte', src).map((f) => `${f.line} ${f.text}`)).toEqual(['2 ⚠', '3 🔗']);
  });

  it('no component draws one', () => {
    const { svelte } = sources();
    const hits = Object.entries(svelte).flatMap(([f, s]) => pictographs(f, s).map((h) => `${f}:${h.line}: ${h.text}`));
    expect(hits).toEqual([]);
  });
});

describe('the copy lint', () => {
  const index = (svelte: Record<string, string>, ts: Record<string, string> = {}) => indexDialogs(svelte, ts, {});
  const dialog = { 'src/lib/FooDialog.svelte': '<Modal label="Foo">x</Modal>' };

  it('flags a button that opens a dialog through its flag, inline or through a function', () => {
    const src = `<script>
  let open = $state(false);
  function ask() {
    if (busy) return;
    open = true;
  }
</script>
<button onclick={() => (open = true)}>Rename</button>
<button onclick={ask}>Delete</button>
<button onclick={() => (open = true)}>Move…</button>
{#if open}<FooDialog />{/if}`;
    const out = lintSvelte('src/X.svelte', src, index(dialog));
    expect(out.map((f) => f.text)).toEqual(['Rename', 'Delete']);
    expect(out.every((f) => f.rule === 'dialog-ellipsis')).toBe(true);
  });

  it('flags a store flag set anywhere, and a module function that sets it', () => {
    const ix = index(
      { ...dialog, 'src/App.svelte': '{#if $fooOpen}<FooDialog />{/if}' },
      { 'src/lib/views.ts': 'export const fooOpen = writable(false);\nexport function openFoo() {\n  fooOpen.set(true);\n}' },
    );
    const src = `<button onclick={() => fooOpen.set(true)}>Settings</button><button onclick={() => openFoo()}>Open foo</button>`;
    expect(lintSvelte('src/Y.svelte', src, ix).map((f) => f.text)).toEqual(['Settings', 'Open foo']);
  });

  it("flags a glyph before a label, but keeps the manual's markers (review r14)", () => {
    const src = `<button>🔍 Review</button><button>♻ Recreate</button><button>✎ What changed</button><button>✦ Proposed</button><button>🎤</button>`;
    expect(lintSvelte('src/X.svelte', src, index({})).map((f) => `${f.rule} ${f.text}`)).toEqual([
      'glyph-prefix 🔍 Review',
      'glyph-prefix ♻ Recreate',
    ]);
  });

  it('flags the old name in copy, not in a link or a tool name (review r14)', () => {
    const src = `<h2>Welcome to claude-fleet</h2>
<p>Allow them for Orbit Fleet (listed as claude-fleet) in system settings.</p>
<a href="https://github.com/martin-janci/claude-fleet">Source</a>
<p>{'mcp__claude-fleet__ask'}</p>`;
    expect(lintSvelte('src/X.svelte', src, index({})).map((f) => `${f.rule} ${f.line}`)).toEqual(['old-name 1']);
  });

  it('wants dialog titles and headings in sentence case, names aside (review r14)', () => {
    const src = `<Modal title="New Background Session">x</Modal>
<Modal title="Connect GitHub">x</Modal>
<h3>Control API (MCP)</h3>
<h4>Hub Health</h4>`;
    expect(lintSvelte('src/X.svelte', src, index({})).map((f) => `${f.rule} ${f.text}`)).toEqual([
      'sentence-case New Background Session',
      'sentence-case Hub Health',
    ]);
  });

  it('leaves a button that only sometimes asks (a confirm for one value) alone', () => {
    const src = `<script>
  let pending = $state(null);
  function write(v) {
    if (needsConfirm(v)) {
      pending = v;
      return;
    }
    commit(v);
  }
</script>
<button onclick={() => write(1)}>Apply</button>
{#if pending !== null}<FooDialog />{/if}`;
    expect(lintSvelte('src/X.svelte', src, index(dialog))).toEqual([]);
  });

  it('does not count a Cancel that closes the dialog, nor what an {:else} shows', () => {
    const src = `<script>
  let open = $state(false);
  let busy = $state(false);
</script>
<button onclick={() => (open = false)}>Cancel</button>
<button onclick={() => (busy = true)}>Go</button>
{#if busy}<p>…</p>{:else}<FooDialog />{/if}
{#if open}<FooDialog />{/if}`;
    expect(lintSvelte('src/X.svelte', src, index(dialog))).toEqual([]);
  });

  it('reads the label a button shows at rest', () => {
    const src = `<button onclick={() => (open = true)}>{#if busy}Starting…{:else}Start{/if}</button>
<button onclick={() => (open = true)}>Restore {n} lost…{#if mine}<span class="muted">({mine} not yours)</span>{/if}</button
>
{#if open}<FooDialog />{/if}`;
    expect(lintSvelte('src/X.svelte', src, index(dialog)).map((f) => f.text)).toEqual(['Start']);
  });

  it('leaves an inline .link and an icon-only button alone', () => {
    const src = `<button class="link" onclick={() => (open = true)}>Settings › Review</button>
<button aria-label="Add" onclick={() => (open = true)}>+</button>
{#if open}<FooDialog />{/if}`;
    expect(lintSvelte('src/X.svelte', src, index(dialog))).toEqual([]);
  });

  it('flags "..." for "…"', () => {
    expect(lintSvelte('src/X.svelte', '<button onclick={go}>Loading...</button>', index({}))).toEqual([
      { rule: 'three-dots', file: 'src/X.svelte', line: 1, text: 'Loading...' },
    ]);
  });

  it('flags a status chip outside the six, literal or computed from a raw state', () => {
    const src = `<span class="claude-chip">inactive</span>
<span class="claude-chip">Idle · process ended</span>
<span class="status-word">{session.status}</span>
<span class="status-word">{claudeStatusLabel(s)}</span>
<StatusChip state="failed" label="broken" />
<StatusChip state="failed" label="Failed · CI red" />
<StatusChip label="github.com" />`;
    expect(lintSvelte('src/X.svelte', src, index({})).map((f) => `${f.rule} ${f.text}`)).toEqual([
      'status-word inactive',
      'status-word {session.status}',
      'status-word broken',
    ]);
  });
});

describe('the app', () => {
  const { svelte, ts } = sources();
  const ix: DialogIndex = indexDialogs(svelte, ts, SELF_GATED);

  it('names the store of every self-gated dialog', () => {
    expect(ix.unnamed).toEqual([]);
    expect(ix.dialogs.has('ReviewDialog')).toBe(true);
    expect(ix.dialogs.has('SessionDetails')).toBe(false);
  });

  it('finds the openers it should (the lint is not blind)', () => {
    const details = buttonsOf(svelte['src/lib/SessionDetails.svelte'], ix, 'SessionDetails');
    const opens = (id: string) => details.find((b) => b.attrs.includes(`data-testid="${id}"`))?.opens;
    expect(opens('open-review')).toBe(true);
    expect(opens('kill-from-details')).toBe(true);
    expect(opens('label-from-details')).toBe(false);
  });

  it('says every status in the six and ends every dialog label with "…"', () => {
    const findings: CopyFinding[] = Object.entries(svelte).flatMap(([f, s]) => lintSvelte(f, s, ix));
    const waits = (f: CopyFinding) => WAITING.some((w) => w.file === f.file && w.text === f.text);
    expect(findings.filter((f) => !waits(f)).map((f) => `${f.rule} ${f.file}:${f.line} "${f.text}"`)).toEqual([]);
    const stale = WAITING.filter((w) => !findings.some((f) => f.file === w.file && f.text === w.text));
    expect(stale, 'fixed: drop these from WAITING').toEqual([]);
  });

  it('points at the Hub & sync settings page by the name the sidebar shows (review r20 D35)', () => {
    // settings_tree.ts labels the page "Hub & sync"; the backend's refusal
    // sentence is shown in the app too, so it is held to the same name.
    const files = { ...svelte, ...ts, 'src-tauri/src/backend/mod.rs': readFileSync('src-tauri/src/backend/mod.rs', 'utf8') };
    const stale = Object.entries(files).flatMap(([f, s]) =>
      s.split('\n').flatMap((line, i) => (/Settings → Hub(?! (&|&amp;) sync)/.test(line) ? [`${f}:${i + 1}`] : [])),
    );
    expect(stale).toEqual([]);
  });

  it("the row menu's labels match the Details buttons they run", () => {
    const details = buttonsOf(svelte['src/lib/SessionDetails.svelte'], ix, 'SessionDetails');
    for (const a of ROW_ACTIONS) {
      const b = details.find((d) => d.attrs.includes(`data-testid="${a.detailsTestId}"`));
      expect(b, a.detailsTestId).toBeDefined();
      expect(a.label.endsWith('…'), a.label).toBe(b!.opens);
    }
  });
});

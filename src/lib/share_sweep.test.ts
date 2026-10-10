// The sweep, multi-user M1 — INVERTED in task F2b.
//
// ── What this file proves, and what it does not ────────────────────────────
//
// It is a "did you think about it" gate, not a proof of behaviour: for every
// WRITE a surface can make to a session, it asks whether an access answer
// (`share.ts`'s `sessionBlocked` / `sessionActionBlocked` / `bulkTargets`, or
// `access.ts`'s `accessOf(row) === 'own'`) is in reach of that write. Whether
// the resulting control disables, what sentence it carries, and whether the
// owner keeps the action are pinned in each surface's own test file, with the
// positive control there. This file's job is to make whoever adds the next
// write come and read the rule instead of discovering it in review.
//
// **The limit, measured rather than suspected:** source scanning proves a gate
// was THOUGHT ABOUT at a call site, never that control flow passes through one —
// F2d's verifier deleted BOTH enforcement points in `outbox.ts` and this file
// stayed green, because the gate identifier is still lexically present elsewhere
// in that file and `gatesInReachOf` reads reach, not execution. What carries
// that weight is each control's own BEHAVIOURAL test, every one of them paired
// with the owner's positive control: `share_fail_closed.test.ts` (the
// fail-closed rule at all five surfaces, each refused on a paired desktop and
// allowed on a standalone one, and the outbox's two enforcement points through
// the real wiring — each mutation-checked), `share_write_paths.test.ts`,
// `share_f2b.test.ts`, and the access blocks in `TidyReview.test.ts`,
// `WorkReview.test.ts`, `TasksPanel.test.ts` and `SessionDetails.test.ts`. A new
// gate needs one of those as well as a green sweep; a green sweep alone means
// only that nobody forgot to write the words down.
//
// ── Why it is keyed on the write and not on the hub question ───────────────
//
// F2a's version keyed on the HUB half: it scanned for `hubActionBlocked('x')`
// and demanded `$sessionBlocked(…, 'x')` beside it. That audits only surfaces
// which already thought about gating — `if (asked.size === 0) continue` made a
// surface that asks NEITHER half invisible, which is exactly the shape of the
// two live write paths the third review found (`HostDetail`'s "Restore n lost
// sessions…", `Sidebar`'s `archived · show` chip). It was also per FILE and
// per ACTION rather than per CALL SITE, so it stayed green when a narrowing was
// deleted as long as some other line in the same file still named the action.
//
// So the question is asked the other way round, and per call site:
//
//   1. derive, from the store modules' own source, which exported function
//      invokes each `SESSION_TIER` command (`WRITERS`);
//   2. for every call to one of those functions in every surface, look for an
//      access answer in reach of THAT call — the enclosing handler, a gate
//      variable it reads, a gated runner it is an argument to, or a gate in the
//      writer's own module (the funnel);
//   3. assert the derived map is COMPLETE against `SESSION_ACTIONS`, so a new
//      tier row cannot be added without either a writer or a written reason
//      why the frontend has none.
//
// Step 2 is what makes a deleted narrowing go red: the gate has to be where the
// write is, not merely somewhere in the file.
//
// The old F2a sweep is kept below as a second, weaker check: a surface that
// asks the hub's half for an action and never the access half is still worth
// catching even when the write itself lives somewhere else.
//
// Modelled on `no_html_on_hub_text.test.ts` (the glob and the allowlist shape)
// and on `hub_verdicts.test.ts` (holding hand-written literals accountable to a
// single table).
import { describe, it, expect } from 'vitest';
import { SESSION_ACTIONS, sessionTierOf, type SessionAction, type SessionTier } from './share';

/** Every `.svelte` and `.ts` source under `src/`, keyed by its path relative to
 *  this module — `./Foo.svelte` for a sibling in `src/lib/`, `../Foo.svelte` for
 *  one in `src/`. Vite's `import.meta.glob`, not `node:fs`, for the reason
 *  `no_html_on_hub_text.test.ts` gives: the project ships no Node types, and the
 *  pattern resolves against THIS module's URL rather than the directory vitest
 *  was started from. */
const RAW = import.meta.glob('../**/*.{svelte,ts}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/**
 * Comments blanked out, offsets preserved.
 *
 * A comment is prose, and this file's prose is full of code: `ShareSheet`'s
 * `run` explains itself by naming `owned`, `SessionRowItem`'s controls quote
 * `$sessionBlocked(...)` in the paragraph above them. Counting that as a gate is
 * how a first draft of this sweep stayed green with `ShareSheet`'s re-ask
 * deleted — the sentence describing the gate stood in for the gate. Every
 * comment character becomes a space (newlines kept), so line numbers and brace
 * matching are unchanged and nothing in a comment can answer for code.
 *
 * String literals are kept verbatim: `=== 'own'` and `'kill_session'` are the
 * things being looked for. A `'` or `"` closes at end of line the way a JS
 * string literal must, so an apostrophe in markup prose costs one line of
 * fidelity rather than running away to the end of the file.
 */
function blankComments(src: string): string {
  const out = src.split('');
  type State = 'code' | 'line' | 'block' | 'html' | "'" | '"' | '`';
  let state: State = 'code';
  const blank = (i: number) => {
    if (out[i] !== '\n') out[i] = ' ';
  };
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    const next = src[i + 1];
    if (state === 'code') {
      if (c === '/' && next === '/') {
        state = 'line';
        blank(i);
      } else if (c === '/' && next === '*') {
        state = 'block';
        blank(i);
      } else if (c === '<' && src.startsWith('<!--', i)) {
        state = 'html';
        blank(i);
      } else if (c === "'" || c === '"' || c === '`') {
        state = c;
      }
      continue;
    }
    if (state === 'line') {
      if (c === '\n') state = 'code';
      else blank(i);
      continue;
    }
    if (state === 'block') {
      blank(i);
      if (c === '*' && next === '/') {
        blank(i + 1);
        i += 1;
        state = 'code';
      }
      continue;
    }
    if (state === 'html') {
      blank(i);
      if (c === '-' && src.startsWith('-->', i)) {
        blank(i + 1);
        blank(i + 2);
        i += 2;
        state = 'code';
      }
      continue;
    }
    // inside a string literal
    if (c === '\\') {
      i += 1;
      continue;
    }
    if (c === state) state = 'code';
    else if (c === '\n' && state !== '`') state = 'code';
  }
  return out.join('');
}

/** Test files and generated fixtures are not surfaces. */
const SOURCES: Record<string, string> = Object.fromEntries(
  Object.entries(RAW)
    .filter(([f]) => !/\.(test|spec)\.ts$/.test(f) && !f.includes('.generated.'))
    .map(([f, src]) => [f, blankComments(src)]),
);
/** The sources as written — only for the exemption pins, which quote real
 *  lines and must not be answered by a blanked comment either way. */
const RAW_SOURCES: Record<string, string> = Object.fromEntries(
  Object.entries(RAW).filter(([f]) => !/\.(test|spec)\.ts$/.test(f) && !f.includes('.generated.')),
);

const ACTIONS = new Set<string>(SESSION_ACTIONS);
/**
 * The rows this sweep gates: everything that WRITES. `share.ts` lists the
 * `watch` rows so the omission is visibly deliberate — "a control that only
 * reads has nothing to disable" — and that is just as true here: a read is not
 * a write path, and demanding a gate on `session_history` would mean an
 * exemption for every panel that draws a timeline.
 */
const WRITE_ACTIONS: readonly SessionAction[] = SESSION_ACTIONS.filter(
  (a) => sessionTierOf(a) !== 'watch',
);
const WRITES = new Set<string>(WRITE_ACTIONS);
const TIER_RANK: Record<SessionTier, number> = { watch: 1, answer: 2, drive: 3, own: 4 };

/**
 * The three modules that ARE the predicates rather than users of them:
 * `hub.ts` lists every routed action, `share.ts` holds the table, `access.ts`
 * derives the answer. Their own doc comments quote the calls, so scanning them
 * finds nothing but themselves.
 */
const PREDICATES = new Set(['./hub.ts', './share.ts', './access.ts']);

/** The one place the frontend's action vocabulary and the backend's command
 *  vocabulary differ — the same exception `hub_verdicts.test.ts` carries for
 *  `ROUTED_ACTIONS`, and for the same reason. */
const ACTION_COMMAND: Readonly<Record<string, string>> = {
  set_friendly_name: 'set_session_friendly_name',
};
const COMMAND_ACTION: Readonly<Record<string, SessionAction>> = Object.fromEntries(
  SESSION_ACTIONS.map((a) => [ACTION_COMMAND[a] ?? a, a]),
) as Record<string, SessionAction>;

// ── tiny source readers ───────────────────────────────────────────────────
//
// Regex alone cannot read an argument list: `bulkTargets(rows.flatMap((c) => {
// … }), 'tidy_apply', $sessionBlocked)` nests two levels deep, and a matcher
// that counts parentheses by pattern silently stops seeing the gate at the
// depth real code reaches (that is how TidyReview's narrowing went unnoticed
// while the regex version of this file was green). So the few things this sweep
// needs from the source — an argument list, a brace-matched block, the extent
// of a declaration — are read by balancing delimiters.

/** Every `{ … }` pair in `src`, as `[open, close]` offsets. */
function bracePairs(src: string): [number, number][] {
  const stack: number[] = [];
  const out: [number, number][] = [];
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    if (c === '{') stack.push(i);
    else if (c === '}') {
      const open = stack.pop();
      if (open !== undefined) out.push([open, i]);
    }
  }
  return out;
}

/** The offset of the `)` matching the `(` at `open`, or `src.length`. */
function matchParen(src: string, open: number): number {
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === '(') depth++;
    else if (src[i] === ')') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return src.length;
}

/** The top-level arguments of the call whose `(` is at `open`. */
function callArgs(src: string, open: number): string[] {
  const close = matchParen(src, open);
  const inner = src.slice(open + 1, close);
  const args: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < inner.length; i++) {
    const c = inner[i];
    if (c === '(' || c === '[' || c === '{') depth++;
    else if (c === ')' || c === ']' || c === '}') depth--;
    else if (c === ',' && depth === 0) {
      args.push(inner.slice(start, i));
      start = i + 1;
    }
  }
  if (inner.slice(start).trim() !== '') args.push(inner.slice(start));
  return args;
}

/** How far a `const NAME = …` declaration's initialiser reaches: to the `;`,
 *  or to the end of a line that does not continue. */
function declExtent(src: string, from: number): number {
  let depth = 0;
  for (let i = from; i < src.length; i++) {
    const c = src[i];
    if (c === '{' || c === '(' || c === '[') depth++;
    else if (c === '}' || c === ')' || c === ']') {
      depth--;
      if (depth < 0) return i;
    } else if (c === ';' && depth === 0) return i;
    else if (c === '\n' && depth === 0 && !'=&|?(,+:'.includes(src[i - 1])) return i;
  }
  return src.length;
}

/** The body block of the function whose name match ends at `afterName`, found
 *  by skipping the parameter list and then the return type — whose own braces
 *  (`Promise<Result<{ kind: 'moved' } & …>>`) are why this cannot simply take
 *  the next `{`. */
function functionBody(src: string, parenOpen: number): [number, number] | null {
  const close = matchParen(src, parenOpen);
  let angle = 0;
  for (let i = close + 1; i < src.length; i++) {
    const c = src[i];
    if (c === '<') angle++;
    else if (c === '>') angle = Math.max(0, angle - 1);
    else if (c === '{' && angle === 0) return [i, matchParen2(src, i)];
    else if (c === ';' && angle === 0) return null;
  }
  return null;
}
function matchParen2(src: string, open: number): number {
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return src.length;
}

/** `name(` occurrences that are CALLS of `name` — not a property access, not a
 *  longer identifier ending in it. */
function callSites(src: string, name: string): number[] {
  const re = new RegExp(String.raw`(?<![\w$.])${name}\s*\(`, 'g');
  const out: number[] = [];
  for (const m of src.matchAll(re)) out.push(m.index);
  return out;
}

// ── the gate shapes ───────────────────────────────────────────────────────

/** Marks a text that holds an `accessOf(row)` answer — on its own not a gate,
 *  but `=== 'own'` on one is the `own` tier itself. */
const ACCESS_VALUE = '@access';
/** A gate asked with the action as a VARIABLE. It counts for every action: the
 *  action it is asked for is the one its caller is about to perform, and the
 *  caller names it (`session_rename.ts`'s `accessBlocked(target, action)`,
 *  `LinkReview`'s `rowBlocked(row, action)`). Paired with the per-call-site
 *  reach check below, it cannot become a blanket licence: the parameterised
 *  gate still has to be where the write is. */
const ANY_ACTION = '*';

const GATE_FNS = [
  'sessionActionBlocked',
  'sessionBlocked',
  'bulkTargets',
  // The id-resolving pair (F2c): a write that holds no row asks these with the
  // session id instead, and they fail closed when the row cannot be resolved.
  'sessionIdActionBlocked',
  'sessionIdBlocked',
] as const;
/** Predicates that take an action NAME as their first argument and write
 *  nothing — the hub half (`hub.ts`) and the access half alike. A call to one
 *  is not a write, and `moveEligibility.ts`'s `moveBlockedReason` is exactly
 *  the helper that would otherwise read as a `move_session` writer. */
const NOT_A_WRITE = new Set<string>([
  ...GATE_FNS,
  'hubActionBlocked',
  'hubBlock',
  'sessionTierOf',
]);

/**
 * Which actions `text` holds an access answer for.
 *
 * The three shapes `share.ts` offers, with the action always the SECOND
 * argument: `$sessionBlocked(row, 'x')`, `sessionActionBlocked(row, 'x')` and
 * `bulkTargets(rows, 'x', $sessionBlocked)`. A literal second argument names
 * one action; an identifier there is {@link ANY_ACTION}.
 */
function gatesIn(text: string): Set<string> {
  const out = new Set<string>();
  for (const fn of GATE_FNS) {
    const re = new RegExp(String.raw`(?<![\w$.])\$?${fn}\s*\(`, 'g');
    for (const m of text.matchAll(re)) {
      const open = m.index + m[0].length - 1;
      const args = callArgs(text, open);
      const action = (args[1] ?? '').trim();
      const literal = /^'([a-z][a-z0-9_]*)'$/.exec(action);
      if (literal) out.add(literal[1]);
      else if (/^[a-z][\w$]*$/.test(action)) out.add(ANY_ACTION);
    }
  }
  if (/(?<![\w$.])\$?accessOf\s*\(/.test(text)) out.add(ACCESS_VALUE);
  return out;
}

/** True when `gates` answers for `action` — a gate for an action of the SAME
 *  OR A HIGHER tier counts, because `share.ts`'s refusal is a function of the
 *  tier and not of the action name: a write guarded by the `own` question is
 *  guarded more tightly than its own `drive` row needs (the argument
 *  `ReplyActions`' Retry already rests on). The reverse never holds. */
function covers(gates: ReadonlySet<string>, action: SessionAction): boolean {
  if (gates.has(ANY_ACTION)) return true;
  const need = TIER_RANK[sessionTierOf(action)];
  for (const g of gates) {
    if (!ACTIONS.has(g)) continue;
    if (TIER_RANK[sessionTierOf(g as SessionAction)] >= need) return true;
  }
  return false;
}

interface Definition {
  from: number;
  to: number;
}

/**
 * Every named definition in a file **at module scope**, with the extent of its
 * initialiser or body: `const x = …`, `let x = …`, `function x(…) { … }`.
 *
 * Module scope — nothing enclosing it in braces — and not merely "every
 * declaration of that name", because a name is reused inside handlers all the
 * time: HostDetail has seven `const r = await …` in seven different functions,
 * one of them a gated restore. Treating them as one definition made `r` read as
 * a gate variable in every one of those handlers, which is how a first draft of
 * this sweep stayed green with HostDetail's restore gate deleted. A local
 * variable needs no entry here: the enclosing-block scan below reads the
 * handler's own text directly.
 */
function definitions(src: string): Map<string, Definition> {
  const pairs = bracePairs(src);
  const atModuleScope = (at: number) => !pairs.some(([o, c]) => o < at && at < c);
  const out = new Map<string, Definition>();
  const add = (name: string, from: number, to: number) => {
    if (!atModuleScope(from)) return;
    const cur = out.get(name);
    if (cur === undefined) out.set(name, { from, to });
    else out.set(name, { from: Math.min(cur.from, from), to: Math.max(cur.to, to) });
  };
  for (const m of src.matchAll(/(?<![\w$.])(?:const|let|var)\s+(\w+)\s*(?::[^=\n]*)?=/g)) {
    const from = m.index + m[0].length;
    add(m[1], from, declExtent(src, from));
  }
  for (const m of src.matchAll(/(?<![\w$.])(?:async\s+)?function\s+(\w+)\s*(?:<[^>]*>)?\(/g)) {
    const body = functionBody(src, m.index + m[0].length - 1);
    if (body) add(m[1], body[0], body[1]);
  }
  return out;
}

/**
 * identifier → the actions reading it answers for, resolved transitively.
 *
 * A component almost never asks the gate at the write: it computes
 * `const killBlocked = $derived(hubActionBlocked(…) ?? $sessionBlocked(row,
 * 'kill_session'))` once and reads `killBlocked` from the handler and from the
 * button's `disabled`. So an identifier inherits the gates of everything its
 * own definition reads, to a fixed point — which is also how `applyItems`'
 * argument `allowed` in `TidyReview` inherits `accessBlocked`'s
 * `$sessionIdBlocked`, two definitions away from the write.
 */
function gateIdentifiers(src: string, defs: Map<string, Definition>): Map<string, Set<string>> {
  const text = new Map<string, string>();
  const gates = new Map<string, Set<string>>();
  for (const [name, d] of defs) {
    const t = src.slice(d.from, d.to);
    text.set(name, t);
    gates.set(name, gatesIn(t));
  }
  /** Transitive closure over the identifiers each definition reads. */
  const spread = () => {
    for (let pass = 0; pass < 8; pass++) {
      let grew = false;
      for (const [name, t] of text) {
        const mine = gates.get(name)!;
        for (const ref of new Set(t.match(/[A-Za-z_$][\w$]*/g) ?? [])) {
          if (ref === name) continue;
          const theirs = gates.get(ref);
          if (!theirs) continue;
          for (const g of theirs) {
            if (!mine.has(g)) {
              mine.add(g);
              grew = true;
            }
          }
        }
      }
      if (!grew) break;
    }
  };
  spread();
  // `accessOf(row) === 'own'` is the `own` tier itself, which is every action.
  // Done BETWEEN two closures, because the comparison is usually one derivation
  // away from the read (`const a = $accessOf(row)` then `const owned = a ===
  // 'own'`) and the result then has to spread to whatever asks `owned`.
  for (const [name, t] of text) {
    const mine = gates.get(name)!;
    if (mine.has(ACCESS_VALUE) && t.includes("'own'")) mine.add(ANY_ACTION);
  }
  spread();
  return gates;
}

/** The enclosing `{ … }` blocks of `at`, innermost first. */
function enclosingBlocks(pairs: readonly [number, number][], at: number): [number, number][] {
  return pairs.filter(([o, c]) => o < at && at < c).sort((a, b) => a[1] - a[0] - (b[1] - b[0]));
}

interface FileIndex {
  src: string;
  pairs: [number, number][];
  defs: Map<string, Definition>;
  gates: Map<string, Set<string>>;
}

const INDEX = new Map<string, FileIndex>();
function indexOf(file: string): FileIndex {
  let idx = INDEX.get(file);
  if (!idx) {
    const src = SOURCES[file];
    const defs = definitions(src);
    idx = { src, pairs: bracePairs(src), defs, gates: gateIdentifiers(src, defs) };
    INDEX.set(file, idx);
  }
  return idx;
}

/**
 * Every access answer in reach of the call at `at`, by the three ways a
 * handler can hold one:
 *
 *  1. **in the enclosing blocks** — the handler asks for itself, or reads a
 *     gate variable (`if (workBusy || workBlocked !== null) return;`);
 *  2. **a gated runner** — the write is an argument to a local function that
 *     asks before it runs it (`WorkReview`'s `run(it, …, () => confirm(…))`,
 *     `SessionRowItem`'s `workAction`, `SessionTasks`' `act`). The gate is one
 *     frame up the call, which no lexical scope contains;
 *  3. nothing — and then the call site is reported.
 */
function gatesInReachOf(idx: FileIndex, at: number): Set<string> {
  const seen = new Set<string>();
  for (const [open, close] of enclosingBlocks(idx.pairs, at)) {
    const text = idx.src.slice(open, close);
    for (const g of gatesIn(text)) seen.add(g);
    for (const ref of new Set(text.match(/[A-Za-z_$][\w$]*/g) ?? [])) {
      const g = idx.gates.get(ref);
      if (g) for (const x of g) seen.add(x);
    }
  }
  // (2): a call whose argument list spans `at`, to a local definition that gates.
  for (const m of idx.src.slice(0, at).matchAll(/(?<![\w$.])([A-Za-z_$][\w$]*)\s*\(/g)) {
    const runner = idx.gates.get(m[1]);
    if (!runner || runner.size === 0) continue;
    const open = m.index + m[0].length - 1;
    if (open < at && at <= matchParen(idx.src, open)) for (const g of runner) seen.add(g);
  }
  return seen;
}

// ── 1. the write map, derived from the store modules ──────────────────────

/** Modules, as opposed to components: this is where a command name appears. */
const STORE_MODULES = Object.keys(SOURCES)
  .filter((f) => f.endsWith('.ts') && !PREDICATES.has(f))
  .sort();

/**
 * Which exported function invokes each `SESSION_TIER` command, read out of the
 * store modules themselves.
 *
 * The command name is a string literal in the first argument of SOME call —
 * `invokeCmd('kill_session', …)` in `sessions.ts`, but also `decide('…')`,
 * `rowCmd('…')` and `write('…')`, the per-module helpers the work graph's
 * stores wrap it in. So the literal is what is looked for, and the writer is
 * the nearest top-level `export` declaration above it. A literal inside a
 * private helper that sits between two exports would be misattributed to the
 * earlier one; none exists today, and the completeness assertion below is what
 * would notice if the shape changed.
 */
function deriveWriters(): { byAction: Map<SessionAction, Set<string>>; home: Map<string, string> } {
  const byAction = new Map<SessionAction, Set<string>>();
  const home = new Map<string, string>();
  for (const file of STORE_MODULES) {
    const src = SOURCES[file];
    const decls: { at: number; exported: boolean; name: string }[] = [];
    for (const m of src.matchAll(/^(export )?(?:async )?(?:function|const|let) (\w+)/gm)) {
      decls.push({ at: m.index, exported: m[1] !== undefined, name: m[2] });
    }
    for (const m of src.matchAll(/(?<![\w$.])([A-Za-z_$][\w$]*)\s*(?:<[^>()]*?>)?\(\s*'([a-z][a-z0-9_]*)'/g)) {
      const action = COMMAND_ACTION[m[2]];
      // A gate predicate takes an action name too; it is not a write.
      if (!action || NOT_A_WRITE.has(m[1].replace(/^\$/, ''))) continue;
      let owner: { exported: boolean; name: string } | undefined;
      for (const d of decls) {
        if (d.at < m.index) owner = d;
        else break;
      }
      if (!owner?.exported) continue;
      home.set(owner.name, file);
      let set = byAction.get(action);
      if (!set) byAction.set(action, (set = new Set()));
      set.add(owner.name);
    }
  }
  return { byAction, home };
}

/**
 * Writers that invoke a command at a NARROWER tier than the command's own row,
 * because of the arguments they send (Orbit Fleet 11.7). `send_prompt` is
 * `drive`, but `send_prompt` with an empty prompt and one key is what the hub
 * admits at `answer`: a key-only writer is recorded as writing its own
 * action. Pinned to the body that makes it true, so a writer that grows a
 * prompt argument goes back to being a `send_prompt` writer.
 */
const KEY_ONLY_WRITERS: readonly {
  writer: string;
  file: string;
  from: SessionAction;
  to: SessionAction;
  pins: readonly string[];
}[] = [
  {
    writer: 'answerDialog',
    file: './sessions.ts',
    from: 'send_prompt',
    to: 'answer_dialog',
    pins: ["args: { host_alias: hostAlias, tmux_name: tmuxName, prompt: '', keys: key, ...(expect ? { expect } : {}) },"],
  },
];

function narrowedWriters(derived: ReturnType<typeof deriveWriters>): ReturnType<typeof deriveWriters> {
  for (const k of KEY_ONLY_WRITERS) {
    derived.byAction.get(k.from)?.delete(k.writer);
    let set = derived.byAction.get(k.to);
    if (!set) derived.byAction.set(k.to, (set = new Set()));
    set.add(k.writer);
  }
  return derived;
}

const { byAction: DERIVED, home: WRITER_HOME } = narrowedWriters(deriveWriters());

/**
 * Second-order writers: a store function that reaches a `SESSION_TIER` command
 * through another store function rather than naming it — DERIVED, since F2c.
 *
 * The hand-written list this replaces was never asserted complete, and that
 * cost twice: `moves.ts`'s lifecycle wrappers and `operator.ts`'s restart were
 * each an invisible write path until someone noticed by hand. A list whose
 * completeness nothing checks is a list that is wrong as soon as code moves.
 *
 * The derivation is deliberately narrow, which is what the old comment's
 * objection ("it spreads a write along every convenience wrapper until half the
 * app is a `kill_session` writer") was really about:
 *
 *  - only MODULE-SCOPE declarations in store modules, resolved inside their own
 *    file first and only then by name across the fleet of modules;
 *  - only names that are CALLED (`f(`), never merely mentioned;
 *  - and the result is used to demand a gate, never to grant one — so a
 *    too-wide derivation costs a gate nobody needed, and a too-narrow one is
 *    exactly the hole this closes. Erring wide is the safe direction here, and
 *    it is the opposite of the direction `funnelGated` errs in.
 *
 * `KNOWN_RELAYS` below pins the ones found by hand in F2b, so a derivation that
 * silently stopped finding them cannot read as "there are no relays".
 */
function moduleDecls(src: string): { name: string; exported: boolean; from: number; to: number }[] {
  const defs = definitions(src);
  const exported = new Set<string>();
  for (const m of src.matchAll(/^export\s+(?:async\s+)?(?:function|const|let|var)\s+(\w+)/gm)) {
    exported.add(m[1]);
  }
  return [...defs].map(([name, d]) => ({ name, exported: exported.has(name), from: d.from, to: d.to }));
}

function deriveRelays(
  derivedWriters: Map<SessionAction, Set<string>>,
  home: Map<string, string>,
): Map<string, { file: string; actions: Set<SessionAction> }> {
  interface Node {
    file: string;
    name: string;
    exported: boolean;
    text: string;
    actions: Set<SessionAction>;
  }
  const nodes = new Map<string, Node>();
  const byFile = new Map<string, Map<string, string>>(); // file → name → key
  for (const file of STORE_MODULES) {
    const src = SOURCES[file];
    const local = new Map<string, string>();
    for (const d of moduleDecls(src)) {
      const key = `${file}::${d.name}`;
      nodes.set(key, {
        file,
        name: d.name,
        exported: d.exported,
        text: src.slice(d.from, d.to),
        actions: new Set(),
      });
      local.set(d.name, key);
    }
    byFile.set(file, local);
  }
  // Seed: the writers that name the command themselves.
  for (const [action, names] of derivedWriters) {
    for (const name of names) {
      const key = `${home.get(name)}::${name}`;
      nodes.get(key)?.actions.add(action);
    }
  }
  /** `name` as called from `file`: its own module first, then any module that
   *  defines it — the same bare-name resolution `WRITER_HOME` already uses. */
  const resolve = (file: string, name: string): Node | undefined => {
    const own = byFile.get(file)?.get(name);
    if (own) return nodes.get(own);
    for (const [f, local] of byFile) {
      if (f === file) continue;
      const k = local.get(name);
      const n = k ? nodes.get(k) : undefined;
      if (n?.exported && n.actions.size > 0) return n;
    }
    return undefined;
  };
  for (let pass = 0; pass < 8; pass++) {
    let grew = false;
    for (const node of nodes.values()) {
      for (const m of node.text.matchAll(/(?<![\w$.])([A-Za-z_$][\w$]*)\s*\(/g)) {
        if (NOT_A_WRITE.has(m[1])) continue;
        const callee = resolve(node.file, m[1]);
        if (!callee || callee === node) continue;
        for (const a of callee.actions) {
          if (!node.actions.has(a)) {
            node.actions.add(a);
            grew = true;
          }
        }
      }
    }
    if (!grew) break;
  }
  const out = new Map<string, { file: string; actions: Set<SessionAction> }>();
  for (const node of nodes.values()) {
    if (!node.exported || node.actions.size === 0) continue;
    // A writer that names the command itself is already in the map.
    if (home.get(node.name) === node.file) continue;
    const cur = out.get(node.name) ?? { file: node.file, actions: new Set<SessionAction>() };
    for (const a of node.actions) cur.actions.add(a);
    out.set(node.name, cur);
  }
  return out;
}

const RELAYS = deriveRelays(DERIVED, WRITER_HOME);

/**
 * The relays F2b found by hand. Each must still be derived, with the action it
 * was found for — the completeness assertion the hand-written list never had.
 */
const KNOWN_RELAYS: Record<string, { file: string; actions: readonly SessionAction[] }> = {
  startMove: { file: './moves.ts', actions: ['move_session'] },
  retryMove: { file: './moves.ts', actions: ['move_session'] },
  cancelWait: { file: './moves.ts', actions: ['move_session'] },
  requestPreflight: { file: './preflight.ts', actions: ['move_session'] },
  nameWorkForSessions: {
    file: './work.ts',
    actions: ['name_session_work', 'link_session_work'],
  },
  applySessionRename: {
    file: './session_rename.ts',
    actions: ['rename_session', 'set_friendly_name'],
  },
  restartOperator: { file: './operator.ts', actions: ['restart_session'] },
};

/**
 * `SESSION_TIER` rows the frontend has no writer for, with the reason. The
 * partition against `SESSION_ACTIONS` is asserted both ways below, so a new row
 * cannot be parked here once something starts writing it, and a row whose
 * writer is deleted or renamed has to be accounted for.
 */
const NO_FRONTEND_WRITER: Partial<Record<SessionAction, string>> = {
  send_message:
    'An MCP tool only (`send_message { deliver, submit }`): no Svelte surface sends one. It is in ' +
    'SESSION_TIER because the hub must answer the same way for it as for `send_prompt`.',
  dispatch_task:
    'Likewise MCP-only — the desktop watches tasks (TasksPanel) and cancels them, but dispatching ' +
    'one is the control API’s.',
  set_session_tags:
    'No desktop command exists yet: tags arrive on the row and are shown, and `set_session_tags` ' +
    'is reachable only through the control API.',
  delete_worktree:
    'MCP-only since the desktop command went (no component called it): fleet-mobile and agents ' +
    'still delete a worktree through the control API.',
  archive_session_work:
    'MCP-only since the desktop command went: the Tidy-up sheet archives through `tidy_apply` ' +
    'items, and fleet-mobile and agents call the `work_link` archive action.',
};

/** Every writer this sweep follows: the derived ones plus the verified relays. */
function writerMap(): Map<string, { actions: Set<SessionAction>; file: string }> {
  const out = new Map<string, { actions: Set<SessionAction>; file: string }>();
  for (const [action, names] of DERIVED) {
    for (const name of names) {
      const file = WRITER_HOME.get(name)!;
      const cur = out.get(name) ?? { actions: new Set<SessionAction>(), file };
      cur.actions.add(action);
      out.set(name, cur);
    }
  }
  for (const [name, r] of RELAYS) {
    const cur = out.get(name) ?? { actions: new Set<SessionAction>(), file: r.file };
    for (const a of r.actions) cur.actions.add(a);
    out.set(name, cur);
  }
  return out;
}

const WRITERS = writerMap();

/**
 * A writer whose OWN module asks the access half before it writes — the gate at
 * the funnel. `session_rename.ts` does it because the rename editor also opens
 * on a double-click, which consults no button; `moves.ts`, `preflight.ts` and
 * `operator.ts` do it because the move lifecycle and the agent's restart are
 * reached from a sheet, a chip, a panel and a toast action alike. Calls to
 * these need no gate of their own, and a surface that adds one is composing,
 * not duplicating.
 */
function funnelGated(name: string, file: string, action: SessionAction): boolean {
  const idx = indexOf(file);
  const own = idx.gates.get(name);
  return own !== undefined && covers(own, action);
}

// ── 2. the call-site exemptions ───────────────────────────────────────────

/**
 * A write whose gate this sweep cannot see, with the argument made here rather
 * than in a review comment, and `pins`: substrings that must still be present
 * in the named files. An exemption with no pin is not an exemption, it is an
 * unverified claim — once the composition it rests on is edited away, this test
 * goes red instead of the exemption quietly becoming false.
 */
interface Exemption {
  writer: string;
  action: SessionAction;
  /**
   * The CALL SITES this exemption covers, each named by a substring of the
   * statement the call is in.
   *
   * F2c. The previous shape was `(file, writer, action)` and skipped the whole
   * site loop, so a SECOND call of the same writer in an exempt file was never
   * looked at — the exemption written for one line silently licensed every
   * other line beside it. A needle names one line, every other call of that
   * writer in the file is checked as usual, and {@link EXEMPT_SITES}'s own
   * test below fails when a needle stops matching an ungated site.
   */
  sites: readonly string[];
  why: string;
  pins: readonly { file: string; needle: string }[];
}

const EXEMPT_SITES: Record<string, readonly Exemption[]> = {
  './ReplyActions.svelte': [
    {
      writer: 'rewindConversation',
      action: 'rewind_conversation',
      sites: ["await rewindConversation(sessionId, 'rewind', view.rewindAnchor)"],
      why:
        'It is given an id, a host and a tmux name — never the row — so it cannot ask for itself; ' +
        'the access half arrives as the `accessBlocked` prop, which ConversationPanel computes from ' +
        'the row it holds and which Retry composes on top of (the rewind gate is `own`, strictly ' +
        'stronger than `send_prompt`’s `drive`).',
      pins: [
        { file: './ReplyActions.svelte', needle: 'accessBlocked?: string | null;' },
        { file: './ReplyActions.svelte', needle: 'rewindBlocked = $derived' },
        {
          file: './ConversationPanel.svelte',
          needle: "$sessionBlocked(session, 'rewind_conversation')",
        },
        { file: './ConversationPanel.svelte', needle: 'accessBlocked={rewindShareBlocked}' },
      ],
    },
  ],
  './forms/FormCard.svelte': [
    {
      writer: 'answerForm',
      action: 'answer_form',
      sites: ['await answerForm(formId, values)'],
      why:
        'The card is given a form id, never the row, so it cannot ask for itself; the access half ' +
        'arrives as the `blocked` prop, which ConversationPanel computes from the row it holds ' +
        '(`hubActionBlocked` ?? `$sessionBlocked`), and the card refuses to write while it is set. The `closed` card has no write path.',
      pins: [
        { file: './forms/FormCard.svelte', needle: 'blocked: string | null;' },
        { file: './forms/FormCard.svelte', needle: 'if (blocked !== null) return;' },
        { file: './ConversationPanel.svelte', needle: "$sessionBlocked(session, 'answer_form')" },
        { file: './ConversationPanel.svelte', needle: 'blocked={formBlocked}' },
      ],
    },
    {
      writer: 'declineForm',
      action: 'decline_form',
      sites: ['await declineForm(formId, note)'],
      why:
        'Same as `answerForm`: `decline_form` is `drive` too, and the one `blocked` prop gates both ' +
        'writes of the card.',
      pins: [
        { file: './forms/FormCard.svelte', needle: 'blocked: string | null;' },
        { file: './forms/FormCard.svelte', needle: 'if (blocked !== null) return;' },
        { file: './ConversationPanel.svelte', needle: "$sessionBlocked(session, 'answer_form')" },
        { file: './ConversationPanel.svelte', needle: 'blocked={formBlocked}' },
      ],
    },
  ],
};

/** The text of the statement the call at `at` lives in: from the start of its
 *  line to the `)` that closes the call. What an {@link Exemption}'s `sites`
 *  needle is matched against, so an exemption names ONE line. */
function callStatement(src: string, at: number): string {
  const lineStart = src.lastIndexOf('\n', at) + 1;
  const paren = src.indexOf('(', at);
  const end = paren === -1 ? at : matchParen(src, paren) + 1;
  return src.slice(lineStart, Math.max(end + 1, at));
}

/**
 * Writes that name the command THEMSELVES, outside a store module's exported
 * writer — the hole F2c closes.
 *
 * `deriveWriters` reads only `.ts` modules and only attributes a literal to the
 * nearest EXPORTED declaration above it, so two shapes were invisible to the
 * whole sweep: a `.svelte` component that calls `invokeCmd('…')` directly
 * (ConversationPanel does, at three call sites), and a `.ts` module's PRIVATE
 * helper that names a command no export is above. Neither has a writer name for
 * a surface to call, so neither could ever appear in `WRITERS`; both are
 * nonetheless a write to a session.
 *
 * So they are call sites in their own right, and the same reach check applies.
 */
function directWriteSites(file: string, src: string): { at: number; action: SessionAction }[] {
  const out: { at: number; action: SessionAction }[] = [];
  const isModule = file.endsWith('.ts');
  const decls = isModule
    ? [...src.matchAll(/^(export )?(?:async )?(?:function|const|let) (\w+)/gm)].map((m) => ({
        at: m.index,
        exported: m[1] !== undefined,
      }))
    : [];
  for (const m of src.matchAll(
    /(?<![\w$.])([A-Za-z_$][\w$]*)\s*(?:<[^>()]*?>)?\(\s*'([a-z][a-z0-9_]*)'/g,
  )) {
    const action = COMMAND_ACTION[m[2]];
    if (!action || !WRITES.has(action)) continue;
    if (NOT_A_WRITE.has(m[1].replace(/^\$/, ''))) continue;
    if (isModule) {
      // An exported writer's own body: `WRITERS` already follows it to its
      // call sites, and demanding a gate inside `sessions.ts` would demand one
      // in every store function there is.
      let owner: { exported: boolean } | undefined;
      for (const d of decls) {
        if (d.at < m.index) owner = d;
        else break;
      }
      if (owner?.exported) continue;
    }
    out.push({ at: m.index, action });
  }
  return out;
}

// ── the checks ────────────────────────────────────────────────────────────

describe('the sweep sees what it claims to', () => {
  const files = Object.keys(SOURCES).sort();

  it('globbed the whole frontend', () => {
    // A broken glob would make every assertion below vacuously green.
    expect(files.length).toBeGreaterThan(200);
    expect(files).toContain('./SessionRowItem.svelte');
    expect(files).toContain('./TidyReview.svelte');
    expect(files).toContain('./share.ts');
    expect(files).toContain('../App.svelte');
    expect(files).not.toContain('./share.test.ts');
    expect(STORE_MODULES).toContain('./sessions.ts');
    expect(STORE_MODULES).toContain('./work.ts');
  });

  it('reads the action set out of SESSION_TIER itself, not a copy', () => {
    expect(ACTIONS.size).toBeGreaterThan(30);
    for (const a of ['kill_session', 'send_prompt', 'tidy_apply', 'decide_work_batch']) {
      expect(ACTIONS.has(a), a).toBe(true);
    }
    // Not a session action: a fleet-wide command, which this sweep must leave
    // alone or every surface in the app would need an exemption.
    expect(ACTIONS.has('add_project')).toBe(false);
    expect(ACTIONS.has('new_session')).toBe(false);
  });

  it('reads an argument list by balancing it, not by pattern', () => {
    // The regression this replaced a regex for: TidyReview's narrowing nests a
    // callback inside the first argument, two levels deep.
    const nested = "bulkTargets(rows.flatMap((c) => { return [c]; }), 'tidy_apply', $sessionBlocked)";
    expect(gatesIn(nested).has('tidy_apply')).toBe(true);
    expect(gatesIn("$sessionBlocked(rowById.get(c.session_id), 'kill_session')").has('kill_session')).toBe(
      true,
    );
    // A bare mention outside a call counts for nothing.
    expect(gatesIn("// sessionBlocked is mentioned for 'kill_session'").size).toBe(0);
    // And a CALL-SHAPED mention in a comment counts for nothing either, because
    // the comment is blanked before anything looks at it. This is the one that
    // matters: every gated control in this app is introduced by a paragraph
    // naming the gate it asks.
    expect(
      gatesIn(blankComments("// see $sessionBlocked(row, 'kill_session') above\n")).size,
    ).toBe(0);
    expect(
      gatesIn(blankComments("<!-- gated on $sessionBlocked(row, 'kill_session') -->")).size,
    ).toBe(0);
    expect(gatesIn(blankComments("/* $sessionBlocked(row, 'kill_session') */")).size).toBe(0);
    // …while the code beside it is untouched, and offsets do not move.
    const mixed = "// $sessionBlocked(r, 'kill_session')\nconst a = $sessionBlocked(r, 'send_prompt');";
    expect(blankComments(mixed).length).toBe(mixed.length);
    expect([...gatesIn(blankComments(mixed))]).toEqual(['send_prompt']);
    // An action passed as a variable gates whatever its caller names.
    expect(gatesIn('sessionActionBlocked(row, action)').has(ANY_ACTION)).toBe(true);
    // `accessOf(row) === 'own'` is the own tier itself.
    const src = "const a = $accessOf(row);\nconst owned = a === 'own';\n";
    const g = gateIdentifiers(src, definitions(src));
    expect(g.get('owned')?.has(ANY_ACTION)).toBe(true);
    expect(g.get('a')?.has(ANY_ACTION)).toBe(false);
  });

  it('a gate for a higher tier covers a lower-tier write, never the reverse', () => {
    expect(covers(new Set(['kill_session']), 'send_prompt')).toBe(true);
    expect(covers(new Set(['send_prompt']), 'kill_session')).toBe(false);
    expect(covers(new Set(['link_session_work']), 'unlink_session_work')).toBe(true);
  });

  it('finds the gate in reach of a write, and misses it when it is deleted', () => {
    // The negative control the F2a version lacked: the matchers are replayed
    // over a known-good handler and over the same handler with the narrowing
    // removed, so "it would notice" is a measurement rather than a hope.
    const gated = [
      '<script>',
      "  const killBlocked = $derived($sessionBlocked(sess, 'kill_session'));",
      '  async function doKill() {',
      '    if (killBlocked !== null) return;',
      '    await killSession(sess.host_alias, sess.tmux_name);',
      '  }',
      '</script>',
    ].join('\n');
    const ungated = gated.replace('    if (killBlocked !== null) return;\n', '');
    const look = (src: string) => {
      const defs = definitions(src);
      const idx: FileIndex = { src, pairs: bracePairs(src), defs, gates: gateIdentifiers(src, defs) };
      return covers(gatesInReachOf(idx, callSites(src, 'killSession')[0]), 'kill_session');
    };
    expect(look(gated)).toBe(true);
    expect(look(ungated)).toBe(false);
  });

  it('sees a write handed to a gated runner', () => {
    const src = [
      '<script>',
      "  const workBlocked = $derived($sessionBlocked(sess, 'link_session_work'));",
      '  async function workAction(run) {',
      '    if (workBlocked !== null) return;',
      '    await run();',
      '  }',
      '  function setWork() {',
      '    void workAction(() => linkSessionWork(sess.id, { key }));',
      '  }',
      '</script>',
    ].join('\n');
    const defs = definitions(src);
    const idx: FileIndex = { src, pairs: bracePairs(src), defs, gates: gateIdentifiers(src, defs) };
    const at = callSites(src, 'linkSessionWork')[0];
    expect(covers(gatesInReachOf(idx, at), 'link_session_work')).toBe(true);
  });
});

describe('the write map is derived and complete', () => {
  it('found a writer for the commands the stores name', () => {
    // Spot checks, so a derivation that silently stopped finding anything
    // cannot read as "nothing writes sessions any more".
    const expected: Record<string, string> = {
      kill_session: 'killSession',
      restore_host_sessions: 'restoreHostSessions',
      unarchive_session_work: 'unarchiveSession',
      tidy_apply: 'applyTidy',
      summarize_past_work: 'summarizePastWork',
      move_session: 'moveSession',
      set_friendly_name: 'setFriendlyName',
      decide_work_batch: 'decideWorkBatch',
    };
    for (const [action, writer] of Object.entries(expected)) {
      expect(
        [...(DERIVED.get(action as SessionAction) ?? [])],
        `${action} should be written by ${writer}`,
      ).toContain(writer);
    }
  });

  it('the rows this sweep skips are exactly the reads', () => {
    const skipped = SESSION_ACTIONS.filter((a) => !WRITES.has(a));
    expect(skipped.every((a) => sessionTierOf(a) === 'watch')).toBe(true);
    expect(skipped).toEqual(['capture_session', 'session_conversation', 'session_history']);
  });

  it('every writing SESSION_TIER row either has a writer or a written reason it has none', () => {
    const missing = WRITE_ACTIONS.filter(
      (a) => (DERIVED.get(a)?.size ?? 0) === 0 && NO_FRONTEND_WRITER[a] === undefined,
    );
    expect(
      missing,
      `These actions are in SESSION_TIER and nothing in src/ writes them, and no reason is ` +
        `recorded:\n  ${missing.join('\n  ')}\n\n` +
        'Either the store function that invokes the command is not exported (export it, or the ' +
        'sweep cannot see the write), or the frontend genuinely has no writer — in which case add ' +
        'it to NO_FRONTEND_WRITER with the reason.',
    ).toEqual([]);
  });

  it('nothing is parked in NO_FRONTEND_WRITER that does have a writer', () => {
    for (const action of Object.keys(NO_FRONTEND_WRITER) as SessionAction[]) {
      expect(WRITES.has(action), `${action} is not a writing SESSION_TIER action`).toBe(true);
      expect(
        [...(DERIVED.get(action) ?? [])],
        `${action} has a writer now — remove it from NO_FRONTEND_WRITER so the sweep gates it`,
      ).toEqual([]);
      expect((NO_FRONTEND_WRITER[action] ?? '').length, action).toBeGreaterThan(40);
    }
  });

  it('the relays are derived, and still find every one F2b found by hand', () => {
    for (const [name, want] of Object.entries(KNOWN_RELAYS)) {
      const got = RELAYS.get(name);
      expect(
        got,
        `${name} is no longer derived as a relay. Either it stopped reaching a writer (then ` +
          `delete its KNOWN_RELAYS row and say why), or deriveRelays() has a hole — which is the ` +
          'bug this pin exists to catch.',
      ).toBeTruthy();
      expect(got!.file, name).toBe(want.file);
      for (const a of want.actions) expect([...got!.actions], name).toContain(a);
    }
  });

  it('the derivation reaches a two-hop relay and stops at a non-writer', () => {
    // A measurement, not a hope: the fixed point is replayed over a shape the
    // real modules have (a wrapper calling a wrapper calling the writer).
    const fake = [
      "export function writerA() { return invokeCmd('kill_session', {}); }",
      'function hidden() { return writerA(); }',
      'export function relayB() { return hidden(); }',
      'export function unrelated() { return somethingElse(); }',
    ].join('\n');
    const saved = SOURCES['./__relay_fixture__.ts'];
    SOURCES['./__relay_fixture__.ts'] = fake;
    STORE_MODULES.push('./__relay_fixture__.ts');
    try {
      const { byAction, home } = deriveWriters();
      const relays = deriveRelays(byAction, home);
      expect([...(byAction.get('kill_session') ?? [])]).toContain('writerA');
      expect([...(relays.get('relayB')?.actions ?? [])]).toEqual(['kill_session']);
      expect(relays.has('unrelated')).toBe(false);
      expect(relays.has('hidden')).toBe(false); // not exported: not a relay
    } finally {
      STORE_MODULES.splice(STORE_MODULES.indexOf('./__relay_fixture__.ts'), 1);
      if (saved === undefined) delete SOURCES['./__relay_fixture__.ts'];
      else SOURCES['./__relay_fixture__.ts'] = saved;
    }
  });
});

describe('every write to a session has an access answer in reach', () => {
  it('no call site writes a SESSION_TIER action with no gate near it', () => {
    const offenders: string[] = [];
    for (const [file, src] of Object.entries(SOURCES)) {
      if (PREDICATES.has(file)) continue;
      let idx: FileIndex | null = null;
      const report = (at: number, what: string, action: SessionAction) => {
        idx ??= indexOf(file);
        if (covers(gatesInReachOf(idx, at), action)) return;
        // F2c: an exemption names its SITE, not the file. Every other call of
        // the same writer in the same file is still checked.
        const stmt = callStatement(src, at);
        const exempt = (EXEMPT_SITES[file] ?? []).some(
          (e) => e.writer === what && e.action === action && e.sites.some((n) => stmt.includes(n)),
        );
        if (exempt) return;
        const line = src.slice(0, at).split('\n').length;
        offenders.push(`${file}:${line} ${what}() → ${action} (${sessionTierOf(action)})`);
      };
      for (const [writer, { actions, file: home }] of WRITERS) {
        if (home === file) continue; // the definition, not a use
        const sites = callSites(src, writer);
        if (sites.length === 0) continue;
        for (const action of actions) {
          if (!WRITES.has(action)) continue; // a read: see WRITE_ACTIONS
          if (funnelGated(writer, home, action)) continue;
          for (const at of sites) report(at, writer, action);
        }
      }
      for (const { at, action } of directWriteSites(file, src)) {
        report(at, 'invokeCmd', action);
      }
    }
    expect(
      offenders.sort(),
      `A session write has no access answer in reach:\n  ${offenders.sort().join('\n  ')}\n\n` +
        'Put the gate where the WRITE is, not only on the control that normally reaches it:\n' +
        "  if (killBlocked !== null) return;   // killBlocked = hubActionBlocked('kill_session', …) ?? $sessionBlocked(row, 'kill_session')\n" +
        'For a batch or a fan-out, narrow per target with `bulkTargets`. Where the surface holds ' +
        'only an indirect id (a link id, a run id, a WorkLink), ask `$sessionIdBlocked(id, action)`, ' +
        'which resolves the row and FAILS CLOSED when it cannot — rather than leaving a gate that ' +
        'answers `null` in its own normal case. Where several writes share one runner, gate the ' +
        'runner. If the gate must live in another file, add the SITE to EXEMPT_SITES with the ' +
        'argument and at least one pin.',
    ).toEqual([]);
  });

  it('sees a command a component names itself, and misses nothing when it moves', () => {
    // The negative control for `directWriteSites`: the same write, once in a
    // component with a gate and once without.
    const gated = [
      '<script>',
      "  const killBlocked = $derived($sessionBlocked(sess, 'kill_session'));",
      '  async function doKill() {',
      '    if (killBlocked !== null) return;',
      "    await invokeCmd('kill_session', { args: { session_id: sess.id } });",
      '  }',
      '</script>',
    ].join('\n');
    const ungated = gated.replace('    if (killBlocked !== null) return;\n', '');
    const look = (src: string) => {
      const sites = directWriteSites('./Fake.svelte', src);
      expect(sites.map((x) => x.action)).toEqual(['kill_session']);
      const defs = definitions(src);
      const idx: FileIndex = { src, pairs: bracePairs(src), defs, gates: gateIdentifiers(src, defs) };
      return covers(gatesInReachOf(idx, sites[0].at), 'kill_session');
    };
    expect(look(gated)).toBe(true);
    expect(look(ungated)).toBe(false);
    // A store module's exported writer is NOT a direct site: WRITERS has it.
    expect(
      directWriteSites('./fake.ts', "export function killSession(id) { return invokeCmd('kill_session', {}); }"),
    ).toEqual([]);
    // …but a private helper in the same module is.
    expect(
      directWriteSites('./fake.ts', "function reap(id) { return invokeCmd('kill_session', {}); }").length,
    ).toBe(1);
  });

  it('an exemption covers its own line and no other call of the same writer', () => {
    // The mutation F2c found: `EXEMPT_SITES[file]?.some(...)` skipped the whole
    // site loop, so a second call site in an exempt file was never checked.
    const src = [
      '<script>',
      "  async function one() { await rewindConversation(id, 'rewind', anchor); }",
      '  async function two() { await rewindConversation(id, "fork", other); }',
      '</script>',
    ].join('\n');
    const sites = callSites(src, 'rewindConversation');
    expect(sites.length).toBe(2);
    const needle = "await rewindConversation(id, 'rewind', anchor)";
    const covered = sites.map((at) => callStatement(src, at).includes(needle));
    expect(covered).toEqual([true, false]);
  });
});

describe('a key-only writer is held to the answer gate, and only while it is key-only', () => {
  it('every KEY_ONLY_WRITERS entry still sends no prompt text', () => {
    for (const k of KEY_ONLY_WRITERS) {
      const src = RAW_SOURCES[k.file];
      expect(src, `${k.file} is gone`).toBeTruthy();
      expect(src.includes(`export async function ${k.writer}(`), `${k.file} lost ${k.writer}`).toBe(true);
      for (const pin of k.pins) expect(src.includes(pin), `${k.writer} no longer sends ${pin}`).toBe(true);
      expect(DERIVED.get(k.to)?.has(k.writer), `${k.writer} writes ${k.to}`).toBe(true);
      expect(DERIVED.get(k.from)?.has(k.writer) ?? false, `${k.writer} is not a ${k.from} writer`).toBe(false);
    }
  });
});

describe('the call-site exemptions are real and stay real', () => {
  const entries = Object.entries(EXEMPT_SITES).flatMap(([file, list]) =>
    list.map((e) => [file, e] as const),
  );

  it('every exempt site exists, writes what it claims, and still has no gate of its own', () => {
    for (const [file, e] of entries) {
      const src = SOURCES[file];
      expect(src, `${file} is exempt but is not a source file`).toBeTruthy();
      const w = WRITERS.get(e.writer);
      expect(w, `${file}: ${e.writer} is not a writer any more`).toBeTruthy();
      expect([...w!.actions], `${file}: ${e.writer} does not write ${e.action}`).toContain(e.action);
      const sites = callSites(src, e.writer);
      expect(sites.length, `${file} no longer calls ${e.writer}`).toBeGreaterThan(0);
      // Each named site must still EXIST and still be ungated. A needle that
      // matches nothing is a stale exemption; a needle whose site now gates
      // itself is dead weight, and leaving it standing is leaving a licence a
      // later edit leans on.
      const idx = indexOf(file);
      expect(e.sites.length, `${file}: an exemption must name its call sites`).toBeGreaterThan(0);
      for (const needle of e.sites) {
        const matched = sites.filter((at) => callStatement(src, at).includes(needle));
        expect(
          matched.length,
          `${file}: no ${e.writer} call site matches the exempt line\n  ${needle}\n` +
            'Either the line moved (update the needle) or the exemption is stale (delete it).',
        ).toBe(1);
        expect(
          covers(gatesInReachOf(idx, matched[0]), e.action),
          `${file} gates ${e.writer}/${e.action} at that line now — remove the site from EXEMPT_SITES`,
        ).toBe(false);
      }
    }
  });

  it('every exemption carries an argument and at least one pin that still holds', () => {
    for (const [file, e] of entries) {
      expect(e.why.length, file).toBeGreaterThan(80);
      expect(e.pins.length, file).toBeGreaterThan(0);
      for (const pin of e.pins) {
        // The source AS WRITTEN: a pin quotes a line, and whether that line is
        // there is not a question about comments.
        const src = RAW_SOURCES[pin.file];
        expect(src, `${file}: pinned file ${pin.file} is gone`).toBeTruthy();
        expect(
          src.includes(pin.needle),
          `${file}'s exemption rests on ${pin.file} containing:\n  ${pin.needle}\n` +
            'It does not any more, so the exemption is now false: either restore that ' +
            'composition or gate this write itself.',
        ).toBe(true);
      }
    }
  });
});

// ── the F2a sweep, kept ───────────────────────────────────────────────────
//
// The weaker question, which the inverted one does not subsume: a surface that
// asks the HUB's half for a `SESSION_TIER` action and never the access half.
// The control may open a dialog that writes somewhere else entirely, so there
// is no call site to anchor on — but a control gated on one half and not the
// other is still the thing `share.ts` exists to prevent.

/** Every hub-half question a file asks with a literal action name. */
function hubAsks(src: string): Set<string> {
  const out = new Set<string>();
  for (const m of src.matchAll(/\bhub(?:ActionBlocked|Block)\(\s*'([a-z0-9_]+)'/g)) out.add(m[1]);
  return out;
}

interface HalfExemption {
  actions: readonly SessionAction[];
  why: string;
  pins: readonly { file: string; needle: string }[];
}

const EXEMPT_HALF: Record<string, HalfExemption> = {
  './answer_send.ts': {
    actions: ['send_prompt'],
    why:
      'Orbit Fleet 11.7: the answer card and ⌘K Approve press a dialog key through `answerDialog`, ' +
      'which is `send_prompt` with no prompt. The HUB half has to name the routed command, ' +
      '`send_prompt`, because that is what `hub.ts` knows; the ACCESS half is `answer_dialog`, ' +
      'the narrower tier the hub admits for a key alone, so an `answer` grantee is not refused ' +
      'a write its grant allows. Any prompt text stays `send_prompt` at `drive`.',
    pins: [
      { file: './answer_send.ts', needle: "sessionActionBlocked(session, 'answer_dialog')" },
      { file: './sessions.ts', needle: "prompt: '', keys: key" },
    ],
  },
  './commands.ts': {
    actions: ['send_prompt'],
    why:
      'Orbit Fleet 11.7: the answer card and ⌘K Approve press a dialog key through `answerDialog`, ' +
      'which is `send_prompt` with no prompt. The HUB half has to name the routed command, ' +
      '`send_prompt`, because that is what `hub.ts` knows; the ACCESS half is `answer_dialog`, ' +
      'the narrower tier the hub admits for a key alone, so an `answer` grantee is not refused ' +
      'a write its grant allows. Any prompt text stays `send_prompt` at `drive`.',
    pins: [
      { file: './commands.ts', needle: "sessionActionBlocked(s, 'answer_dialog')" },
      { file: './sessions.ts', needle: "prompt: '', keys: key" },
    ],
  },
  './AnswerPrompt.svelte': {
    actions: ['send_prompt'],
    why:
      'Orbit Fleet 11.7: the answer card and ⌘K Approve press a dialog key through `answerDialog`, ' +
      'which is `send_prompt` with no prompt. The HUB half has to name the routed command, ' +
      '`send_prompt`, because that is what `hub.ts` knows; the ACCESS half is `answer_dialog`, ' +
      'the narrower tier the hub admits for a key alone, so an `answer` grantee is not refused ' +
      'a write its grant allows. Any prompt text stays `send_prompt` at `drive`.',
    pins: [
      { file: './AnswerPrompt.svelte', needle: "$sessionBlocked(session, 'answer_dialog')" },
      { file: './sessions.ts', needle: "prompt: '', keys: key" },
    ],
  },
  './moveEligibility.ts': {
    actions: ['move_session'],
    why:
      'A pure helper. It is handed a hub status and a connection and NO session at all, ' +
      'so it has no `owner_person_id` to ask about; it answers the hub half and each of its ' +
      'call sites composes the access half onto it. Since F2b the move lifecycle also gates ' +
      'at the funnel, in `moves.ts`.',
    pins: [
      { file: './SessionDetails.svelte', needle: "$sessionBlocked(session, 'move_session')" },
      { file: './TransferSheet.svelte', needle: "$sessionIdBlocked(id, 'move_session')" },
      { file: './moves.ts', needle: "'move_session'" },
    ],
  },
  './SidebarFilters.svelte': {
    actions: ['send_prompt', 'kill_session'],
    why:
      'The select-mode toolbar holds `selectedCount`, a NUMBER — no rows, so nothing to ' +
      'narrow. Its two buttons only open Sidebar’s own dialogs, and Sidebar narrows the ' +
      'selection per target with `bulkTargets` before anything is sent or killed; its kill ' +
      'dialog names the narrowed count and says how many rows were left out.',
    pins: [
      {
        file: './Sidebar.svelte',
        needle: "bulkTargets(selectedRows, 'kill_session', $sessionBlocked)",
      },
      {
        file: './Sidebar.svelte',
        needle: "bulkTargets(selectedRows, 'send_prompt', $sessionBlocked)",
      },
    ],
  },
  './ReplyActions.svelte': {
    actions: ['rewind_conversation', 'send_prompt'],
    why:
      'It is given an id, a host and a tmux name — never the row — so it cannot ask ' +
      '`$sessionBlocked` for itself; the access half arrives as the `accessBlocked` prop, ' +
      'which ConversationPanel computes from the row it holds. Retry is a rewind followed ' +
      'by a send, so it composes on top of the rewind gate (`own`, strictly stronger than ' +
      '`send_prompt`’s `drive`) rather than keeping a second copy of it in step.',
    pins: [
      { file: './ReplyActions.svelte', needle: 'accessBlocked?: string | null;' },
      {
        file: './ReplyActions.svelte',
        needle: "rewindBlocked ?? hubActionBlocked('send_prompt'",
      },
      {
        file: './ConversationPanel.svelte',
        needle: "$sessionBlocked(session, 'rewind_conversation')",
      },
      { file: './ConversationPanel.svelte', needle: 'accessBlocked={rewindShareBlocked}' },
    ],
  },
  './session_rename.ts': {
    actions: ['set_friendly_name', 'rename_session'],
    why:
      'It composes both halves, but through a local `accessBlocked(target, action)` that asks ' +
      'by SESSION ID: the editor also opens on a DOUBLE-CLICK, which consults no button, so ' +
      'the gate lives at the one place both routes funnel through and the action reaches ' +
      '`sessionIdActionBlocked` as a parameter rather than as a literal. F2e moved it off a ' +
      'hand-rolled `(host_alias, tmux_name)` lookup, which failed open on a miss and could ' +
      'resolve a NAMESAKE row. The pins hold both literals and the call they end up in.',
    pins: [
      { file: './session_rename.ts', needle: 'sessionIdActionBlocked(target.id, action)' },
      { file: './session_rename.ts', needle: "accessBlocked(target, 'set_friendly_name')" },
      { file: './session_rename.ts', needle: "accessBlocked(target, 'rename_session')" },
    ],
  },
};

describe('every surface that asks the hub half asks the access half too', () => {
  it('no surface gates a SESSION_TIER action on the hub alone', () => {
    const offenders: string[] = [];
    for (const [file, src] of Object.entries(SOURCES)) {
      if (PREDICATES.has(file)) continue;
      const asked = hubAsks(src);
      if (asked.size === 0) continue;
      const composed = gatesIn(src);
      const exempt = new Set<string>(EXEMPT_HALF[file]?.actions ?? []);
      for (const action of asked) {
        if (!ACTIONS.has(action)) continue; // a fleet-wide command: not ours
        if (covers(composed, action as SessionAction)) continue;
        if (exempt.has(action)) continue;
        offenders.push(`${file} → ${action} (${sessionTierOf(action as SessionAction)})`);
      }
    }
    expect(
      offenders.sort(),
      `A session action is gated on the hub half alone in:\n  ${offenders.sort().join('\n  ')}\n\n` +
        'Compose the access half the way SessionRowItem does:\n' +
        "  hubActionBlocked('x', $hubStatus, $hubConnection) ?? $sessionBlocked(row, 'x')\n" +
        'Where the surface holds a batch or a fan-out list, narrow per target with ' +
        '`bulkTargets`. If the surface truly must be exempt, add it to EXEMPT_HALF with the ' +
        'argument and at least one pin — do not widen the matchers.',
    ).toEqual([]);
  });

  it('every half-exemption still asks the hub half and still carries its pins', () => {
    for (const [file, e] of Object.entries(EXEMPT_HALF)) {
      const src = SOURCES[file];
      expect(src, `${file} is exempt but is not a source file`).toBeTruthy();
      const asked = hubAsks(src);
      for (const action of e.actions) {
        expect(ACTIONS.has(action), `${file}: ${action} is not a SESSION_TIER action`).toBe(true);
        expect(asked.has(action), `${file} no longer asks hubActionBlocked('${action}')`).toBe(true);
      }
      expect(e.why.length, file).toBeGreaterThan(80);
      expect(e.pins.length, file).toBeGreaterThan(0);
      for (const pin of e.pins) {
        const pinned = RAW_SOURCES[pin.file];
        expect(pinned, `${file}: pinned file ${pin.file} is gone`).toBeTruthy();
        expect(
          pinned.includes(pin.needle),
          `${file}'s exemption rests on ${pin.file} containing:\n  ${pin.needle}`,
        ).toBe(true);
      }
    }
  });
});

describe('the four tiers F2a decided', () => {
  // Written against the plan's table rather than against `SESSION_TIER`, so a
  // later edit that widens one of them has to argue with this file.
  // `docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`, F2a.
  it('tidy_apply is own, because it can safe-kill', () => {
    expect(sessionTierOf('tidy_apply')).toBe('own');
    expect(sessionTierOf('safe_kill_session')).toBe('own');
  });

  it('request_work_handover is drive, because it types into the pane', () => {
    expect(sessionTierOf('request_work_handover')).toBe('drive');
    expect(sessionTierOf('send_message')).toBe('drive');
  });

  it('set_primary_work is drive, like every other per-session work write', () => {
    expect(sessionTierOf('set_primary_work')).toBe('drive');
    expect(sessionTierOf('link_session_work')).toBe('drive');
  });

  it('decide_work_batch is drive, because its parts are', () => {
    expect(sessionTierOf('decide_work_batch')).toBe('drive');
    expect(sessionTierOf('confirm_session_work')).toBe('drive');
  });

  it('restore_host_sessions is own, because it is recreate_session in bulk', () => {
    expect(sessionTierOf('restore_host_sessions')).toBe('own');
    expect(sessionTierOf('recreate_session')).toBe('own');
  });
});

describe('the three tiers F2c decided', () => {
  // Same shape as the block above: written against the round's own table, so a
  // later edit that widens one of them has to argue with this file rather than
  // with a diff nobody reads.
  it('resume_work is own, because it takes over a conversation', () => {
    expect(sessionTierOf('resume_work')).toBe('own');
    expect(sessionTierOf('rewind_conversation')).toBe('own');
  });

  it('reconsider_work_link and ack_work_link are drive, like the rest of the family', () => {
    expect(sessionTierOf('reconsider_work_link')).toBe('drive');
    expect(sessionTierOf('ack_work_link')).toBe('drive');
    expect(sessionTierOf('link_session_work')).toBe('drive');
    expect(sessionTierOf('decide_work_batch')).toBe('drive');
  });

  it('new_session has deliberately NO row: a tier is about an EXISTING session', () => {
    // HostDetail's "Find lost conversations → Resume" is a `new_session` with
    // `resume_claude_session_id`, and it is gated on the SOURCE row instead —
    // see the comment on `candidateBlocked` there. Giving `new_session` a row
    // would demand a gate on every creation path in the app, none of which has
    // a session to ask about.
    expect(ACTIONS.has('new_session')).toBe(false);
    expect(SOURCES['./HostDetail.svelte']).toContain('candidateBlocked');
    expect(SOURCES['./HostDetail.svelte']).toContain(
      "$sessionIdBlocked(c.existing_session_id, 'recreate_session')",
    );
  });
});

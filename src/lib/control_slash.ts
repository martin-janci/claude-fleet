// Gap plan G3.9: Control's slash commands, run by the desktop itself.
//
// `/task`, `/done`, `/assign` and `/start` each map onto ONE backend action
// the Work view already uses (`create_work_task` + `edit_work_item`,
// `set_work_status`, `link_session_work`, `preview_start_work` +
// `start_work`), so typing them in Control does exactly what the matching
// button does, at once and without a model in between. The person typed the
// command and pressed Enter: that is the decision, so nothing here is a
// proposal. `/plan` is NOT run here: planning subtasks is the agent's work
// (`work_link` `propose_tree`, accepted on a card), so it goes to Control's
// agent as its project command does (`CONTROL_COMMANDS` in
// crates/fleet-core/src/service/operator.rs). The phone sends every one of
// them to the agent, which applies the same grammar from those bodies.
//
// Grammar (the menu's `usage`, `control_commands.ts`):
//   /task <title> [due:<day>] [@owner]
//   /done #KEY
//   /assign #KEY @session
//   /start #KEY [@host]
// `#KEY` is a task key (`TASK-219`, `PD-2592`); the `#` is optional.
import { get, writable } from 'svelte/store';
import type { Result } from './result';
import { createWorkTask, editWorkItem, linkSessionWork, setWorkStatus, type WorkItemStatus } from './work';
import { workTask, showTaskInWorkView } from './work_view';
import { previewStartWork, previewIsClean, startFromPreview, baseStartArgs } from './start_preview';
import { sessions, type SessionRow } from './sessions';
import { sessionActionBlocked } from './share';
import { hubActionBlocked } from './hub';

export type ControlCommand =
  | { cmd: 'task'; title: string; due?: string; owner?: string }
  | { cmd: 'done'; key: string }
  | { cmd: 'assign'; key: string; session: string }
  | { cmd: 'start'; key: string; host?: string };

/** What a typed line is to Control: one of its commands, a command of its
 *  own typed wrong (`usage` says how), or not one it runs (`null`: `/plan`,
 *  a built-in like `/clear`, a plain prompt), which goes to the agent. */
export type ParsedCommand = { ok: true; command: ControlCommand } | { ok: false; usage: string } | null;

export const USAGE: Record<ControlCommand['cmd'], string> = {
  task: '/task <title> [due:<day>] [@owner]',
  done: '/done #KEY',
  assign: '/assign #KEY @session',
  start: '/start #KEY [@host]',
};

const KEY = /^#?([A-Za-z][A-Za-z0-9_]*-\d+)$/;
const WEEKDAYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];

function ymd(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** `due:` as a `YYYY-MM-DD` in `now`'s local calendar: `today`,
 *  `tomorrow`, a weekday (the coming one; today's name is today), or a
 *  date. `null` when it does not parse. */
export function parseDue(raw: string, now: Date = new Date()): string | null {
  const v = raw.trim().toLowerCase();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const plus = (n: number) => ymd(new Date(today.getFullYear(), today.getMonth(), today.getDate() + n));
  if (v === 'today') return plus(0);
  if (v === 'tomorrow') return plus(1);
  const wd = WEEKDAYS.findIndex((w) => w === v || w.slice(0, 3) === v);
  if (wd >= 0) return plus((wd - today.getDay() + 7) % 7);
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v);
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return d.getMonth() === Number(m[2]) - 1 ? ymd(d) : null;
}

function keyOf(tok: string | undefined): string | null {
  const m = KEY.exec(tok ?? '');
  return m ? m[1].toUpperCase() : null;
}

/** Read a line typed in Control. Pure; `now` dates `due:`. */
export function parseControlCommand(text: string, now: Date = new Date()): ParsedCommand {
  const m = /^\/([a-z]+)(?:\s+([\s\S]*))?$/.exec(text.trim());
  if (!m) return null;
  const name = m[1];
  const toks = (m[2] ?? '').split(/\s+/).filter(Boolean);
  const bad = (cmd: ControlCommand['cmd']): ParsedCommand => ({ ok: false, usage: USAGE[cmd] });
  switch (name) {
    case 'task': {
      let due: string | undefined;
      let owner: string | undefined;
      const words: string[] = [];
      for (const t of toks) {
        if (/^due:/i.test(t)) {
          const d = parseDue(t.slice(4), now);
          if (d === null || due !== undefined) return bad('task');
          due = d;
        } else if (t.startsWith('@') && t.length > 1 && owner === undefined) {
          owner = t.slice(1);
        } else words.push(t);
      }
      const title = words.join(' ');
      if (!title) return bad('task');
      return { ok: true, command: { cmd: 'task', title, ...(due ? { due } : {}), ...(owner ? { owner } : {}) } };
    }
    case 'done': {
      const key = keyOf(toks[0]);
      return key && toks.length === 1 ? { ok: true, command: { cmd: 'done', key } } : bad('done');
    }
    case 'assign': {
      const key = keyOf(toks.find((t) => !t.startsWith('@')));
      const at = toks.filter((t) => t.startsWith('@') && t.length > 1);
      if (!key || at.length !== 1 || toks.length !== 2) return bad('assign');
      return { ok: true, command: { cmd: 'assign', key, session: at[0].slice(1) } };
    }
    case 'start': {
      const key = keyOf(toks.find((t) => !t.startsWith('@')));
      const at = toks.filter((t) => t.startsWith('@') && t.length > 1);
      if (!key || at.length > 1 || toks.length !== 1 + at.length) return bad('start');
      return { ok: true, command: { cmd: 'start', key, ...(at[0] ? { host: at[0].slice(1) } : {}) } };
    }
    default:
      return null;
  }
}

/** What a run left to show under the composer. */
export interface CommandReceipt {
  id: number;
  /** The line as typed. */
  typed: string;
  state: 'running' | 'done' | 'failed';
  /** One sentence: what happened, or why not. */
  line: string;
  /** Where the result lives: a task in Work, or a session. */
  open?: { kind: 'task'; taskId: string } | { kind: 'session'; id: number; label: string };
  /** Puts back what the command changed, when it can be put back. */
  undo?: () => Promise<Result<unknown>>;
}

export const commandReceipts = writable<CommandReceipt[]>([]);
export const MAX_COMMAND_RECEIPTS = 3;
let nextId = 1;

function put(r: CommandReceipt): void {
  commandReceipts.update((rs) => [...rs.filter((x) => x.id !== r.id), r].slice(-MAX_COMMAND_RECEIPTS));
}

export function dismissCommandReceipt(id: number): void {
  commandReceipts.update((rs) => rs.filter((x) => x.id !== id));
}

/** Run Undo on receipt `id`; the receipt then says it was undone. */
export async function undoCommand(id: number): Promise<void> {
  const r = get(commandReceipts).find((x) => x.id === id);
  if (!r?.undo) return;
  const undo = r.undo;
  put({ ...r, undo: undefined, line: `${r.line} Undoing…` });
  const out = await undo();
  put({ ...r, undo: undefined, line: out.ok ? 'Undone.' : `Undo failed: ${out.error.message}` });
}

function sessionLabel(s: SessionRow): string {
  return s.friendly_name || s.tmux_name;
}

/** The live sessions `name` means: by its shown name or its tmux name. */
export function sessionsNamed(name: string, rows: readonly SessionRow[]): SessionRow[] {
  const want = name.toLowerCase();
  return rows.filter(
    (s) => s.lost_at == null && (s.tmux_name.toLowerCase() === want || (s.friendly_name ?? '').toLowerCase() === want),
  );
}

async function resolveItem(key: string): Promise<{ ok: true; itemId: number; status: string; title: string } | { ok: false; line: string }> {
  const r = await workTask(`ref:${key}`);
  if (!r.ok) return { ok: false, line: r.error.code === 'E_NOTFOUND' ? `No task ${key}.` : r.error.message };
  const t = r.value.task;
  if (t.item_id == null) return { ok: false, line: `${key} is not a task Fleet can change here.` };
  return { ok: true, itemId: t.item_id, status: t.status_category ?? 'todo', title: t.title ?? key };
}

async function run(c: ControlCommand): Promise<Omit<CommandReceipt, 'id' | 'typed'>> {
  switch (c.cmd) {
    case 'task': {
      const r = await createWorkTask({ title: c.title });
      if (!r.ok) return { state: 'failed', line: r.error.message };
      const item = r.value;
      const name = item.key ?? `#${item.id}`;
      const open = { kind: 'task' as const, taskId: `item:${item.id}` };
      if (c.due === undefined && c.owner === undefined) {
        return { state: 'done', line: `Created ${name} · ${item.title}.`, open };
      }
      const e = await editWorkItem(item.id, {
        ...(c.due !== undefined ? { due_at: c.due } : {}),
        ...(c.owner !== undefined ? { assignees: [c.owner] } : {}),
      });
      if (!e.ok) return { state: 'failed', line: `Created ${name}, but its owner and due date were not set: ${e.error.message}`, open };
      const extra = [c.owner ? `owner ${c.owner}` : null, c.due ? `due ${c.due}` : null].filter(Boolean).join(', ');
      return { state: 'done', line: `Created ${name} · ${item.title} (${extra}).`, open };
    }
    case 'done': {
      const it = await resolveItem(c.key);
      if (!it.ok) return { state: 'failed', line: it.line };
      const open = { kind: 'task' as const, taskId: `item:${it.itemId}` };
      if (it.status === 'done') return { state: 'done', line: `${c.key} was already done.`, open };
      const r = await setWorkStatus(it.itemId, 'done');
      if (!r.ok) return { state: 'failed', line: r.error.message, open };
      const back = (it.status === 'in_progress' ? 'in_progress' : 'todo') as WorkItemStatus;
      return { state: 'done', line: `${c.key} · ${it.title} is done.`, open, undo: () => setWorkStatus(it.itemId, back) };
    }
    case 'assign': {
      const found = sessionsNamed(c.session, get(sessions));
      if (found.length === 0) return { state: 'failed', line: `No live session named ${c.session}.` };
      if (found.length > 1) return { state: 'failed', line: `${found.length} sessions are named ${c.session}: assign it from Work.` };
      const s = found[0];
      // The same gate as the session's own Link button: a session shared to
      // you below drive cannot be linked from here either.
      const blocked = hubActionBlocked('link_session_work') ?? sessionActionBlocked(s, 'link_session_work');
      if (blocked !== null) return { state: 'failed', line: blocked };
      const r = await linkSessionWork(s.id, { key: c.key });
      if (!r.ok) return { state: 'failed', line: r.error.message };
      return {
        state: 'done',
        line: `${c.key} is assigned to ${sessionLabel(s)}.`,
        open: { kind: 'session', id: s.id, label: sessionLabel(s) },
      };
    }
    case 'start': {
      const base = baseStartArgs({ item_id: null, key: c.key, project_id: null });
      if (c.host) base.host_alias = c.host;
      const p = await previewStartWork(base);
      if (!p.ok) return { state: 'failed', line: p.error.message };
      const preview = p.value;
      const open = { kind: 'task' as const, taskId: preview.item_id != null ? `item:${preview.item_id}` : `ref:${c.key}` };
      if (!previewIsClean(preview)) {
        const why = preview.missing ? `pick a ${preview.missing}` : (preview.conflicts[0]?.message ?? 'it needs a choice');
        return { state: 'failed', line: `${c.key} did not start: ${why}. Start it from Work.`, open };
      }
      const r = await startFromPreview(base, preview);
      if (!r.ok) return { state: 'failed', line: r.error.message, open };
      const s = r.value;
      return {
        state: 'done',
        line: `Started ${sessionLabel(s)} on ${s.host_alias} for ${c.key}.`,
        open: { kind: 'session', id: s.id, label: sessionLabel(s) },
      };
    }
  }
}

/**
 * Control's composer hands every slash line here first. `true`: the line
 * was Control's to run (or to refuse with its usage) and must not reach the
 * agent; `false`: send it on as typed.
 */
export function takeControlCommand(text: string, now: Date = new Date()): boolean {
  const parsed = parseControlCommand(text, now);
  if (parsed === null) return false;
  const id = nextId++;
  const typed = text.trim();
  if (!parsed.ok) {
    put({ id, typed, state: 'failed', line: `Usage: ${parsed.usage}` });
    return true;
  }
  put({ id, typed, state: 'running', line: 'Running…' });
  void run(parsed.command).then(
    (out) => put({ id, typed, ...out }),
    (e: unknown) => put({ id, typed, state: 'failed', line: e instanceof Error ? e.message : String(e) }),
  );
  return true;
}

/** Follow a receipt's link. */
export function openCommandResult(r: CommandReceipt, focus: (id: number, label: string) => void): void {
  if (!r.open) return;
  if (r.open.kind === 'task') showTaskInWorkView(r.open.taskId);
  else focus(r.open.id, r.open.label);
}

/** Test seam. */
export function resetCommandReceiptsForTests(): void {
  commandReceipts.set([]);
  nextId = 1;
}

// A person's prompt as the conversation shows it (owner report 2026-11-06):
// the words they typed, with what the app put in front of them drawn as
// chips instead of raw text. The app adds three things to what a person
// types before it reaches an agent:
//
// - the composer's context line (`agentContext` in agent_context.ts):
//   `[context] the person is looking at session "…" on host …`;
// - a mission thread's or the Mission room's prefix (`steerPrefix`):
//   `About mission #4 "Payments v2":`;
// - a dashboard's "Change with the agent" (`changePrompt` in dashboards.ts):
//   the dashboard as a fleet-ui block and the instructions to save it,
//   ending in `Change it: ` or `Change the widget "…": `.
//
// Each is recognised only at the head of the prompt and only in the exact
// shape the app writes, so a person who types something alike keeps their
// words. The copy button still copies the prompt as sent.
// fleet-mobile's `PromptParts.kt` is a port: change both together.

export type PromptChip =
  | { kind: 'session'; name: string; host: string; branch: string | null }
  | { kind: 'host'; host: string }
  | { kind: 'mission'; id: number; name: string }
  | { kind: 'dashboard'; title: string; widget: string | null; copy: boolean };

export interface PromptParts {
  chips: PromptChip[];
  /** What the person typed. */
  text: string;
}

const SESSION_CONTEXT =
  /^\[context\] the person is looking at session "(.*)" on host (\S+?)(?: \(branch (.+)\))?\. "this one" and "here" mean that session unless they say otherwise\.(?:\n\n|$)/;
const HOST_CONTEXT =
  /^\[context\] the person is looking at host (\S+) with no session selected\. "here" means that host\.(?:\n\n|$)/;
const MISSION = /^About mission #(\d+) "(.*)":(?:\n\n|$)/;
const DASHBOARD =
  /^(?:My pinned dashboard "(.*)", as it is now:|The built-in dashboard "(.*)", to make my own copy of \(keep the id [A-Za-z0-9._:-]+\):)\n\n```fleet-ui\n[\s\S]*?\n```\n\n(?:Save the change with `dashboards \{ pin \}` under the same id, then answer with the `dashboard` block\.|Answer with a `dashboard` block with the same id\.) (?:Change the widget "(.*)": |Change it: )/;

/** `prompt` with the app's own head lines read into chips. */
export function promptParts(prompt: string): PromptParts {
  const chips: PromptChip[] = [];
  let rest = prompt;
  for (;;) {
    let m = SESSION_CONTEXT.exec(rest);
    if (m) {
      chips.push({ kind: 'session', name: m[1], host: m[2], branch: m[3] ?? null });
      rest = rest.slice(m[0].length);
      continue;
    }
    m = HOST_CONTEXT.exec(rest);
    if (m) {
      chips.push({ kind: 'host', host: m[1] });
      rest = rest.slice(m[0].length);
      continue;
    }
    m = MISSION.exec(rest);
    if (m) {
      chips.push({ kind: 'mission', id: Number(m[1]), name: m[2] });
      rest = rest.slice(m[0].length);
      continue;
    }
    m = DASHBOARD.exec(rest);
    if (m) {
      const copy = m[2] !== undefined;
      chips.push({ kind: 'dashboard', title: (copy ? m[2] : m[1]) ?? '', widget: m[3] ?? null, copy });
      rest = rest.slice(m[0].length);
      continue;
    }
    break;
  }
  return { chips, text: chips.length ? rest.trim() : prompt };
}

/** A chip's words. */
export function chipLabel(c: PromptChip): string {
  switch (c.kind) {
    case 'session':
      return [c.name, c.host, ...(c.branch ? [c.branch] : [])].join(' · ');
    case 'host':
      return c.host;
    case 'mission':
      return `Mission ${c.name}`;
    case 'dashboard': {
      const what = c.copy ? `Dashboard “${c.title}” (your copy)` : `Dashboard “${c.title}”`;
      return c.widget ? `${what} › ${c.widget}` : what;
    }
  }
}

/** What a chip says it is, for a screen reader and the tooltip. */
export function chipTitle(c: PromptChip): string {
  switch (c.kind) {
    case 'session':
      return 'You were looking at this session';
    case 'host':
      return 'You were looking at this host';
    case 'mission':
      return 'About this mission';
    case 'dashboard':
      return c.widget ? 'A change to this widget of the dashboard' : 'A change to this dashboard';
  }
}

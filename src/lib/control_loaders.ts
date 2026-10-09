// Control chat loaders (Orbit Fleet redesign step 9.13): what the Atom says
// while Control's agent works, and when it becomes the Constellation. Both
// read the running turn's own tool calls, never a timer: "Planning · reading
// 3 sessions and 2 PRs" counts the fleet tools it has called so far; once it
// has sent work to a session (a prompt, a task, a new session) the plan is
// being carried out and the Constellation takes over.
import type { Conversation, ConvItem } from './conversation';
import type { LoaderName } from './loader-kit.generated';

/** What the indicator draws in place of the plain Atom line. */
export interface ThinkingAs {
  loader: LoaderName;
  label: string;
}

/** Tools that read one session (the `mcp__<server>__` prefix dropped). */
const SESSION_READS = new Set([
  'capture_session',
  'session_activity',
  'session_conversation',
  'session_conversations',
  'session_history',
  'session_transcript',
  'session_tool_detail',
  'session_presence',
  'related_sessions',
  'agent_status',
]);

/** Tools that read pull requests or a repository's changes. */
const PR_READS = new Set(['prs', 'repo_changes', 'repo_branch_diff', 'repo_diff', 'repo_range_diff']);

/** Tools that read tasks or the work graph. */
const TASK_READS = new Set(['list_tasks', 'work']);

/** Tools that hand work to a session: the plan going out. */
const SENDS = new Set([
  'send_prompt',
  'queue_prompt',
  'broadcast_prompt',
  'run_prompt',
  'dispatch_task',
  'new_session',
  'new_bg_session',
  'spawn_review',
]);

/** `mcp__claude-fleet__list_sessions` → `list_sessions`; a built-in tool keeps its name. */
export function fleetTool(name: string): string {
  if (!name.startsWith('mcp__')) return name;
  const parts = name.slice('mcp__'.length).split('__');
  return parts.length > 1 ? parts.slice(1).join('__') : name;
}

type Tool = Extract<ConvItem, { kind: 'tool' }>;

/** The last turn's tool calls after its last interrupt. */
function runningTools(conv: Conversation | null): Tool[] {
  if (!conv || conv.turns.length === 0) return [];
  const items = conv.turns[conv.turns.length - 1].items;
  const out: Tool[] = [];
  for (const it of items) {
    if (it.kind === 'interrupt') out.length = 0;
    else if (it.kind === 'tool') out.push(it);
  }
  return out;
}

/** Distinct reads: by target where the call names one, else one per call. */
function count(tools: Tool[], names: Set<string>): number {
  const seen = new Set<string>();
  let anonymous = 0;
  for (const t of tools) {
    if (!names.has(fleetTool(t.name))) continue;
    if (t.target) seen.add(t.target);
    else anonymous += 1;
  }
  return seen.size + anonymous;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function joinAnd(parts: string[]): string {
  if (parts.length <= 1) return parts.join('');
  return `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`;
}

/**
 * The Control chat's indicator while its agent works: the Atom with
 * "Planning · reading 3 sessions and 2 PRs", or "Planning" before it has
 * read anything; the Constellation with "Planning · sent work to 2
 * sessions" once it has handed work on in this turn.
 */
export function controlThinking(conv: Conversation | null): ThinkingAs {
  const tools = runningTools(conv);
  const sent = count(tools, SENDS);
  if (sent > 0) {
    return { loader: 'constellation', label: `Planning · sent work to ${plural(sent, 'session', 'sessions')}` };
  }
  const read = [
    [count(tools, SESSION_READS), 'session', 'sessions'],
    [count(tools, PR_READS), 'PR', 'PRs'],
    [count(tools, TASK_READS), 'task list', 'task lists'],
  ] as const;
  const parts = read.filter(([n]) => n > 0).map(([n, one, many]) => plural(n, one, many));
  return { loader: 'atom', label: parts.length > 0 ? `Planning · reading ${joinAnd(parts)}` : 'Planning' };
}

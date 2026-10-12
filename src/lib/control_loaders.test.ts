import { describe, expect, it } from 'vitest';
import { controlThinking, fleetTool } from './control_loaders';
import type { Conversation, ConvItem } from './conversation';

function tool(name: string, target: string | null = null, done = true): ConvItem {
  return { kind: 'tool', summary: `${name}()`, id: null, name, target, at: null, ended_at: null, done };
}

function conv(items: ConvItem[]): Conversation {
  return {
    truncated: false,
    context: null,
    events: [],
    turns: [{ prompt: 'plan the release', at: '2026-10-08T10:00:00.000Z', ended_at: null, items }],
  } as unknown as Conversation;
}

const F = 'mcp__claude-fleet__';

describe('controlThinking (step 9.13)', () => {
  it('says "Planning" with the Atom before anything is read', () => {
    expect(controlThinking(null)).toEqual({ loader: 'atom', label: 'Planning' });
    expect(controlThinking(conv([{ kind: 'text', text: 'on it' } as ConvItem]))).toEqual({ loader: 'atom', label: 'Planning' });
  });

  it('counts the sessions and PRs read so far: "reading 3 sessions and 2 PRs"', () => {
    const c = conv([
      tool(`${F}session_conversation`, 'pd-1'),
      tool(`${F}capture_session`, 'pd-2'),
      tool(`${F}session_conversation`, 'pd-1'),
      tool(`${F}session_activity`, 'pd-3', false),
      tool(`${F}prs`),
      tool(`${F}repo_changes`, 'claude-fleet'),
      tool('Read', 'notes.md'),
    ]);
    expect(controlThinking(c)).toEqual({ loader: 'atom', label: 'Planning · reading 3 sessions and 2 PRs' });
  });

  it('turns into the Constellation once work goes out to sessions', () => {
    const c = conv([tool(`${F}list_sessions`), tool(`${F}send_prompt`, 'pd-1'), tool(`${F}dispatch_task`, 'pd-2', false)]);
    expect(controlThinking(c)).toEqual({ loader: 'constellation', label: 'Planning · sent work to 2 sessions' });
  });

  it('forgets what came before an interrupt', () => {
    const c = conv([tool(`${F}send_prompt`, 'pd-1'), { kind: 'interrupt' } as ConvItem, tool(`${F}prs`)]);
    expect(controlThinking(c)).toEqual({ loader: 'atom', label: 'Planning · reading 1 PR' });
  });

  it('drops the MCP prefix and keeps built-in names', () => {
    expect(fleetTool('mcp__claude-fleet__list_sessions')).toBe('list_sessions');
    expect(fleetTool('Bash')).toBe('Bash');
  });
});

import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { FLEET_PROMPT_MARKS, fleetLine, isFleetPrompt, parseFleetPrompt } from './fleet_events';

// Thread room (2026-10-14): fleet's own prompts in Control's chat.

describe('fleet events', () => {
  it('uses the marks fleet-core uses (prompt_origin::FLEET_PROMPT_MARKS)', () => {
    const rs = readFileSync('crates/fleet-core/src/service/prompt_origin.rs', 'utf8');
    const m = /pub const FLEET_PROMPT_MARKS: &\[&str\] = &\[(.*?)\];/s.exec(rs);
    expect(m).not.toBeNull();
    const marks = [...m![1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    expect([...FLEET_PROMPT_MARKS]).toEqual(marks);
  });

  it('is fleet only at the head of the prompt', () => {
    expect(isFleetPrompt('[fleet events] What changed')).toBe(true);
    expect(isFleetPrompt('  [fleet] Your session was restarted')).toBe(true);
    expect(isFleetPrompt('look at this [fleet events] note')).toBe(false);
    expect(isFleetPrompt(null)).toBe(false);
    expect(parseFleetPrompt('fix the bug')).toBeNull();
  });

  it("reads the operator feed's digest into its events, counting the ones it did not list", () => {
    const digest = [
      '[fleet events] What changed since the last note. Nothing here asks for a reply: act only if one of these is yours to follow up.',
      '- session fix-flake on mercury (#12, you started it) finished a turn',
      '- host mercury is low on memory (6% available)',
      '- … and 3 more.',
    ].join('\n');
    const n = parseFleetPrompt(digest)!;
    expect(n.kind).toBe('digest');
    expect(n.header).toContain('What changed since the last note');
    expect(n.events.map((e) => e.text)).toEqual([
      'session fix-flake on mercury (#12, you started it) finished a turn',
      'host mercury is low on memory (6% available)',
    ]);
    expect(n.events.map((e) => e.sessionId)).toEqual([12, null]);
    expect(n.more).toBe(3);
    expect(fleetLine(n)).toBe('Fleet · 5 events');
  });

  it("a restart's note is one event", () => {
    const n = parseFleetPrompt('[fleet] Your session was restarted automatically after its process stopped on mercury.')!;
    expect(n.kind).toBe('note');
    expect(n.events).toHaveLength(1);
    expect(fleetLine(n)).toBe('Fleet · 1 event');
  });
});

// The Work list row's priority chip: urgent and high only, as on the task page.
import { render, screen, cleanup } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import WorkTaskRow from './WorkTaskRow.svelte';
import { task } from './work_view_fixture';

afterEach(cleanup);

describe('WorkTaskRow priority', () => {
  it('shows urgent and high, and nothing for medium, low or none', () => {
    for (const [priority, shown] of [
      ['urgent', 'Urgent'],
      ['high', 'High'],
      ['medium', null],
      ['low', null],
      [undefined, null],
    ] as const) {
      render(WorkTaskRow, { task: task({ priority, sessions: [] }), onselect: () => {} });
      const chip = screen.queryByTestId('work-task-priority');
      expect(chip?.textContent ?? null, String(priority)).toBe(shown);
      cleanup();
    }
  });
});

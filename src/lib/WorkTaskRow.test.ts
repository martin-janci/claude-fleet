// The Work list row's priority chip (urgent and high only, as on the task
// page) and its due chip (overdue in red, none once the task is done).
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

describe('WorkTaskRow due date', () => {
  const day = (offset: number) => {
    const d = new Date();
    d.setDate(d.getDate() + offset);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  };

  it('says when it is due, and flags a date that has passed', () => {
    render(WorkTaskRow, { task: task({ due_at: day(0), sessions: [] }), onselect: () => {} });
    let chip = screen.getByTestId('work-task-due');
    expect(chip.textContent).toBe('Due Today');
    expect(chip.classList.contains('failed')).toBe(false);
    cleanup();
    render(WorkTaskRow, { task: task({ due_at: day(-3), sessions: [] }), onselect: () => {} });
    chip = screen.getByTestId('work-task-due');
    expect(chip.textContent).toMatch(/^Overdue · /);
    expect(chip.classList.contains('failed')).toBe(true);
  });

  it('shows nothing without a date, or once the task is done', () => {
    render(WorkTaskRow, { task: task({ sessions: [] }), onselect: () => {} });
    expect(screen.queryByTestId('work-task-due')).toBeNull();
    cleanup();
    render(WorkTaskRow, { task: task({ due_at: day(-3), status_category: 'done', sessions: [] }), onselect: () => {} });
    expect(screen.queryByTestId('work-task-due')).toBeNull();
  });
});

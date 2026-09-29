import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import Harness from './ViewBoundaryHarness.test.svelte';
import { fail } from './view_boundary_harness.test-state';
import { resetErrorReportingForTests } from './error_report';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockClear();
  resetErrorReportingForTests();
  fail.throwing = false;
});

describe('ViewBoundary', () => {
  it('renders the view when nothing throws', () => {
    render(Harness);
    expect(screen.getByTestId('child').textContent).toBe('hosts content');
    expect(screen.queryByTestId('view-failed')).toBeNull();
  });

  it('shows the error instead of a dead panel, and reports it', async () => {
    fail.throwing = true;
    render(Harness);
    await tick();
    expect(screen.queryByTestId('child')).toBeNull();
    expect(screen.getByTestId('view-failed').textContent).toContain('Hosts could not be shown');
    expect(screen.getByTestId('view-failed-message').textContent).toBe('Error: boom in render');
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([cmd]) => cmd === 'report_client_error',
    );
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({
      args: { component: 'frontend:hosts', message: 'Error: boom in render' },
    });
  });

  it('Retry renders the view again once it no longer throws', async () => {
    fail.throwing = true;
    render(Harness);
    await tick();
    fail.throwing = false;
    await fireEvent.click(screen.getByTestId('view-failed-retry'));
    await tick();
    expect(screen.getByTestId('child').textContent).toBe('hosts content');
  });

  it('Copy details copies the message and stack', async () => {
    const writeText = vi.fn(async (_text: string) => {});
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    fail.throwing = true;
    render(Harness);
    await tick();
    await fireEvent.click(screen.getByTestId('view-failed-copy'));
    await tick();
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText.mock.calls[0][0]).toMatch(/^Hosts view failed: Error: boom in render\n/);
    expect(screen.getByTestId('view-failed-copy').textContent).toBe('Copied');
  });
});

// Step 10.7: the notification centre's list. Every toast this window showed,
// newest first; a toast's button only while that toast is still up; Mark
// all read, Clear all and one entry's ✕.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';
import NotificationList from './NotificationList.svelte';
import { clearNotices, notices, unreadNotices } from './notifications';
import { clearToasts, dismiss, push } from './toasts';

beforeEach(() => {
  clearToasts();
  clearNotices();
});

describe('NotificationList', () => {
  it('says so when nothing was shown, with both buttons off', () => {
    render(NotificationList);
    expect(screen.getByTestId('notices-empty')).toBeInTheDocument();
    expect(screen.getByTestId('notices-mark-read')).toBeDisabled();
    expect(screen.getByTestId('notices-clear')).toBeDisabled();
  });

  it('lists every toast newest first, with its kind, code and a repeat count', async () => {
    push({ message: 'Host mercury is back', kind: 'success' });
    push({ message: 'Copy failed', kind: 'error', code: 'E_SSH' });
    push({ message: 'Copy failed', kind: 'error', code: 'E_SSH' });
    render(NotificationList);
    const rows = screen.getAllByTestId('notice-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('Copy failed');
    expect(rows[0]).toHaveTextContent('×2');
    expect(rows[0]).toHaveTextContent('E_SSH');
    expect(rows[0].dataset.kind).toBe('error');
    expect(rows[1]).toHaveTextContent('Host mercury is back');
    expect(rows[1]).toHaveTextContent('just now');
    expect(rows[0].classList.contains('unread')).toBe(true);
  });

  it('offers a toast’s button only while the toast is up, and runs it from here', async () => {
    const run = vi.fn();
    const id = push({ message: 'Session killed', action: { label: 'Undo', run } });
    render(NotificationList);
    const row = screen.getByTestId('notice-row');
    await fireEvent.click(within(row).getByTestId('notice-action'));
    expect(run).toHaveBeenCalledOnce();
    // Running it dismissed the toast: the entry is something to read now.
    expect(within(screen.getByTestId('notice-row')).queryByTestId('notice-action')).toBeNull();
    // A toast that faded on its own leaves no button either.
    push({ message: 'Saved', action: { label: 'Open', run: vi.fn() } });
    await tick();
    const saved = () => screen.getAllByTestId('notice-row')[0];
    expect(within(saved()).getByTestId('notice-action')).toHaveTextContent('Open');
    dismiss(get(notices)[0].toastId);
    await tick();
    expect(within(saved()).queryByTestId('notice-action')).toBeNull();
    expect(id).toBeGreaterThan(0);
  });

  it('marks all read, removes one, and clears all', async () => {
    push({ message: 'One' });
    push({ message: 'Two' });
    render(NotificationList);
    expect(get(unreadNotices)).toBe(2);
    await fireEvent.click(screen.getByTestId('notices-mark-read'));
    expect(get(unreadNotices)).toBe(0);
    expect(screen.getAllByTestId('notice-row').every((r) => !r.classList.contains('unread'))).toBe(true);
    expect(screen.getByTestId('notices-mark-read')).toBeDisabled();
    await fireEvent.click(within(screen.getAllByTestId('notice-row')[0]).getByRole('button', { name: 'Remove' }));
    expect(screen.getAllByTestId('notice-row')).toHaveLength(1);
    expect(screen.getByTestId('notice-row')).toHaveTextContent('One');
    await fireEvent.click(screen.getByTestId('notices-clear'));
    expect(screen.getByTestId('notices-empty')).toBeInTheDocument();
  });
});

// Toasts board (G4.8): the centre keeps a toast's second line, offers both
// of its buttons while it is up, and ⚙ goes to Settings › Notifications.
describe('NotificationList, G4.8', () => {
  it('keeps the second line and offers both buttons while the toast is up', async () => {
    const add = vi.fn();
    const later = vi.fn();
    push({
      message: 'You picked acme/pos for PD-* 5 times',
      sub: 'Add rule PD-* → acme/pos?',
      action: { label: 'Add rule', run: add },
      secondary: { label: 'Not now', run: later },
    });
    render(NotificationList);
    expect(screen.getByTestId('notice-sub')).toHaveTextContent('Add rule PD-* → acme/pos?');
    await fireEvent.click(screen.getByTestId('notice-secondary'));
    expect(later).toHaveBeenCalledOnce();
    await tick();
    // The toast is gone, so its buttons are too; the line stays to read.
    expect(screen.queryByTestId('notice-action')).toBeNull();
    expect(screen.getByTestId('notice-sub')).toBeInTheDocument();
  });

  it('⚙ leaves the sheet and opens Settings at Notifications', async () => {
    const { settingsOpen, settingsSection } = await import('./app_views');
    settingsOpen.set(false);
    const onleave = vi.fn();
    render(NotificationList, { props: { onleave } });
    await fireEvent.click(screen.getByRole('button', { name: 'Notification settings' }));
    expect(onleave).toHaveBeenCalledOnce();
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toBe('notifications');
  });
});

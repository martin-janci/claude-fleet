// A task's Attachments: the list, image thumbnails from the bytes, the
// client-side size refusal, delete behind a confirm, and paste.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskAttachments from './TaskAttachments.svelte';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { attachRefusal, formatSize, isImage, pastedName, revokeAttachmentUrls } from './task_attachments';
import type { TaskAttachment } from './work_view';

const row = (over: Partial<TaskAttachment>): TaskAttachment => ({
  id: 1,
  item_id: 77,
  name: 'notes.txt',
  mime: 'text/plain',
  size: 1200,
  sha256: 'ab',
  author: 'client:phone',
  created_at: 1_790_000_000,
  ...over,
});

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
const calls = (cmd: string) =>
  vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
  await new Promise((r) => setTimeout(r, 0));
  for (let i = 0; i < 5; i++) await tick();
}

function pasteEvent(files: File[]): ClipboardEvent {
  const e = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent;
  Object.defineProperty(e, 'clipboardData', {
    value: { items: files.map((f) => ({ kind: 'file', type: f.type, getAsFile: () => f })) },
  });
  return e;
}

describe('TaskAttachments', () => {
  beforeEach(() => {
    revokeAttachmentUrls();
    fleetSettings.set({ ...SETTING_DEFAULTS });
    vi.mocked(invoke).mockReset();
    handlers = {
      work_attachment: (a) => ({ attachment: row({ id: a.attachment_id as number, mime: 'image/png' }), data_base64: 'iVBORw0KGgo=' }),
      attach_to_work: (a) => row({ id: 50, name: a.name as string, mime: a.mime as string, author: 'desktop' }),
      delete_work_attachment: (a) => row({ id: a.attachment_id as number }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
    URL.createObjectURL = vi.fn(() => 'blob:fake');
    URL.revokeObjectURL = vi.fn();
  });
  afterEach(() => revokeAttachmentUrls());

  it('lists files with size and author, and draws an image from its bytes', async () => {
    render(TaskAttachments, {
      itemId: 77,
      attachments: [row({ id: 2, name: 'shot.png', mime: 'image/png' }), row({ id: 1 })],
    });
    await flush();
    expect(screen.getByTestId('task-attachments-count').textContent).toBe('2');
    const cards = screen.getAllByTestId('task-attachment');
    expect(cards[1].textContent).toContain('notes.txt');
    expect(cards[1].textContent).toContain('1 KB');
    expect(cards[1].textContent).toContain('phone');
    expect(calls('work_attachment')).toEqual([{ attachment_id: 2 }]);
    const img = cards[0].querySelector('img') as HTMLImageElement;
    expect(img.getAttribute('alt')).toBe('shot.png');
    expect(img.getAttribute('src')).toBe('blob:fake');
    // The lightbox opens on click and closes on Escape.
    await fireEvent.click(screen.getByTestId('task-attachment-open'));
    expect(screen.getByTestId('task-attachment-lightbox')).toBeTruthy();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByTestId('task-attachment-lightbox')).toBeNull();
  });

  it('refuses a file over the limit without calling the hub', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.attachment_max_mb': '1' });
    render(TaskAttachments, { itemId: 77, attachments: [] });
    const input = screen.getByTestId('task-attachments-input') as HTMLInputElement;
    const big = new File([new Uint8Array(1024 * 1024 + 1)], 'big.bin');
    Object.defineProperty(input, 'files', { value: [big], configurable: true });
    await fireEvent.change(input);
    await flush();
    expect(calls('attach_to_work')).toHaveLength(0);
    expect(screen.getByTestId('task-attachments-error').textContent).toContain('at most 1.0 MB');
  });

  it('uploads a picked file as base64 and shows it at once', async () => {
    render(TaskAttachments, { itemId: 77, attachments: [] });
    const input = screen.getByTestId('task-attachments-input') as HTMLInputElement;
    Object.defineProperty(input, 'files', { value: [new File(['hi'], 'a.md')], configurable: true });
    await fireEvent.change(input);
    await flush();
    expect(calls('attach_to_work')).toEqual([{ item_id: 77, name: 'a.md', mime: 'text/markdown', data_base64: 'aGk=', comment_id: null }]);
    expect(screen.getByTestId('task-attachments-count').textContent).toBe('1');
    // The reader's own: it offers Delete.
    expect(screen.getByTestId('task-attachment-delete')).toBeTruthy();
  });

  it('deletes only the reader’s own, behind a confirm', async () => {
    render(TaskAttachments, { itemId: 77, attachments: [row({ id: 3, mine: true }), row({ id: 4 })] });
    await flush();
    expect(screen.getAllByTestId('task-attachment-delete')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('task-attachment-delete'));
    expect(calls('delete_work_attachment')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('task-attachment-delete-confirm'));
    await flush();
    expect(calls('delete_work_attachment')).toEqual([{ attachment_id: 3 }]);
    expect(screen.getAllByTestId('task-attachment')).toHaveLength(1);
  });

  it('a pasted image becomes an attachment named pasted-…', async () => {
    render(TaskAttachments, { itemId: 77, attachments: [] });
    const img = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], 'image.png', { type: 'image/png' });
    screen.getByTestId('task-attachments').dispatchEvent(pasteEvent([img]));
    await flush();
    const sent = calls('attach_to_work');
    expect(sent).toHaveLength(1);
    expect(String(sent[0].name)).toMatch(/^pasted-\d{8}-\d{6}\.png$/);
    expect(sent[0].mime).toBe('image/png');
  });

  it('shows an error inline when the hub refuses', async () => {
    handlers.attach_to_work = () => {
      throw { code: 'E_INVALID', message: 'an SVG cannot be attached' };
    };
    render(TaskAttachments, { itemId: 77, attachments: [] });
    const input = screen.getByTestId('task-attachments-input') as HTMLInputElement;
    Object.defineProperty(input, 'files', { value: [new File(['x'], 'a.txt', { type: 'text/plain' })], configurable: true });
    await fireEvent.change(input);
    await flush();
    expect(screen.getByRole('alert').textContent).toContain('SVG');
  });
});

describe('task_attachments helpers', () => {
  it('names images, sizes and refusals', () => {
    expect(isImage('image/png')).toBe(true);
    expect(isImage('image/svg+xml')).toBe(false);
    expect(isImage('application/pdf')).toBe(false);
    expect(formatSize(12)).toBe('12 B');
    expect(formatSize(2048)).toBe('2 KB');
    expect(formatSize(15 * 1024 * 1024)).toBe('15 MB');
    expect(attachRefusal({ name: 'a.svg', size: 10, type: 'image/svg+xml' }, 100)).toContain('SVG');
    expect(attachRefusal({ name: 'e', size: 0, type: '' }, 100)).toContain('empty');
    expect(attachRefusal({ name: 'ok', size: 100, type: '' }, 100)).toBeNull();
    expect(pastedName('image/jpeg', new Date(2026, 9, 11, 14, 30, 5))).toBe('pasted-20261011-143005.jpg');
  });
});

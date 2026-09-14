import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const writeText = vi.fn();
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: vi.fn(),
  writeText: (...a: unknown[]) => writeText(...a),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { copyDiagnostics, openLogFolder } from './diagnostics';
import { toasts, clearToasts } from './toasts';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const bundle = {
  text: 'claude-fleet diagnostics\nversion: 0.2.10\n',
  log_dir: '/data/logs',
  log_file: '/data/logs/claude-fleet.2026-09-11.log',
};

describe('diagnostics', () => {
  beforeEach(() => {
    inv.mockReset();
    writeText.mockReset();
    clearToasts();
  });

  it('copyDiagnostics collects, copies the text and shows a success toast', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'collect_diagnostics') return bundle;
      throw new Error(`unexpected ${cmd}`);
    });
    writeText.mockResolvedValue(undefined);

    const r = await copyDiagnostics();

    expect(inv).toHaveBeenCalledWith('collect_diagnostics', undefined);
    expect(writeText).toHaveBeenCalledWith(bundle.text);
    expect(r).toEqual(bundle);
    const t = get(toasts);
    expect(t).toHaveLength(1);
    expect(t[0].kind).toBe('success');
    expect(t[0].message).toMatch(/copied/i);
  });

  it('copyDiagnostics surfaces a backend failure and copies nothing', async () => {
    inv.mockRejectedValue({ code: 'E_LOCK', message: 'store mutex poisoned' });

    const r = await copyDiagnostics();

    expect(r).toBeNull();
    expect(writeText).not.toHaveBeenCalled();
    const t = get(toasts);
    expect(t).toHaveLength(1);
    expect(t[0].kind).toBe('error');
    expect(t[0].code).toBe('E_LOCK');
  });

  it('copyDiagnostics surfaces a clipboard failure', async () => {
    inv.mockResolvedValue(bundle);
    writeText.mockRejectedValue(new Error('denied'));

    const r = await copyDiagnostics();

    expect(r).toBeNull();
    const t = get(toasts);
    expect(t[0].kind).toBe('error');
    expect(t[0].code).toBe('E_CLIPBOARD');
  });

  it('openLogFolder returns the folder path', async () => {
    inv.mockResolvedValue('/data/logs');
    const r = await openLogFolder();
    expect(inv).toHaveBeenCalledWith('open_log_folder', undefined);
    expect(r).toEqual({ ok: true, value: '/data/logs' });
  });
});

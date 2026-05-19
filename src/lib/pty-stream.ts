// Owns the single global listener for `pty-data` events from Rust.
//
// Registering listen() once at app boot (rather than per-click) eliminates
// any race window where Rust might emit before the listener is wired. Any
// component that wants to consume PTY bytes calls `setPtyWriteSink(cb)` to
// install a write callback, and `setPtyWriteSink(null)` to detach.
//
// A diagnostic ring buffer keeps the last 30 events received so we can
// surface them in the UI when the terminal looks dead.

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';

let writeSink: ((chunk: string) => void) | null = null;
let unlisten: UnlistenFn | null = null;
let initialized = false;

export interface PtyDebugEntry {
  ts: number;
  bytes: number;
  preview: string;
}

export const ptyDebug = writable<PtyDebugEntry[]>([]);
export const ptyEventCount = writable(0);

const MAX_DEBUG = 30;

export function setPtyWriteSink(cb: ((chunk: string) => void) | null): void {
  writeSink = cb;
}

export async function initPtyStream(): Promise<void> {
  if (initialized) return;
  initialized = true;
  try {
    unlisten = await listen<string>('pty-data', (event) => {
      const chunk = event.payload;
      ptyEventCount.update((n) => n + 1);
      ptyDebug.update((buf) => {
        const preview = chunk
          .replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '')
          .replace(/\r/g, '\\r')
          .replace(/\n/g, '\\n')
          .slice(0, 60);
        const next = buf.concat({ ts: Date.now(), bytes: chunk.length, preview });
        return next.length > MAX_DEBUG ? next.slice(-MAX_DEBUG) : next;
      });
      writeSink?.(chunk);
    });
  } catch (e) {
    console.error('initPtyStream failed:', e);
    initialized = false;
  }
}

export function shutdownPtyStream(): void {
  unlisten?.();
  unlisten = null;
  initialized = false;
}

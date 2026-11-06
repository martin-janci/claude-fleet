// The hub's own disk and backups, on the desktop (hardening survey
// 2026-10-26).
//
// `fleet_health.storage` (the hub's `fleet_health`; a desktop on its own
// reports none) carries free space under the hub's data dir and its newest
// backup, each flagged past its warning line by the hub. Each flag raises ONE
// Attention item, linking to Settings → Hub & sync: "Hub disk almost full",
// "Hub backup stale". Absent from an older hub and for a scoped caller.
import { writable } from 'svelte/store';

/** `service::hub_storage::StorageHealth`. Null-stripped on the hub's wire. */
export interface StorageHealth {
  free_bytes?: number | null;
  total_bytes?: number | null;
  low_disk?: boolean;
  last_backup_at?: number | null;
  backup_stale?: boolean;
}

/** The latest one the desktop has read (null: none). */
export const storageHealth = writable<StorageHealth | null>(null);

/** The Settings section the items open (`settings_tree.ts`). */
export const HUB_SECTION = 'hub';

export interface StorageAttentionItem {
  key: 'disk' | 'backup';
  label: string;
  /** Tooltip: the numbers. */
  detail: string;
  section: typeof HUB_SECTION;
}

function gb(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function ago(secs: number): string {
  if (secs < 3600) return `${Math.max(1, Math.round(secs / 60))} min`;
  if (secs < 86400) return `${Math.round(secs / 3600)} h`;
  return `${Math.round(secs / 86400)} days`;
}

/** One item per flag the hub raised, in a fixed order. */
export function storageAttentionItems(
  h: StorageHealth | null | undefined,
  nowSec: number = Math.floor(Date.now() / 1000),
): StorageAttentionItem[] {
  const out: StorageAttentionItem[] = [];
  if (h?.low_disk) {
    const free = typeof h.free_bytes === 'number' ? gb(h.free_bytes) : 'little';
    const of = typeof h.total_bytes === 'number' ? ` of ${gb(h.total_bytes)}` : '';
    out.push({
      key: 'disk',
      label: 'Hub disk almost full',
      detail: `${free} free${of} under the hub's data folder · its database stops saving when the disk fills · Open Settings → Hub & sync`,
      section: HUB_SECTION,
    });
  }
  if (h?.backup_stale) {
    const when =
      typeof h.last_backup_at === 'number'
        ? `the newest backup is ${ago(nowSec - h.last_backup_at)} old`
        : 'the hub has no backup';
    out.push({
      key: 'backup',
      label: 'Hub backup stale',
      detail: `${when} · the hub takes one daily; check its log and its backups folder · Open Settings → Hub & sync`,
      section: HUB_SECTION,
    });
  }
  return out;
}

// Task attachments: files and images on a task, kept in fleet (migration
// 166) and never written to a tracker. The bytes travel as base64 through
// `attach_to_work` / `work_attachment`, routed to the hub like comments.
import { get } from 'svelte/store';
import { fleetSettings, SETTING_KEYS } from './fleet_settings';
import { invokeCmd, type Result } from './result';
import { bumpWorkChanged } from './work';
import type { TaskAttachment } from './work_view';

/** The images the page draws inline; SVG is never one (the hub refuses it). */
const IMAGE_MIMES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);

export function isImage(mime: string | null | undefined): boolean {
  return IMAGE_MIMES.has((mime ?? '').toLowerCase());
}

/** `1.4 MB`, `820 KB`, `12 B` (binary units, as the limit is). */
export function formatSize(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  const mb = n / (1024 * 1024);
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

/** The largest attachment, in bytes: `work.attachment_max_mb` (the hub
 *  checks it again). */
export function attachmentMaxBytes(): number {
  const raw = get(fleetSettings)[SETTING_KEYS.workAttachmentMaxMb];
  const mb = Number.parseInt(raw ?? '', 10);
  return (Number.isFinite(mb) && mb > 0 ? mb : 10) * 1024 * 1024;
}

/** Why `file` cannot be attached, or null. */
export function attachRefusal(file: Pick<File, 'name' | 'size' | 'type'>, max = attachmentMaxBytes()): string | null {
  if (file.size === 0) return `${file.name} is empty.`;
  if (file.size > max) return `${file.name} is ${formatSize(file.size)}; an attachment is at most ${formatSize(max)}.`;
  if ((file.type ?? '').toLowerCase().includes('svg')) return `${file.name} is an SVG, which can carry script: attach a PNG of it.`;
  return null;
}

const EXT_MIMES: Record<string, string> = {
  md: 'text/markdown',
  markdown: 'text/markdown',
  txt: 'text/plain',
  log: 'text/plain',
  csv: 'text/csv',
  json: 'application/json',
};

/** The type to send: the browser's, else one the extension names. */
export function mimeOf(file: Pick<File, 'name' | 'type'>): string {
  if (file.type) return file.type;
  const ext = file.name.split('.').pop()?.toLowerCase() ?? '';
  return EXT_MIMES[ext] ?? 'application/octet-stream';
}

function bytesToBase64(bytes: Uint8Array): string {
  let bin = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(bin);
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function readBytes(file: Blob): Promise<Uint8Array> {
  if (typeof file.arrayBuffer === 'function') return new Uint8Array(await file.arrayBuffer());
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
    r.onerror = () => reject(r.error);
    r.readAsArrayBuffer(file);
  });
}

/** Attach `file` to work item `itemId`. Over the limit (or an SVG) is
 *  refused here, without a call. */
export async function attachToWork(itemId: number, file: File, commentId?: number | null): Promise<Result<TaskAttachment>> {
  const refusal = attachRefusal(file);
  if (refusal) return { ok: false, error: { code: 'E_INVALID', message: refusal } };
  const data = bytesToBase64(await readBytes(file));
  const r = await invokeCmd<TaskAttachment>('attach_to_work', {
    args: { item_id: itemId, name: file.name, mime: mimeOf(file), data_base64: data, comment_id: commentId ?? null },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}

export async function deleteWorkAttachment(attachmentId: number): Promise<Result<TaskAttachment>> {
  const r = await invokeCmd<TaskAttachment>('delete_work_attachment', { args: { attachment_id: attachmentId } });
  if (r.ok) {
    revokeAttachmentUrl(attachmentId);
    bumpWorkChanged();
  }
  return r;
}

export interface AttachmentData {
  attachment: TaskAttachment;
  data_base64: string;
}

export function workAttachment(attachmentId: number): Promise<Result<AttachmentData>> {
  return invokeCmd<AttachmentData>('work_attachment', { args: { attachment_id: attachmentId } });
}

// One object URL per attachment, fetched once; an attachment's bytes never
// change (a new file is a new attachment).
const urls = new Map<number, Promise<Result<string>>>();

/** An object URL for the attachment's bytes, cached per id. */
export function workAttachmentBlobUrl(attachmentId: number): Promise<Result<string>> {
  const hit = urls.get(attachmentId);
  if (hit) return hit;
  const p = workAttachment(attachmentId).then((r): Result<string> => {
    if (!r.ok) {
      urls.delete(attachmentId);
      return r;
    }
    const blob = new Blob([base64ToBytes(r.value.data_base64) as BlobPart], { type: r.value.attachment.mime });
    return { ok: true, value: URL.createObjectURL(blob) };
  });
  urls.set(attachmentId, p);
  return p;
}

/** Free one attachment's object URL. */
export function revokeAttachmentUrl(attachmentId: number): void {
  const p = urls.get(attachmentId);
  if (!p) return;
  urls.delete(attachmentId);
  void p.then((r) => {
    if (r.ok) URL.revokeObjectURL(r.value);
  });
}

/** Free every object URL (the page unmounting). */
export function revokeAttachmentUrls(): void {
  for (const id of [...urls.keys()]) revokeAttachmentUrl(id);
}

/** `pasted-20261011-143005.png` for an image pasted at `now`. */
export function pastedName(mime: string, now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp = `${now.getFullYear()}${p(now.getMonth() + 1)}${p(now.getDate())}-${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`;
  const ext = mime === 'image/jpeg' ? 'jpg' : (mime.split('/')[1] ?? 'png');
  return `pasted-${stamp}.${ext}`;
}

/** The image files a paste carries, each named `pasted-…`. */
export function pastedImages(e: ClipboardEvent, now = new Date()): File[] {
  const out: File[] = [];
  for (const item of Array.from(e.clipboardData?.items ?? [])) {
    if (item.kind !== 'file' || !item.type.startsWith('image/')) continue;
    const f = item.getAsFile();
    if (!f) continue;
    out.push(new File([f], pastedName(item.type, now), { type: item.type }));
  }
  return out;
}

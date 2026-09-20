/**
 * The composer's attachment list. The limits, the naming and the error
 * wording live here as pure functions so they are tested without a DOM; the
 * component only draws what this decides.
 */

export type AttachKind = 'image' | 'text' | 'binary';

/** What `pick_attachments` returns, per file. */
export interface PickedFile {
  path: string;
  name: string;
  size: number;
  kind: AttachKind;
}

export interface Attachment {
  id: string;
  path: string;
  name: string;
  size: number;
  kind: AttachKind;
  /** Data URL from `attachment_preview`; null until it arrives or never. */
  thumb: string | null;
  state: 'ready' | 'reading' | 'error';
  error: string | null;
  /**
   * True for a file that arrived from the clipboard rather than the OS
   * picker. A pasted file has no filesystem path, so nothing authorised it
   * for the Rust allow-list — it can never be read or uploaded, and is
   * always added as an `error` tile rather than a `reading` one.
   */
  pasted: boolean;
}

export const MAX_FILES = 10;
export const MAX_BYTES = 10 * 1024 * 1024;
export const MAX_TOTAL = 25 * 1024 * 1024;

const MAX_BYTES_MB = MAX_BYTES / (1024 * 1024);
const MAX_TOTAL_MB = MAX_TOTAL / (1024 * 1024);

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** A screenshot pastes as "image.png"; ten of them would be indistinguishable. */
export function pastedName(now: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `pasted-${p(now.getHours())}.${p(now.getMinutes())}.${p(now.getSeconds())}.png`;
}

let seq = 0;

/**
 * Add picked files to the list. Rejections are returned rather than thrown:
 * every one of them is a sentence the composer shows, and the tiles that did
 * fit still get added.
 *
 * A picked file with an empty `path` is a paste (clipboard bytes, no
 * filesystem path — the Rust picker never returns one empty). It is always
 * added, honestly, as an `error` tile: pasting is not wired up to the
 * allow-list yet, and it would otherwise silently duplicate against every
 * other paste, or fail much later as a confusing permission error.
 */
export function addFiles(
  current: Attachment[],
  picked: PickedFile[],
): { next: Attachment[]; rejected: string[] } {
  const next = [...current];
  const rejected: string[] = [];
  let total = current.reduce((n, a) => n + a.size, 0);

  for (const f of picked) {
    if (f.path === '') {
      next.push({
        id: `att-${++seq}`,
        path: '',
        name: f.name,
        size: f.size,
        kind: f.kind,
        thumb: null,
        state: 'error',
        error: "Pasted files aren't supported yet — drop the file instead, or attach it from disk.",
        pasted: true,
      });
      continue;
    }
    if (next.some((a) => a.path === f.path)) continue;
    if (next.length >= MAX_FILES) {
      rejected.push(`${f.name} was not added — the limit is ${MAX_FILES} files.`);
      continue;
    }
    if (f.size > MAX_BYTES) {
      rejected.push(`${f.name} is ${fmtBytes(f.size)} — the limit is ${MAX_BYTES_MB} MB.`);
      continue;
    }
    if (total + f.size > MAX_TOTAL) {
      rejected.push(
        `${f.name} would make ${fmtBytes(total + f.size)} in total — the limit is ${MAX_TOTAL_MB} MB in total.`,
      );
      continue;
    }
    total += f.size;
    next.push({
      id: `att-${++seq}`,
      path: f.path,
      name: f.name,
      size: f.size,
      kind: f.kind,
      // The tile is reserved at full size before the preview decodes, so
      // decoding causes no reflow.
      thumb: null,
      state: 'reading',
      error: null,
      pasted: false,
    });
  }
  return { next, rejected };
}

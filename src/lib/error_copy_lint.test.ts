import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// Review w16-1: r13's rule (one sentence, never transport noise) held only in
// `pushError`. Every inline error that showed `r.error.message` put the
// backend's log text in front of a person ("ssh: connect to host mercury
// port 22: Connection timed out"). A display site shows `errorText(r.error)`
// (or `errorSentence`) instead; the backend's own words go under Details.
//
// A line may still read `.error.message` when it matches on the text, files
// it under Details, reports it, or builds another error from it.
const ALLOWED_LINE =
  /\.error\.message\.(includes|startsWith|endsWith|match)\(|\.error\.message\s*===|reportError|console\.|detail:|details:|new Error\(|\.\.\.\w+\.error, message|\$\{\w+\.error\.code\}(: | · )\$\{\w+\.error\.message\}/;

// Whole lines that show the raw text on purpose, each with why. An entry that
// no longer matches fails below, so the list only shrinks.
const WAITING: { file: string; text: string; why: string }[] = [
  {
    file: 'src/lib/fleets.ts',
    text: 'rows.error = failed ? failed.error.message : null;',
    why: "an extra fleet's 401 is not this app's hub pairing; errorText would send the person to Settings → Hub & sync",
  },
  {
    file: 'src/lib/TransferSheet.svelte',
    text: '<p class="note" data-testid="transfer-lost-contact-detail">{run.error.message}</p>',
    why: 'the one code covers a refused connection and a 404; this line shows what was actually said',
  },
  {
    file: 'src/lib/TransferSheet.svelte',
    text: '{run.error.message}{#if typeof run.error.details',
    why: 'the "Raw details" disclosure',
  },
];

function sources(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of readdirSync('src', { recursive: true }).map((p) => String(p).replace(/\\/g, '/'))) {
    if (f.includes('.test.') || f.endsWith('.d.ts')) continue;
    if (f.endsWith('.svelte') || f.endsWith('.ts')) out[`src/${f}`] = readFileSync(`src/${f}`, 'utf8');
  }
  return out;
}

describe('error copy at display sites', () => {
  const files = sources();

  it('shows errorText(e), never e.message', () => {
    const offenders: string[] = [];
    for (const [file, text] of Object.entries(files)) {
      text.split('\n').forEach((line, i) => {
        if (!line.includes('.error.message') || ALLOWED_LINE.test(line)) return;
        if (WAITING.some((w) => w.file === file && line.includes(w.text))) return;
        offenders.push(`${file}:${i + 1}: ${line.trim()}`);
      });
    }
    expect(
      offenders,
      'Show errorText(r.error) (src/lib/error_copy.ts) instead of r.error.message; ' +
        "the backend's message is log text and belongs under Details:\n  " +
        offenders.join('\n  '),
    ).toEqual([]);
  });

  it('every waiting entry still matches a line', () => {
    for (const w of WAITING) {
      expect(files[w.file] ?? '', `${w.file}: ${w.text} (${w.why})`).toContain(w.text);
    }
  });
});

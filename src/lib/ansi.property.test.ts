/**
 * Property tests for the ANSI screen buffer (OPS-9). The parser sits on the
 * raw PTY byte stream, so it must survive anything: partial escapes, junk
 * bytes, hostile parameters, and chunk boundaries landing mid-sequence.
 */
import { describe, it, expect } from 'vitest';
import fc from 'fast-check';
import { Screen, type Cell } from './ansi';
import { firstCharWidth } from './wcwidth';

// 200 keeps the suite fast; `FC_NUM_RUNS=5000 pnpm test ansi.property` for a
// deeper local soak after touching the parser. (Read via globalThis — the
// frontend tsconfig has no node types.)
const NUM_RUNS =
  Number((globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env?.FC_NUM_RUNS) || 200;

// ─── Input arbitraries ──────────────────────────────────────────────────

/** Any code point, including lone surrogates (String.fromCodePoint accepts
 *  0xD800–0xDFFF and yields an unpaired unit) and the C0/C1 controls. */
const anyCodePoint = fc.integer({ min: 0, max: 0x10ffff }).map((cp) => String.fromCodePoint(cp));

/** A handful of code points the parser treats specially. */
const interesting = fc.constantFrom(
  '\x1b', '[', ']', 'P', '_', '^', 'X', '\\', ';', '?', '>', '!', ' ', 'q', 'm', 'H', 'J', 'K',
  '\x07', '\x9c', '\x18', '\x1a', '\r', '\n', '\t', '\x08', '\x0e', '\x0f', '\x7f',
  '0', '1', '2', '5', '9', '~', 'a', 'Z', 'b', 'I', 'n', 'c',
  '😀', '中', 'é', '́', '‍', '️', '\ud83d', '\ude00', '\ud800',
);

const csiFinal = fc.constantFrom(...'ABCDEFGHJKLMPSTXZbcdfhlmnqrsu@I'.split(''));
const csiParams = fc.array(fc.integer({ min: -5, max: 99999 }), { maxLength: 6 }).map((ps) =>
  ps.map((p) => (p < 0 ? '' : String(p))).join(';'),
);
const csiMarker = fc.constantFrom('', '', '', '?', '>', '!', '=');
const csiIntermediate = fc.constantFrom('', '', ' ', '$', '"');
const csi = fc
  .tuple(csiMarker, csiParams, csiIntermediate, csiFinal)
  .map(([mk, ps, im, fin]) => `\x1b[${mk}${ps}${im}${fin}`);

/** Well-formed SGR: exercises the 38/48 sub-parameter paths. */
const sgr = fc.constantFrom(
  '\x1b[0m', '\x1b[1;31m', '\x1b[38;5;123m', '\x1b[48;2;1;2;3m', '\x1b[7m', '\x1b[38;2m', '\x1b[48;5m',
);

const oscBody = fc.string({ maxLength: 12 });
const osc = fc.tuple(fc.constantFrom('0;', '52;c;aGVsbG8=', '8;;', ''), oscBody, fc.constantFrom('\x07', '\x1b\\', '\x9c'))
  .map(([pfx, body, term]) => `\x1b]${pfx}${body}${term}`);
const dcsLike = fc
  .tuple(fc.constantFrom('P', '_', '^', 'X'), fc.string({ maxLength: 12 }), fc.constantFrom('\x1b\\', '\x9c'))
  .map(([intro, body, term]) => `\x1b${intro}${body}${term}`);
const plainEsc = fc.constantFrom('\x1b7', '\x1b8', '\x1bM', '\x1bD', '\x1bE', '\x1bc', '\x1b(0', '\x1b(B', '\x1b)0', '\x1b#8', '\x1b=', '\x1b>');
const decMode = fc.tuple(fc.constantFrom(1, 25, 47, 1047, 1048, 1049, 1000, 1002, 1006, 2004), fc.constantFrom('h', 'l'))
  .map(([m, hl]) => `\x1b[?${m}${hl}`);
const scrollRegion = fc.tuple(fc.integer({ min: 0, max: 12 }), fc.integer({ min: 0, max: 12 }))
  .map(([a, b]) => `\x1b[${a};${b}r`);

/** A chunk of terminal output: text, sequences and junk fragments mixed. */
const fragment = fc.oneof(
  { arbitrary: fc.string({ maxLength: 8 }), weight: 4 },
  { arbitrary: anyCodePoint, weight: 3 },
  { arbitrary: interesting, weight: 3 },
  { arbitrary: csi, weight: 3 },
  { arbitrary: sgr, weight: 1 },
  { arbitrary: osc, weight: 1 },
  { arbitrary: dcsLike, weight: 1 },
  { arbitrary: plainEsc, weight: 1 },
  { arbitrary: decMode, weight: 1 },
  { arbitrary: scrollRegion, weight: 1 },
);

const stream = fc.array(fragment, { maxLength: 40 }).map((xs) => xs.join(''));
const dims = fc.record({ rows: fc.integer({ min: 1, max: 12 }), cols: fc.integer({ min: 1, max: 20 }) });

// ─── Helpers ────────────────────────────────────────────────────────────

function snapshot(s: Screen): string {
  const grid = s.cells.map((row) => row.map((c) => `${c.ch}|${c.fg}|${c.bg}|${c.attrs}`).join('')).join('\n');
  return `${grid}\n${s.cursorRow},${s.cursorCol},${s.curFg},${s.curBg},${s.curAttrs},${s.cursorVisible},${s.takeReplies()}`;
}

function checkRow(row: Cell[], cols: number) {
  expect(row).toHaveLength(cols);
  for (let c = 0; c < cols; c++) {
    const cell = row[c];
    if (cell.ch === '') {
      // A trailing half must have a wide head directly before it.
      expect(c).toBeGreaterThan(0);
      expect(firstCharWidth(row[c - 1].ch)).toBe(2);
    } else if (firstCharWidth(cell.ch) === 2) {
      // A wide head must have its trailing half inside the row.
      expect(c + 1).toBeLessThan(cols);
      expect(row[c + 1].ch).toBe('');
    }
  }
}

/** Split `s` at the given (sorted, deduped) UTF-16 offsets. */
function splitAt(s: string, cuts: number[]): string[] {
  const out: string[] = [];
  let prev = 0;
  for (const c of cuts) {
    out.push(s.slice(prev, c));
    prev = c;
  }
  out.push(s.slice(prev));
  return out;
}

// ─── Properties ─────────────────────────────────────────────────────────

describe('ansi.Screen properties', () => {
  it('(1) never throws on arbitrary input, fed in arbitrary chunks', () => {
    fc.assert(
      fc.property(dims, fc.array(stream, { maxLength: 6 }), ({ rows, cols }, chunks) => {
        const s = new Screen(rows, cols);
        for (const chunk of chunks) s.write(chunk);
        s.takeReplies();
        s.resize(cols, rows); // and a resize on top
        s.write(chunks.join(''));
      }),
      { numRuns: NUM_RUNS },
    );
  });

  it('(1b) the carried-over parser state stays bounded however the input is cut', () => {
    fc.assert(
      fc.property(dims, fc.array(stream, { maxLength: 6 }), ({ rows, cols }, chunks) => {
        const s = new Screen(rows, cols);
        for (const chunk of chunks) {
          s.write(chunk);
          // CSI_MAX (1024) + the opener, or OSC_MAX for a kept OSC body.
          expect(s.bufferedLength).toBeLessThanOrEqual(64 * 1024);
        }
      }),
      { numRuns: NUM_RUNS },
    );
  });

  it('(2) the cursor stays within the screen after any input', () => {
    fc.assert(
      fc.property(dims, fc.array(stream, { maxLength: 6 }), ({ rows, cols }, chunks) => {
        const s = new Screen(rows, cols);
        for (const chunk of chunks) {
          s.write(chunk);
          expect(s.cursorRow).toBeGreaterThanOrEqual(0);
          expect(s.cursorRow).toBeLessThan(rows);
          expect(s.cursorCol).toBeGreaterThanOrEqual(0);
          // `cols` itself is the deferred-wrap position after the last column.
          expect(s.cursorCol).toBeLessThanOrEqual(cols);
        }
      }),
      { numRuns: NUM_RUNS },
    );
  });

  it('(3) chunk-splitting invariance: one write equals the same bytes split at any boundaries', () => {
    fc.assert(
      fc.property(
        dims,
        stream.chain((str) =>
          fc.tuple(
            fc.constant(str),
            fc.uniqueArray(fc.integer({ min: 0, max: Math.max(0, str.length) }), { maxLength: 8 }).map((xs) =>
              xs.sort((a, b) => a - b),
            ),
          ),
        ),
        ({ rows, cols }, [str, cuts]) => {
          const whole = new Screen(rows, cols);
          whole.write(str);
          const pieces = new Screen(rows, cols);
          for (const part of splitAt(str, cuts)) pieces.write(part);
          expect(snapshot(pieces)).toBe(snapshot(whole));
        },
      ),
      { numRuns: NUM_RUNS },
    );
  });

  it('(4) width invariant: every row has exactly `cols` cells and wide pairs are never split', () => {
    fc.assert(
      fc.property(dims, fc.array(stream, { maxLength: 6 }), dims, ({ rows, cols }, chunks, after) => {
        const s = new Screen(rows, cols);
        for (const chunk of chunks) {
          s.write(chunk);
          expect(s.cells).toHaveLength(rows);
          for (const row of s.cells) checkRow(row, cols);
        }
        // Narrowing/widening must keep the invariant too.
        s.resize(after.rows, after.cols);
        expect(s.cells).toHaveLength(after.rows);
        for (const row of s.cells) checkRow(row, after.cols);
      }),
      { numRuns: NUM_RUNS },
    );
  });
});

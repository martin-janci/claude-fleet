// Assets M6 (Rulings R7): the drift diff is computed here, from the two
// texts `catalog_drift_diff` answers. Myers' O((N+M)·D) line diff; past
// `maxD` edits it gives up and replaces the whole file (a drift that large
// is read as "different", not line by line). Output is the unified format
// `DiffView`'s `parseUnifiedDiff` reads.

export interface DiffOp { t: ' ' | '-' | '+'; line: string }

export function splitLines(text: string | null | undefined): string[] {
  if (!text) return [];
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}

export function diffLines(a: string[], b: string[], maxD = 2000): DiffOp[] {
  const n = a.length;
  const m = b.length;
  const max = n + m;
  if (max === 0) return [];
  const off = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    if (d > maxD) return [...a.map((line) => ({ t: '-' as const, line })), ...b.map((line) => ({ t: '+' as const, line }))];
    trace.push(v.slice(off - d - 1, off + d + 2));
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) { x++; y++; }
      v[off + k] = x;
      if (x >= n && y >= m) return backtrack(trace, a, b);
    }
  }
  return [];
}

function backtrack(trace: Int32Array[], a: string[], b: string[]): DiffOp[] {
  const out: DiffOp[] = [];
  let x = a.length;
  let y = b.length;
  for (let d = trace.length - 1; d >= 0; d--) {
    const v = trace[d];
    const at = (k: number) => v[k + d + 1];
    const k = x - y;
    const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
    const prevX = d === 0 ? 0 : at(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) { out.push({ t: ' ', line: a[x - 1] }); x--; y--; }
    if (d > 0) {
      if (x === prevX) { out.push({ t: '+', line: b[y - 1] }); y--; }
      else { out.push({ t: '-', line: a[x - 1] }); x--; }
    }
  }
  return out.reverse();
}

/** Unified diff of two texts (a `null` side is empty), `ctx` lines of context. '' when equal. */
export function unifiedDiff(a: string | null | undefined, b: string | null | undefined, aName: string, bName: string, ctx = 3): string {
  const ops = diffLines(splitLines(a), splitLines(b));
  if (!ops.some((o) => o.t !== ' ')) return '';
  const lines = [`--- ${aName}`, `+++ ${bName}`];
  // Positions of each op in a and b (1-based line numbers before the op).
  let ai = 0;
  let bi = 0;
  const pos = ops.map((o) => {
    const p = { a: ai, b: bi };
    if (o.t !== '+') ai++;
    if (o.t !== '-') bi++;
    return p;
  });
  let i = 0;
  while (i < ops.length) {
    if (ops[i].t === ' ') { i++; continue; }
    const start = Math.max(0, i - ctx);
    let end = i;
    // Extend while the next change is within 2·ctx of the last one.
    let j = i;
    while (j < ops.length) {
      if (ops[j].t !== ' ') { end = j; j++; continue; }
      let k = j;
      while (k < ops.length && ops[k].t === ' ') k++;
      if (k < ops.length && k - j <= 2 * ctx) { j = k; continue; }
      break;
    }
    const stop = Math.min(ops.length, end + ctx + 1);
    const hunk = ops.slice(start, stop);
    const aLen = hunk.filter((o) => o.t !== '+').length;
    const bLen = hunk.filter((o) => o.t !== '-').length;
    const aStart = aLen === 0 ? pos[start].a : pos[start].a + 1;
    const bStart = bLen === 0 ? pos[start].b : pos[start].b + 1;
    lines.push(`@@ -${aStart},${aLen} +${bStart},${bLen} @@`);
    for (const o of hunk) lines.push(o.t + o.line);
    i = stop;
  }
  return lines.join('\n') + '\n';
}

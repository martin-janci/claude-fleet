// Pure lane-assignment for the commit graph (the "branch tree"). Input is
// commits in `git log` order (child before parent). Output is per-row layout:
// the commit's column + color, and the lane occupancy entering (`lanesIn`) and
// leaving (`lanesOut`) the row, from which CommitGraph.svelte draws connectors.
// No git ASCII --graph: keeping this here makes it testable and interactive.

export interface GraphInput {
  hash: string;
  parents: string[];
}

export interface GraphRow {
  hash: string;
  column: number;
  color: number;
  /** Lane→hash occupancy at the top of this row's cell. */
  lanesIn: (string | null)[];
  /** Lane→hash occupancy at the bottom of this row's cell. */
  lanesOut: (string | null)[];
  /** hash → color index, for every lane referenced by this row. */
  colors: Record<string, number>;
}

export function computeGraph(commits: GraphInput[]): GraphRow[] {
  const lanes: (string | null)[] = []; // persists across rows
  const color = new Map<string, number>();
  let nextColor = 0;
  const rows: GraphRow[] = [];

  const firstFree = (): number => {
    const i = lanes.indexOf(null);
    return i === -1 ? lanes.length : i;
  };

  for (const c of commits) {
    // Ensure a lane awaits this commit; a tip allocates a fresh lane+color.
    let col = lanes.indexOf(c.hash);
    if (col === -1) {
      col = firstFree();
      lanes[col] = c.hash;
      if (!color.has(c.hash)) color.set(c.hash, nextColor++);
    }
    const lanesIn = lanes.slice();
    const myColor = color.get(c.hash)!;

    // Free this commit's lane; parents are routed below.
    lanes[col] = null;

    c.parents.forEach((p, idx) => {
      if (lanes.indexOf(p) !== -1) return; // a lane already awaits this parent
      if (idx === 0) {
        lanes[col] = p; // first parent inherits the commit's lane + color
        if (!color.has(p)) color.set(p, myColor);
      } else {
        const f = firstFree();
        lanes[f] = p;
        if (!color.has(p)) color.set(p, nextColor++);
      }
    });

    while (lanes.length > 0 && lanes[lanes.length - 1] === null) lanes.pop();
    const lanesOut = lanes.slice();

    const colors: Record<string, number> = {};
    for (const h of new Set<string | null>([...lanesIn, ...lanesOut, c.hash])) {
      if (h && color.has(h)) colors[h] = color.get(h)!;
    }

    rows.push({ hash: c.hash, column: col, color: myColor, lanesIn, lanesOut, colors });
  }
  return rows;
}

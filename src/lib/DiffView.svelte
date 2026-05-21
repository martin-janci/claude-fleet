<script lang="ts" module>
  interface DiffRow {
    kind: 'hunk' | 'add' | 'del' | 'ctx' | 'meta';
    oldNo: number | null;
    newNo: number | null;
    text: string;
  }

  // Parse a unified diff into render rows, tracking old/new line numbers.
  // Exported for testing.
  export function parseUnifiedDiff(diff: string): DiffRow[] {
    const rows: DiffRow[] = [];
    let oldNo = 0;
    let newNo = 0;
    const lines = diff.split('\n');
    // A diff that ends in \n yields a trailing '' — drop it so it isn't
    // rendered as a spurious blank context row.
    if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();

    for (const line of lines) {
      if (line.startsWith('@@')) {
        const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
        if (m) {
          oldNo = parseInt(m[1], 10);
          newNo = parseInt(m[2], 10);
        }
        rows.push({ kind: 'hunk', oldNo: null, newNo: null, text: line });
        continue;
      }
      if (
        line.startsWith('diff --git') ||
        line.startsWith('index ') ||
        line.startsWith('--- ') ||
        line.startsWith('+++ ') ||
        line.startsWith('old mode') ||
        line.startsWith('new mode') ||
        line.startsWith('similarity ') ||
        line.startsWith('rename ') ||
        line.startsWith('copy ') ||
        line.startsWith('new file') ||
        line.startsWith('deleted file') ||
        line.startsWith('\\')
      ) {
        rows.push({ kind: 'meta', oldNo: null, newNo: null, text: line });
        continue;
      }
      if (line.startsWith('+')) {
        rows.push({ kind: 'add', oldNo: null, newNo: newNo++, text: line.slice(1) });
      } else if (line.startsWith('-')) {
        rows.push({ kind: 'del', oldNo: oldNo++, newNo: null, text: line.slice(1) });
      } else {
        // Context line (leading space) or a genuinely empty line.
        rows.push({ kind: 'ctx', oldNo: oldNo++, newNo: newNo++, text: line.slice(1) });
      }
    }
    return rows;
  }
</script>

<script lang="ts">
  let { diff }: { diff: string } = $props();
  const rows = $derived(parseUnifiedDiff(diff));
</script>

<div class="diff" data-testid="diff-view">
  {#each rows as row}
    <div class="row {row.kind}">
      <span class="gutter old">{row.oldNo ?? ''}</span>
      <span class="gutter new">{row.newNo ?? ''}</span>
      <span class="marker"
        >{row.kind === 'add' ? '+' : row.kind === 'del' ? '-' : ' '}</span
      >
      <span class="text">{row.text || ' '}</span>
    </div>
  {/each}
</div>

<style>
  .diff {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.78rem;
    line-height: 1.5;
    white-space: pre;
    overflow: auto;
    height: 100%;
  }
  .row {
    display: flex;
    align-items: baseline;
  }
  .gutter {
    flex: 0 0 auto;
    width: 3.2em;
    padding: 0 0.5em;
    text-align: right;
    color: var(--fg-muted);
    user-select: none;
    opacity: 0.6;
  }
  .marker {
    flex: 0 0 auto;
    width: 1.2em;
    text-align: center;
    user-select: none;
    color: var(--fg-muted);
  }
  .text {
    flex: 1 1 auto;
  }
  .row.add {
    background: color-mix(in srgb, #3fb950 16%, transparent);
  }
  .row.add .marker {
    color: #3fb950;
  }
  .row.del {
    background: color-mix(in srgb, #f85149 16%, transparent);
  }
  .row.del .marker {
    color: #f85149;
  }
  .row.hunk {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }
  .row.hunk .gutter,
  .row.hunk .marker {
    opacity: 0;
  }
  .row.meta {
    color: var(--fg-muted);
    opacity: 0.7;
  }
  .row.meta .gutter,
  .row.meta .marker {
    opacity: 0;
  }
</style>

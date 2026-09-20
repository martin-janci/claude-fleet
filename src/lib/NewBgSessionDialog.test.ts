import { describe, it, expect } from 'vitest';

// Vite raw import, not `node:fs` — this project ships no Node types (see
// conversation_theme.test.ts for the same pattern), so `?raw` keeps
// svelte-check clean while still reading the file as source text.
const SOURCE = import.meta.glob('./NewBgSessionDialog.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const css = SOURCE['./NewBgSessionDialog.svelte'];

describe('NewBgSessionDialog', () => {
  // `not.toContain` alone passes against an empty string, which is exactly
  // what a broken glob or a renamed file would hand this test. Prove the
  // source was read, and that the thing the absence is ABOUT is present.
  it('reads the component source at all', () => {
    expect(css).toBeTruthy();
    expect(css).toContain('<script lang="ts">');
    expect(css).toContain('.modal-actions');
  });

  it('the primary action needs no specificity escape hatch', () => {
    expect(css).toContain('class="btn btn--primary"');
    // `:not(.btn)` is what makes that possible: the bare `button` type
    // selector would otherwise out-specify the primitive class and need
    // `!important` to be beaten back.
    expect(css).toContain('.modal-actions button:not(.btn)');
    expect(css).not.toContain('!important');
  });
});

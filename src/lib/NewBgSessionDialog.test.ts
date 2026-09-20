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
  it('the primary action needs no specificity escape hatch', () => {
    expect(css).not.toContain('!important');
  });
});

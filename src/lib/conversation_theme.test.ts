import { describe, it, expect } from 'vitest';

/** The Conversation tab's components, read as source through Vite's `?raw`
 *  (no node types needed, and no transform between the file and the test). */
const SOURCES = import.meta.glob('./{ConversationPanel,ConversationHeader,ToolLine,SubagentBlock}.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** The two ad-hoc colours the chat grew: a warn orange and an error red.
 *  Both have theme-aware tokens in app.css (--usage-warn / --usage-crit);
 *  the literals are only legible in the dark theme — on the light one they
 *  sit near 2:1 against the background, well under the 4.5:1 text floor. */
const LITERALS = /#e6a23c|#e64a4a/gi;

// That the tokens themselves exist and are what the chat asks for is
// attention.test.ts's job (contextColor / contextTint); vitest does not
// process CSS imports, so app.css cannot be read from here.

describe('chat colour tokens', () => {
  it('covers every component of the Conversation tab', () => {
    expect(Object.keys(SOURCES)).toHaveLength(4);
  });

  for (const [path, source] of Object.entries(SOURCES)) {
    it(`${path} states warn and error through the theme tokens`, () => {
      expect(source.match(LITERALS) ?? []).toEqual([]);
    });
  }
});

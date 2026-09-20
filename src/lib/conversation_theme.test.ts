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

describe('chat column width', () => {
  const panel = SOURCES['./ConversationPanel.svelte'];

  it('states the reading column once, as a token', () => {
    // The thread, the chips, the composer row, the slash menu and the
    // toolbar all have to agree; seven copies of `80ch` is how they drift.
    expect(panel.match(/80ch/g) ?? []).toHaveLength(1);
    expect(panel).toContain('--chat-col: 80ch');
    expect(panel.match(/max-width: var\(--chat-col\)/g)?.length ?? 0).toBeGreaterThanOrEqual(6);
  });

  it('centres the sticky toolbar over the reading column', () => {
    // Full-bleed background and border, but the controls track the text.
    expect(panel).toContain('max(1.1rem, calc((100% - var(--chat-col)) / 2))');
  });
});

describe('chat stylesheet hygiene', () => {
  const panel = SOURCES['./ConversationPanel.svelte'];

  it('states the composer status once, without a wrapper that carries nothing', () => {
    // .composer-foot was a flex row with space-between built for two items;
    // only the status note was ever left in it.
    expect(panel).not.toContain('composer-foot');
    // ...and the status must not be declared twice, once in a shared group
    // and once on its own with a conflicting margin.
    expect(panel.match(/^\s*\.composer-status[\s,{]/gm) ?? []).toHaveLength(1);
  });
});

describe('chat sizes itself to its pane', () => {
  const panel = SOURCES['./ConversationPanel.svelte'];
  const header = SOURCES['./ConversationHeader.svelte'];

  it('declares a query container on the panel', () => {
    // The Conversation tab is a resizable pane, not the window: everything
    // that adapts has to ask the pane's width, not the viewport's.
    expect(panel).toContain('container-type: inline-size');
    expect(panel).toContain('@container');
  });

  it('sizes the pop-ups against the pane rather than the viewport', () => {
    // A 80vw dropdown inside a narrow pane of a wide window overflows it.
    expect(panel).not.toMatch(/\d+vw/);
    expect(header).not.toMatch(/\d+vw/);
    expect(panel).toMatch(/cqw/);
  });
});

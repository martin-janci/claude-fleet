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
// attention.test.ts's job (`contextColor`); that they match app.css is
// tokens.test.ts's. Vite's `?raw` cannot read a `.css` module's text, which
// is why neither happens here.

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
  const header = SOURCES['./ConversationHeader.svelte'];

  it('states the reading column once, as a token', () => {
    // The thread, the chips, the composer row, the slash menu and the
    // toolbar all have to agree; seven copies of `80ch` is how they drift.
    expect(panel.match(/80ch/g) ?? []).toHaveLength(1);
    expect(panel).toContain('--chat-col: 80ch');
    // Task 8: the chips/composer-row/slash-menu/composer-error/composer-status
    // max-width copies are gone — they inherit the column from the
    // composer's --chat-inset padding instead of repeating it themselves.
    expect(panel).toContain('--chat-inset:');
    expect(panel.match(/var\(--chat-inset\)/g)?.length ?? 0).toBeGreaterThanOrEqual(2);
  });

  it('centres the sticky bar over the reading column', () => {
    // Full-bleed background and border, but the controls track the text.
    // The one bar is the header now: the toolbar was folded into it.
    // Task 8: the literal inset expression became the shared --chat-inset
    // token, defined once on .conversation-panel.
    expect(header).toContain('var(--chat-inset)');
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
    // Both pop-ups — the conversation switcher and the turn index — hang off
    // the header now, so that is where the container units live. Named
    // individually: the switcher's `90cqw` alone predates the move, so a
    // bare /cqw/ on the header would pass without the turn index.
    expect(header).toContain('max-width: 90cqw');
    expect(header).toContain('width: min(60ch, 90cqw)');
    expect(header.match(/cqw/g) ?? []).toHaveLength(2);
  });
});

describe('chat motion and scroll containment', () => {
  it('every component that animates also honours prefers-reduced-motion', () => {
    const animating = Object.entries(SOURCES).filter(([, source]) =>
      /\btransition:|\banimation:/.test(source),
    );
    // Without this the whole test is vacuous the day the last transition is
    // deleted — or the day the glob stops resolving — and it would go on
    // passing while claiming to guard something.
    expect(animating.length, 'no component animates: this test is asserting nothing').toBeGreaterThan(0);
    for (const [path, source] of animating) {
      expect(source, `${path} animates but never asks about reduced motion`).toContain(
        'prefers-reduced-motion: reduce',
      );
    }
  });

  it('the thread and its pop-ups do not chain their scroll outwards', () => {
    // Reaching the end of the transcript must not start scrolling whatever
    // is behind the pane.
    const panel = SOURCES['./ConversationPanel.svelte'];
    expect(panel.match(/overscroll-behavior: contain/g)?.length ?? 0).toBeGreaterThanOrEqual(1);
    expect(SOURCES['./ConversationHeader.svelte']).toContain('overscroll-behavior: contain');
  });
});

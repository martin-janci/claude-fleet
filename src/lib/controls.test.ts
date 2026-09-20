import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// `readFileSync`, not a Vite `?raw` import — Vitest strips `.css` module
// content by default regardless of the raw query, so the only reliable way
// to read this file's source text in a test is via Node's fs (see
// `node-fs.d.ts` for the type shim this needs).
const css = readFileSync('src/lib/controls.css', 'utf8');

// `.btn--chip:hover:not(:disabled)` is (0,3,0); `.btn--toggle[aria-pressed='true']`
// is (0,2,0). Composed as `.btn .btn--chip .btn--toggle` (HostChips,
// SegmentedControl) on a PRESSED control, hovering it let the chip hover
// rule win and silently dropped the accent border back to
// --control-border-strong — destroying the selected-state signal the
// comment above the pressed rules says the boundary carries.
describe('controls.css chip/toggle hover specificity', () => {
  it('the chip hover rule does not win over a pressed/active toggle', () => {
    const chipHoverRule = css.match(/\.btn--chip:hover:not\(:disabled\)[^{]*\{/)?.[0] ?? '';
    expect(chipHoverRule).toContain("not([aria-pressed='true'])");
    expect(chipHoverRule).toContain('not(.is-active)');
  });

  it('hovering a pressed/active toggle still gives feedback', () => {
    expect(css).toMatch(/\.btn--toggle\[aria-pressed='true'\]:hover:not\(:disabled\)/);
    expect(css).toMatch(/\.btn--toggle\.is-active:hover:not\(:disabled\)/);
  });
});

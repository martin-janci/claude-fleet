// Review w18-10: the design manual's keyboard page names every live global
// chord as the registry spells it, on the Mac and off it.
import { describe, it, expect } from 'vitest';
import { SHORTCUTS, bindingsFor, formatBinding } from './shortcuts';

const DOCS = import.meta.glob('/docs/ux/2026-10-08-orbit-fleet-redesign/design-system/keyboard.md', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const doc = Object.values(DOCS)[0] ?? '';

describe('keyboard.md follows the shortcut registry', () => {
  it('reads the page', () => {
    expect(doc).toContain('# Keyboard');
  });

  it('every live global chord appears in its row, in both spellings', () => {
    const rows = doc.split('\n').filter((l) => l.startsWith('|'));
    const missing: string[] = [];
    for (const s of SHORTCUTS.filter((x) => x.scope === 'global' && x.status === 'live')) {
      const mac = bindingsFor(s, true)[0];
      const other = bindingsFor(s, false)[0];
      const macText = mac ? formatBinding(mac, true) : '—';
      const otherText = other ? formatBinding(other, false) : '—';
      const row = rows.find((l) => {
        const cells = l.split('|').map((c) => c.trim());
        return cells[1]?.split(/,\s+|–/).includes(macText) && cells[2]?.split(/,\s+|–/).includes(otherText);
      });
      if (!row) missing.push(`${s.id}: ${macText} / ${otherText}`);
    }
    expect(missing).toEqual([]);
  });
});

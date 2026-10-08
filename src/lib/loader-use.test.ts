import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// The manual: inside rows, buttons and the status bar use only the Comet or
// the 16 px Orbit. This reads every <Loader> in the app and checks where it
// sits: inside a <button>, or anywhere in a row, chip or status-bar component.

interface LoaderUse {
  file: string;
  tag: string;
  inline: boolean;
}

const ROW_OR_BAR = /(Row[A-Za-z]*|Chip[A-Za-z]*|StatusBar[A-Za-z]*)\.svelte$/;

function loaderUses(file: string, src: string): LoaderUse[] {
  const out: LoaderUse[] = [];
  const markup = src.replace(/<script[\s\S]*?<\/script>/g, (m) => ' '.repeat(m.length)).replace(/<!--[\s\S]*?-->/g, '');
  for (const m of markup.matchAll(/<Loader\b[\s\S]*?\/>/g)) {
    const before = markup.slice(0, m.index);
    const open = (before.match(/<button\b/g) ?? []).length;
    // Prettier closes a long button as `</button` and `>` on the next line.
    const closed = (before.match(/<\/button\s*>/g) ?? []).length;
    out.push({ file, tag: m[0].replace(/\s+/g, ' '), inline: ROW_OR_BAR.test(file) || open > closed });
  }
  return out;
}

/** A Comet, or the Orbit (the default) at 16 px. Names must be literal. */
function allowedInline(tag: string): boolean {
  const name = tag.match(/\bname="([^"]+)"/)?.[1] ?? (/\bname=/.test(tag) ? null : 'orbit');
  if (name === 'comet') return true;
  return name === 'orbit' && /\bsize=\{16\}/.test(tag);
}

describe('loader use', () => {
  it('reads a loader inside a button or a row component as inline', () => {
    expect(loaderUses('src/lib/X.svelte', '<button><Loader name="orbit" /></button><Loader name="galaxy" />')).toEqual([
      { file: 'src/lib/X.svelte', tag: '<Loader name="orbit" />', inline: true },
      { file: 'src/lib/X.svelte', tag: '<Loader name="galaxy" />', inline: false },
    ]);
    expect(loaderUses('src/lib/SessionRowItem.svelte', '<span><Loader name="radar" /></span>')[0].inline).toBe(true);
    expect(loaderUses('src/lib/StatusBar.svelte', '<Loader />')[0].inline).toBe(true);
    expect(loaderUses('src/lib/X.svelte', '<button>Open</button\n  ><Loader name="galaxy" />')[0].inline).toBe(false);
  });

  it('allows only the Comet or the 16 px Orbit inline', () => {
    expect(allowedInline('<Loader name="comet" size={12} />')).toBe(true);
    expect(allowedInline('<Loader size={16} />')).toBe(true);
    expect(allowedInline('<Loader name="orbit" size={16} />')).toBe(true);
    expect(allowedInline('<Loader name="orbit" size={24} />')).toBe(false);
    expect(allowedInline('<Loader />')).toBe(false);
    expect(allowedInline('<Loader name="dot-wave" />')).toBe(false);
    expect(allowedInline('<Loader name={pick} />')).toBe(false);
  });

  it('every row, button and status-bar loader in the app is a Comet or the 16 px Orbit', () => {
    const files = readdirSync('src', { recursive: true })
      .filter((n) => n.endsWith('.svelte'))
      .map((n) => `src/${n.replaceAll('\\', '/')}`);
    const uses = files.flatMap((f) => loaderUses(f, readFileSync(f, 'utf8')));
    expect(uses.length).toBeGreaterThan(0);
    const bad = uses.filter((u) => u.inline && !allowedInline(u.tag)).map((u) => `${u.file}: ${u.tag}`);
    expect(bad).toEqual([]);
  });
});

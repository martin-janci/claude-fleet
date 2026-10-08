// A chord as the manual writes it (the Mac form: ⌘K, ⌥⌘T, ⌘⇧E) and as it
// reads off the Mac: ⌘ becomes Ctrl, ⌥ Alt, ⇧ Shift, ⌃ Ctrl, joined with +
// in Ctrl, Alt, Shift order (Keyboard section: ⌥⌘ becomes Ctrl+Alt).

const MODS: Record<string, string> = { '⌘': 'Ctrl', '⌃': 'Ctrl', '⌥': 'Alt', '⇧': 'Shift' };
const ORDER = ['Ctrl', 'Alt', 'Shift'];

export function platformChord(chord: string, mac: boolean): string {
  if (mac) return chord;
  const mods = new Set<string>();
  let key = '';
  for (const ch of chord) {
    if (MODS[ch]) mods.add(MODS[ch]);
    else key += ch;
  }
  if (mods.size === 0) return chord;
  return [...ORDER.filter((m) => mods.has(m)), key].join('+');
}

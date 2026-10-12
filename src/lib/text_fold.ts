// One fold for every search box, so "uloha" finds "Úloha" and "zluty"
// finds "žltý": lower-case, combining marks dropped, and the Latin-1 /
// Latin Extended-A letters (U+00C0–U+017F) mapped to their bare letter.
// A table rather than `normalize('NFD')`, so the hub
// (`crates/fleet-core/src/search_text.rs`, whose test checks this string)
// and the mobile app fold exactly the same way.

/** U+00C0..U+0180, lower-cased: the bare letter, or `.` to keep it. */
const LATIN =
  'aaaaaa.ceeeeiiiidnooooo.ouuuuy..aaaaaa.ceeeeiiiidnooooo.ouuuuy.y' +
  'aaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiii..jjkkklllllll' +
  'lllnnnnnnn..oooooo..rrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs';

// eslint-disable-next-line no-control-regex
const ASCII = /^[\x00-\x7f]*$/;

/** `s` lower-cased with its diacritics dropped. */
export function fold(s: string): string {
  const lower = s.toLowerCase();
  if (ASCII.test(lower)) return lower;
  let out = '';
  for (const c of lower) {
    const cp = c.codePointAt(0)!;
    if (cp >= 0x300 && cp <= 0x36f) continue;
    if (cp >= 0xc0 && cp < 0x180) {
      const b = LATIN[cp - 0xc0];
      if (b !== '.') {
        out += b;
        continue;
      }
    }
    out += c;
  }
  return out;
}

/** Whether `hay` contains `needle`, case and accents ignored. `needle` is
 *  expected already folded. */
export function foldedIncludes(hay: string | null | undefined, needle: string): boolean {
  return !!hay && fold(hay).includes(needle);
}

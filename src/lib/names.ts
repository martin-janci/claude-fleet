// Docker-style memorable session names: `<adjective>-<noun>` ("blue-sirius").
//
// The word lists live in `names.json` and are shared verbatim with the Rust
// twin (`src-tauri/src/service/names.rs`, `include_str!`), so a name minted
// on either side comes from the same vocabulary. Nouns are stars,
// constellations and other celestial bodies; adjectives are colours and
// short evocative qualities. Every word is lowercase ASCII, 3–8 letters, so
// the slug is always a legal git branch name AND a legal tmux session name
// without any further cleaning.
//
// Collision policy (mirrors Docker's `GetRandomName`): draw random pairs
// until one is free; after `MAX_TRIES` misses fall back to the last pair
// with a numeric suffix (`blue-sirius-2`, `-3`, …). Docker appends a random
// digit instead; a counting suffix is preferred here because it reads as
// "the second blue sirius" rather than as noise.
import words from './names.json';

export const ADJECTIVES: readonly string[] = words.adjectives;
export const NOUNS: readonly string[] = words.nouns;
export const SEPARATOR = '-';
/** Random draws before falling back to a numeric suffix. */
export const MAX_TRIES = 24;
/** Shared-prefix length that makes a pair redundant ("lunar luna", "cosmic cosmos"). */
export const SAME_ROOT_PREFIX = 4;

/** True when the adjective and noun share a root and read as a stutter. */
export function sameRoot(adjective: string, noun: string): boolean {
  let i = 0;
  while (i < adjective.length && i < noun.length && adjective[i] === noun[i]) i++;
  return i >= SAME_ROOT_PREFIX;
}

/** Uniform random in [0, 1). Injectable so tests are deterministic. */
export type Rng = () => number;

function pick(list: readonly string[], rng: Rng): string {
  const i = Math.floor(rng() * list.length);
  // Guard against an rng that returns exactly 1 (or NaN) — clamp into range.
  return list[Math.min(Math.max(i, 0), list.length - 1)] ?? list[0];
}

/**
 * Mint a name that is not in `existing`. Comparison is case-insensitive and
 * `existing` is expected to hold slugs (worktree names, tmux-name suffixes,
 * slugified friendly names) — see `takenSlugsForProject` in the dialog.
 */
export function generateName(
  existing: ReadonlySet<string> = new Set(),
  rng: Rng = Math.random,
): string {
  const taken = new Set<string>();
  for (const e of existing) taken.add(e.toLowerCase());
  let last = '';
  for (let i = 0; i < MAX_TRIES; i++) {
    const adjective = pick(ADJECTIVES, rng);
    const noun = pick(NOUNS, rng);
    // A same-root draw still spends a try, so the budget stays MAX_TRIES.
    if (sameRoot(adjective, noun)) continue;
    last = `${adjective}${SEPARATOR}${noun}`;
    if (!taken.has(last)) return last;
  }
  if (!last) last = `${ADJECTIVES[0]}${SEPARATOR}${NOUNS[0]}`;
  // Every draw collided (tiny pool, or a hostile rng). Count up from 2 so the
  // result is still readable and still unique.
  for (let n = 2; ; n++) {
    const candidate = `${last}${SEPARATOR}${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** `blue-sirius` → `blue sirius` (what the friendly-name field shows). */
export function nameWords(slug: string): string {
  return slug.split(SEPARATOR).filter(Boolean).join(' ');
}

/** True when `slug` is `<adjective>-<noun>` (optionally `-<n>`) from the lists. */
export function isGeneratedName(slug: string): boolean {
  const parts = slug.toLowerCase().split(SEPARATOR);
  if (parts.length < 2 || parts.length > 3) return false;
  if (parts.length === 3 && !/^\d+$/.test(parts[2])) return false;
  return ADJECTIVES.includes(parts[0]) && NOUNS.includes(parts[1]);
}

/**
 * The suffix a tmux name carries after the project prefix:
 * `dev-<owner>-<repo>--fix-login-term` → `fix-login`. Returns null when the
 * name is not of that shape (a bare `dev-<owner>-<repo>` or a foreign name).
 */
export function tmuxNameSuffix(tmuxName: string, owner: string, repo: string): string | null {
  const prefix = `dev-${owner}-${repo}--`;
  if (!tmuxName.startsWith(prefix)) return null;
  let rest = tmuxName.slice(prefix.length);
  if (rest.endsWith('-term')) rest = rest.slice(0, -'-term'.length);
  return rest || null;
}

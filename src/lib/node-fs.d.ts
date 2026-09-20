// Type shim for `node:fs`'s `readFileSync` — this project ships no Node
// types (no `@types/node`; see `names.json.d.ts` for the same kind of
// shim for a different gap). Vitest runs on Node and resolves the real
// module at runtime; this only gives svelte-check a type for the one
// function a source-text test needs when Vite's `?raw` import can't be
// used (Vitest strips `.css` module content, `?raw` query or not).
declare module 'node:fs' {
  export function readFileSync(path: string, encoding: 'utf8'): string;
}

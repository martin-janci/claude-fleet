// Type shim for the shared word lists. The JSON itself is the single source
// of truth for BOTH the TS generator (`names.ts`) and the Rust twin
// (`src-tauri/src/service/names.rs`, via `include_str!`), so the two can
// never drift. Vite/Vitest load the JSON natively; this file only gives
// svelte-check a type for the import without enabling `resolveJsonModule`
// project-wide.
declare const words: {
  readonly adjectives: readonly string[];
  readonly nouns: readonly string[];
};
export default words;

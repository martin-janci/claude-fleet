# claude-fleet

A native cross-platform desktop app for managing long-lived [Claude Code](https://claude.com/claude-code) sessions running in tmux across multiple machines (mac, mefistos, hetzner). Built with Rust + Tauri 2 + Svelte.

> Status: Phase 1 (bootstrap & UI shell). See [docs/specs](docs/specs/2026-05-19-claude-fleet-design.md) for the full design and [docs/plans](docs/plans/) for the per-phase implementation plans.

## Requirements

- macOS 13+ (primary) or Linux (mefistos / hetzner)
- Rust 1.83+ (`rustup install stable`)
- pnpm 9+ (`npm i -g pnpm`)
- Tauri 2 prerequisites: https://v2.tauri.app/start/prerequisites/

## Build & run

```bash
pnpm install
pnpm tauri dev      # dev mode (hot-reload frontend, debug Rust)
pnpm tauri build    # release bundle in src-tauri/target/release/bundle/
```

## Test

```bash
pnpm test                      # frontend (Vitest)
pnpm check                     # frontend Svelte/TS type-check
cd src-tauri && cargo test     # backend (rusqlite + commands)
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo fmt --check
```

## Project layout

```
src/                # Svelte + TS frontend
src-tauri/          # Rust backend (Tauri 2 app + commands)
src-tauri/migrations/  # SQLite migrations
docs/specs/         # design specs
docs/plans/         # per-phase implementation plans
```

## License

Personal project. No license declared yet.

# claude-fleet — Phase 1 (Bootstrap & UI Shell) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold the `claude-fleet` Tauri 2 + Svelte project at `~/projects/github.com/martin-janci/claude-fleet` with a working 3-pane resizable layout, light/dark theming following the OS, a SQLite store with the full schema and migrations, a smoke-test Tauri IPC command, GitHub Actions CI, and a README. Exit: `cargo tauri dev` opens an empty 3-pane app on macOS.

**Architecture:** Cargo workspace with `src-tauri/` (Rust backend) and project root (Svelte+TS frontend). Tauri 2 wires them. SQLite via `rusqlite` lives in the Rust side and is opened lazily on first command. Frontend uses Vite + Svelte 5 + TypeScript with Vitest for unit tests. Layout is a CSS grid with three resizable columns driven by CSS variables, mutated by drag handles.

**Tech Stack:** Rust 1.83+ · Tauri 2 · Svelte 5 + TypeScript · Vite · pnpm · Vitest · @testing-library/svelte · rusqlite (bundled) · GitHub Actions.

**Reference:** [Spec](../specs/2026-05-19-claude-fleet-design.md) §5 (architecture), §6 (schema), §9 Phase 1 (exit criteria).

---

## File Structure (created during this plan)

```
~/projects/github.com/martin-janci/claude-fleet/
├── .github/workflows/ci.yml
├── .gitignore
├── README.md
├── docs/
│   ├── specs/2026-05-19-claude-fleet-design.md   # moved from dotfiles
│   └── plans/2026-05-19-claude-fleet-phase-1.md  # moved from dotfiles
├── package.json
├── pnpm-lock.yaml
├── tsconfig.json
├── vite.config.ts
├── vitest.config.ts
├── index.html
├── src/
│   ├── main.ts                # Svelte entry
│   ├── App.svelte             # 3-pane layout
│   ├── app.css                # global styles + CSS vars
│   ├── lib/
│   │   ├── Pane.svelte        # single pane wrapper (data-testid, empty state)
│   │   ├── Resizer.svelte     # drag handle between panes
│   │   ├── theme.ts           # theme store (auto/light/dark)
│   │   └── ipc.ts             # tiny typed Tauri invoke wrapper
│   └── App.test.ts
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── icons/                 # default Tauri icons (auto-generated)
    ├── migrations/
    │   └── 001_init.sql       # full schema from spec §6
    └── src/
        ├── main.rs            # Tauri app + command registration
        ├── store.rs           # rusqlite open + migrate
        └── commands/
            ├── mod.rs
            └── health.rs      # smoke-test command
```

---

## Task 1: Create the repo and scaffold Tauri 2 + Svelte

**Files:**
- Create: entire repo at `~/projects/github.com/martin-janci/claude-fleet/`

- [ ] **Step 1: Verify tooling is available**

Run:
```bash
which cargo && cargo --version
which pnpm && pnpm --version
which proj-clean && proj-clean --help | head -3
```
Expected: all three resolve. `cargo` ≥ 1.83, `pnpm` ≥ 8, `proj-clean` prints help.

If `pnpm` is missing: `npm install -g pnpm`. If `cargo create-tauri-app` is missing: `cargo install create-tauri-app --locked`.

- [ ] **Step 2: Create the project directory via proj-clean**

Run:
```bash
proj-clean new claude-fleet
```
Expected: creates `~/projects/github.com/martin-janci/claude-fleet/` and `cd`s you in (or prints the path).

If `proj-clean new` requires a template and fails: fall back to:
```bash
mkdir -p ~/projects/github.com/martin-janci/claude-fleet
cd ~/projects/github.com/martin-janci/claude-fleet
git init -b main
```

- [ ] **Step 3: Scaffold Tauri 2 + Svelte-TS inside the new dir**

Run (from inside `~/projects/github.com/martin-janci/claude-fleet`):
```bash
cargo create-tauri-app . --template svelte-ts --manager pnpm --identifier sk.rlt.claude-fleet
```
Expected: scaffolds `src/`, `src-tauri/`, `package.json`, `tauri.conf.json`. Answers any remaining prompts with defaults; project name = `claude-fleet`.

- [ ] **Step 4: Install dependencies and verify dev mode opens**

Run:
```bash
pnpm install
pnpm tauri dev
```
Expected: a window opens showing the Tauri+Svelte stock template (greet form). Close it with Cmd+Q.

- [ ] **Step 5: Move the spec and plan into the new repo**

Run:
```bash
mkdir -p docs/specs docs/plans
cp /Users/martinjanci/dotfiles/.claude/worktrees/angry-bhabha-d299b3/docs/superpowers/specs/2026-05-19-claude-fleet-design.md docs/specs/
cp /Users/martinjanci/dotfiles/.claude/worktrees/angry-bhabha-d299b3/docs/superpowers/plans/2026-05-19-claude-fleet-phase-1.md docs/plans/
```
Expected: both files now sit under `docs/`.

- [ ] **Step 6: Commit the scaffold**

Run:
```bash
git add -A
git status
git commit -m "chore: scaffold Tauri 2 + Svelte project with spec and phase-1 plan"
```
Expected: a single commit landing the scaffold + docs.

---

## Task 2: Replace the stock template with the 3-pane layout

**Files:**
- Modify: `src/App.svelte` (full replace)
- Create: `src/lib/Pane.svelte`
- Create: `src/app.css`
- Create: `src/App.test.ts`
- Modify: `src/main.ts` (import `./app.css`)
- Modify: `package.json` (add `vitest`, `@testing-library/svelte`, `@testing-library/jest-dom`, `jsdom`)
- Create: `vitest.config.ts`

- [ ] **Step 1: Install test dependencies**

Run:
```bash
pnpm add -D vitest @testing-library/svelte @testing-library/jest-dom jsdom
```

- [ ] **Step 2: Create Vitest config**

Create `vitest.config.ts`:
```ts
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte({ hot: false })],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./vitest.setup.ts'],
  },
});
```

Create `vitest.setup.ts`:
```ts
import '@testing-library/jest-dom/vitest';
```

Add a `test` script to `package.json` scripts block:
```json
"test": "vitest run",
"test:watch": "vitest"
```

- [ ] **Step 3: Write the failing layout test**

Create `src/App.test.ts`:
```ts
import { render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import App from './App.svelte';

describe('App layout', () => {
  it('renders sidebar, center, and terminal panes', () => {
    const { getByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-center')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
  });

  it('arranges panes left-to-right via CSS grid', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    expect(getComputedStyle(layout).display).toBe('grid');
  });
});
```

- [ ] **Step 4: Run the test, confirm it fails**

Run:
```bash
pnpm test
```
Expected: FAIL — `App` still renders the stock template; no `pane-*` test IDs.

- [ ] **Step 5: Create the Pane component**

Create `src/lib/Pane.svelte`:
```svelte
<script lang="ts">
  export let id: 'sidebar' | 'center' | 'terminal';
  export let title: string = '';
  export let empty: string = '';
</script>

<section data-testid="pane-{id}" class="pane pane-{id}">
  {#if title}
    <header class="pane-header">{title}</header>
  {/if}
  <div class="pane-body">
    <slot>
      {#if empty}
        <p class="empty">{empty}</p>
      {/if}
    </slot>
  </div>
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg-pane);
    color: var(--fg);
    border-right: 1px solid var(--border);
  }
  .pane-terminal { border-right: none; }
  .pane-header {
    padding: 0.5rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
  }
  .pane-body {
    flex: 1;
    overflow: auto;
    padding: 0.75rem;
  }
  .empty {
    color: var(--fg-muted);
    font-size: 0.9rem;
  }
</style>
```

- [ ] **Step 6: Create global CSS with design tokens**

Create `src/app.css`:
```css
:root {
  --bg: #ffffff;
  --bg-pane: #fafafa;
  --fg: #1a1a1a;
  --fg-muted: #6b6b6b;
  --border: #e5e5e5;
  --accent: #2563eb;

  --sidebar-w: 25%;
  --center-w: 30%;
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0f0f0f;
    --bg-pane: #161616;
    --fg: #ededed;
    --fg-muted: #999999;
    --border: #262626;
    --accent: #60a5fa;
  }
}

:root[data-theme='light'] {
  --bg: #ffffff;
  --bg-pane: #fafafa;
  --fg: #1a1a1a;
  --fg-muted: #6b6b6b;
  --border: #e5e5e5;
  --accent: #2563eb;
}

:root[data-theme='dark'] {
  --bg: #0f0f0f;
  --bg-pane: #161616;
  --fg: #ededed;
  --fg-muted: #999999;
  --border: #262626;
  --accent: #60a5fa;
}

html, body, #app {
  margin: 0;
  padding: 0;
  height: 100%;
  background: var(--bg);
  color: var(--fg);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  font-size: 14px;
}
```

- [ ] **Step 7: Replace App.svelte with the 3-pane layout**

Overwrite `src/App.svelte`:
```svelte
<script lang="ts">
  import Pane from './lib/Pane.svelte';
</script>

<main class="layout">
  <Pane id="sidebar" title="Projects" empty="No projects yet" />
  <Pane id="center" empty="Pick a session to see details" />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    grid-template-columns: var(--sidebar-w) var(--center-w) 1fr;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
</style>
```

- [ ] **Step 8: Wire app.css into the Svelte entry**

Modify `src/main.ts` so it imports the new stylesheet. Replace its body with:
```ts
import App from './App.svelte';
import './app.css';
import { mount } from 'svelte';

const app = mount(App, { target: document.getElementById('app')! });
export default app;
```

- [ ] **Step 9: Run the test, confirm it passes**

Run:
```bash
pnpm test
```
Expected: PASS — both assertions in `App.test.ts` succeed.

- [ ] **Step 10: Smoke-test in the dev window**

Run:
```bash
pnpm tauri dev
```
Expected: the window shows three vertical panes — left ~25% labeled "Projects" with "No projects yet", center ~30% with "Pick a session to see details", right ~45% with "No terminal attached". Background respects your OS theme.

Close the window.

- [ ] **Step 11: Commit**

Run:
```bash
git add -A
git commit -m "feat(ui): replace stock template with 3-pane layout and design tokens"
```

---

## Task 3: Resizable columns via drag handles

**Files:**
- Create: `src/lib/Resizer.svelte`
- Modify: `src/App.svelte`
- Create: `src/lib/Resizer.test.ts`

- [ ] **Step 1: Write the failing resizer test**

Create `src/lib/Resizer.test.ts`:
```ts
import { fireEvent, render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import Resizer from './Resizer.svelte';

describe('Resizer', () => {
  it('emits a "resize" event with the new pixel offset on pointer drag', async () => {
    const { getByTestId, component } = render(Resizer, { props: { id: 'a' } });
    const handle = getByTestId('resizer-a');

    let lastDelta: number | null = null;
    component.$on('resize', (e: CustomEvent<number>) => { lastDelta = e.detail; });

    await fireEvent.pointerDown(handle, { clientX: 100, pointerId: 1 });
    await fireEvent.pointerMove(window, { clientX: 150, pointerId: 1 });
    await fireEvent.pointerUp(window, { clientX: 150, pointerId: 1 });

    expect(lastDelta).toBe(50);
  });
});
```

- [ ] **Step 2: Run the test, confirm it fails**

Run:
```bash
pnpm test
```
Expected: FAIL — `Resizer.svelte` does not exist.

- [ ] **Step 3: Implement the Resizer**

Create `src/lib/Resizer.svelte`:
```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let id: string;

  const dispatch = createEventDispatcher<{ resize: number }>();

  let startX = 0;
  let dragging = false;

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    startX = e.clientX;
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp, { once: true });
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    dispatch('resize', e.clientX - startX);
  }

  function onPointerUp() {
    dragging = false;
    window.removeEventListener('pointermove', onPointerMove);
  }
</script>

<div
  data-testid="resizer-{id}"
  class="resizer"
  role="separator"
  aria-orientation="vertical"
  on:pointerdown={onPointerDown}
></div>

<style>
  .resizer {
    width: 4px;
    cursor: col-resize;
    background: var(--border);
  }
  .resizer:hover { background: var(--accent); }
</style>
```

- [ ] **Step 4: Run the test, confirm it passes**

Run:
```bash
pnpm test
```
Expected: PASS.

- [ ] **Step 5: Wire two resizers into App.svelte**

Overwrite `src/App.svelte`:
```svelte
<script lang="ts">
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';

  let sidebarPx = 280;
  let centerPx = 360;

  function onResizeSidebar(e: CustomEvent<number>) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + e.detail));
  }
  function onResizeCenter(e: CustomEvent<number>) {
    centerPx = Math.max(220, Math.min(800, centerPx + e.detail));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="Projects" empty="No projects yet" />
  <Resizer id="sidebar" on:resize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" on:resize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
</style>
```

- [ ] **Step 6: Re-run the layout test**

Run:
```bash
pnpm test
```
Expected: PASS — the original `App.test.ts` still passes (the test IDs are unchanged).

- [ ] **Step 7: Smoke-test the drag in dev mode**

Run:
```bash
pnpm tauri dev
```
Expected: dragging the 4px stripes between panes resizes the columns smoothly. Limits clamp at 180/640 for sidebar and 220/800 for center.

- [ ] **Step 8: Commit**

Run:
```bash
git add -A
git commit -m "feat(ui): drag-to-resize sidebar and center panes"
```

---

## Task 4: Theme store (auto / light / dark)

**Files:**
- Create: `src/lib/theme.ts`
- Create: `src/lib/theme.test.ts`
- Modify: `src/App.svelte` (add toggle button in sidebar header)
- Modify: `src/main.ts` (initialize theme on boot)

- [ ] **Step 1: Write the failing theme-store test**

Create `src/lib/theme.test.ts`:
```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { theme, applyTheme, cycleTheme } from './theme';

describe('theme store', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  it('defaults to "auto"', () => {
    expect(get(theme)).toBe('auto');
  });

  it('applyTheme("light") sets data-theme on <html>', () => {
    applyTheme('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    expect(get(theme)).toBe('light');
  });

  it('applyTheme("auto") removes data-theme', () => {
    applyTheme('dark');
    applyTheme('auto');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('cycleTheme moves auto → light → dark → auto', () => {
    expect(get(theme)).toBe('auto');
    cycleTheme();
    expect(get(theme)).toBe('light');
    cycleTheme();
    expect(get(theme)).toBe('dark');
    cycleTheme();
    expect(get(theme)).toBe('auto');
  });

  it('persists the choice to localStorage', () => {
    applyTheme('dark');
    expect(localStorage.getItem('cf:theme')).toBe('dark');
  });
});
```

- [ ] **Step 2: Run the test, confirm it fails**

Run:
```bash
pnpm test
```
Expected: FAIL — `./theme` does not exist.

- [ ] **Step 3: Implement the theme store**

Create `src/lib/theme.ts`:
```ts
import { writable, get } from 'svelte/store';

export type Theme = 'auto' | 'light' | 'dark';
const KEY = 'cf:theme';
const ORDER: Theme[] = ['auto', 'light', 'dark'];

function read(): Theme {
  const v = localStorage.getItem(KEY);
  return v === 'light' || v === 'dark' || v === 'auto' ? v : 'auto';
}

export const theme = writable<Theme>(read());

export function applyTheme(next: Theme): void {
  theme.set(next);
  localStorage.setItem(KEY, next);
  if (next === 'auto') {
    document.documentElement.removeAttribute('data-theme');
  } else {
    document.documentElement.setAttribute('data-theme', next);
  }
}

export function cycleTheme(): void {
  const current = get(theme);
  const idx = ORDER.indexOf(current);
  applyTheme(ORDER[(idx + 1) % ORDER.length]);
}

export function initTheme(): void {
  applyTheme(read());
}
```

- [ ] **Step 4: Run the test, confirm it passes**

Run:
```bash
pnpm test
```
Expected: PASS — all five assertions.

- [ ] **Step 5: Call initTheme on app boot**

Modify `src/main.ts`:
```ts
import App from './App.svelte';
import './app.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';

initTheme();
const app = mount(App, { target: document.getElementById('app')! });
export default app;
```

- [ ] **Step 6: Add a toggle button to App.svelte**

Modify `src/App.svelte` — at the top of the sidebar pane, render a header with a toggle. Replace the file with:
```svelte
<script lang="ts">
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { theme, cycleTheme } from './lib/theme';

  let sidebarPx = 280;
  let centerPx = 360;

  function onResizeSidebar(e: CustomEvent<number>) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + e.detail));
  }
  function onResizeCenter(e: CustomEvent<number>) {
    centerPx = Math.max(220, Math.min(800, centerPx + e.detail));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="claude-fleet" empty="No projects yet">
    <button class="theme-toggle" on:click={cycleTheme} title="Theme: {$theme}">
      theme: {$theme}
    </button>
  </Pane>
  <Resizer id="sidebar" on:resize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" on:resize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
  .theme-toggle {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }
</style>
```

- [ ] **Step 7: Run all tests**

Run:
```bash
pnpm test
```
Expected: PASS for App.test, Resizer.test, theme.test.

- [ ] **Step 8: Smoke-test theme cycling**

Run:
```bash
pnpm tauri dev
```
Expected: clicking the "theme: auto" button cycles to light, then dark, then back to auto. Background and text colors update each click. Reloading the window remembers the last choice.

- [ ] **Step 9: Commit**

Run:
```bash
git add -A
git commit -m "feat(ui): theme store with auto/light/dark cycling and localStorage persistence"
```

---

## Task 5: SQLite store and migrations

**Files:**
- Create: `src-tauri/migrations/001_init.sql`
- Create: `src-tauri/src/store.rs`
- Modify: `src-tauri/Cargo.toml` (add `rusqlite`)
- Modify: `src-tauri/src/main.rs` (declare `mod store;`)

- [ ] **Step 1: Add rusqlite to Cargo.toml**

Modify `src-tauri/Cargo.toml` — add to the `[dependencies]` table:
```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

(`bundled` ships the SQLite C library so users don't need a system SQLite.)

- [ ] **Step 2: Write the migration SQL**

Create `src-tauri/migrations/001_init.sql`:
```sql
CREATE TABLE IF NOT EXISTS hosts (
  alias            TEXT PRIMARY KEY,
  last_pinged_at   INTEGER,
  reachable        INTEGER NOT NULL DEFAULT 0,
  claude_version   TEXT,
  tmux_version     TEXT,
  hidden           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS projects (
  id               INTEGER PRIMARY KEY,
  owner            TEXT NOT NULL,
  repo             TEXT NOT NULL,
  base_path        TEXT NOT NULL,
  last_session_at  INTEGER,
  UNIQUE (owner, repo)
);

CREATE TABLE IF NOT EXISTS worktrees (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id),
  name         TEXT NOT NULL,
  path         TEXT NOT NULL,
  branch       TEXT,
  UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS sessions (
  id                  INTEGER PRIMARY KEY,
  tmux_name           TEXT NOT NULL,
  host_alias          TEXT NOT NULL REFERENCES hosts(alias),
  project_id          INTEGER REFERENCES projects(id),
  worktree_id         INTEGER REFERENCES worktrees(id),
  created_at          INTEGER NOT NULL,
  last_activity_at    INTEGER NOT NULL,
  status              TEXT NOT NULL,
  frozen_scrollback   TEXT,
  notes               TEXT,
  UNIQUE (host_alias, tmux_name)
);

CREATE TABLE IF NOT EXISTS handoffs (
  id            INTEGER PRIMARY KEY,
  session_id    INTEGER NOT NULL REFERENCES sessions(id),
  from_host     TEXT NOT NULL,
  to_host       TEXT NOT NULL,
  mode          TEXT NOT NULL,
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  status        TEXT NOT NULL,
  error         TEXT
);

CREATE TABLE IF NOT EXISTS settings (
  key    TEXT PRIMARY KEY,
  value  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER PRIMARY KEY
);

INSERT OR IGNORE INTO schema_version (version) VALUES (1);
```

- [ ] **Step 3: Write the failing store test**

Create `src-tauri/src/store.rs`:
```rust
use rusqlite::{Connection, Result};

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_tables() -> Vec<&'static str> {
        vec![
            "hosts", "projects", "worktrees", "sessions",
            "handoffs", "settings", "schema_version",
        ]
    }

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in expected_tables() {
            let count: i64 = store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |row| row.get(0),
                )
                .expect("query");
            assert_eq!(count, 1, "missing table: {t}");
        }
    }

    #[test]
    fn schema_version_is_one() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(store.schema_version().expect("version"), 1);
    }
}
```

- [ ] **Step 4: Declare the module in main.rs**

Modify `src-tauri/src/main.rs` — add at the top, after the existing prelude/attribute lines, ensuring it stays compatible with the scaffolded content:
```rust
mod store;
```

(If `main.rs` was renamed to `lib.rs` by the template, do this in `lib.rs` instead. Leave the existing `tauri::Builder` block intact for now.)

- [ ] **Step 5: Run the Rust tests, confirm they pass**

Run:
```bash
cd src-tauri
cargo test
```
Expected: 3 tests in `store::tests` all PASS. (`bundled` triggers a one-time SQLite C build — first run is slow.)

- [ ] **Step 6: Commit**

Run (from repo root):
```bash
cd ..
git add -A
git commit -m "feat(store): SQLite schema and migrations with in-memory tests"
```

---

## Task 6: Smoke-test Tauri command (`health_check`)

**Files:**
- Create: `src-tauri/src/commands/mod.rs`
- Create: `src-tauri/src/commands/health.rs`
- Modify: `src-tauri/src/main.rs` (register the command)
- Create: `src/lib/ipc.ts`
- Modify: `src/App.svelte` (call command on mount, show result in footer)
- Create: `src/lib/ipc.test.ts`

- [ ] **Step 1: Write the Rust command and test**

Create `src-tauri/src/commands/mod.rs`:
```rust
pub mod health;
```

Create `src-tauri/src/commands/health.rs`:
```rust
use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
}

#[tauri::command]
pub fn health_check() -> Health {
    let store_ok = crate::store::Store::open_in_memory().is_ok();
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ready: store_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_check_reports_version_and_db_ready() {
        let h = health_check();
        assert_eq!(h.version, env!("CARGO_PKG_VERSION"));
        assert!(h.db_ready);
    }
}
```

- [ ] **Step 2: Register the command**

Modify `src-tauri/src/main.rs` (or `lib.rs`) — declare the module and register the command. The relevant region should look like:
```rust
mod store;
mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![commands::health::health_check])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

(If the scaffold has a `greet` command in there, remove it — we don't need the template's example.)

If `main.rs` is the entry (not `lib.rs`), keep `fn main()` calling `run()`.

- [ ] **Step 3: Run the Rust test, confirm it passes**

Run:
```bash
cd src-tauri
cargo test
```
Expected: 4 tests pass (3 from store + 1 from health).

- [ ] **Step 4: Build the typed IPC wrapper**

Create `src/lib/ipc.ts`:
```ts
import { invoke } from '@tauri-apps/api/core';

export interface Health {
  version: string;
  db_ready: boolean;
}

export async function healthCheck(): Promise<Health> {
  return invoke<Health>('health_check');
}
```

- [ ] **Step 5: Write the failing frontend IPC test**

Create `src/lib/ipc.test.ts`:
```ts
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.1.0', db_ready: true };
    throw new Error('unexpected command');
  }),
}));

import { healthCheck } from './ipc';

describe('ipc.healthCheck', () => {
  it('returns version and db_ready from the backend', async () => {
    const h = await healthCheck();
    expect(h.version).toBe('0.1.0');
    expect(h.db_ready).toBe(true);
  });
});
```

- [ ] **Step 6: Run the test, confirm it passes**

Run (from repo root):
```bash
cd ..
pnpm test
```
Expected: PASS — the mock satisfies the wrapper.

- [ ] **Step 7: Display the health result in the UI footer**

Modify `src/App.svelte` to call `healthCheck()` on mount and surface the result. Replace its `<script>` and add a footer:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { theme, cycleTheme } from './lib/theme';
  import { healthCheck, type Health } from './lib/ipc';

  let sidebarPx = 280;
  let centerPx = 360;
  let health: Health | null = null;
  let healthError: string | null = null;

  onMount(async () => {
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = String(e);
    }
  });

  function onResizeSidebar(e: CustomEvent<number>) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + e.detail));
  }
  function onResizeCenter(e: CustomEvent<number>) {
    centerPx = Math.max(220, Math.min(800, centerPx + e.detail));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="claude-fleet" empty="No projects yet">
    <button class="theme-toggle" on:click={cycleTheme} title="Theme: {$theme}">
      theme: {$theme}
    </button>
  </Pane>
  <Resizer id="sidebar" on:resize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" on:resize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<footer class="status">
  {#if healthError}
    <span class="err">ipc error: {healthError}</span>
  {:else if health}
    <span>v{health.version} · db: {health.db_ready ? 'ok' : 'fail'}</span>
  {:else}
    <span class="muted">connecting…</span>
  {/if}
</footer>

<style>
  .layout {
    display: grid;
    height: calc(100vh - 24px);
    width: 100vw;
    background: var(--bg);
  }
  .status {
    height: 24px;
    line-height: 24px;
    padding: 0 0.75rem;
    background: var(--bg-pane);
    border-top: 1px solid var(--border);
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .status .err { color: #e64a4a; }
  .theme-toggle {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }
</style>
```

- [ ] **Step 8: Smoke-test the live IPC**

Run:
```bash
pnpm tauri dev
```
Expected: the footer reads `v0.1.0 · db: ok` (or whatever your `CARGO_PKG_VERSION` is). The 3-pane layout is intact above.

- [ ] **Step 9: Commit**

Run:
```bash
git add -A
git commit -m "feat(ipc): health_check command end-to-end with footer status"
```

---

## Task 7: GitHub Actions CI

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the CI workflow**

Create `.github/workflows/ci.yml`:
```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  rust:
    runs-on: macos-latest
    defaults:
      run:
        working-directory: src-tauri
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test

  frontend:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'pnpm'
      - run: pnpm install --frozen-lockfile
      - run: pnpm run test
      - run: pnpm run build
```

- [ ] **Step 2: Run the same checks locally to make sure they pass**

Run (from repo root):
```bash
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
pnpm install --frozen-lockfile
pnpm run test
pnpm run build
```
Expected: all six commands exit 0. If `cargo fmt --check` complains, run `cargo fmt` and re-check. If clippy complains, fix lints (most likely unused imports from the template).

- [ ] **Step 3: Commit**

Run:
```bash
git add -A
git commit -m "ci: rust + frontend checks on push and PR"
```

---

## Task 8: README

**Files:**
- Modify: `README.md` (overwrite the stock scaffold readme)

- [ ] **Step 1: Write the README**

Overwrite `README.md`:
````markdown
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
````

- [ ] **Step 2: Verify it renders**

Run:
```bash
cat README.md | head -30
```
(Just to eyeball it.)

- [ ] **Step 3: Commit**

Run:
```bash
git add README.md
git commit -m "docs: project README with build/test/run instructions"
```

---

## Task 9: Exit-criteria verification

**Files:** none (verification only)

- [ ] **Step 1: Clean build from scratch**

Run:
```bash
rm -rf node_modules src-tauri/target
pnpm install --frozen-lockfile
(cd src-tauri && cargo build)
```
Expected: both succeed, no warnings beyond Tauri scaffold defaults.

- [ ] **Step 2: All tests green**

Run:
```bash
pnpm test
(cd src-tauri && cargo test)
```
Expected: frontend = 4+ passing (App layout x2, Resizer, theme x5, ipc). Backend = 4+ passing (store x3, health x1).

- [ ] **Step 3: Lint clean**

Run:
```bash
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings)
pnpm run build
```
Expected: all clean.

- [ ] **Step 4: Manual UI smoke test**

Run:
```bash
pnpm tauri dev
```

Verify:
- Window opens with a 3-pane layout (sidebar + center + terminal).
- Sidebar shows "claude-fleet" header with a `theme: auto` button.
- Center shows "Pick a session to see details".
- Terminal pane shows "No terminal attached".
- Status footer shows `v0.1.0 · db: ok`.
- Dragging the stripe between sidebar/center resizes the columns.
- Dragging the stripe between center/terminal resizes the columns.
- Clicking `theme: auto` cycles to `light`, then `dark`, then back. Colors change accordingly. Reloading remembers the choice.

If anything fails, do **not** mark the phase complete — open a follow-up task.

- [ ] **Step 5: Push the branch and verify CI**

If you have a GitHub remote configured (`gh repo create martin-janci/claude-fleet --private --source=. --remote=origin --push` if not):
```bash
git push -u origin main
```
Then open the Actions tab on GitHub and confirm both jobs (`rust` and `frontend`) are green.

- [ ] **Step 6: Mark Phase 1 done**

Run:
```bash
git tag v0.1.0-phase1
git push --tags
```
Commit (no-op tag, but useful as a marker).

---

## Spec coverage check (self-review)

Mapping spec requirements to tasks:

| Spec section | Requirement | Covered by |
|---|---|---|
| §5.1 | Tauri 2 + Svelte stack | Task 1 |
| §5.3 | Rust module skeleton (store, commands) | Task 5, Task 6 |
| §5.4 | Svelte module skeleton (App, lib/Pane, lib/Resizer, lib/theme) | Tasks 2, 3, 4, 6 |
| §6 | SQLite schema (7 tables) | Task 5 |
| §7.1 | 3-pane resizable layout | Tasks 2, 3 |
| §7.1 | Empty states in each pane | Task 2 |
| §7.2 | Status bar footer | Task 6 (basic version with health) |
| §9 Phase 1 | Theme support, follows OS | Task 4 |
| §9 Phase 1 | CI: cargo check, clippy, pnpm build | Task 7 |
| §9 Phase 1 | README | Task 8 |
| §9 Phase 1 | Exit criteria | Task 9 |

**Gaps:** Status bar shows only version + db state in Phase 1; reachability dots and session count come in Phases 4–5. This is consistent with the spec's phasing.

**Placeholder scan:** No `TBD` / `TODO` / `appropriate error handling` strings in tasks. Every code step has actual code.

**Type consistency:** `Health { version: string, db_ready: bool }` defined in `commands/health.rs` matches `Health` interface in `lib/ipc.ts` matches the mock in `lib/ipc.test.ts`. `Theme = 'auto' | 'light' | 'dark'` and `cycleTheme()` / `applyTheme()` / `initTheme()` are consistent across `lib/theme.ts`, the test, and `main.ts`. `Store::open` / `Store::open_in_memory` / `Store::schema_version` signatures are stable across tasks.

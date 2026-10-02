# F3c — Codex skills move to `~/.agents/skills` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The asset catalog renders Codex skills where Codex actually reads them, `~/.agents/skills/<install name>/`, moves every skill fleet already synced to `~/.codex/skills/` there on the next sync without touching anything fleet does not own, and never writes Codex skills through a symlinked skills directory.

**Architecture:** `CODEX_SKILLS_DIR` becomes `~/.agents/skills`. The Codex scan hashes the new directory and keeps hashing the old one (minus Codex's own `.system`) only so that the existing planner path — a manifest entry whose files the render no longer produces rides along as `remove_entry` (plan rule 8) — can delete the old copy under compare-and-swap. `installed()` lists skills from the new directory only. The scan also prints `##LINK <path>` followed by its target on the next line for a symlinked `~/.agents`, `~/.agents/skills`, entry in it, or `~/.codex/skills` (`HostSnapshot::links`); `compute_host_plan` blocks every action that would write, adopt or delete under one, with the reason from a new `Harness::symlink_reason`. Rule 4 gains one refinement — a moved asset whose new location holds a copy fleet never wrote is an `Overwrite`, not an `Update` — and a pure `plan::block_cross_harness_collisions`, called once per host in `plan_sync`, blocks any action whose file another harness's plan on that host also touches.

**Tech Stack:** Rust (fleet-core), POSIX sh scan scripts, `tempfile` + real local scans for end-to-end tests.

**Spec:** `docs/superpowers/specs/2026-09-29-multi-harness-agents-design.md` — §5.2 item 4 ("Shared skills dir", the Codex part) and roadmap **F3**. F3a/F3b (harness set, Codex subagents) are already on `main` (`docs/superpowers/plans/2026-09-30-f3ab-harness-set-and-codex-agents.md`). This plan is **F3c** only.

## Global Constraints

- **Build environment:** every local cargo command runs with `export CARGO_TARGET_DIR=<shared-target-dir>`. A fleet-core rebuild is slow on a shared machine, so each task makes ALL its edits first and runs only its targeted test filters (libtest takes several filters, OR-ed); one whole-crate run happens in Task 6. When the `mercury-run` skill/CLI is available, long runs (the whole-crate suite, clippy) may go to the remote Linux box instead: `mercury-run cargo test -p fleet-core` from the repo root snapshots the working tree, uncommitted changes included.
- **Known non-regressions:** `service::rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` fails in deep scratch directories (unix socket path too long) — not ours; report it, do not fix it. `store::schema::tests_upgrade` and `service::work::scale_tests` are load-sensitive timing tests: re-run a failure in isolation (`cargo test -p fleet-core --lib <test path>`) before calling it a regression; anything else that fails must also be checked against `origin/main` before it is called pre-existing.
- **Codex facts (binding):** Codex CLI reads user skills from `~/.agents/skills` (also `.agents/skills` inside repos and `/etc/codex/skills`), NOT from `~/.codex/skills`, where Codex keeps only its own built-ins (`~/.codex/skills/.system`). Fleet writes only the user-level `~/.agents/skills`; it never touches a repo's `.agents/skills` or `/etc/codex/skills`.
- **Safety (binding):** fleet deletes only paths its own manifest lists. Nothing under `~/.codex/skills/.system` and nothing absent from fleet's manifest is ever written or removed. No Codex action ever writes, adopts or deletes through a symlinked `~/.agents`, `~/.agents/skills`, entry of `~/.agents/skills`, or `~/.codex/skills`. Claude's plans are unaffected by both guards.
- **Existing planner rules stay:** an unmanaged same-name copy in `~/.agents/skills` is `Adopt` when identical and `Overwrite("present but differs; not managed")` otherwise — never an automatic `Update`.
- **Parallel initiative** (assets-workspace S1b) also edits `sync::plan_sync`. Keep that diff to: one `let first_plan = host_plans.len();` before the per-harness loop, one `plan::block_cross_harness_collisions(&mut host_plans[first_plan..]);` after it, plus one comment line. Everything else lives in `sync/plan.rs`, `harness/*.rs` and tests.
- Scan scripts are POSIX sh with no single quote (the caller wraps the whole script in `shell::quote`); every path in them is double-quoted.
- Host output (`##LINK` targets) is untrusted: it is only ever shown in a reason string, never interpolated into a script; control characters are dropped and it is capped at 256 characters.
- Every child process via `fleet_core::proc::command` / `std_command`; no `eprintln!`/`println!`/`dbg!` in production code (use `tracing`). Tests may use `std::process::Command` as the existing scan tests do.
- No migration, MCP tool, tool description, `CatalogAdminParams`, hub contract field or frontend change in this plan, so no `REGEN_*` run is needed. `HostSnapshot` is not on the wire (it is `#[serde(skip)]` in `HostPlan`).
- Public repo: no personal paths in code, tests or docs — tests use temp dirs and synthetic paths like `/home/u/.claude/skills`.
- Every existing test assertion stays. The only existing tests this plan changes are named in their task, with the reason: in `harness/codex.rs` `skill_renders_to_codex_skills_dir` (renamed + new path), `skill_renders_under_install_as`, `hooks_and_plugins_are_unsupported_and_render_as_skill_still_wins` and `agent_as_skill_renders_under_install_as` (new path), and the `scan_fixture` helper (its skill line moves to `.agents/skills`); in `author.rs` `lint_errors_when_a_codex_skill_agent_shares_a_skills_install_name` (message names the new directory) — all Task 1. `codex::tests::scan_script_runs_under_bash_and_parses_cleanly` and `sync::tests::plan_sync_retires_codex_when_turned_off_but_still_managed` deliberately stay unchanged: the legacy directory is still hashed, and a manifest entry under it is still removable.
- Commits: Conventional Commits, no attribution lines. Work on branch `feat/codex-agents-skills-dir`, never on `main`.

## Decisions and findings this plan rests on

1. **The migration already works through rule 8 — with one prerequisite and one gap.** Reading `sync/plan.rs` and `sync/apply.rs`: a manifest entry `skill/s → ["~/.codex/skills/s/SKILL.md"]` against a render of `~/.agents/skills/s/SKILL.md` plans, by what the new location holds:
   - nothing there (the normal case) ⇒ **`Create`**, not `Update` (rule 4: "not present ⇒ Create"), with `remove_entry` = the old entry (rule 8). `collect_file_writes` writes the new file and queues a `delete_write` for every `remove_entry` file the render no longer produces; `build_manifest` replaces the entry with the new paths. Only manifest-listed paths are deleted.
   - an identical copy there ⇒ `Update` via `has_stale_locations`, same deletion.
   - a *different* copy there ⇒ **`Update`** today (manifest names the asset, hash differs) — a silent overwrite of a copy fleet never wrote. **Gap**, fixed in Task 4: when the asset moved (`has_stale_locations`) and a planned file the entry does not list already exists, it is `Overwrite` with a reason.
   - **Prerequisite:** `delete_write` takes its compare-and-swap hash from `plan.snapshot`. If the scan stopped hashing `~/.codex/skills`, the old file would read as absent, the delete would report `CONFLICT`, the action would never be recorded, and every later sync would conflict again. So the scan keeps hashing `~/.codex/skills` (Task 1).
   A `Create` that also deletes an old copy shows only the new path in `files`, so it now carries a reason (`MOVED_CREATE_REASON`), and the existing `Update` reason is generalised from "identifier" to "location" (Task 4). The deletion backs the old file up (`delete_write` sets `backup: true`), so `~/.codex/skills/<name>/SKILL.md.fleet-bak-*` remains — invisible to Codex, documented.
2. **Legacy unmanaged skills in `~/.codex/skills` are not listed.** `installed()`/`installed_detail` read `~/.agents/skills` only. A skill left in `~/.codex/skills` is invisible to Codex, so listing it as Codex inventory would claim Codex has something it does not load, and it would fold into identity/host-set signatures as a Codex copy. Fleet-managed ones migrate automatically (and an orphaned one still shows as `orphan`, which comes from the manifest, not `installed()`); hand-made ones are documented: move them to `~/.agents/skills/` and they show up as unmanaged (or adopt). The legacy scan prunes `.codex/skills/.system`, which also stops today's bogus unmanaged row named `.system`.
3. **Symlink detection.** The Codex scan's link probe (`CODEX_LINK_PROBE`) walks `.agents`, `.agents/skills`, each entry of `.agents/skills` (including dot-entries), `.codex/skills` and each entry of `.codex/skills` (including dot-entries, but never `.codex/skills/.system`), right after the presence probe; for each one that is a symlink it prints two lines, like a `##CONFIG` block: `##LINK <path>` (the `~/`-relative path) then `readlink`'s output on its own line as the target — not `##LINK <path> -> <target>` on one line, since a target itself could contain `" -> "`. The parser keeps only `~/` paths and sanitises the target (control characters dropped, capped at 256 characters; an empty line reads as "an unknown target"). Included beyond the brief's two paths: each entry of `~/.agents/skills` (a per-skill link into dotfiles has the same hazard at a finer grain; the generic prefix check makes it free) and `~/.codex/skills` (the migration now deletes there — if it points at `~/.claude/skills` the "old copy" *is* Claude's file). The guard is harness-agnostic (`HostSnapshot::links` + prefix match in `compute_host_plan`, compared case-insensitively since a case-insensitive filesystem can report `readlink`'s path in different case than the catalog's rendered path); the wording is per harness (`Harness::symlink_reason`, default text + Codex override, whose legacy-dir advice omits "turn Codex off" because a retiring host still runs those removals, and whose removal wording differs from its write/adopt wording). `Adopt` is blocked too: a plain skill renders byte-identically for Claude and Codex, so through the link Codex would adopt Claude's file and two manifests would claim one path. `Noop` and already-`Blocked` actions are left alone. Claude is unaffected: its scan reports no links, and it never writes under those directories.
4. **Cross-harness collision guard placement.** A pure `plan::block_cross_harness_collisions(&mut [HostPlan])` over one host's plans, called once per host in `plan_sync` right after its per-harness loop (two lines). A path is claimed by every non-`Blocked` action that names it in `files` or in its `remove_entry` (so `Noop`s claim what they manage); every non-`Noop`, non-`Blocked` action on a path another harness also claims becomes `Blocked`. Files only — config merges are per-harness files by design today; F6 must revisit if two harnesses ever merge into one config. Today Claude (`~/.claude/…`) and Codex (`~/.agents/…`, `~/.codex/…`) never share a path string, so this never fires; it is the safety net for F6 harnesses that will also render into `~/.agents/skills` (they must share one manifest owner first, or the guard blocks both sides loudly instead of letting them thrash).
5. **Out of scope:** spec §5.2 item 4's import half ("Import learns `~/.agents/skills` and `~/.codex`") — `import_assets` is Claude-only today; it belongs with F3d/F4. Removing the empty `~/.codex/skills/<name>/` directory left beside its backup. Other harnesses sharing `~/.agents/skills` (F6).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/catalog/harness/codex.rs` | `CODEX_SKILLS_DIR = "~/.agents/skills"`, `CODEX_LEGACY_SKILLS_DIR`, scan hashes new + legacy (minus `.system`), `##LINK` probe, `installed` from the new dir only, `symlink_reason` override; tests |
| `crates/fleet-core/src/service/catalog/harness/mod.rs` | `HostSnapshot::links`, `##LINK` parsing (`link_target`), `Harness::symlink_reason` default; tests |
| `crates/fleet-core/src/service/catalog/sync/plan.rs` | `block_symlinked` (called from `compute_host_plan`), rule-4 refinement + reason consts, `holds_unlisted_copy`, `touched_paths`, `block`, `linked_dir`, `block_cross_harness_collisions`; tests |
| `crates/fleet-core/src/service/catalog/sync/mod.rs` | two lines in `plan_sync` calling the collision guard |
| `crates/fleet-core/src/service/catalog/sync/apply.rs` | two local end-to-end tests (symlink refusal, migration) |
| `crates/fleet-core/src/service/catalog/author.rs` | doc comments name the new dir; one test assertion |
| `docs/concepts.md`, `docs/hub.md`, `docs/superpowers/specs/2026-09-14-asset-catalog-design.md`, `CLAUDE.md` | docs |

---

### Task 1: Codex skills render to and are read from `~/.agents/skills`

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs:23` (const), after `:26` (legacy const), `:350-354` (scan doc), `:368-372` (scan loop), `:415` (comment in `installed`)
- Modify (tests): `codex.rs:562-578`, `:587-588`, `:693`, `:930`, `:1032-1040`; new tests appended before the closing `}` of `mod tests` (line 1248)
- Modify: `crates/fleet-core/src/service/catalog/author.rs:323`, `:400-401`, `:1440-1441`, `:1459`

**Interfaces:**
- Produces:
  - `pub const CODEX_SKILLS_DIR: &str = "~/.agents/skills";`
  - `pub const CODEX_LEGACY_SKILLS_DIR: &str = "~/.codex/skills";`
  - Codex scan hashes `.agents/skills`, `.codex/skills` (pruning `.codex/skills/.system`) and `.codex/agents`
  - `Codex::installed`/`installed_detail` list skills under `CODEX_SKILLS_DIR` only
  - test helper `codex::tests::scan_home(home: &std::path::Path) -> HostSnapshot` (runs the real scan under `sh -c` with `HOME=home`) — reused by Task 2

- [ ] **Step 1: Write the failing tests (and change the five existing ones)**

In `crates/fleet-core/src/service/catalog/harness/codex.rs` tests:

**Change** `skill_renders_to_codex_skills_dir` (lines 562-578). Reason: F3c moves the render to where Codex reads skills. Replace it whole:

```rust
    #[test]
    fn skill_renders_to_agents_skills_dir() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: worktree\ndescription: Make one.\nallowed_tools: [bash]\n",
        )
        .unwrap();
        a.body = "body\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.agents/skills/worktree/SKILL.md");
        assert_eq!(
            String::from_utf8(plan.files[0].bytes.clone()).unwrap(),
            "---\nname: worktree\ndescription: Make one.\n---\nbody\n"
        );
    }
```

**Change** in `skill_renders_under_install_as` (line 588) only the path assertion:

```rust
        assert_eq!(plan.files[0].path, "~/.agents/skills/foo_bar/SKILL.md");
```

**Change** in `hooks_and_plugins_are_unsupported_and_render_as_skill_still_wins` (line 693) only:

```rust
        assert_eq!(plan.files[0].path, "~/.agents/skills/pm/SKILL.md");
```

**Change** in `agent_as_skill_renders_under_install_as` (line 930) only:

```rust
        assert_eq!(plan.files[0].path, "~/.agents/skills/foo_bar/SKILL.md");
```

**Change** the `scan_fixture` helper (line 1037) so `installed_lists_skill_and_server` still finds `worktree` where Codex now reads it:

```rust
        format!(
            "##HASHES\naaaa  .agents/skills/worktree/SKILL.md\n##CONFIG ~/.codex/config.toml\n{b64}\n##CONFIG ~/.codex/.fleet-assets.json\n##END\n"
        )
```

Append to `mod tests` (before its closing `}`):

```rust
    /// Runs the real scan under plain `sh` against `home`.
    #[cfg(unix)]
    fn scan_home(home: &std::path::Path) -> HostSnapshot {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", home)
            .output()
            .expect("run scan script");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Codex
            .parse_scan(&String::from_utf8(out.stdout).unwrap())
            .unwrap()
    }

    /// F3c: skills are listed only from `~/.agents/skills`, where Codex reads
    /// them. A copy left in `~/.codex/skills` is invisible to Codex, and
    /// Codex's own `.system` is never a skill of the user's.
    #[test]
    fn codex_skills_are_listed_only_from_agents_skills() {
        let mut s = HostSnapshot::default();
        s.files
            .insert("~/.agents/skills/new/SKILL.md".into(), "aa".into());
        s.files
            .insert("~/.codex/skills/old/SKILL.md".into(), "bb".into());
        s.files
            .insert("~/.codex/skills/.system/builtin/SKILL.md".into(), "cc".into());
        assert_eq!(Codex.installed(&s), vec![(Kind::Skill, "new".to_string())]);
        let d = Codex.installed_detail(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].hash.as_deref(),
            Some(sha256_hex(b"SKILL.md=aa").as_str())
        );
    }

    /// F3c: the real scan hashes `~/.agents/skills` and still the legacy
    /// `~/.codex/skills` — a pre-F3c manifest entry there needs its hash for
    /// the compare-and-swap that deletes the old copy — but never Codex's
    /// `.system` built-ins.
    #[cfg(unix)]
    #[test]
    fn scan_hashes_agents_skills_and_the_legacy_dir_but_never_codex_system() {
        let home = tempfile::TempDir::new().unwrap();
        let h = home.path();
        for (rel, body) in [
            (".agents/skills/new/SKILL.md", "n"),
            (".codex/skills/old/SKILL.md", "o"),
            (".codex/skills/.system/builtin/SKILL.md", "b"),
        ] {
            let p = h.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        let snap = scan_home(h);
        assert!(
            snap.files.contains_key("~/.agents/skills/new/SKILL.md"),
            "{:?}",
            snap.files
        );
        assert!(
            snap.files.contains_key("~/.codex/skills/old/SKILL.md"),
            "the legacy copy keeps a hash for its removal: {:?}",
            snap.files
        );
        assert!(
            !snap.files.keys().any(|p| p.contains(".system")),
            "{:?}",
            snap.files
        );
        assert_eq!(Codex.installed(&snap), vec![(Kind::Skill, "new".to_string())]);
    }
```

In `crates/fleet-core/src/service/catalog/author.rs`, **change** in `lint_errors_when_a_codex_skill_agent_shares_a_skills_install_name` (line 1459) only the path in the assertion. Reason: the message interpolates `CODEX_SKILLS_DIR`, which now names `~/.agents/skills`:

```rust
            report.errors[0].message.contains("~/.agents/skills/pm")
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness::codex::tests service::catalog::author::tests::lint_errors_when_a_codex_skill_agent_shares_a_skills_install_name 2>&1 | tail -25
```
Expected: FAIL — `skill_renders_to_agents_skills_dir`, `skill_renders_under_install_as`, `hooks_and_plugins_are_unsupported_and_render_as_skill_still_wins`, `agent_as_skill_renders_under_install_as` (`left: "~/.codex/skills/…"`), `installed_lists_skill_and_server` (no `worktree`), `codex_skills_are_listed_only_from_agents_skills` (lists `old` and `.system` instead of `new`), `scan_hashes_agents_skills_and_the_legacy_dir_but_never_codex_system` (no `.agents` hash) and the author lint test.

- [ ] **Step 3: Implement**

`crates/fleet-core/src/service/catalog/harness/codex.rs` line 23 — replace `pub const CODEX_SKILLS_DIR: &str = "~/.codex/skills";` with:

```rust
/// Where Codex reads user skills (multi-harness F3c): `~/.agents/skills`, the
/// cross-harness skills directory — not `~/.codex/skills`, which Codex keeps
/// for its own built-ins (`.system`).
pub const CODEX_SKILLS_DIR: &str = "~/.agents/skills";
```

After `pub const CODEX_AGENTS_DIR: &str = "~/.codex/agents";` (line 26) add:

```rust
/// Where fleet rendered Codex skills before F3c. Still hashed by the scan
/// (minus Codex's `.system`) so a manifest entry pointing here can be
/// deleted under compare-and-swap when its skill moves to
/// `CODEX_SKILLS_DIR`; never listed as installed, since Codex does not read
/// it.
pub const CODEX_LEGACY_SKILLS_DIR: &str = "~/.codex/skills";
```

Replace the `scan_script` doc comment (lines 350-354) with:

```rust
    /// Same shape as `Claude::scan_script`: hasher detection, the presence
    /// probe, `##HASHES` + file hashes under `.agents/skills`,
    /// `.codex/skills` (legacy, minus `.system`) and `.codex/agents`, a hash
    /// for each config file, then one `##CONFIG <path>` block per config
    /// file (base64, one line), then `##END`. No single quotes: the caller
    /// wraps the whole script in `shell::quote`.
```

Replace the hashing loop (lines 368-372, the comment line and the `s.push_str("for d in .codex/skills .codex/agents; …")` call) with:

```rust
        // `-exec $H {} +` (not `-print0 | xargs -0 $H`): see `Claude::scan_script`
        // for why this matters for an existing-but-empty directory.
        // F3c: `.agents/skills` is where Codex reads skills. `.codex/skills`
        // is where fleet put them before — hashed only so a manifest entry
        // pointing there can be deleted under compare-and-swap; Codex's own
        // `.codex/skills/.system` is pruned, it is never fleet's.
        s.push_str(
            "for d in .agents/skills .codex/skills .codex/agents; do if [ -d \"$d\" ]; then find -L \"$d\" -path .codex/skills/.system -prune -o -type f -exec $H {} + 2>/dev/null; fi; done; ",
        );
```

In `installed` (line 415), directly above `if let Some(rest) = path.strip_prefix(&format!("{CODEX_SKILLS_DIR}/")) {`, add:

```rust
            // F3c: only `CODEX_SKILLS_DIR`. A skill left in
            // `CODEX_LEGACY_SKILLS_DIR` is invisible to Codex, so it is not
            // Codex inventory (fleet's own copies there migrate on sync).
```

`crates/fleet-core/src/service/catalog/author.rs` — three comment edits (the lint's message already interpolates `CODEX_SKILLS_DIR`, so no code changes there). Line 323 becomes:

```rust
/// Whether Codex renders `a` into `~/.agents/skills/` (`CODEX_SKILLS_DIR`): a skill, or an agent
```

Line 401 (the second line of the `// F3b: Codex renders an agent …` comment) becomes:

```rust
    // `~/.agents/skills/<install name>/` (F3c), where a skill of that install name
```

Line 1441 (the second line of the `/// F3b: an agent Codex renders as a skill …` test doc) becomes:

```rust
    /// `~/.agents/skills/<install name>/` as a skill of that install name —
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness::codex::tests service::catalog::author::tests service::catalog::inventory::tests 2>&1 | tail -8
```
Expected: all PASS — including the unchanged `scan_script_runs_under_bash_and_parses_cleanly` (its `.codex/skills/worktree` file is still hashed as legacy), `scan_script_is_home_relative_and_quoted` (no single quote) and `only_codex_own_state_reads_as_present`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness/codex.rs crates/fleet-core/src/service/catalog/author.rs
git commit -m "feat(catalog): render Codex skills to ~/.agents/skills (F3c)"
```

---

### Task 2: The Codex scan reports symlinked skill directories

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/mod.rs:143-155` (`HostSnapshot`), before `:502` (new `link_target`), `:520-524` (parse branch), tests after `:745`
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs` after `:43` (`CODEX_LINK_PROBE`), `scan_script` (push it after `CODEX_PRESENT_PROBE`), tests

**Interfaces:**
- Consumes: `codex::tests::scan_home` (Task 1).
- Produces:
  - `HostSnapshot::links: BTreeMap<String, String>` — `~/`-relative symlinked directory → its sanitised target
  - scan line `##LINK ~/<rel> -> <target>`; `parse_scan_blocks` fills `links`
  - Codex scan probes `.agents`, `.agents/skills`, `.agents/skills/*`, `.codex/skills`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/service/catalog/harness/mod.rs`:

```rust
    /// `##LINK <path> -> <target>` (multi-harness F3c) records a symlinked
    /// directory: a path not under `~/` is ignored, a target loses control
    /// characters, an empty one reads as unknown, and none is a hash line.
    #[test]
    fn parse_scan_blocks_reads_symlinked_dirs() {
        let snap = parse_scan_blocks(
            "##LINK ~/.agents/skills -> /home/u/.claude/skills\n##LINK ~/.codex/skills -> \n##LINK /etc/x -> /y\n##LINK ~/.agents/skills/a b -> ../x\u{7}y\n##HASHES\n##END\n",
            &|_, _| None,
        )
        .unwrap();
        assert_eq!(
            snap.links,
            BTreeMap::from([
                (
                    "~/.agents/skills".to_string(),
                    "/home/u/.claude/skills".to_string()
                ),
                ("~/.agents/skills/a b".to_string(), "../xy".to_string()),
                (
                    "~/.codex/skills".to_string(),
                    "an unknown target".to_string()
                ),
            ])
        );
        assert!(snap.files.is_empty());
    }
```

Append to `mod tests` in `crates/fleet-core/src/service/catalog/harness/codex.rs`:

```rust
    /// F3c: the scan reports a symlinked `~/.agents/skills` with its target
    /// (as `readlink` prints it) and still hashes what it points at — that
    /// is what Codex sees. A real directory reports no link.
    #[cfg(unix)]
    #[test]
    fn scan_reports_a_symlinked_agents_skills_dir() {
        use std::os::unix::fs::symlink;
        let home = tempfile::TempDir::new().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/skills/s")).unwrap();
        std::fs::write(h.join(".claude/skills/s/SKILL.md"), b"claude").unwrap();
        std::fs::create_dir_all(h.join(".agents")).unwrap();
        symlink(h.join(".claude/skills"), h.join(".agents/skills")).unwrap();
        let snap = scan_home(h);
        assert_eq!(
            snap.links,
            std::collections::BTreeMap::from([(
                "~/.agents/skills".to_string(),
                h.join(".claude/skills").to_string_lossy().to_string()
            )])
        );
        assert!(
            snap.files.contains_key("~/.agents/skills/s/SKILL.md"),
            "{:?}",
            snap.files
        );

        let plain = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(plain.path().join(".agents/skills/s")).unwrap();
        assert!(scan_home(plain.path()).links.is_empty());
    }

    /// A whole linked `~/.agents`, one linked skill inside a real
    /// `~/.agents/skills`, and a linked legacy `~/.codex/skills` are each
    /// reported.
    #[cfg(unix)]
    #[test]
    fn scan_reports_a_linked_agents_dir_a_linked_skill_and_a_linked_legacy_dir() {
        use std::os::unix::fs::symlink;
        let whole = tempfile::TempDir::new().unwrap();
        let w = whole.path();
        std::fs::create_dir_all(w.join("dotfiles/agents/skills")).unwrap();
        symlink(w.join("dotfiles/agents"), w.join(".agents")).unwrap();
        let keys: Vec<String> = scan_home(w).links.into_keys().collect();
        assert_eq!(keys, vec!["~/.agents"]);

        let mixed = tempfile::TempDir::new().unwrap();
        let m = mixed.path();
        std::fs::create_dir_all(m.join(".claude/skills/x")).unwrap();
        std::fs::create_dir_all(m.join(".agents/skills")).unwrap();
        std::fs::create_dir_all(m.join(".codex")).unwrap();
        symlink(m.join(".claude/skills/x"), m.join(".agents/skills/x")).unwrap();
        symlink(m.join(".claude/skills"), m.join(".codex/skills")).unwrap();
        let keys: Vec<String> = scan_home(m).links.into_keys().collect();
        assert_eq!(keys, vec!["~/.agents/skills/x", "~/.codex/skills"]);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness::tests service::catalog::harness::codex::tests 2>&1 | tail -20
```
Expected: compile error — `no field links on type HostSnapshot`.

- [ ] **Step 3: Implement**

`crates/fleet-core/src/service/catalog/harness/mod.rs` — in `HostSnapshot`, after `pub present: bool,` (line 154) add:

```rust
    /// Multi-harness F3c: every directory the scan found to be a symlink,
    /// `~/`-relative path → its target as `readlink` printed it (control
    /// characters dropped, at most 256 characters). Only Codex's scan
    /// reports any (`##LINK`); `sync::plan` refuses every write, adopt or
    /// removal under one (`Harness::symlink_reason`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
```

Directly above the `parse_scan_blocks` doc comment (before line 494) add:

```rust
/// A `##LINK` target as a plan reason may show it: host output, so control
/// characters are dropped and it is capped at 256 characters; an empty one
/// (a `readlink` that printed nothing) reads as unknown.
fn link_target(raw: &str) -> String {
    let t: String = raw.chars().filter(|c| !c.is_control()).take(256).collect();
    if t.is_empty() {
        "an unknown target".to_string()
    } else {
        t
    }
}
```

In `parse_scan_blocks`, after the `##PRESENT` branch (lines 520-524) add:

```rust
        if let Some(rest) = line.strip_prefix("##LINK ") {
            current_config = None;
            // `<~/path> -> <target>`: `" -> "` rather than a space, so a
            // name with a space still parses. A path outside `~/` names
            // nothing fleet writes and is dropped.
            if let Some((path, target)) = rest.split_once(" -> ") {
                if path.starts_with("~/") {
                    snap.links.insert(path.to_string(), link_target(target));
                }
            }
            continue;
        }
```

`crates/fleet-core/src/service/catalog/harness/codex.rs` — after `CODEX_PRESENT_PROBE` (line 43) add:

```rust
/// The scan's symlink probe (multi-harness F3c): `~/.agents`,
/// `~/.agents/skills` and each entry in it (where Codex skills are written)
/// and `~/.codex/skills` (where the migration deletes old copies). Some
/// setups point one of them at `~/.claude/skills`; fleet must never write
/// Codex skills through it (`sync::plan`). POSIX sh, no single quote; a
/// glob that matches nothing stays literal and fails `-L`.
const CODEX_LINK_PROBE: &str = "for l in .agents .agents/skills .agents/skills/* .codex/skills; do if [ -L \"$l\" ]; then echo \"##LINK ~/$l -> $(readlink \"$l\")\"; fi; done; ";
```

In `scan_script`, directly after `s.push_str(CODEX_PRESENT_PROBE);` add:

```rust
        s.push_str(CODEX_LINK_PROBE);
```

In the `scan_script` doc comment (as rewritten in Task 1), replace its second line

```rust
    /// probe, `##HASHES` + file hashes under `.agents/skills`,
```

with

```rust
    /// probe, the symlink probe (`CODEX_LINK_PROBE`, `##LINK` lines),
    /// `##HASHES` + file hashes under `.agents/skills`,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::harness::tests service::catalog::harness::codex::tests service::catalog::harness::claude::tests 2>&1 | tail -8
```
Expected: all PASS (`scan_script_is_home_relative_and_quoted` still finds no single quote; `scan_script_probes_for_codex` still finds `##PRESENT` before `##HASHES`).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness/mod.rs crates/fleet-core/src/service/catalog/harness/codex.rs
git commit -m "feat(catalog): Codex scan reports symlinked skill directories (F3c)"
```

---

### Task 3: Never write, adopt or delete through a symlinked directory

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/harness/mod.rs` trait `Harness` (after `fn manifest_path`, line 264)
- Modify: `crates/fleet-core/src/service/catalog/harness/codex.rs` `impl Harness for Codex` (after `fn manifest_path`, lines 482-484)
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs:315-317` (rule 9 in the doc), before `:430` (call), after `:728` (helpers after `has_stale_locations`); tests appended at the end of `mod tests`
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` tests (new e2e after `a_codex_agent_is_written_as_toml_and_removed_locally`, before line 2421)

**Interfaces:**
- Consumes: `HostSnapshot::links` (Task 2), `CODEX_LEGACY_SKILLS_DIR` (Task 1).
- Produces:
  - `Harness::symlink_reason(&self, link: &str, target: &str) -> String` (default method)
  - in `sync::plan` (private unless noted): `fn touched_paths(action: &Action) -> impl Iterator<Item = &String>`, `fn block(action: &mut Action, reason: String)`, `fn linked_dir<'a>(links: &'a BTreeMap<String, String>, path: &str) -> Option<(&'a str, &'a str)>`, `fn block_symlinked(actions: &mut [Action], snap: &HostSnapshot, harness: &dyn Harness)` — `touched_paths` and `block` are reused by Task 5

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/service/catalog/sync/plan.rs`:

```rust
    /// F3c: with `~/.agents/skills` a symlink, every Codex action that would
    /// write or adopt under it is refused with the reason; Codex's MCP merge
    /// (`~/.codex/config.toml`) and Claude's plan are unaffected.
    #[test]
    fn writes_through_a_symlinked_skills_dir_are_blocked() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.agents/skills".into(), "/home/u/.claude/skills".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, MCP]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let s = act(&hp, "s");
        assert_eq!(s.op, ActionOp::Blocked);
        assert_eq!(
            s.reason.as_deref(),
            Some("~/.agents/skills is a symlink (to /home/u/.claude/skills); fleet won't write Codex skills through it — replace it with a real directory or turn Codex off for this host")
        );
        assert!(s.plan.is_none() && s.remove_entry.is_none() && !s.backup);
        assert_eq!(
            act(&hp, "fleet").op,
            ActionOp::Create,
            "config.toml is not under the link"
        );

        // Adopting is refused too: Claude's copy of a plain skill is
        // byte-identical to Codex's render, and both manifests would claim it.
        let mut identical = snap.clone();
        satisfy(&mut identical, &substituted(&Codex, &asset(SKILL), &secrets_map()));
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &identical,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Blocked);

        // Claude never writes under ~/.agents: its plan is unchanged.
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Create);
    }

    /// The outermost link names the reason; a linked single skill blocks
    /// only that skill; a sibling path that merely shares a prefix is not
    /// under the link.
    #[test]
    fn the_outermost_link_names_the_reason_and_unlinked_skills_still_plan() {
        const T: &str = "kind: skill\nname: t\ndescription: d\n";
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.agents/skills/s".into(), "/x/s".into());
        snap.links.insert("~/.agent".into(), "/not/a/parent".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, T]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Blocked);
        assert!(act(&hp, "s")
            .reason
            .as_deref()
            .unwrap()
            .starts_with("~/.agents/skills/s is a symlink (to /x/s);"));
        assert_eq!(act(&hp, "t").op, ActionOp::Create);

        snap.links
            .insert("~/.agents".into(), "/dotfiles/agents".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, T]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        for name in ["s", "t"] {
            assert!(
                act(&hp, name)
                    .reason
                    .as_deref()
                    .unwrap()
                    .starts_with("~/.agents is a symlink (to /dotfiles/agents);"),
                "{name}"
            );
        }
    }

    /// F3c: a symlinked legacy `~/.codex/skills` blocks the removal of a
    /// manifest-listed old copy — it may be Claude's own file — with advice
    /// that does not suggest turning Codex off (a retiring host still
    /// removes).
    #[test]
    fn removing_an_old_copy_through_a_symlinked_legacy_dir_is_blocked() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.codex/skills".into(), "/home/u/.claude/skills".into());
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                hash: "h".into(),
                files: vec!["~/.codex/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        let hp = plan_for(
            &Catalog::default(),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "gone");
        assert_eq!(a.op, ActionOp::Blocked);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.codex/skills is a symlink (to /home/u/.claude/skills); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        );
    }
```

Append to `mod tests` in `crates/fleet-core/src/service/catalog/sync/apply.rs`, after `a_codex_agent_is_written_as_toml_and_removed_locally`:

```rust
    /// F3c end to end: with `~/.agents/skills` a symlink to
    /// `~/.claude/skills`, the real Codex scan reports the link and both
    /// Codex skill actions are blocked — the one that differs from Claude's
    /// copy and the one byte-identical to it (adopting would make two
    /// manifests claim one file) — so Claude's files are untouched and no
    /// Codex manifest is written.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn codex_never_writes_through_a_symlinked_agents_skills_locally() {
        use crate::service::catalog::harness::codex::Codex;
        use crate::service::catalog::model::Kind;
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            repo_dir.path(),
            &[
                ("skills/s/asset.yaml", "kind: skill\nname: s\ndescription: d\n"),
                ("skills/s/body.md", "codex body\n"),
                ("skills/t/asset.yaml", "kind: skill\nname: t\ndescription: d\n"),
                ("skills/t/body.md", "same body\n"),
            ],
        );
        let claude_skills = home.path().join(".claude/skills");
        std::fs::create_dir_all(claude_skills.join("s")).unwrap();
        std::fs::write(claude_skills.join("s/SKILL.md"), "claude's copy\n").unwrap();
        let t_bytes = Codex
            .render(catalog.find(Kind::Skill, "t").unwrap())
            .unwrap()
            .files[0]
            .bytes
            .clone();
        std::fs::create_dir_all(claude_skills.join("t")).unwrap();
        std::fs::write(claude_skills.join("t/SKILL.md"), &t_bytes).unwrap();
        std::fs::create_dir_all(home.path().join(".agents")).unwrap();
        std::os::unix::fs::symlink(&claude_skills, home.path().join(".agents/skills")).unwrap();
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1_000,
        };

        let plan = codex_plan_for(&ssh, &catalog).await;
        let target = claude_skills.to_string_lossy().to_string();
        let reason = format!(
            "~/.agents/skills is a symlink (to {target}); fleet won't write Codex skills through it — replace it with a real directory or turn Codex off for this host"
        );
        for name in ["s", "t"] {
            let a = plan.actions.iter().find(|a| a.name == name).unwrap();
            assert_eq!(a.op, ActionOp::Blocked, "{name}: {:?}", a.reason);
            assert_eq!(a.reason.as_deref(), Some(reason.as_str()));
        }
        let res = apply_host(&ctx, &Codex, &plan).await;
        assert!(res.actions.iter().all(|r| r.outcome == BLOCKED), "{res:?}");
        assert_eq!(
            std::fs::read_to_string(claude_skills.join("s/SKILL.md")).unwrap(),
            "claude's copy\n"
        );
        assert_eq!(std::fs::read(claude_skills.join("t/SKILL.md")).unwrap(), t_bytes);
        assert!(
            !home.path().join(".codex/.fleet-assets.json").exists(),
            "nothing recorded"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync::plan::tests service::catalog::sync::apply::tests::codex_never_writes_through 2>&1 | tail -20
```
Expected: FAIL — the four new tests see `Overwrite`/`Adopt`/`Create`/`Remove` where they expect `Blocked` (`left: Create, right: Blocked` and similar).

- [ ] **Step 3: Implement**

`crates/fleet-core/src/service/catalog/harness/mod.rs` — in `pub trait Harness`, after `fn manifest_path(&self) -> &'static str;` (line 264) add:

```rust
    /// Why an action touching a path under `link` — a directory the scan
    /// reported as a symlink to `target` (`HostSnapshot::links`) — is
    /// refused (multi-harness F3c). Only a scan that reports links ever
    /// makes the planner ask.
    fn symlink_reason(&self, link: &str, target: &str) -> String {
        format!(
            "{link} is a symlink (to {target}); fleet won't write {} files through it — replace it with a real directory",
            self.id()
        )
    }
```

`crates/fleet-core/src/service/catalog/harness/codex.rs` — in `impl Harness for Codex`, after `fn manifest_path` (lines 482-484) add:

```rust
    /// F3c. A linked legacy `~/.codex/skills` only blocks removing old
    /// copies, and turning Codex off would not avoid that (a retiring host
    /// still runs those removals), so its advice differs.
    fn symlink_reason(&self, link: &str, target: &str) -> String {
        if link == CODEX_LEGACY_SKILLS_DIR {
            format!("{link} is a symlink (to {target}); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        } else {
            format!("{link} is a symlink (to {target}); fleet won't write Codex skills through it — replace it with a real directory or turn Codex off for this host")
        }
    }
```

`crates/fleet-core/src/service/catalog/sync/plan.rs`:

In the `compute_host_plan` doc, after rule 8's last line (line 315, the one ending "rule 4 reads the asset as `Create`, not `Update`.") add:

```rust
/// 9. (F3c) An action that would write, adopt or delete a path under a
///    directory the scan reported as a symlink (`HostSnapshot::links`) ⇒
///    `Blocked(Harness::symlink_reason)`, with no plan and no
///    `remove_entry` left to apply. `Noop`s and earlier refusals stay.
```

In `compute_host_plan`, directly before `HostPlan {` (line 430) add:

```rust
    // Rule 9 (F3c): nothing is written or deleted through a symlinked
    // directory.
    block_symlinked(&mut actions, snap, harness);
```

After `has_stale_locations` (its closing `}` at line 728) add:

```rust
/// Every host path `action` writes, adopts or deletes: its own `files` (the
/// planned files, or a `Remove`'s entry files) plus the files of the entry
/// it supersedes, which the applier deletes when the render no longer
/// produces them.
fn touched_paths(action: &Action) -> impl Iterator<Item = &String> {
    action
        .files
        .iter()
        .chain(action.remove_entry.iter().flat_map(|e| e.files.iter()))
}

/// Turn `action` into a refusal: nothing of it is written or deleted.
fn block(action: &mut Action, reason: String) {
    action.op = ActionOp::Blocked;
    action.reason = Some(reason);
    action.backup = false;
    action.plan = None;
    action.remove_entry = None;
}

/// The outermost symlinked directory in `links` that `path` lies under (or
/// is), with its target. `BTreeMap` order puts `~/.agents` before
/// `~/.agents/skills`, so the first match is the outermost; the `/` check
/// keeps `~/.agent` from matching `~/.agents/…`.
fn linked_dir<'a>(links: &'a BTreeMap<String, String>, path: &str) -> Option<(&'a str, &'a str)> {
    links
        .iter()
        .find(|(link, _)| {
            path.strip_prefix(link.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .map(|(link, target)| (link.as_str(), target.as_str()))
}

/// Rule 9 (multi-harness F3c): refuse every action that would write, adopt
/// or delete anything under a symlinked directory. Some setups point
/// `~/.agents/skills` at `~/.claude/skills`: writing Codex's render through
/// it would replace Claude's copies, adopting would make two manifests
/// claim one file, and the two harnesses would undo each other on every
/// sync.
fn block_symlinked(actions: &mut [Action], snap: &HostSnapshot, harness: &dyn Harness) {
    if snap.links.is_empty() {
        return;
    }
    for action in actions.iter_mut() {
        if matches!(action.op, ActionOp::Noop | ActionOp::Blocked) {
            continue;
        }
        let hit = touched_paths(action).find_map(|p| linked_dir(&snap.links, p));
        if let Some((link, target)) = hit {
            let reason = harness.symlink_reason(link, target);
            block(action, reason);
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync::plan::tests service::catalog::sync::apply::tests service::catalog::harness 2>&1 | tail -8
```
Expected: all PASS (every existing plan/apply test included — their snapshots have no links).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/harness/mod.rs crates/fleet-core/src/service/catalog/harness/codex.rs crates/fleet-core/src/service/catalog/sync/plan.rs crates/fleet-core/src/service/catalog/sync/apply.rs
git commit -m "feat(catalog): never write Codex skills through a symlinked directory (F3c)"
```

---

### Task 4: Move fleet's pre-F3c Codex skills, never over a copy fleet did not write

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs:288-297` (rule 4 doc), new consts above `fn action_for` (~line 497), `:639-672` (rule 4 decision), new fn after `has_stale_locations`; tests appended at the end of `mod tests`
- Modify: `crates/fleet-core/src/service/catalog/sync/apply.rs` tests (new e2e after the Task 3 test)

**Interfaces:**
- Consumes: `CODEX_SKILLS_DIR`, legacy hashing (Task 1); `has_stale_locations` (existing).
- Produces:
  - `pub(crate) const MOVED_CREATE_REASON: &str`, `pub(crate) const MOVED_UPDATE_REASON: &str`, `pub(crate) const MOVED_ONTO_FOREIGN_REASON: &str` in `sync::plan`
  - `fn holds_unlisted_copy(entry: &ManifestEntry, plan: &RenderPlan, snap: &HostSnapshot) -> bool`
  - rule 4: moved + nothing there ⇒ `Create` with `MOVED_CREATE_REASON`; moved + identical there ⇒ `Update` with `MOVED_UPDATE_REASON`; moved + a different copy the entry does not list ⇒ `Overwrite` with `MOVED_ONTO_FOREIGN_REASON`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/service/catalog/sync/plan.rs`:

```rust
    /// F3c: a Codex skill fleet synced to `~/.codex/skills` before F3c,
    /// planned by what the new `~/.agents/skills` location already holds:
    /// nothing ⇒ `Create` that deletes the old copy; an identical copy ⇒
    /// `Update` that deletes the old copy; a different copy fleet never
    /// wrote ⇒ `Overwrite` (backed up), never a silent `Update`. Without a
    /// manifest entry the ordinary rules hold: identical ⇒ `Adopt`,
    /// different ⇒ `Overwrite("present but differs; not managed")`.
    #[test]
    fn a_codex_skill_moving_to_agents_skills_plans_by_what_the_new_location_holds() {
        let new_plan = substituted(&Codex, &asset(SKILL), &secrets_map());
        let new_path = new_plan.files[0].path.clone();
        assert_eq!(new_path, "~/.agents/skills/s/SKILL.md");
        let old_path = "~/.codex/skills/s/SKILL.md".to_string();
        let mut old_plan = new_plan.clone();
        old_plan.files[0].path = old_path.clone();
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );
        let mut old_only = HostSnapshot::default();
        satisfy(&mut old_only, &old_plan);
        let catalog = catalog_of(&[SKILL]);
        let old_files = |a: &Action| a.remove_entry.as_ref().map(|e| e.files.clone());

        // Nothing at the new location yet: the normal migration.
        let hp = plan_for(&catalog, &Codex, &old_only, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Create);
        assert_eq!(a.reason.as_deref(), Some(MOVED_CREATE_REASON));
        assert_eq!(a.files, vec![new_path.clone()]);
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));
        assert_eq!(a.expected[&new_path], None);

        // An identical copy is already there.
        let mut identical = old_only.clone();
        satisfy(&mut identical, &new_plan);
        let hp = plan_for(&catalog, &Codex, &identical, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Update);
        assert_eq!(a.reason.as_deref(), Some(MOVED_UPDATE_REASON));
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));

        // A diverged hand-made copy is there: an overwrite, backed up.
        let mut diverged = old_only.clone();
        diverged.files.insert(new_path.clone(), "edited".into());
        let hp = plan_for(&catalog, &Codex, &diverged, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(a.reason.as_deref(), Some(MOVED_ONTO_FOREIGN_REASON));
        assert!(a.backup);
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));

        // Never managed: the ordinary unmanaged rules.
        let mut fresh_identical = HostSnapshot::default();
        satisfy(&mut fresh_identical, &new_plan);
        let hp = plan_for(
            &catalog,
            &Codex,
            &fresh_identical,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Adopt);
        let mut fresh_diverged = HostSnapshot::default();
        fresh_diverged
            .files
            .insert(new_path.clone(), "edited".into());
        let hp = plan_for(
            &catalog,
            &Codex,
            &fresh_diverged,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(
            a.reason.as_deref(),
            Some("present but differs; not managed")
        );
    }
```

Append to `mod tests` in `crates/fleet-core/src/service/catalog/sync/apply.rs`, after the Task 3 test:

```rust
    /// F3c end to end against a real temp `$HOME`: a host fleet synced
    /// before F3c holds a Codex skill at `~/.codex/skills/s/` and a manifest
    /// pointing there. The next plan creates it in `~/.agents/skills/s/` and
    /// deletes the old copy (backed up); Codex's own `.system` and a
    /// hand-made `~/.codex/skills/hand` are never touched; the manifest
    /// lists only the new path; the plan after that is a no-op.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_codex_skill_synced_before_f3c_moves_to_agents_skills_locally() {
        use crate::service::catalog::harness::codex::Codex;
        use crate::service::catalog::model::Kind;
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            repo_dir.path(),
            &[
                ("skills/s/asset.yaml", "kind: skill\nname: s\ndescription: d\n"),
                ("skills/s/body.md", "body\n"),
            ],
        );

        // The pre-F3c layout: the same bytes, where fleet used to write them,
        // and a manifest entry naming that path.
        let rendered = Codex
            .render(catalog.find(Kind::Skill, "s").unwrap())
            .unwrap();
        let bytes = rendered.files[0].bytes.clone();
        let old_dir = home.path().join(".codex/skills/s");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("SKILL.md"), &bytes).unwrap();
        let mut old_plan = rendered.clone();
        old_plan.files[0].path = "~/.codex/skills/s/SKILL.md".into();
        let mut old_manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        old_manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 1, "personal"),
        );
        let manifest_file = home.path().join(".codex/.fleet-assets.json");
        std::fs::write(&manifest_file, old_manifest.to_json()).unwrap();
        // Codex's built-ins and a hand-made skill: never fleet's to touch.
        let system = home.path().join(".codex/skills/.system/builtin/SKILL.md");
        std::fs::create_dir_all(system.parent().unwrap()).unwrap();
        std::fs::write(&system, "builtin\n").unwrap();
        let hand = home.path().join(".codex/skills/hand/SKILL.md");
        std::fs::create_dir_all(hand.parent().unwrap()).unwrap();
        std::fs::write(&hand, "mine\n").unwrap();

        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 2_000,
        };
        let plan = codex_plan_for(&ssh, &catalog).await;
        assert_eq!(plan.actions.len(), 1, "{:?}", plan.actions);
        let a = &plan.actions[0];
        assert_eq!(a.op, ActionOp::Create, "{:?}", a.reason);
        assert_eq!(a.reason.as_deref(), Some(plan::MOVED_CREATE_REASON));
        assert_eq!(a.files, vec!["~/.agents/skills/s/SKILL.md".to_string()]);
        assert_eq!(
            a.remove_entry.as_ref().map(|e| e.files.clone()),
            Some(vec!["~/.codex/skills/s/SKILL.md".to_string()])
        );

        let res = apply_host(&ctx, &Codex, &plan).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert_eq!(
            std::fs::read(home.path().join(".agents/skills/s/SKILL.md")).unwrap(),
            bytes
        );
        assert!(!old_dir.join("SKILL.md").exists(), "the old copy is gone");
        assert_eq!(backups(&old_dir).len(), 1, "backed up, like any removal");
        assert_eq!(std::fs::read_to_string(&system).unwrap(), "builtin\n");
        assert_eq!(std::fs::read_to_string(&hand).unwrap(), "mine\n");
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert_eq!(
            manifest.assets["skill/s"].files,
            vec!["~/.agents/skills/s/SKILL.md".to_string()]
        );
        assert_eq!(manifest.assets.len(), 1, "{manifest:?}");

        let again = codex_plan_for(&ssh, &catalog).await;
        assert_eq!(again.actions[0].op, ActionOp::Noop);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync::plan::tests::a_codex_skill_moving service::catalog::sync::apply::tests::a_codex_skill_synced_before_f3c 2>&1 | tail -20
```
Expected: compile error — `cannot find value MOVED_CREATE_REASON` (and the other two consts) in `sync::plan`.

- [ ] **Step 3: Implement**

`crates/fleet-core/src/service/catalog/sync/plan.rs`:

Replace the rule 4 doc lines 288-297 (from the line beginning "matching ⇒ `Noop` if the manifest names it" through the line "the manifest does not name it at all.") with:

```rust
///    matching ⇒ `Noop` if the manifest names it *at the locations this
///    render produces*, `Update` if the entry still lists a path or merge
///    the render has moved away from (a re-pointed `install_as`, or a Codex
///    skill moving from `~/.codex/skills` to `~/.agents/skills` in F3c,
///    whose new location already held an identical copy — the old one has
///    to go), else `Adopt`; present and differing ⇒ `Overwrite` when the
///    asset moved (as above) onto a planned file the entry does not list —
///    a copy fleet never wrote there, so not a catalog update — else
///    `Update` when the manifest names it with a *different*
///    hash (the catalog moved on), `Overwrite("edited on host")` when the
///    manifest names it with the *same* hash (so the difference came from
///    the host), and `Overwrite("present but differs; not managed")` when
///    the manifest does not name it at all. A `Create` whose entry lists
///    locations the render moved away from says so in its reason: rule 8
///    deletes those old files with it, and `files` names only the new ones.
```

Directly above `fn action_for(` add:

```rust
/// Rule 4's reasons for an asset whose manifest entry lists locations the
/// render moved away from (F3c: Codex skills moving to `~/.agents/skills`;
/// also a re-pointed `install_as`).
pub(crate) const MOVED_CREATE_REASON: &str =
    "the last sync's files at its old location are removed";
pub(crate) const MOVED_UPDATE_REASON: &str =
    "installed at a different location; the old copy is removed";
pub(crate) const MOVED_ONTO_FOREIGN_REASON: &str = "moved to a location that already holds a different copy fleet did not write; it is replaced (backed up) and the old copy removed";
```

Replace the rule 4 decision (lines 639-672, from `let (op, reason) = if !present {` through the closing `};` of the `else` arm) with:

```rust
    let (op, reason) = if !present {
        (
            ActionOp::Create,
            // F3c: the entry lists files this render no longer produces (a
            // Codex skill fleet synced to `~/.codex/skills` before it moved
            // to `~/.agents/skills`). Rule 8's `remove_entry` deletes them;
            // the reason says so, since `files` lists only the new ones.
            manifest_entry
                .filter(|entry| has_stale_locations(entry, plan))
                .map(|_| MOVED_CREATE_REASON.to_string()),
        )
    } else if matches {
        match manifest_entry {
            // Present, identical and managed — but the entry points at a
            // location the render no longer produces. `install_as` can
            // re-point an asset at an identifier that already holds an
            // identical copy (and F3c moves Codex skills to a new
            // directory): the content check then passes at the new
            // location while the old files are still on the host and the
            // entry still claims them. `Update` (whose `remove_entry`,
            // rule 8, carries the previous entry) deletes them and
            // refreshes the entry; a `Noop` would leak them forever.
            Some(entry) if has_stale_locations(entry, plan) => {
                (ActionOp::Update, Some(MOVED_UPDATE_REASON.into()))
            }
            Some(_) => (ActionOp::Noop, None),
            None => (ActionOp::Adopt, None),
        }
    } else {
        match manifest_entry {
            // F3c: the asset moved, and its new location already holds a
            // different copy the entry never listed — fleet did not write
            // it, so replacing it is an overwrite for a person to see, not
            // a catalog update.
            Some(entry)
                if has_stale_locations(entry, plan) && holds_unlisted_copy(entry, plan, snap) =>
            {
                (ActionOp::Overwrite, Some(MOVED_ONTO_FOREIGN_REASON.into()))
            }
            Some(entry) if entry.hash == plan.hash() => (
                ActionOp::Overwrite,
                Some("edited on host; the catalog has not changed".into()),
            ),
            Some(_) => (ActionOp::Update, None),
            None => (
                ActionOp::Overwrite,
                Some("present but differs; not managed".into()),
            ),
        }
    };
```

After `has_stale_locations` (and before the Task 3 helpers) add:

```rust
/// Does the host already hold one of `plan`'s files at a path `entry` does
/// not list — a copy no earlier sync of this asset wrote there? Only asked
/// for an asset that moved (`has_stale_locations`): an entry recorded
/// without files (as some tests build them) never reads as moved.
fn holds_unlisted_copy(entry: &ManifestEntry, plan: &RenderPlan, snap: &HostSnapshot) -> bool {
    plan.files
        .iter()
        .any(|f| snap.files.contains_key(&f.path) && !entry.files.contains(&f.path))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync service::catalog::inventory 2>&1 | tail -8
```
Expected: all PASS — including the unchanged `a_re_pointed_install_name_updates_instead_of_noop` (still `Update`, now with `MOVED_UPDATE_REASON`), `managed_and_stale_is_an_update_managed_and_edited_is_an_overwrite` (its entry lists no files, so it never reads as moved and stays `Update`), `a_changed_hook_carries_the_previous_manifest_entry_to_unmerge` (same merge location, reason stays `None`) and `plan_sync_retires_codex_when_turned_off_but_still_managed`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/sync/plan.rs crates/fleet-core/src/service/catalog/sync/apply.rs
git commit -m "feat(catalog): move pre-F3c Codex skills, never over an unmanaged copy (F3c)"
```

---

### Task 5: Block an action whose file another harness's plan also touches

**Files:**
- Modify: `crates/fleet-core/src/service/catalog/sync/plan.rs` (new fn after the Task 3 helpers, before `pub fn counts`; tests appended at the end of `mod tests`)
- Modify: `crates/fleet-core/src/service/catalog/sync/mod.rs:375-377`, `:419-421` (`plan_sync`, two lines + one comment)

**Interfaces:**
- Consumes: `touched_paths`, `block` (Task 3).
- Produces: `pub(crate) fn block_cross_harness_collisions(plans: &mut [HostPlan])` in `sync::plan`, called once per host by `sync::plan_sync`.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/fleet-core/src/service/catalog/sync/plan.rs`:

```rust
    fn planned(harness: &str, actions: Vec<Action>) -> HostPlan {
        HostPlan {
            host_alias: "h".into(),
            harness: harness.into(),
            status: "planned".into(),
            detail: None,
            actions,
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        }
    }

    fn file_action(name: &str, op: ActionOp, path: &str) -> Action {
        Action {
            kind: "skill".into(),
            name: name.into(),
            op,
            catalog: None,
            reason: None,
            files: vec![path.into()],
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: BTreeMap::new(),
            secret_files: BTreeSet::new(),
            remove_entry: None,
            plugin: None,
        }
    }

    /// F3c (future-proofing for harnesses sharing `~/.agents/skills`): two
    /// harnesses on one host writing one file are both blocked, each naming
    /// the other; actions on paths nobody else touches are left alone.
    #[test]
    fn two_harnesses_writing_one_path_are_both_blocked() {
        let shared = "~/.agents/skills/s/SKILL.md";
        let mut plans = vec![
            planned(
                "codex",
                vec![
                    file_action("s", ActionOp::Create, shared),
                    file_action("t", ActionOp::Create, "~/.agents/skills/t/SKILL.md"),
                ],
            ),
            planned("gemini", vec![file_action("s", ActionOp::Update, shared)]),
            planned(
                "claude",
                vec![file_action("s", ActionOp::Create, "~/.claude/skills/s/SKILL.md")],
            ),
        ];
        block_cross_harness_collisions(&mut plans);
        assert_eq!(plans[0].actions[0].op, ActionOp::Blocked);
        assert_eq!(
            plans[0].actions[0].reason.as_deref(),
            Some("~/.agents/skills/s/SKILL.md is also managed by the gemini plan on this host; fleet won't let two harnesses write one file")
        );
        assert_eq!(plans[1].actions[0].op, ActionOp::Blocked);
        assert!(plans[1].actions[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("by the codex plan"));
        assert_eq!(plans[0].actions[1].op, ActionOp::Create);
        assert_eq!(plans[2].actions[0].op, ActionOp::Create);
    }

    /// A `Noop` claims the file it manages (it stays a `Noop`); a removal
    /// claims its entry's files through `remove_entry`; an already-blocked
    /// action claims nothing.
    #[test]
    fn noops_and_removals_claim_their_files_and_blocked_actions_do_not() {
        let managed = "~/.agents/skills/s/SKILL.md";
        let mut moving = file_action("s", ActionOp::Create, "~/.agents/skills/s2/SKILL.md");
        moving.remove_entry = Some(ManifestEntry {
            files: vec!["~/.agents/skills/old/SKILL.md".into()],
            ..Default::default()
        });
        let mut plans = vec![
            planned(
                "codex",
                vec![
                    file_action("s", ActionOp::Noop, managed),
                    file_action("b", ActionOp::Blocked, "~/.agents/skills/b/SKILL.md"),
                ],
            ),
            planned(
                "gemini",
                vec![
                    file_action("s", ActionOp::Create, managed),
                    moving,
                    file_action("b", ActionOp::Create, "~/.agents/skills/b/SKILL.md"),
                ],
            ),
            planned(
                "agy",
                vec![file_action("old", ActionOp::Remove, "~/.agents/skills/old/SKILL.md")],
            ),
        ];
        block_cross_harness_collisions(&mut plans);
        assert_eq!(plans[0].actions[0].op, ActionOp::Noop);
        assert_eq!(plans[1].actions[0].op, ActionOp::Blocked);
        assert_eq!(plans[1].actions[1].op, ActionOp::Blocked, "its old file is agy's removal");
        assert_eq!(plans[2].actions[0].op, ActionOp::Blocked);
        assert_eq!(
            plans[1].actions[2].op,
            ActionOp::Create,
            "a blocked action claims nothing"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync::plan::tests::two_harnesses service::catalog::sync::plan::tests::noops_and_removals 2>&1 | tail -15
```
Expected: compile error — `cannot find function block_cross_harness_collisions`.

- [ ] **Step 3: Implement**

`crates/fleet-core/src/service/catalog/sync/plan.rs` — after `block_symlinked` (Task 3), before `pub fn counts`, add:

```rust
/// Multi-harness F3c: no two harnesses on one host may manage one file.
/// `plans` are one host's plans, one per harness. A path is claimed by
/// every action that is not `Blocked` and names it in `files` or in its
/// `remove_entry` (so a `Noop` claims what it already manages); every
/// action that would write, adopt or delete a path another harness also
/// claims becomes `Blocked`, naming that harness. Config merges are not
/// compared: each harness merges into its own config files.
///
/// Today Claude (`~/.claude/…`) and Codex (`~/.agents/…`, `~/.codex/…`)
/// never share a path, so this never fires; it is the guard for later
/// harnesses that also render into `~/.agents/skills`.
pub(crate) fn block_cross_harness_collisions(plans: &mut [HostPlan]) {
    let mut claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for hp in plans.iter() {
        for action in hp.actions.iter().filter(|a| a.op != ActionOp::Blocked) {
            for path in touched_paths(action) {
                claims
                    .entry(path.clone())
                    .or_default()
                    .insert(hp.harness.clone());
            }
        }
    }
    for hp in plans.iter_mut() {
        let harness = hp.harness.clone();
        for action in hp.actions.iter_mut() {
            if matches!(action.op, ActionOp::Noop | ActionOp::Blocked) {
                continue;
            }
            let clash = touched_paths(action).find_map(|path| {
                claims
                    .get(path)?
                    .iter()
                    .find(|other| **other != harness)
                    .map(|other| (path.clone(), other.clone()))
            });
            if let Some((path, other)) = clash {
                block(
                    action,
                    format!(
                        "{path} is also managed by the {other} plan on this host; fleet won't let two harnesses write one file"
                    ),
                );
            }
        }
    }
}
```

`crates/fleet-core/src/service/catalog/sync/mod.rs` — in `plan_sync`, replace lines 375-377:

```rust
        // Every scanning harness is scanned, even one this host may not
        // serve: the scan is what detects it and reads its manifest.
        for harness in &scanning {
```

with:

```rust
        // Every scanning harness is scanned, even one this host may not
        // serve: the scan is what detects it and reads its manifest.
        let first_plan = host_plans.len();
        for harness in &scanning {
```

and replace the end of that loop and of the host loop (lines 419-421):

```rust
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
    }
```

with:

```rust
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
        // F3c: no two harnesses on this host may manage one file.
        plan::block_cross_harness_collisions(&mut host_plans[first_plan..]);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo test -p fleet-core --lib service::catalog::sync 2>&1 | tail -8
```
Expected: all PASS (every `plan_sync` test included: Claude and Codex never share a path).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/catalog/sync/plan.rs crates/fleet-core/src/service/catalog/sync/mod.rs
git commit -m "feat(catalog): block a file two harnesses would both manage (F3c)"
```

---

### Task 6: Docs and full verification

**Files:**
- Modify: `docs/concepts.md` (new paragraph after line 142)
- Modify: `docs/hub.md` (new paragraph after line 259)
- Modify: `docs/superpowers/specs/2026-09-14-asset-catalog-design.md:162`
- Modify: `CLAUDE.md` (new bullet after the "Multi-harness F3a / F3b" bullet, which ends at line 203)

**Interfaces:**
- Consumes: everything above. Produces: nothing new.

(`docs/control-api.md` names no skill directory and needs no change.)

- [ ] **Step 1: Write the docs**

`docs/concepts.md`, after the "*Upgrading to per-host harnesses.*" paragraph (ends line 142, "(a limitation that predates this change)."), insert a blank line and:

```markdown
*Codex skills live in `~/.agents/skills`.* Codex reads user skills from
`~/.agents/skills/<install name>/` (a directory several agent CLIs share),
not from `~/.codex/skills`, which it keeps for its own built-ins
(`.system`). Fleet renders Codex skills — and an agent with
`targets.codex.render_as: skill` — there. A host synced by an older fleet
moves on its next sync: the plan writes each fleet-managed Codex skill to
`~/.agents/skills/` and deletes the copy its manifest names under
`~/.codex/skills/` (a `.fleet-bak-*` backup of it stays beside it), so
Codex starts seeing them. Nothing else under `~/.codex/skills` is touched —
not `.system`, not a skill you put there yourself. Codex cannot see such a
skill, so fleet no longer lists it either; move it to `~/.agents/skills/`
to use it. Where `~/.agents/skills` already holds a same-named skill fleet
did not write, the plan adopts it when identical and otherwise shows an
`overwrite`, as for any unmanaged copy. If `~/.agents`, `~/.agents/skills`,
a skill directory in it, or `~/.codex/skills` is a symlink — some setups
point `~/.agents/skills` at `~/.claude/skills` — every Codex action that
would write, adopt or delete through it is `blocked`, with the link's
target in the reason: Codex's copy would replace Claude's, and the two
would undo each other on every sync. Replace the link with a real
directory, or turn Codex off for that host (for `~/.codex/skills` only the
first helps); Claude is unaffected. A plan also blocks any action whose
file another harness's plan on the same host would write too.
```

`docs/hub.md`, after the "**Codex on upgraded hosts.**" paragraph (ends line 259, "and drops its comments, as before."), insert a blank line and:

```markdown
**Codex skills move to `~/.agents/skills`.** Codex only reads user skills
from `~/.agents/skills`, so a release with this change renders Codex skills
there, and the first sync after upgrading moves every Codex skill fleet put
in `~/.codex/skills` (backing the old copy up, never touching Codex's
`.system` or skills you added yourself). A host whose `~/.agents/skills`
(or `~/.agents`, or `~/.codex/skills`) is a symlink shows those Codex
actions as `blocked` until the link is replaced with a real directory; see
*Codex skills live in `~/.agents/skills`* in `docs/concepts.md`.
```

`docs/superpowers/specs/2026-09-14-asset-catalog-design.md` line 162 becomes:

```markdown
| skill | `~/.agents/skills/<name>/SKILL.md` + resources, 1:1 — since multi-harness F3c (`docs/superpowers/plans/2026-10-01-f3c-codex-skills-agents-dir.md`); `~/.codex/skills/` before, which Codex does not read for user skills |
```

`CLAUDE.md`, after the "Multi-harness F3a / F3b" bullet (ends line 203, "  crate."), insert:

```markdown
- **Multi-harness F3c** (plan
  `docs/superpowers/plans/2026-10-01-f3c-codex-skills-agents-dir.md`):
  Codex skills render to `~/.agents/skills/<install name>/`
  (`CODEX_SKILLS_DIR`); `~/.codex/skills` (`CODEX_LEGACY_SKILLS_DIR`, minus
  Codex's `.system`) is still hashed so a pre-F3c manifest entry's old copy
  can be deleted under compare-and-swap — the planner's existing rule-8
  `remove_entry` path does the move, and a moved asset whose new location
  holds a copy fleet did not write is an `overwrite`, not an `update`. The
  Codex scan prints `##LINK <path> -> <target>` (`HostSnapshot::links`) for
  a symlinked `~/.agents`, `~/.agents/skills`, entry in it, or
  `~/.codex/skills`; `compute_host_plan` blocks every write/adopt/delete
  under one (rule 9, `Harness::symlink_reason`).
  `plan::block_cross_harness_collisions`, called once per host in
  `plan_sync`, blocks any action whose file another harness's plan on that
  host also touches.
```

- [ ] **Step 2: Format, lint, and run the whole suites**

Run (locally; or prefix the test lines with `mercury-run` from the repo root when it is available):
```bash
export CARGO_TARGET_DIR=<shared-target-dir>
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p fleet-core 2>&1 | grep -E '^test result|FAILED|panicked' | tail -15
cargo test -p claude-fleet --lib 2>&1 | tail -3
cargo build -p fleet-hub --locked 2>&1 | tail -2
```
Expected: fmt shows no diff (if it does, run `cargo fmt --all` and include it in the commit); clippy clean; `fleet-core` all PASS except the known `service::rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in` when run in a deep scratch directory — a failure in `store::schema::tests_upgrade` or `service::work::scale_tests` is re-run in isolation first, and anything else that fails is checked against `origin/main` before it is called pre-existing, and named in the report; `claude-fleet` PASS; fleet-hub builds.

- [ ] **Step 3: Commit**

```bash
git add docs/concepts.md docs/hub.md docs/superpowers/specs/2026-09-14-asset-catalog-design.md CLAUDE.md
git commit -m "docs: Codex skills in ~/.agents/skills, migration and symlink guard (F3c)"
```

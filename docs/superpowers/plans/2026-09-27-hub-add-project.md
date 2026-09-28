# Add project from a hub client — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A desktop paired with a hub can add a project (clone URL, GitHub browse, new repository) on any fleet host, through two new hub tools.

**Architecture:** `add_project` and `list_github_repos` become MCP tools in `crates/fleet-core/src/mcp/tools/repo.rs`, thin wrappers over the existing `service::add_project` functions (the clone / `gh` work already runs on the target host over `SshExec`). The two desktop commands switch from `LocalOnly` to `Routed` in `verdicts.rs`, following the `probe_host` pattern. `GithubRepo` joins the wire contract, revision 4 → 5. The frontend drops the refusal, gates the opener on the live link, and hides the folder source on a hub client.

**Tech Stack:** Rust (rmcp `#[tool]`, schemars, serde, tokio), Svelte 5 runes, Vitest, the repo's generated-artifact tests (`REGEN_*` env vars).

**Spec:** `docs/superpowers/specs/2026-09-27-hub-add-project-design.md`

## Global Constraints

- Work in the worktree `.claude/worktrees/nostalgic-bardeen-635719`, branch `feat/hub-add-project` (cut from `origin/main` at `baeaeab9`). Never `cd` to the main checkout.
- Before any `cargo` command: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/nostalgic-bardeen-635719` (the `cargo` shell function otherwise shares the main repo's target dir).
- Frontend commands use `npx`: `npx vitest run <file>`, `npx svelte-check`. `pnpm test` / `pnpm check` fail on this machine (binary not on PATH).
- Run `pnpm install --frozen-lockfile` once before the first frontend test.
- No `git stash`. No attribution lines in commit messages. Commit after every task.
- Generated artifacts are regenerated, never hand-edited: `hub_verdicts.generated.json`, the refusal table in `docs/hub.md`, `docs/control-api-reference.md`, `hub_contract.golden.json`.
- Tool descriptions are one or two sentences: `the_served_definition_budget_stays_bounded` caps the served surface and is raised only by the measured amount, with the numbers written into its doc comment.
- Every tool parameter and enum variant has a doc comment (`every_tool_parameter_is_documented`).
- `call_id` never crosses the wire to the hub (`#[schemars(skip)]` on the service struct; the routed call spells arguments out).

---

### Task 1: Wire-ready `AddProjectArgs` and `AddProjectSource`

**Files:**
- Modify: `crates/fleet-core/src/service/add_project.rs:60-99` (the two type definitions) and its `mod tests`

**Interfaces:**
- Produces: `AddProjectSource` derives `Serialize, Deserialize, rmcp::schemars::JsonSchema`, with a doc comment on every variant and field. `AddProjectArgs` derives `Deserialize, rmcp::schemars::JsonSchema` with `#[schemars(crate = "rmcp::schemars", rename = "AddProjectParams")]`, docs on `host_alias` and `source`, and `#[schemars(skip)]` on `call_id`. Tasks 2 and 3 rely on exactly these names.

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/fleet-core/src/service/add_project.rs`:

```rust
    // ── wire shape (hub tool `add_project`) ─────────────────────────────

    #[test]
    fn the_source_enum_serialises_with_its_kind_tag_for_the_hub_call() {
        let v = serde_json::to_value(AddProjectSource::New {
            owner: "o".into(),
            repo: "r".into(),
            create_remote: true,
            confirm: Some("tok".into()),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "kind": "new", "owner": "o", "repo": "r",
                "create_remote": true, "confirm": "tok"
            })
        );
        let v = serde_json::to_value(AddProjectSource::Clone {
            url: "https://github.com/o/r".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "clone", "url": "https://github.com/o/r" }));
    }

    #[test]
    fn the_served_schema_has_no_call_id_and_documents_every_field() {
        let schema = serde_json::to_value(rmcp::schemars::schema_for!(AddProjectArgs)).unwrap();
        let props = schema["properties"].as_object().expect("an object schema");
        assert!(props.contains_key("host_alias"));
        assert!(props.contains_key("source"));
        assert!(
            !props.contains_key("call_id"),
            "call_id is this process's cancellation key, never a hub argument: {props:?}"
        );
        for (name, p) in props {
            assert!(p.get("description").is_some(), "{name} has no description");
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/nostalgic-bardeen-635719
cargo test -p fleet-core --lib service::add_project::tests::the_source_enum_serialises 2>&1 | tail -20
```

Expected: compile error — `AddProjectSource` does not implement `Serialize`, `AddProjectArgs` does not implement `JsonSchema`.

- [ ] **Step 3: Add the derives and docs**

Replace the two definitions (currently `#[derive(Deserialize)]` each) with:

```rust
/// Where the project comes from. The hub tool `add_project` serves this
/// schema, so every variant and field is documented here.
#[derive(serde::Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddProjectSource {
    /// Clone a GitHub repository onto the host.
    Clone {
        /// `https://github.com/<owner>/<repo>` or `git@github.com:<owner>/<repo>.git`.
        url: String,
    },
    /// Adopt a checkout that already exists on the `local` host.
    Folder {
        /// Absolute path of the checkout (or a directory inside one).
        path: String,
    },
    /// Create a new repository on the host, optionally on GitHub too.
    New {
        /// GitHub owner (user or organisation).
        owner: String,
        /// Repository name.
        repo: String,
        /// Also run `gh repo create` on the host; the first call is refused
        /// with a `confirm` token to send back.
        #[serde(default)]
        create_remote: bool,
        /// The token the previous `create_remote` refusal returned.
        #[serde(default)]
        confirm: Option<String>,
    },
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AddProjectParams")]
pub struct AddProjectArgs {
    /// Fleet alias of the host to add the project on.
    pub host_alias: String,
    /// What to add: `kind` is `clone`, `folder` or `new`.
    pub source: AddProjectSource,
    /// Injected by the frontend's `invokeCmdAbortable` (see `cancel.rs`) so
    /// the Add-project dialog's Cancel button can abort a clone / new-project
    /// run in flight, exactly like `NewSessionArgs::call_id`
    /// (`service::sessions::lifecycle`). `add_project` binds this id to a
    /// fresh `CancellationToken` in the process-wide `CancellationRegistry`
    /// and threads the token into `add_project_with`, which races it
    /// directly against the long-running script — via
    /// `SshExec::run_bounded_cancellable` on the remote branch and a
    /// `tokio::select!` in `run_local_script` on the local one — rather than
    /// stopping at the command layer, where cancelling would do nothing.
    /// A remote cancel only stops the local ssh client, not the run on the
    /// host: see [`add_project`]'s doc comment.
    ///
    /// Never part of the hub tool's schema: on a hub the registry mints an
    /// anonymous token, and the desktop's routed call does not send it.
    #[serde(default)]
    #[schemars(skip)]
    pub call_id: Option<u64>,
}
```

Keep the existing doc paragraph on `call_id` (it is reproduced above); only the last paragraph and the `#[schemars(skip)]` line are new.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::add_project::tests 2>&1 | tail -5
```

Expected: `test result: ok` with the two new tests included.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/add_project.rs
git commit -m "feat(core): AddProjectArgs and AddProjectSource carry a served schema"
```

---

### Task 2: The hub tools `add_project` and `list_github_repos`

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/repo.rs` (after `refresh_projects`)
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (after `ListHostWorktreesParams`)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES`, after the `refresh_projects` row)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (new tests; `BUDGET_BYTES`)
- Modify: `docs/control-api.md:245` (the *Projects & worktrees* bullet)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `AddProjectArgs` / `AddProjectSource` from Task 1; `add_project::add_project(args, &Mutex<Store>, &dyn SshExec, &Arc<CancellationRegistry>)`; `add_project::list_github_repos_with(&str, &Mutex<Store>, &dyn SshExec)`.
- Produces: hub tools named exactly `add_project` and `list_github_repos`; `ListGithubReposParams { host_alias: String }` in `params.rs`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/fleet-core/src/mcp/tools/tests.rs`:

```rust
// ---- add_project / list_github_repos (a hub client adds a project) --------

#[test]
fn add_project_tools_are_client_reachable_with_the_right_flags() {
    let add = guard::policy("add_project").expect("add_project has a TOOL_POLICIES row");
    assert!(guard::is_client_tool("add_project"), "a full client adds projects");
    assert!(!guard::is_admin_tool("add_project"));
    assert!(!add.readonly, "it writes a project row");
    assert!(!add.confirm, "it has its own create_remote confirm token");
    assert_eq!(
        crate::mcp::tool_deadline("add_project"),
        crate::mcp::tools::support::LONG_POLL_CAP,
        "a clone's wall clock is 600 s; the lifecycle cap (300 s) would cut it"
    );
    let ls = guard::policy("list_github_repos").expect("list_github_repos has a row");
    assert!(guard::is_client_tool("list_github_repos"));
    assert!(ls.readonly, "gh repo list observes");
    assert!(guard::is_readonly_tool("list_github_repos"));
    assert!(!guard::is_readonly_tool("add_project"));
}

#[test]
fn a_readonly_client_may_browse_repos_but_not_add_a_project() {
    let ro = client_caller("phone", TokenMode::Readonly);
    assert!(enforce_mode(&ro, "list_github_repos").is_ok());
    assert!(enforce_mode(&ro, "add_project").is_err());
    let full = client_caller("laptop", TokenMode::Full);
    assert!(enforce_mode(&full, "add_project").is_ok());
}

#[tokio::test]
async fn add_project_refuses_a_hostile_alias_before_any_ssh() {
    let t = test_tools(Store::open_in_memory().unwrap());
    let err = t
        .add_project(Parameters(AddProjectArgs {
            host_alias: "-oProxyCommand=x".into(),
            source: AddProjectSource::Clone {
                url: "https://github.com/o/r".into(),
            },
            call_id: None,
        }))
        .await
        .unwrap_err();
    assert!(err.message.starts_with("E_"), "{}", err.message);
    let err = t
        .list_github_repos(Parameters(ListGithubReposParams {
            host_alias: "-oProxyCommand=x".into(),
        }))
        .await
        .unwrap_err();
    assert!(err.message.starts_with("E_"), "{}", err.message);
}

#[test]
fn add_project_serves_the_source_variants_and_no_call_id() {
    let tools = FleetTools::tool_router_for_doc().list_all();
    let t = tools
        .iter()
        .find(|t| t.name == "add_project")
        .expect("add_project is served");
    let props = t.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("host_alias"));
    assert!(props.contains_key("source"));
    assert!(!props.contains_key("call_id"));
    let text = serde_json::to_string(&t.input_schema).unwrap();
    for kind in ["clone", "folder", "new"] {
        assert!(text.contains(&format!("\"{kind}\"")), "source kind {kind} missing: {text}");
    }
}
```

Add to the test module's imports (top of `tests.rs`, next to the other `use crate::service::…` lines):

```rust
use crate::service::add_project::{AddProjectArgs, AddProjectSource};
```

`ListGithubReposParams` comes in through `super::*` like `ListHostWorktreesParams` does (check the existing import of that name and add `ListGithubReposParams` beside it if it is imported explicitly).

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib mcp::tools::tests::add_project 2>&1 | tail -20
```

Expected: compile errors — no `add_project` method on `FleetTools`, no `ListGithubReposParams`.

- [ ] **Step 3: Add the params struct**

In `crates/fleet-core/src/mcp/tools/params.rs`, after `ListHostWorktreesParams`:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListGithubReposParams {
    /// Host whose `gh` login lists the repositories.
    pub host_alias: String,
}
```

- [ ] **Step 4: Add the two tools**

In `crates/fleet-core/src/mcp/tools/repo.rs`, directly after the `refresh_projects` tool (before `// ---- sessions ----`):

```rust
    #[tool(description = "Add a project on a host: clone a GitHub URL, adopt a \
        folder (the hub's local host only) or create a new repository \
        (create_remote is refused once with a confirm token to send back). \
        git and gh run on the host with its own credentials. Returns the \
        project row.")]
    pub(super) async fn add_project(
        &self,
        Parameters(args): Parameters<add_project::AddProjectArgs>,
    ) -> Result<CallToolResult, McpError> {
        let target = match &args.source {
            add_project::AddProjectSource::Clone { url } => format!("kind=clone url={url}"),
            add_project::AddProjectSource::Folder { path } => format!("kind=folder path={path}"),
            add_project::AddProjectSource::New {
                owner,
                repo,
                create_remote,
                ..
            } => format!("kind=new repo={owner}/{repo} create_remote={create_remote}"),
        };
        audit(
            "add_project",
            &format!("host={} {target}", args.host_alias),
        );
        // `call_id` is never set here (it is `#[schemars(skip)]`): the
        // registry mints an anonymous token and `CancelGuard` releases it.
        let row = add_project::add_project(args, &self.store, &*self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&row)
    }

    #[tool(description = "Repositories gh on the host can see, for choosing \
        what to clone with add_project.")]
    pub(super) async fn list_github_repos(
        &self,
        Parameters(p): Parameters<ListGithubReposParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_github_repos", &format!("host={}", p.host_alias));
        let repos = add_project::list_github_repos_with(&p.host_alias, &self.store, &*self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&repos)
    }
```

Add to the file's imports: `use crate::service::add_project;`. If `ok_json` is not already in scope through `super::*`, import it the way `fleet.rs` does.

- [ ] **Step 5: Add the policy rows**

In `crates/fleet-core/src/mcp/guard.rs`, after the `refresh_projects` `ToolPolicy` block:

```rust
    // Clones or creates a repository on a host: a write, and a long one — a
    // clone's wall clock is 600 s (`service::add_project::CLONE_WALL_CLOCK`),
    // which the lifecycle cap (300 s) would cut in half, so it takes the
    // long-poll cap. Not `confirm`: `create_remote` has its own single-use
    // token (`service::add_project::ConfirmTokens`).
    ToolPolicy {
        name: "add_project",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::LongPoll,
    },
    // `gh repo list` on one host: a read with a 30 s wall clock.
    ToolPolicy {
        name: "list_github_repos",
        access: Access::Client,
        readonly: true,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

- [ ] **Step 6: Run the new tests and the whole MCP tool suite**

```bash
cargo test -p fleet-core --lib mcp:: 2>&1 | grep -E "^test .*(FAILED|panicked)|test result|budget|over" | head -20
```

Expected: everything green except `the_served_definition_budget_stays_bounded`, which prints the new measured size.

- [ ] **Step 7: Raise the budget by the measured amount**

In `crates/fleet-core/src/mcp/tools/tests.rs`, change `const BUDGET_BYTES: usize = 57_050;` to the measured size rounded up to the next 50, and add one paragraph to the doc comment above the test in the same style as the previous raises:

```rust
    /// Raised from 57,050 to <NEW> for `add_project` and `list_github_repos`
    /// (a hub client adds a project). The surface before them measured
    /// <BEFORE>; the two together, with the `source` enum's three variants
    /// documented as `every_tool_parameter_is_documented` requires, add
    /// <DELTA>. Headroom is again deliberately small.
```

Replace `<NEW>`, `<BEFORE>` and `<DELTA>` with the numbers the failing run printed. Re-run the test and confirm it passes.

- [ ] **Step 8: Regenerate the control API reference and update the docs bullet**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current 2>&1 | tail -3
cargo test -p fleet-core reference_is_current 2>&1 | tail -3
```

Edit `docs/control-api.md`'s bullet to:

```markdown
- **Projects & worktrees** — `list_projects`, `refresh_projects`,
  `add_project` (clone, adopt or create a repository on a host — `git` and
  `gh` run there), `list_github_repos` (what `gh` on a host can see),
  `list_worktrees`, `list_host_worktrees` (one host scanned over SSH, for the
  worktrees fleet's own rows do not cover), `delete_worktree`.
```

- [ ] **Step 9: Run the fleet-core suite, clippy and fmt**

```bash
cargo test -p fleet-core 2>&1 | grep -E "test result|FAILED" | head
cargo clippy -p fleet-core --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --all --check
```

Expected: all `ok`, clippy clean, fmt clean.

- [ ] **Step 10: Commit**

```bash
git add crates/fleet-core/src/mcp docs/control-api.md docs/control-api-reference.md
git commit -m "feat(mcp): add_project and list_github_repos as hub tools"
```

---

### Task 3: The desktop routes both commands to the hub

**Files:**
- Modify: `src-tauri/src/backend/verdicts.rs:155-168` (the two rows)
- Modify: `src-tauri/src/commands/projects.rs` (module doc, both handlers, `routed`)
- Modify: `src-tauri/src/backend/tests_routing.rs` (`routed_read_cases`, `routed_mutation_cases`, a payload constant)
- Regenerate: `src/lib/hub_verdicts.generated.json`, the refusal table in `docs/hub.md`

**Interfaces:**
- Consumes: `AddProjectSource: Serialize` (Task 1); hub tools of the same names (Task 2).
- Produces: `commands::projects::routed::add_project(backend, AddProjectArgs, &Mutex<Store>, &Arc<SshClient>, &Arc<CancellationRegistry>) -> Result<ProjectTreeRow, IpcError>` and `commands::projects::routed::list_github_repos(backend, ListGithubReposArgs, &Mutex<Store>, &Arc<SshClient>) -> Result<Vec<GithubRepo>, IpcError>`.

- [ ] **Step 1: Write the failing routing cases**

In `src-tauri/src/backend/tests_routing.rs`, add a payload constant next to `HOST_PAYLOAD`:

```rust
/// A `ProjectTreeRow` as the hub answers `add_project`. `last_session_at` is
/// `None` and stripped, the way `ok_json_compact` sends it.
const PROJECT_TREE_PAYLOAD: &str = r#"{"project":{"id":7,"owner":"o","repo":"r","base_path":"/p/o/r","adopted":false,"system":false},"worktrees":[]}"#;
```

In `routed_read_cases()`, after the `refresh_projects` case:

```rust
        (
            "list_github_repos",
            "list_github_repos",
            json!({ "host_alias": "trn" }),
            r#"[{"name_with_owner":"acme/widget","is_private":true}]"#,
            Box::new(|b, s, h| {
                block_on(commands::projects::routed::list_github_repos(
                    b,
                    commands::projects::ListGithubReposArgs {
                        host_alias: "trn".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
```

In `routed_mutation_cases()`, after the `new_session` case. The `new` source with every field set proves the tagged enum crosses the wire, and `call_id: Some(123)` proves it does not:

```rust
        (
            "add_project",
            "add_project",
            json!({
                "host_alias": "trn",
                "source": {
                    "kind": "new",
                    "owner": "o",
                    "repo": "r",
                    "create_remote": true,
                    "confirm": "tok"
                }
            }),
            PROJECT_TREE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::projects::routed::add_project(
                    b,
                    AddProjectArgs {
                        host_alias: "trn".into(),
                        source: AddProjectSource::New {
                            owner: "o".into(),
                            repo: "r".into(),
                            create_remote: true,
                            confirm: Some("tok".into()),
                        },
                        call_id: Some(123),
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
```

Add to that function's `use` block: `use fleet_core::service::add_project::{AddProjectArgs, AddProjectSource};`.

- [ ] **Step 2: Run the routing tests to verify they fail**

```bash
cargo test -p claude-fleet --lib backend::tests_routing 2>&1 | tail -15
```

Expected: compile error — `routed::add_project` / `routed::list_github_repos` do not exist.

- [ ] **Step 3: Change the verdict rows**

In `src-tauri/src/backend/verdicts.rs`, replace the two `LocalOnly` rows with:

```rust
    (
        "add_project",
        Verdict::Routed {
            tool: "add_project",
        },
    ),
    (
        "list_github_repos",
        Verdict::Routed {
            tool: "list_github_repos",
        },
    ),
```

- [ ] **Step 4: Route the commands**

In `src-tauri/src/commands/projects.rs`, replace the module doc's last paragraph with:

```rust
//! Remote mode: all four commands route to the hub tools of the same names.
//! `add_project` and `list_github_repos` clone / run `gh` ON THE HOST over
//! the hub's transport to it, so the credentials are the host's, not this
//! machine's. `call_id` stays local: it keys this process's cancellation
//! registry, and on a hub the run simply completes (or hits its deadline)
//! after the desktop stops waiting.
```

Replace the two handler bodies:

```rust
#[tauri::command]
pub async fn add_project(
    args: AddProjectArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProjectTreeRow, IpcError> {
    routed::add_project(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn list_github_repos(
    args: ListGithubReposArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<GithubRepo>, IpcError> {
    routed::list_github_repos(&backend, args, &store, &ssh).await
}
```

Add to `mod routed`:

```rust
    /// `commands::projects::add_project`. `call_id` is this process's own
    /// cancellation-registry key and has no hub counterpart, so the hub
    /// branch spells the arguments out rather than serialising the struct.
    pub async fn add_project(
        backend: &FleetBackend,
        args: AddProjectArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<ProjectTreeRow, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "add_project",
                    &serde_json::json!({
                        "host_alias": args.host_alias,
                        "source": args.source,
                    }),
                )
                .await
            }
            None => add_project::add_project(args, store, &**ssh, reg).await,
        }
    }

    pub async fn list_github_repos(
        backend: &FleetBackend,
        args: ListGithubReposArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<GithubRepo>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "list_github_repos",
                    &serde_json::json!({ "host_alias": args.host_alias }),
                )
                .await
            }
            None => add_project::list_github_repos(&args.host_alias, store, ssh).await,
        }
    }
```

Make `ListGithubReposArgs` `pub` with a `pub host_alias` (it already is) so the test can build it.

- [ ] **Step 5: Run the routing tests to verify they pass**

```bash
cargo test -p claude-fleet --lib backend::tests_routing 2>&1 | tail -5
```

Expected: `test result: ok`. If a test complains that the handler body of a Routed command does not call `routed::`, re-read Step 4: both bodies must go through `routed::`.

- [ ] **Step 6: Regenerate the verdict artifacts**

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen 2>&1 | tail -3
cargo test -p claude-fleet --lib verdict_gen 2>&1 | tail -3
git diff --stat src/lib/hub_verdicts.generated.json docs/hub.md
```

Expected: both files change; `add_project` and `list_github_repos` move from `local_only` to `routed`, and the two rows leave the table in `docs/hub.md`.

- [ ] **Step 7: Desktop crate suite, clippy, fmt**

```bash
cargo test -p claude-fleet --lib 2>&1 | grep -E "test result|FAILED" | head
cargo clippy -p claude-fleet --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --all --check
```

Expected: `hub_contract` tests still pass (nothing on the wire changed yet); everything green.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/backend/verdicts.rs src-tauri/src/commands/projects.rs src-tauri/src/backend/tests_routing.rs src/lib/hub_verdicts.generated.json docs/hub.md
git commit -m "feat(desktop): route add_project and list_github_repos to the hub"
```

---

### Task 4: `GithubRepo` joins the wire contract; revision 5

**Files:**
- Modify: `src-tauri/src/backend/tests_contract.rs` (import, sample, `put`)
- Modify: `crates/fleet-core/src/wire_contract.rs:90` and its doc list
- Modify: `src-tauri/src/backend/contract.rs:107,116` and their doc comments
- Regenerate: `src-tauri/src/backend/hub_contract.golden.json`

**Interfaces:**
- Consumes: `fleet_core::service::add_project::GithubRepo` (Serialize + Deserialize, no serde defaults).

- [ ] **Step 1: Register the type in the contract test**

In `src-tauri/src/backend/tests_contract.rs`, add the import:

```rust
use fleet_core::service::add_project::GithubRepo;
```

Add a sample next to `sample_host_worktrees`:

```rust
fn sample_github_repo() -> GithubRepo {
    GithubRepo {
        name_with_owner: "acme/widget".into(),
        description: Some("w".into()),
        is_private: true,
        updated_at: Some("2026-09-01T10:00:00Z".into()),
    }
}
```

In `the_whole_contract()`, after `put("HostWorktrees", …)`:

```rust
    put("GithubRepo", wire_keys(&sample_github_repo()));
```

- [ ] **Step 2: Run the contract tests to see the golden mismatch**

```bash
cargo test -p claude-fleet --lib backend::tests_contract 2>&1 | grep -E "FAILED|GithubRepo|test result" | head
```

Expected: the golden comparison fails, naming `GithubRepo` as missing from the golden.

- [ ] **Step 3: Bump the revision**

In `crates/fleet-core/src/wire_contract.rs`, extend the doc list before the constant and change it:

```rust
//! - **5** — *a brand-new tool the desktop routes to.* `add_project` and
//!   `list_github_repos` become hub tools, and the desktop routes both to
//!   them instead of refusing them as local-only. A revision-4 hub serves
//!   neither: the sidebar's "Add project" would be enabled and every attempt
//!   would fail with an unknown-tool error. `GithubRepo` also joins the
//!   report types the desktop deserialises.
pub const CONTRACT_REVISION: u32 = 5;
```

In `src-tauri/src/backend/contract.rs`, extend the paragraph above `MIN_HUB_CONTRACT` and set both bounds:

```rust
/// Raised to 5 for revision 5: `add_project` and `list_github_repos` became
/// hub tools the desktop routes to. A revision-4 hub does not serve them, so
/// a desktop paired with one would offer "Add project" and fail every
/// attempt with an unknown tool. Refusing that hub with the skew banner says
/// what to do (update the hub); leaving it `InRange` would not.
pub const MIN_HUB_CONTRACT: u32 = 5;
```

and `pub const MAX_HUB_CONTRACT: u32 = 5;` (keep its existing doc comment).

Search the crate for tests that hard-code the bounds `4..=4` or `revision 4` (`grep -rn "MIN_HUB_CONTRACT\|MAX_HUB_CONTRACT\|hub_contract: 4" src-tauri/src src/lib`) and update any literal expectations they carry.

- [ ] **Step 4: Regenerate the golden and verify**

```bash
REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib backend::tests_contract 2>&1 | tail -3
cargo test -p claude-fleet --lib backend::tests_contract 2>&1 | tail -3
python3 -c "import json;d=json.load(open('src-tauri/src/backend/hub_contract.golden.json'));print(d['revision'], d['types']['GithubRepo'])"
```

Expected (second run): `test result: ok`; the print shows `5 ['description', 'is_private', 'name_with_owner', 'updated_at']`. The regen run itself may still report FAILED; the re-run is what counts.

- [ ] **Step 5: Whole workspace tests, clippy, fmt**

```bash
cargo test --workspace 2>&1 | grep -E "test result|FAILED" | head -20
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --all --check
```

Expected: all green. The hub e2e (`scripts/hub-e2e.sh`) is optional here and needs Homebrew bash first on PATH.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/wire_contract.rs src-tauri/src/backend/contract.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json
git commit -m "feat(contract): revision 5 — add_project routes, GithubRepo on the wire"
```

---

### Task 5: The sidebar opener follows the live link

**Files:**
- Modify: `src/lib/hub.ts:225-226` (remove the `REASONS` entry) and `:330-333` (`ROUTED_ACTIONS`)
- Modify: `src/lib/hub_verdicts.test.ts:184-186`
- Modify: `src/lib/Sidebar.svelte:635`
- Modify: `src/lib/Sidebar.test.ts` (the hub-client `describe` around line 1810)

**Interfaces:**
- Consumes: `hub_verdicts.generated.json` from Task 3 (both commands `routed`); `hubActionBlocked(action, status, conn)` from `hub.ts`.

- [ ] **Step 1: Install and see the generated-JSON tests fail**

```bash
pnpm install --frozen-lockfile
npx vitest run src/lib/hub_verdicts.test.ts 2>&1 | tail -20
```

Expected: FAIL — `add_project` is a `REASONS` key but no longer `local_only`; `list_github_repos` is allowlisted but no longer `local_only`.

- [ ] **Step 2: Write the failing sidebar tests**

In `src/lib/Sidebar.test.ts`, inside the `describe` that defines the `remote` `HubStatus` (the block with "shows the connection banner’s own sentence"), add:

```ts
  it('Add project is enabled on a connected hub client', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    const addRow = screen.getByTestId('add-project-row') as HTMLButtonElement;
    expect(addRow.disabled).toBe(false);
    expect(addRow.title).toBe('');
  });

  it('Add project is disabled with the offline sentence while the hub is unreachable', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'reconnecting', attempt: 2 });
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    const addRow = screen.getByTestId('add-project-row') as HTMLButtonElement;
    expect(addRow.disabled).toBe(true);
    expect(addRow.title).toContain('unreachable');
    expect(addRow.title).toContain('fleet.example.com');
  });
```

If `HubConnection`'s `reconnecting` shape carries more required fields than `attempt` (check `src/lib/hub_connection.ts`), fill them the way the existing tests in that file do.

- [ ] **Step 3: Run the sidebar tests to verify they fail**

```bash
npx vitest run src/lib/Sidebar.test.ts -t "Add project is" 2>&1 | tail -20
```

Expected: the first new test fails (the row is disabled with the old refusal sentence).

- [ ] **Step 4: Edit `hub.ts`**

Remove the two lines of the `add_project` entry from `REASONS` (keep `purge_project`). Add `'add_project',` to `ROUTED_ACTIONS` before `] as const;`.

- [ ] **Step 5: Edit `hub_verdicts.test.ts`**

Delete the three lines of `gatedByAddProjectDialog` (the comment pair and the entry).

- [ ] **Step 6: Edit `Sidebar.svelte`**

Replace line 635 and the comment above it with:

```svelte
  // Add project ROUTES to the hub now (the clone runs on the host through the
  // hub's transport), so it is gated on the live link like every other routed
  // mutation. Purge still uses this machine's SSH and stays refused.
  const addProjectBlocked = $derived(hubActionBlocked('add_project', $hubStatus, $hubConnection));
  const purgeProjectBlocked = $derived(hubBlock('purge_project', $hubStatus));
```

- [ ] **Step 7: Run the three test files and the type check**

```bash
npx vitest run src/lib/hub_verdicts.test.ts src/lib/Sidebar.test.ts src/lib/hub.test.ts 2>&1 | tail -8
npx svelte-check 2>&1 | tail -3
```

Expected: all pass; svelte-check reports 0 errors.

- [ ] **Step 8: Commit**

```bash
git add src/lib/hub.ts src/lib/hub_verdicts.test.ts src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "feat(ui): Add project is routed on a hub client, gated on the live link"
```

---

### Task 6: The dialog on a hub client: no folder source, every host is remote

**Files:**
- Modify: `src/lib/AddProjectDialog.svelte:37-42, 63, 145-155, 200, 327`
- Modify: `src/lib/AddProjectDialog.test.ts`

**Interfaces:**
- Consumes: `hubStatus` and `STANDALONE` from `./hub`.

- [ ] **Step 1: Write the failing tests**

In `src/lib/AddProjectDialog.test.ts`, add the import:

```ts
import { hubStatus, STANDALONE } from './hub';
```

In the file's `beforeEach`, add `hubStatus.set({ ...STANDALONE });`. Then add a `describe`:

```ts
describe('on a hub client', () => {
  const remote = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
  };

  it('offers no Existing folder source (it would be a folder on the hub machine)', async () => {
    hubStatus.set(remote);
    mount();
    await tick();
    expect(screen.queryByTestId('add-mode-folder')).toBeNull();
    for (const m of ['clone', 'github', 'new']) {
      expect(screen.getByTestId(`add-mode-${m}`)).toBeInTheDocument();
    }
  });

  it('offers Existing folder standalone', async () => {
    mount();
    await tick();
    expect(screen.getByTestId('add-mode-folder')).toBeInTheDocument();
  });

  it('says the host may still finish when stopping a run, even on the hub’s local host', async () => {
    hubStatus.set(remote);
    hosts.set([{ alias: 'local', reachable: true, hidden: false } as any]);
    const inflight = deferred<typeof row>();
    route({ add_project: () => inflight.promise });
    mount();
    await tick();
    await fireEvent.input(screen.getByTestId('add-clone-url'), { target: { value: 'https://github.com/o/r' } });
    await fireEvent.click(screen.getByTestId('add-submit'));
    await tick();
    expect(screen.getByTestId('add-project-dialog').textContent).toContain('may still finish');
  });
});
```

Use the existing test ids for the URL field and submit button (search the file for `add-clone-url` / `add-submit`; if the ids differ, use the ones the existing clone tests use — the behaviour under test is the note's text, not the ids).

- [ ] **Step 2: Run the dialog tests to verify they fail**

```bash
npx vitest run src/lib/AddProjectDialog.test.ts -t "hub client" 2>&1 | tail -20
```

Expected: the first and third tests fail.

- [ ] **Step 3: Edit the dialog**

Imports: add `import { hubStatus } from './hub';`.

Modes: rename the constant and derive the visible list:

```ts
  const ALL_MODES: { id: Mode; label: string }[] = [
    { id: 'clone', label: 'Clone URL' },
    { id: 'github', label: 'My GitHub' },
    { id: 'folder', label: 'Existing folder' },
    { id: 'new', label: 'New project' },
  ];
  // On a hub client `local` is the hub's machine, and the folder picker is
  // this machine's — so an existing folder cannot be offered there.
  const MODES = $derived($hubStatus.remote ? ALL_MODES.filter((m) => m.id !== 'folder') : ALL_MODES);
```

Remote host rule, next to `const host = …`:

```ts
  /** Whether stopping cannot reach the run: any host but this machine's own
   *  `local`. On a hub client every host is remote, `local` included. */
  const hostIsRemote = (h: string) => h !== 'local' || $hubStatus.remote;
```

`inflightNote`: replace `if (inflight.host === 'local') {` with `if (!hostIsRemote(inflight.host)) {`.

The `E_CANCELLED` branch: replace `h === 'local' ? 'Cancelled.'` with `!hostIsRemote(h) ? 'Cancelled.'`.

The `<AddProjectActions … remote={inflight !== null && inflight.host !== 'local'}` prop: replace with `remote={inflight !== null && hostIsRemote(inflight.host)}`.

- [ ] **Step 4: Run the dialog tests and the type check**

```bash
npx vitest run src/lib/AddProjectDialog.test.ts 2>&1 | tail -6
npx svelte-check 2>&1 | tail -3
```

Expected: all pass, 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/AddProjectDialog.svelte src/lib/AddProjectDialog.test.ts
git commit -m "feat(ui): the Add-project dialog on a hub client hides the folder source"
```

---

### Task 7: Docs, the full local CI, the PR

**Files:**
- Modify: `docs/hub.md` (the *What is different from standalone* list, around line 1813)
- Modify: `CLAUDE.md` (the hub-client paragraph in *Architecture*, one clause)

- [ ] **Step 1: Docs**

In `docs/hub.md`, add a bullet to *What is different from standalone* after **The fleet is the hub's.**:

```markdown
- **Projects are added through the hub.** "＋ Add project…" clones or
  creates the repository on the host you pick, with that host's `git` and
  `gh`; the new row arrives like any other change. Cancel stops the desktop
  waiting, not the run on the host. The *Existing folder* source is absent
  here, because it would mean a folder on the hub's machine.
```

In `CLAUDE.md`, in the *Hub client mode* bullet, after "every command routes to a hub tool, refuses with `E_LOCAL_ONLY`, or is the same in both modes", nothing structural changes; add to the *Status & known issues* hub paragraph one sentence: "Since contract revision 5 a hub client adds projects through the hub (`add_project` / `list_github_repos` tools), per `docs/superpowers/specs/2026-09-27-hub-add-project-design.md`."

- [ ] **Step 2: Full local CI, both halves, unpiped**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/nostalgic-bardeen-635719
PATH=/opt/homebrew/bin:$PATH scripts/ci-local.sh
```

Expected: every step green. Do not judge it through `| tail`; read the whole output. If `cargo deny` is missing, install it (`cargo install cargo-deny --locked`) rather than skipping.

- [ ] **Step 3: Commit and push**

```bash
git add docs/hub.md CLAUDE.md
git commit -m "docs(hub): a hub client adds projects through the hub"
git push -u origin feat/hub-add-project
```

- [ ] **Step 4: Open the PR**

```bash
gh pr create --base main --head feat/hub-add-project --title "feat: add a project from a hub client" --body-file /dev/stdin <<'EOF'
## Why
A desktop paired with a hub could not add a project: `add_project` was LocalOnly, and the hub had no tool, CLI or scan that could register one on `mac`, `mefistos` or an agent host. The refusal's advice ("do it on the hub") pointed at nothing.

## What
- `add_project` and `list_github_repos` are hub tools (`mcp/tools/repo.rs`), thin over the existing service, which already runs `git`/`gh` on the target host.
- The desktop routes both (`verdicts.rs`, `commands/projects.rs`); `call_id` stays local, so Cancel stops waiting and the hub run completes.
- Contract revision 5 (`GithubRepo` on the wire; a revision-4 hub does not serve the tools).
- The sidebar opener is gated on the live link; the dialog hides *Existing folder* on a hub client.

Spec: `docs/superpowers/specs/2026-09-27-hub-add-project-design.md`. Plan: `docs/superpowers/plans/2026-09-27-hub-add-project.md`.

## Rollout
Deploy the hub first (the desktop refuses a revision-4 hub with the skew banner), then the desktop.
EOF
```

Then wait for GitHub CI on the final head (local clippy is older than CI's) and report the result. Merging needs the user's explicit go.

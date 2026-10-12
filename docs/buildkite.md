# The persistent Buildkite builder

An agent verifies a change in three steps (CLAUDE.md → *Validation ladder*):
`cargo fleet-fast-check` while it works, `scripts/verify.sh` before a commit,
and the full gate before a push. The full gate costs about 3 minutes on a
warm 4-core machine, and a new worktree pays roughly 13 minutes of cold
compiles first (RUST-BUILD-PERFORMANCE-AUDIT.md, Appendix K). A persistent
builder takes that off the agent's machine:

```bash
git push -u origin <branch>
scripts/verify.sh remote        # = scripts/buildkite-verify.sh
```

`verify.sh remote` asks Buildkite for a build of the pushed HEAD, waits for
it, prints each state change, and exits 0 only when the build passed. On a
failure it prints the end of each failed job's log, so the agent can fix the
change without opening Buildkite. `--no-wait` starts the build and prints its
URL; Ctrl-C cancels it. On a detached HEAD, name the branch:
`--branch <name>` (and `--commit <ref>` for a commit other than HEAD).

The builder runs `scripts/verify.sh full` (= `scripts/ci-local.sh`, narrowed
to the jobs the change touches) against a `target/` that survives between
builds, with incremental compilation on. Measured on 4 vCPU in Appendix I: a
typical next commit 3–4 minutes against 6:37 on a GitHub-hosted runner, and
2:27 when no Rust changed.

Builds run only when an agent (or a person) asks: pushes do not trigger
them, and GitHub Actions CI is unchanged.

## How it fits together

| Piece | Where | What it does |
|---|---|---|
| `scripts/buildkite-verify.sh` | the agent's machine | creates the build over the REST API, polls it, prints failed logs |
| `.buildkite/pipeline.yml` | the repo | one step on queue `rust-persistent`, concurrency 1 |
| `scripts/buildkite-step.sh` | the builder | guards (target outside the checkout, incremental on, free disk), then `verify.sh full` |
| `deploy/buildkite/hooks/environment` | the builder | fixed checkout path; `CARGO_HOME`, `RUSTUP_HOME`, `CARGO_TARGET_DIR` and the pnpm store under `/srv/ci` |
| `deploy/buildkite/buildkite-agent.cfg` | the builder | one agent (`spawn=1`), tag `queue=rust-persistent` |

Why each rule (Appendix I.5):

* **Everything reused lives outside the checkout.** Buildkite cleans the
  checkout with `git clean -ffxdq` before each build, which deletes ignored
  files. Nothing worth keeping is in it, so the default is fine.
* **The checkout path is fixed.** Cargo's fingerprints and incremental state
  record absolute source paths, and about 150 tests read files through the
  compile-time `CARGO_MANIFEST_DIR`. A moving checkout would rebuild
  everything.
* **One agent, one target dir, one build at a time.** Two builds writing the
  same `target/` corrupt each other's state. A second agent needs its own
  `CARGO_TARGET_DIR` and about 25 GB more disk.
* **Incremental on.** GitHub-hosted runners set `CARGO_INCREMENTAL=0`, which
  suits a throwaway machine. The hook unsets it, and the step refuses to run
  with it set.

## Set up the builder (Linux x86_64)

The commands assume Ubuntu 24.04 and a user `buildkite-agent`, which the
agent's package creates.

1. **Disk.** Plan for 100–200 GB of NVMe under `/srv/ci`. This is a sizing
   recommendation, not a measured minimum (Appendix I.4). The step frees
   space itself below `BUILDER_MIN_FREE_GB` (default 30): first the
   incremental caches, then the whole target dir.

   ```bash
   sudo mkdir -p /srv/ci && sudo chown buildkite-agent: /srv/ci
   ```

2. **The Buildkite agent.** Install it with Buildkite's own Linux
   instructions (Buildkite → Agents → *Install*), then copy this
   repository's configuration and hook:

   ```bash
   sudo cp deploy/buildkite/buildkite-agent.cfg /etc/buildkite-agent/buildkite-agent.cfg
   sudo install -m 755 deploy/buildkite/hooks/environment /etc/buildkite-agent/hooks/environment
   sudoedit /etc/buildkite-agent/buildkite-agent.cfg   # the agent token
   ```

3. **System packages.** These are the Tauri prerequisites from `ci.yml`,
   plus what the scripts call:

   ```bash
   sudo apt-get install -y --no-install-recommends \
     libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev \
     libssl-dev libayatana-appindicator3-dev librsvg2-dev \
     libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev pkg-config \
     git python3 tmux sqlite3 minisign
   ```

   `minisign` is optional. Without it `ci-local.sh` skips
   `release-update-scripts-test.sh`.

4. **Rust, as `buildkite-agent`, into the persistent directories.** Run
   this from the repository root: it installs the toolchain pinned in
   `rust-toolchain.toml` as the default. Without a default toolchain,
   `cargo install` outside a checkout fails with *rustup could not choose a
   version of cargo to run*. Later pin bumps install themselves on the first
   `cargo` call in the checkout.

   ```bash
   pin=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
   sudo -u buildkite-agent -H bash -c '
     export CARGO_HOME=/srv/ci/cargo-home RUSTUP_HOME=/srv/ci/rustup
     curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path \
       --profile minimal --default-toolchain "$1" -c clippy,rustfmt
     /srv/ci/cargo-home/bin/cargo install cargo-deny --locked' _ "$pin"
   ```

5. **Node 22 and pnpm 10.** Use any Node 22 install (NodeSource, nvm, or a
   distro package) on the agent's `PATH`, then `sudo corepack enable`. pnpm's
   version comes from `packageManager` in `package.json`.

6. **Start the agent:** `sudo systemctl enable --now buildkite-agent`.

## Create the pipeline

In Buildkite, create a pipeline named **claude-fleet** for this GitHub
repository. Its slug must match `BUILDKITE_PIPELINE`, which defaults to
`claude-fleet`.

* **Steps** (in the pipeline settings). Only the upload step lives there; it
  runs on the same queue:

  ```yaml
  steps:
    - label: ":pipeline: upload"
      command: buildkite-agent pipeline upload .buildkite/pipeline.yml
      agents:
        queue: rust-persistent
  ```

* **GitHub settings.** Turn off building on pushes and on pull requests.
  Builds are created only through the API. Commit statuses are optional:
  the agent reads the result from the API, not from GitHub.

## Give the agents access

Create an API access token (Buildkite → Personal settings → *API Access
Tokens*), limited to this organisation, with the scopes `read_builds`,
`write_builds` and `read_build_logs`. Every machine an agent verifies from
needs:

```bash
export BUILDKITE_API_TOKEN=bkua_…      # the token above
export BUILDKITE_ORG=<organisation slug>
# export BUILDKITE_PIPELINE=claude-fleet   # only if the slug differs
```

Put these in the agent host's shell profile, or in the `env` block of the
Claude Code settings the agent runs with. Never put them in a file in this
repository. `scripts/buildkite-verify.sh --help` lists the optional
variables: API URL, poll interval, log lines and timeout.

## Check it

1. From a pushed branch: `scripts/verify.sh remote`. The first build is
   cold, about 10 minutes on 4 vCPU (Appendix I.2).
2. Push a one-line change and run it again. This is the warm next commit
   that Appendix I.5 asks to measure, against the GitHub-hosted legs (ubuntu
   6:37).
3. Make a deliberate test failure, and check that the agent sees the failing
   test's lines in the output.

`scripts/buildkite-verify-test.sh` tests the client against a fake API,
with no Buildkite involved. It runs in CI's `version-consistency` job.

## Operating it

* **Free space:** `rm -rf /srv/ci/target/claude-fleet`. The next build is
  cold.
* **Toolchain bump** (`rust-toolchain.toml`): nothing to do. rustup installs
  the new pin on the next build, and cargo rebuilds everything once. Old
  toolchains stay in `/srv/ci/rustup` until `rustup toolchain uninstall`.
* **A second agent:** a second `buildkite-agent.cfg` (or `spawn=2`) needs its
  own checkout path and its own `CARGO_TARGET_DIR`. Key both on
  `BUILDKITE_AGENT_NAME` in the environment hook, and keep `concurrency: 1`
  per target.
* **Limits:**
  * The builder builds what GitHub has. A branch that isn't pushed is
    refused, and uncommitted changes are not built (the client warns).
  * One build at a time; others queue.
  * macOS and Windows still run only in GitHub Actions.

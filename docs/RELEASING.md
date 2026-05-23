# Releasing claude-fleet

Releases are automated with [release-please](https://github.com/googleapis/release-please).
You never bump versions or write the changelog by hand.

## How it works

1. Land changes on `main` using **Conventional Commits** (see below).
2. The `release-please` workflow watches `main` and maintains an open
   **"chore: release X.Y.Z"** pull request. It computes the next semantic
   version from the commit types, updates every version file, and rewrites
   `CHANGELOG.md`.
3. When you're ready to ship, **merge that PR**. release-please creates the git
   tag (`vX.Y.Z`) and a **GitHub Release** with the generated notes.
4. Publishing the Release triggers the `docs` workflow, which builds rustdoc and
   deploys it to GitHub Pages.

No binaries are built or attached — releases are tag + notes only.

## Conventional Commits

Commit subjects drive the version bump and changelog section:

| Prefix      | Bump  | Changelog section |
|-------------|-------|-------------------|
| `feat:`     | minor | Features          |
| `fix:`      | patch | Bug Fixes         |
| `perf:`     | patch | Performance       |
| `refactor:` | patch | Refactors         |
| `docs:`     | patch | Documentation     |
| `chore:` / `test:` / `ci:` | none | hidden |

A `feat!:` / `fix!:` or a `BREAKING CHANGE:` footer triggers a **major** bump.

## Version files kept in sync

release-please updates all four on each release:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

Config lives in `release-please-config.json` (a `rust`-type package rooted at
`src-tauri/`, with `package.json` and `tauri.conf.json` as JSON `extra-files`)
and `.release-please-manifest.json` (the current version).

## Generated docs

`docs/control-api-reference.md` is **generated** from the live MCP tool router —
do not edit it by hand. After changing any `#[tool(...)]` description or the
`generate_handler!` command list, regenerate and commit it, or CI fails:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

## One-time setup

- **Settings → Actions → General → Workflow permissions:** enable
  "Allow GitHub Actions to create and approve pull requests".
- **Settings → Pages:** set Source = "GitHub Actions" (for the rustdoc site).

## Troubleshooting

- **No release PR appears:** check the workflow permissions setting above; check
  the `release-please` workflow run logs.
- **A version file didn't update:** confirm its path/jsonpath in
  `release-please-config.json`. If `Cargo.lock` is not updated by the `rust`
  type for this layout, add a `generic` extra-file targeting the
  `name = "claude-fleet"` package entry's `version` line in `Cargo.lock`.
- **Wrong bump computed:** check the commit prefixes since the last tag.

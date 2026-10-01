# Independent check: does the Fleet Projects seam fit the permissions system?

You are checking whether one design's assumptions survive contact with another
design that is being built in parallel. **Disagreement is the useful outcome.** If
the two cannot be reconciled, say so and say which should give way.

## Before you start — where everything is

The spec under review is **not on `main`**. It lives only on a feature branch, so a
fresh clone of the default branch will not contain it.

| What | Where |
|---|---|
| Repository | `martin-janci/claude-fleet` |
| Branch holding the spec | `claude/youthful-heisenberg-lnexvr` — read its **head**, not a pinned commit |
| The spec | [docs/superpowers/specs/2026-10-01-fleet-projects-design.md](https://github.com/martin-janci/claude-fleet/blob/claude/youthful-heisenberg-lnexvr/docs/superpowers/specs/2026-10-01-fleet-projects-design.md) |
| This review request | `docs/superpowers/reviews/` on the same branch |

To get it:

```bash
git clone --branch claude/youthful-heisenberg-lnexvr https://github.com/martin-janci/claude-fleet.git
# or, in an existing clone:
git fetch origin claude/youthful-heisenberg-lnexvr && git checkout claude/youthful-heisenberg-lnexvr
```

If you cannot clone, read the files through the GitHub API or the blob URL above.
Should that URL 404 because the branch name contains slashes, use a commit form
instead: `.../blob/<sha>/<path>`, taking `<sha>` from the branch head (it was
`2b2b7138` when this request was written, and the spec may have moved forward
since — prefer the head).

**Which files are where** — this matters, because two of the documents named below
are also not on `main`:

- Everything under `crates/`, `src-tauri/`, `tools/`, `CLAUDE.md`, and every
  `docs/superpowers/specs/2026-09-*` document **is on `main`** and also on this
  branch. Reading them from either is fine.
- `docs/superpowers/specs/2026-10-01-fleet-projects-design.md` and the review
  requests exist **only on `claude/youthful-heisenberg-lnexvr`**.
- `docs/superpowers/plans/2026-09-30-assets-m2-sync.md` and
  `crates/fleet-core/migrations/091_catalog_ids.sql` exist **only on the branch
  `feat/assets-m2-sync`**, which is open pull request
  [martin-janci/claude-fleet#416](https://github.com/martin-janci/claude-fleet/pull/416).
  Read them from that branch or from the pull request.

If the repository is unreachable to you entirely, say so rather than guessing —
but note that the vendor-documentation half of this check (the claims about what
the Claude Code CLI and its docs do or do not support) stands on its own and can
still be answered.

## The two sides

**Side A — Fleet Projects** (a draft spec, nothing built):
`martin-janci/claude-fleet`, `docs/superpowers/specs/2026-10-01-fleet-projects-design.md`.
Read `CLAUDE.md` first, then that spec's §5, §9 and §16 Q5.

Its position, in short:

- A Project is **not** an access boundary. Org stays the only one
  (`service::orgs::OrgScope`, built only by `Caller::org_scope`).
- It models **no** subject, role or grant table — no `people`, no members.
  Membership is deliberately absent.
- Attribution uses the word `service::settings::Actor` already produces
  (`Person`, `PersonVia(client)`, `Agent(name)`, `System`), stored in
  `fleet_projects.updated_by` and `fleet_project_context.author`, which is the
  convention `settings::set_by` and `work_placements.updated_by` already use.
- It claims a real subject id can be added **beside** that word later as a
  nullable column, the way migration 086 added `origin` (backfill a derived
  value, read NULL as a default), so phase FP0 need not wait.

**Side B — the permissions and synchronisation system now in development.** Find
it rather than assuming where it is. Start with the open pull requests and the
recently-updated branches of `martin-janci/claude-fleet`, and with
`docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md` plus
`docs/superpowers/plans/2026-09-30-assets-m2-sync.md`. The pieces already visible
are: per-org `catalogs` (migration 090), asset `scope: private | shared`, the pure
`acceptance()` rule and `effective_for_host()` (migration 091, assets M2), and
milestone M3's `host_catalogs` (host admissions) and `client_catalog_grants`
(per-client, per-catalog grants). Access today is held by **devices and hosts**
(`client_tokens` with `mode`/`trusted_at`/`org_id`/`assets_admin_at`,
`host_tokens`, the master) — there is no person entity.

## What to answer

Side A says it needs four things from Side B (§9). Judge each one: is it what Side
B is actually building, is it incompatible, or is it a question Side B has not
reached yet?

1. A **stable subject id** for a person, resolvable from a caller, able to replace
   the `Actor` word without losing history.
2. Somewhere to express **"this person is on this Project"** — a role on the
   subject, a group, or a grant — that Projects *reads* and never copies.
3. A rule for how a Project-level role **composes with the org boundary**. Side A
   needs intersection: a Project never grants what the org denies.
4. Whether `client_catalog_grants` is the **same mechanism** as a Project's
   membership or a different one, given that a Project's assets ride a catalog
   (Side A §12).

Then the two questions that actually matter:

- **Is Side A's "subject id is additive" claim true?** Check how a subject would
  have to be keyed. If Side B's subject turns out to be a client token, a host, an
  org member or something external (an SSO identity, a tracker account), does the
  `Actor` word still backfill into it, or is the history unrecoverable? Name the
  migration that would be needed and how large it is.
- **Is Side A right to have no membership at all in v1?** Or does leaving it out
  force a shape that will have to be undone — for example, is `fleet_projects.org_id`
  with `ON DELETE RESTRICT` the right coupling if Side B later makes org
  membership itself a first-class thing?

## How to answer

- One verdict per numbered requirement: **Side B already provides it / will
  provide it / conflicts with it / has not reached it**, with the `file:line` or
  document section it rests on.
- A yes/no on whether FP0 can land its attribution columns now without regret, and
  if no, exactly what it should land instead.
- Anything Side A assumes about the permissions system that is simply **not true**.
- If you find the parallel system somewhere other than the places named above, say
  where and what it actually does — do not force it into this framing.

Do not edit the repository, push, or comment on any pull request. This check is
read-only; report back in chat.

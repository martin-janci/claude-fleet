# Writing a claude-fleet guide

A **guide** is a page in claude-fleet's Settings → Guides that walks a
person through one task, one step at a time (Back / Next / Done). It is not
prose: it is a small JSON spec (`fleet.page/1`, `"layout": "guide"`) that
only *names* things fleet already knows — settings, pages, page actions and
read-only numbers. Fleet draws every control itself, from its own registry:
the label, help, bounds, units and confirmations of each setting.

You write the spec and **propose** it through the `claude-fleet` MCP server's
`guide` tool. Nothing is shown until a person approves it. You cannot
approve, and must not try.

## The workflow

1. **Read the catalog once:** `guide { "action": "catalog" }`. It returns:
   - the rules (id pattern, parent, max steps, text limits);
   - `items`: the item types a step may hold;
   - `settings`: every setting's `key`, `label`, `help`, `kind` (with its
     bounds or options), `unit`, `default`, `zero_means` (what `0` does,
     when it means something) and `page` (the page it lives on) — never
     its current value, which may differ per fleet;
   - `pages`: every page id a `link` may name, live guides included;
   - `actions`: page actions (buttons that run one fleet command);
   - `sources`: read-only numbers a `stat` / `record` item may show;
   - `example`: a guide that checks. Start from its shape.
2. **Plan the steps** before writing JSON (see *A good guide* below). Choose
   settings by their `label`, `key` and `help`. **Read `help` before you
   recommend a value**: it says what the number counts (a `count` of patch
   releases is not a count of versions) and when the check applies. Anchor
   a recommendation on `default` — say when to move away from it and which
   way — and never contradict `help`. **Never invent a key or a fact**: if
   the task needs a setting the catalog does not list, the guide cannot
   change it — say so in a `notice`; if you do not know how fleet behaves,
   leave the claim out rather than guess. A notice states only what `help`
   or the catalog says; your own advice goes in a `hint`, worded as advice
   ("Keep 90 % unless…"). No `zero_means` means `0` is not a special value:
   do not recommend it.
3. **Write the spec** (shape below).
4. **Validate:** `guide { "action": "validate", "spec": {…} }`. The answer is
   `{ ok, page_id, problems }`; each problem says where it is
   (`guide.x › section 2 › item 1: …`). Fix every one and validate again.
   If it still fails after three rounds, stop and show the user the problems.
5. **Propose:** `guide { "action": "propose", "spec": {…}, "why": "…" }`.
   `why` is one or two sentences for the person who reviews it (≤500
   characters): what the guide is for and who asked. The answer has the
   proposal `id`.
6. **Tell the user** it waits for review: in the app, Settings → Guides
   (Read the steps, then Approve); on a hub, `fleet-hub guides show <id>`
   and `fleet-hub guides approve <id>`. Do not say it is live.

To **revise** a guide, propose it again with the same `id`: a new proposal
replaces a pending one, and an approved one stays live until its revision
is approved. `guide { "action": "list" }` shows the live guides and what
waits; its `can_write` says whether *this caller* may approve, and for a
session on a host it is always `false` — that is expected, and proposing
still works.

## Shape

```json
{
  "spec": "fleet.page/1",
  "id": "guide.cleanup",
  "title": "Let fleet tidy up idle sessions",
  "parent": "guides",
  "layout": "guide",
  "intro": "Three steps: what cleanup does, turn it on, and when it acts.",
  "sections": [
    { "title": "What it does", "items": [
      { "type": "notice", "tone": "info",
        "text": "Garbage collection stops background sessions that sat idle, so they stop using a host." } ] },
    { "title": "Turn it on", "items": [ { "type": "field", "key": "gc.enabled" } ] },
    { "title": "When it acts", "when": { "key": "gc.enabled", "truthy": true },
      "items": [
        { "type": "field", "key": "gc.bg_idle_secs", "hint": "A day suits most fleets." },
        { "type": "link", "page": "settings.automation", "label": "Every automation setting" } ] }
  ]
}
```

- `id` is `guide.<lowercase_name>` (dots and underscores allowed); `parent`
  is always `"guides"`; `layout` is always `"guide"`.
- `sections` are the **steps**, in order: 1–12, each with a distinct
  `title`, never `tabs`, never `collapsible` / `advanced`.
- A step's `when` hides it until the condition holds — use it for a step
  that only matters after an earlier choice. Forms (one per condition):
  `{"key": K, "eq": V}`, `{"key": K, "in": [V, …]}`,
  `{"key": K, "truthy": true}`, `{"all": […]}`, `{"any": […]}`,
  `{"not": {…}}`. `eq` / `in` values must be values the setting can take
  (a choice's options, `"true"` / `"false"`).

Items a step may hold:

| `type` | Fields | Use for |
|---|---|---|
| `field` | `key`, optional `hint` (≤120 chars), `when` | One setting the person changes here. Saved as they change it |
| `notice` | `tone` (`info` / `warn` / `danger`), `text` (≤300 chars) | Why, what happens, what to check |
| `link` | `page`, optional `label` | Where to go next, or where the rest of a topic lives |
| `action` | `action` (an id from `actions`) | A button the person presses, e.g. a sweep |
| `stat` | `source` `{ "id": … }`, `field` for a record source, optional `label` | One number that shows the state |
| `record` | `source` | A few key → value lines |

Every text is **plain**: no `<` or `>`, no markup, no Markdown. The
guide's `title` and every step's `title` are at most 60 characters; the
guide's `intro`, a step's `intro` and every notice's `text` at most 300; a
`hint` at most 120. The whole spec is at most 16 KiB. A setting appears at
most once per guide.

A field shows its value in the setting's `unit`: a `secs` setting with
unit `hours` is typed in hours, so write hints in that unit ("6 hours"),
never in raw seconds. A `percent` is typed as a number of percent.

## A good guide

- **One task, 3–6 steps.** First step: why, in one notice. Middle steps: one
  decision each, with the field(s) for it. Last step: how to check it
  worked, and a `link` to the settings' own `page` from the catalog, where
  the rest of the topic lives. A `stat` / `record` shows the state when a
  source for it exists; otherwise describe the check in a notice, naming
  only screens the catalog's `pages` list.
- **Say which value to pick, and why**, in the field's `hint` — the registry
  already shows the label, the help and the range, so do not repeat them.
- **Warn before a consequence**: a `warn` notice next to a setting that
  stops sessions, deletes rows or costs money.
- **Hide what does not apply**: a `when` on the steps that only matter once
  a switch is on.
- **Write in the language of the request** (Slovak if asked in Slovak);
  setting labels, page titles and the status words `help` quotes (such as
  "disk low") stay as fleet shows them, so the person finds them on screen.
- A guide changes nothing by itself: every write is the person's, through
  the field. Never put a token, password or secret in a guide.

## When something is refused

- `E_INVALID` on propose: the spec does not check; the message lists every
  problem. Validate, fix, propose again.
- `E_RATE_LIMITED`: 20 guides already wait. Tell the user; a person decides
  those first.
- `E_FORBIDDEN` on `decide` / `remove`: those are a person's. Do not retry.
- No `claude-fleet` MCP server (the tool is missing): this session is not
  running under claude-fleet. Say so, and give the user the spec to propose
  from a session that is.
- The server IS there and `guide` is missing from it: this host's token is
  `readonly`, which is refused the whole tool — reads included, since one of
  its actions proposes. Not the same thing as "claude-fleet is not running
  here". Say which it is; only the operator can change the token's mode.

# Decisions (Jev) — experimental, off

fleet can ask a **decision model** — [Jev](https://docs.typesafe.ai/api.md),
TypeSafe AI's "System One" model — a closed-set question: which of these
tickets is this session working on, which status category is this Asana
section. This page is the user guide for that path. It is an **evaluation**
(decisions D31–D47 in
`docs/superpowers/specs/2026-09-27-jev-language-census-design.md`): everything
is **off by default**, and nothing in fleet asks Jev yet. The envelope
described here is what the use cases will go through when they are built.

**A model answer never grants a permission and never runs a risky action.**
At most it pre-selects a suggestion a person confirms.

## What has to be true before anything is sent

A request leaves the machine only when **all** of these hold, checked in this
order on every call. The first that fails is the call's *fallback* (below),
and nothing is sent:

1. **This process owns the fleet** (`not_owner`). A hub, or a standalone
   desktop. A desktop paired to a hub, a phone and an agent host never call
   out: the hub decides for them.
2. **The kill switch is on** (`flag_off`): `decide.jev.enabled`, the toggle
   in Settings → *Decisions (Jev)*. Turning it off stops every call at once.
3. **The feature's mode is not `off`** (`mode_off`): `decide.jev.status_map`,
   `decide.jev.work_link`.
4. **The org consented** (`org_off`). Each organisation opts in on its own
   (Settings → Organisations → *send to Jev*, or `fleet-hub org set <id>
   --jev on`); off by default. A session or item with no org follows
   `decide.jev.unassigned` (off by default).
5. **A key is configured** (`no_key`): `fleet-hub decide set-key`.
6. **The circuit breaker is closed** (`breaker_open`): after
   `decide.jev.breaker_failures` failed calls in a row (timeouts, HTTP errors,
   rate limits) no call is made for `decide.jev.breaker_open_secs`; then one
   call goes through, and a success closes it.
7. **Today's budget is not spent** (`budget`): `decide.jev.daily_token_budget`
   input tokens per UTC day, counted from the record.

## Modes

| Mode | What the feature does with an answer |
|---|---|
| `off` | Nothing is asked. |
| `shadow` | Asks and records the answer next to what the current rule decided; acts on the rule only. For comparison. |
| `assist` | Asks, records, and may **propose** the answer (a pre-selected suggestion); a person confirms. |

`auto` (acting without a person) is not offered: no feature has passed
acceptance.

## What a call does

- **Redaction.** Every text in the request is redacted first: URLs become
  `[url]`, email addresses `[email]`, and tokens and keys fleet recognises
  (the same patterns as its logs) `[REDACTED]`. A feature may redact more.
- **One attempt.** Bounded by `decide.jev.timeout_ms`, never retried on the
  path that asked.
- **Checked answers.** A choice must be one of the options offered, the
  probabilities a distribution over them, a score inside its levels;
  anything else is `invalid_answer` and is not used. A feature may set a
  confidence floor: below it the answer is recorded as `low_confidence` and
  not used.
- **Pinned model.** `decide.jev.model` is `jev-1.13.0` by default: TypeSafe
  advises pinning a version when thresholds are tuned against it.

## Fallbacks

Every call ends answered or with one of these; the feature then does what it
does today (its rule, or nothing):

| Fallback | Meaning |
|---|---|
| `not_owner` | This process is a window onto a hub. |
| `flag_off` | `decide.jev.enabled` is off. |
| `mode_off` | The feature's mode is `off`. |
| `org_off` | The org has not consented (or no org, and `decide.jev.unassigned` is off). |
| `no_key` | No key, or its `env:` / `file:` reference cannot be read. |
| `breaker_open` | Too many failed calls in a row, recently. |
| `budget` | Today's input-token budget is spent. |
| `timeout` | No answer within `decide.jev.timeout_ms`. |
| `http_error` | The API refused (401, 422, 5xx), the connection failed, or the request failed a local check the API would refuse (too many options, too large). |
| `rate_limited` | 429 or 529 from the API. |
| `invalid_answer` | An answer that does not fit the question. |
| `low_confidence` | A valid answer below the feature's confidence floor. |

## What is recorded — and what never is

Every call is recorded in `decision_runs`, **fallbacks included**, so a
shadow comparison sees how often the model was not asked:

- when, the feature, the org, what was decided about (`session 42`,
  `tracker 3`), the mode, the provider, the model version that answered, the
  feature's question version;
- the **candidates** offered (ids and vocabulary words only), the answer, its
  probabilities and confidence, the fallback, and what the current rule
  decided (the baseline);
- whether a request was sent, the latency, the input tokens and their cost;
- later, what became of it: `confirmed`, `rejected`, `corrected` (and to
  what), or `ignored`.

**Never raw text.** No prompt, no ticket title, no description, no
instruction. The store refuses a run whose fields are not ids, words or
numbers. The input is kept only as a **fingerprint**: an HMAC-SHA256 of the
redacted request under a key generated on this machine and never sent
anywhere — so identical inputs can be matched, but a guessed prompt cannot be
confirmed from the record without that key.

The API key itself is stored in `decision_secrets` (or as a reference to an
environment variable or a file), read only when a call is made, never
returned by any read path, never printed, and masked in the diagnostics
bundle.

## Retention

`decide.retention_days` (90 by default; `0` keeps them forever). The GC
sweep deletes older runs a batch at a time, whether or not the GC's session
killer is on.

## Turning it off

- **Instantly, everything:** Settings → *Decisions (Jev)* → off, or on a hub
  `set_setting { key: "decide.jev.enabled", value: "false" }` with the
  master token. The next call is refused; nothing in flight is retried.
- **One org:** `fleet-hub org set <id> --jev off`, or its checkbox in
  Settings → Organisations.
- **One feature:** its mode to `off`.
- **The key:** `fleet-hub decide clear-key`.

## Settings

| Key | Default | What it does |
|---|---|---|
| `decide.jev.enabled` | `false` | The kill switch. Off: nothing is ever sent. |
| `decide.jev.status_map` | `off` | `off` / `shadow` / `assist` for Asana section → status category proposals. |
| `decide.jev.work_link` | `off` | `off` / `shadow` / `assist` for choosing a work item for an unlinked session. |
| `decide.jev.unassigned` | `false` | Sessions and items with no org may be sent too. |
| `decide.jev.timeout_ms` | `1500` | One call's whole budget (100–30000 ms). |
| `decide.jev.breaker_failures` | `5` | Failed calls in a row that open the breaker (1–100). |
| `decide.jev.breaker_open_secs` | `300` | How long an open breaker refuses calls (10–86400 s). |
| `decide.jev.daily_token_budget` | `2000000` | Input tokens per UTC day (`0` = none). At $0.042 per million, the default is under $0.09 a day. |
| `decide.jev.model` | `jev-1.13.0` | `jev-1.13.0` (pinned) or `jev-latest`. |
| `decide.retention_days` | `90` | Days a run is kept (`0` = forever). |

On a standalone desktop they are in Settings → *Decisions (Jev)*. On a hub
they are set over the API with `set_setting` (master token), like the
`work.*` settings; a paired desktop shows them read-only there.

## The command line (hub)

```bash
fleet-hub decide set-key                    # the key on stdin (one line)
fleet-hub decide set-key --from-env JEV_KEY # from this shell's variable
fleet-hub decide set-key --ref file:/run/secrets/jev   # or env:NAME, read at use
fleet-hub decide clear-key
fleet-hub decide status [--days 30] [--json]
fleet-hub decide runs [--feature work_link] [--limit 50] [--json]
```

The key is never an argument (shell history, `ps`). `set-key` and
`clear-key` write the hub's database directly, like `fleet-hub tracker
webhook`; the running hub reads the key at its next call. `status` and
`runs` open the database read-only (no running hub needed; `--db FILE`
reads a desktop's `state.db`) and print ids, words and numbers only:
the flag, the modes, which orgs consented, whether a key is configured
(never the key), the breaker, today's tokens and cost, and runs per
feature, provider, fallback and org.

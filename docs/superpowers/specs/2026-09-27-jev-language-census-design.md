# Jev evaluation: the language census (D40)

**Date:** 2026-09-27
**Status:** built (`fleet-hub census languages`, `fleet-core::service::nl`); the
census itself is the owner's to run on the real hub.
**Scope:** the first step of evaluating a decision model in fleet. It needs no
decision model and nothing leaves the machine.

## Why a census first

We are evaluating **Jev**, TypeSafe AI's decision model, as an optional reader
for fleet's closed-set decisions. The best-placed one is choosing a work item for
a session no rule could link. Jev is English-first: TypeSafe says other languages
are "handled but not equally well" and must be tested on your own content. So
the benchmark gets a language axis. Before building that axis we need to know
which languages fleet's texts are in, per organisation. The census measures it
from what the store already keeps.

## What is verified about Jev (sources, 2026-09-27)

| Fact | Source |
|---|---|
| TypeSafe AI's first "System One" model, announced 2026-09-15, early access; `jev-1.13.0` behind the aliases `jev-latest` and `jev-preview` | [blog](https://typesafe.ai/blog/introducing-system-one-models-and-jev), [Models](https://docs.typesafe.ai/models.md) |
| `POST https://api.typesafe.ai/v1/systemone` takes a `state` (text/JSON) and typed questions: Noul (0–1), Choice (≤ 255 options, probabilities + confidence), Score (2–10 levels) | [API](https://docs.typesafe.ai/api.md) |
| $0.042 per million input tokens, output free; 64k tokens per request (32k for state plus the longest question); 250k tok/s and 1,200 RPM "adjusting dynamically"; text only | [Models](https://docs.typesafe.ai/models.md) |
| English-first, other languages lower; the same weights for every account (no fine-tuning); ZDR only on enterprise plans | [Models](https://docs.typesafe.ai/models.md), [Legal](https://docs.typesafe.ai/legal.md) |
| Known weak spots: literal reading, numbers and dates, indirection, a large state full of unrelated detail, **adversarial content**, no text generation | [Jev 1.13 jaggedness](https://docs.typesafe.ai/model-jaggedness/jev-1.13.md) |
| No Rust SDK (Python, JS); fleet would call the HTTP API directly | [SDKs](https://docs.typesafe.ai/sdk.md) |

Nothing about Jev has been tested **in fleet**. The envelope every call
will go through is built (D35–D37 below), off by default; no use case calls
it yet.

## Decisions so far

The numbers continue after the work-graph roadmap's D30, so the two tables
never collide. The roadmap's table points here.

| # | Question | Answer |
|---|---|---|
| D31 | May prompts and ticket titles go to TypeSafe? | **Yes, per org, opt-in, off by default** |
| D32 | First use case | **J3 `status_map` first** (Asana sections the keyword rule cannot classify; shadow/assist, proposals a person applies — built), **J1 `work_link` offline only** (phase 0 benchmark built; no live adapter until its acceptance lines pass) |
| D33 | Baselines | **`claude -p --model haiku` on the host is a baseline and the generative fallback** (asynchronous decisions only). Built as the benchmarks' `haiku` provider (`service/decide/haiku.rs`): a named host of the case's own org only, prompt on stdin; not a live fallback yet |
| D34 | Label hygiene before any benchmark | **Built**: an agent cannot overturn a person's rejection (`E_FORBIDDEN`); who decided is recorded (`store::Decider`, `Caller::work_decider()`: agents record `agent` / `agent_started`, persons `manual` / `started`; write-back, auto-trust and person counts read `PERSON_SOURCES` only); withdrawn / decayed / carried suggestions leave a `work_suggestion_withdrawn` event (ids only); auto-trust is branch-only; a person's Clear work holds against the unchanged branch / PR (R9u, migration 070 `work_unlinks`). UI impressions stay unrecorded |
| D35 | Architecture | **Built** (`fleet-core::service::decide`): one envelope — `gate()` (owner process only: a store with `hub.remote_url` never calls out; flag, mode, org consent, key, breaker, budget), redaction of every text (`redact_state`: URLs, emails, `logging::redact`), one call under `decide.jev.timeout_ms` with no retry, answer checks (`jev::validate_answer`), an optional confidence floor, and a record in every case — with thin per-use-case adapters to come. The seam is `DecisionBackend` (`JevBackend` over `net::https::HttpTransport`, fenced to exactly `api.typesafe.ai`; a fake in tests). 429/529 are `rate_limited` and count toward the breaker; the breaker and the daily budget are derived from `decision_runs`, so `fleet-hub decide status` sees them. |
| D36 | Feature flag | **Built**: `decide.jev.enabled` (off) in the Settings dialog's "Decisions (Jev)" section; `decide.jev.status_map` / `decide.jev.work_link` = `off / shadow / assist` (off; `auto` not offered until a feature passes acceptance); per-org consent `orgs.jev_allowed` (migration 068, off; `fleet-hub org set <id> --jev on\|off`, `work_admin update_org { jev }`, Settings → Organisations); `decide.jev.unassigned` (off) for rows with no org; `decide.jev.timeout_ms` 1500, `breaker_failures` 5, `breaker_open_secs` 300, `daily_token_budget` 2,000,000, `model` `jev-1.13.0` (pinned). The phone is unchanged. Guide: `docs/decisions.md`. |
| D37 | What a decision records | **Built**: `decision_runs` (migration 069), one row per `decide()` call including every fallback (`not_owner flag_off org_off mode_off no_key breaker_open budget timeout http_error rate_limited invalid_answer low_confidence`); ids, vocabulary words and numbers only (the store refuses anything else); `input_fp` = HMAC-SHA256 of the canonical redacted request under a local key in `decision_secrets` (`fp_key`); followups (`confirmed rejected corrected ignored`, `corrected_to`) filled by adapters; `decide.retention_days` 90 swept by the GC tick. The API key is `decision_secrets.jev_api_key`, read only by `Store::resolve_decision_credential`, set by `fleet-hub decide set-key` (stdin / `--from-env` / `--ref`). |
| D38 | Language as an axis | **Yes**, detailed by D40–D46 |
| D39 | Hand labels (~150 sessions without a link) | Open — the tooling is built: `fleet-hub decide bench work-link --export-unlinked N --out FILE` (redacted prompts and candidates, `0600`, never overwritten) and `--labels FILE` (dataset H of the J1 benchmark); the labeling is the owner's |
| D40 | Measure the languages locally first | **Yes: this census** |
| D41 | Routing | **A static table** `(use case, language bucket, code bucket) → Jev / haiku / rule-or-person`, from the benchmark and shadow data, changed only by review; automatic only toward the conservative side (a degraded cell falls back by itself; a person re-enables it) |
| D42 | Programming language | **A stratum, not an axis**: `code` density (and later the dominant language of changed files), plus an A/B of replacing code blocks by a placeholder |
| D43 | Parallel corpus translation | **LLM translation, spot-checked by a person** |
| D44 | Languages the detector tells apart | **en, sk, cs, de**, with **pl, hu** as controls |
| D45 | Form of the census | **A `fleet-hub` CLI**, not `work_admin { usage }` |
| D46 | May the owner's own first prompts validate the detector? | **Yes, locally**: `--export-sample` / `--labels` |
| D47 | lingua's models grow `fleet-hub` from 30.0 MB to 75.8 MB (release, linux x86_64, measured 2026-09-27): keep them in the hub, ship a separate `fleet-census` binary, or load the models from files at run time? | **Keep them in the hub**: the static router (D41) will need the same detector there |

## What was built

- `fleet-core::service::nl` (cargo feature `nl-detect`): `NlBucket` (`en sk cs
  de pl hu other mixed unknown`), `CodeDensity` (`none low high`), and the
  `folded` flag for Slovak or Czech with no diacritics. The code is taken out
  first, then lingua names the language of the prose. Slovak vs Czech is settled
  by words and letters only one of them uses. `DETECTOR_VERSION` is in every
  output.
- `store::nl_census`: the read-only queries. The org of a prompt or journal row
  is the session's (`session_org_sql!`); the org of an item is its tracker's.
- `service::nl::census`: counts per org and source, the prompt × title pairs of
  confirmed links, fleet's own prompts left out (the loop guard, the start,
  resume, handover and safe-kill templates, quick-reply chips), and counts under
  5 shown as `<5`.
- `fleet-hub census languages [--days] [--org] [--max-per-source] [--db] [--json]
  [--labels FILE | --export-sample N --out FILE]`. See `docs/hub.md` → *Language
  census*.

## Choosing the detector (measured, not assumed)

Both candidates were measured on the same rules; the whatlang row comes from a scratch prototype of those rules, the lingua row from the shipped code. `cases.jsonl` (203 synthetic
texts, LLM-written per D43) was used while writing the rules. `holdout.jsonl`
(43) was written after the rules and never tuned against.

| | written-against set | holdout | binary cost |
|---|---|---|---|
| lingua 1.8 (6 languages) + rules | 98.0% (every mistake Slovak ↔ Czech) | **95.3%** | ~45 MB |
| whatlang 0.18 + rules | 96.1% | 81.4% (Slovak/Czech read as Polish or English) | ~1 MB |

lingua wins exactly where fleet needs it: Slovak and Czech typed without
diacritics. That is the Slovak/Czech counterpart of the Darija-in-Latin-script
drop a third party measured on Jev itself. Its models are why the feature exists:
only `fleet-hub` enables `nl-detect`; the desktop bundle does not carry them.

**These numbers are synthetic.** The real check is D46: the owner exports a
sample of their own prompts, corrects it and runs `--labels`. Treat the
detector's per-language counts as trustworthy only after that.

## What the census cannot see

It lists these itself instead of guessing:
- prompts after a conversation's first;
- conversations of deleted sessions;
- Claude's replies as such (the journal stands in for them);
- commit subjects and PR titles (on the hosts);
- tracker items never synced.

## How the result feeds the next decisions

- **Router cells:** a language gets its own router cell only if it is ≥ 5% of
  the prompt traffic in some org that would enable Jev. Everything else is
  `other` and falls back.
- **Sample sizes:** fewer than ~200 real prompts in a bucket over 90 days
  means that bucket's comparison rests on the paired translated corpus.
- **Cross-language share of confirmed links:** decides whether the work-link
  benchmark is built as cross-language first.
- **No-diacritics share:** decides how much of the perturbation tier goes to
  it.

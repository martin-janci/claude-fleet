# Task → session: manual flow, operator threads, brainstorm → plan → agents

**Date:** 2026-10-06
**Status:** brainstorming draft. Nothing here is approved. The owner's decisions are open (§9), and none of it is built.
**How it was made:**
- Three UX design passes ran in parallel, each with its own lens: A, the manual desktop flow; B, the operator's conversations; C, brainstorm → plan → agents and the autonomy dial. The passes were merged into this one document.
- An adversarial review then checked the merged draft against the code. Its fixes are folded in, and its findings are listed in §8.
**Builds on:**
- `2026-09-29-ai-task-system-brainstorming.md`: the vision and roadmap parts 1–5. This spec places the owner's new flow onto those parts.
- `2026-09-29-shared-work-context-design.md`: part 1. It is implemented: `TASK-<id>`, subtasks, proposals, job mirrors and steps.
- `2026-09-20-ux-agent-fab-design.md`: the operator (AgentFab / AgentPanel).
- `2026-09-27-work-view-design.md`: the Work view.

## Vízia (owner, 2026-10-06, v jeho slovách)

> Mám tásku, či je to v Asane, v Jire, alebo v mojom vlastnom systéme. Keď
> nemám žiadnu Asanu, zobrazí sa mi v mojom vlastnom. Z tasky si viem
> vytvoriť session, alebo sa môžem atačnúť na session, ktorá už existuje, a
> pracujem ďalej v tej session na tej taske. Konflikty treba vyriešiť.
> Momentálne mi toto chýba hlavne na desktope a musí to byť veľmi
> prepracované.
>
> Druhá alternatíva: pracujem v spoločnom agentovi, ktorý je nad všetkými,
> a ten si automaticky manažuje konverzácie. Keď zmením konverzáciu, spraví
> to tak, ako keby založil novú session alebo clearoval, ale dá sa vrátiť k
> tým starým.
>
> Keď chcem vytvoriť niečo nové, použijem tohto agenta. Agent môže použiť
> **brainstorming** (poznačiť si, aby sa nezabudol), vytvorí si tasky a na
> tých taskoch môže spawnúť jedného alebo viacerých agentov, takže si vie
> optimalizovať vykonanie. Dá sa to robiť manuálne (vytvorím session,
> spustím), alebo plne automaticky. Mám plnú kontrolu.

There are three modes, all over the **same** task, link and start primitives:

| Mode | Who drives | One line |
|---|---|---|
| **A. Manual** | the person | Every task has one **Work** button that starts, continues, opens or attaches a session. Conflicts are shown before anything happens. |
| **B. Operator threads** | the person, talking to the operator | The operator keeps one conversation *thread* per task or topic. Changing topic parks the thread, and any old thread can be resumed. |
| **C. Brainstorm → plan → run** | the operator, within limits the person sets | A brainstorm becomes a plan, the plan becomes a task tree, and the tree runs on one or more agents. An autonomy dial runs from L0 (manual) to L3 (full auto), always inside a budget. |

## What exists today (verified on `main`, 2026-10-06)

- **Own tasks** (`TASK-<id>`), subtasks, proposals that a person accepts, job mirrors and agent steps are all landed (migration 086, `service/work/local.rs`, `service/work/steps.rs`).
- **Work tab.** The List view (`TaskList.svelte`) has To do / Doing / Done, "+ New task" and a Start button. Start is shown only when the task has no active session.
- **Task detail** (`WorkTaskDetail.svelte`, `TaskWorkSections.svelte`) has Open, Continue and Start new, plus subtasks, proposals, jobs and steps.
- **Start** (`service/trackers/tickets.rs`: `start_work`, `resolve_start`, `plan_resolved`, `with_native_defaults`) chooses:
  - the project: the one passed, else the last one used for the key prefix, else `E_AMBIGUOUS`;
  - the host: the one passed, else the project's last host, else `E_AMBIGUOUS`;
  - a worktree per `branch_slug(key + title)`.

  A live session on the key returns `E_EXISTS`. The brief goes through `enqueue_handover` and `spawn_start_prompt`.
- **Operator.** One long-lived session (`service/operator.rs`). Its starts and resumes need confirmation (`OPERATOR_CONFIRMS`, D12). It knows nothing about tasks: `agent_context.ts` has no task concept.
- **Conversations.** `/clear`, `/resume` and compaction are tracked. The ConversationPanel switcher can show old conversations, read-only. `rewind_conversation` swaps a pane onto another conversation through `rebind_conversation`, `restart` and `revert_rebind`.
- **Brainstorming** does not exist in the product.

**Gaps this spec closes:**
1. A task has no "attach an existing session". Linking works only from the session side, and its search skips own tasks.
2. Start in TaskList and WorkTaskDetail has no project or host picker for `E_AMBIGUOUS`, and no preview of the brief or worktree.
3. A tracker ticket started from the Work tab gets no brief. `with_brief` is not passed.
4. Linking a session to a task that already has a live session gives no warning, and "move this session from A to B" is not one action.
5. Links are per session, not per conversation.
6. The operator has no task context and cannot brainstorm.
7. Multi-repo start cannot be reached from the Work tab.
8. The phone cannot create or see own tasks. **Suspected bug:** the phone's by-key start (`TaskViewModel.kt:221`) passes no `item_id`, so `with_native_defaults` presumably never runs for `TASK-n`. Verify this before the phase that touches it.

**Found by the review, true today:**

9. **The operator can accept proposals, including a worker's.** The operator is the unbound `ux-agent` client, so `Caller::org_scope` gives it `OrgScope::All`. `service/work/local.rs::decide` refuses only callers whose scope is not `All`, and the `accept` / `reject` path in `mcp/tools/orchestration.rs` calls no confirm gate. That contradicts the shared-context rule that only a person decides. **Fixed 2026-10-06 (TS15):** `work_link { accept | reject }` refuses `caller.is_operator()` with `E_FORBIDDEN` (test `the_operator_never_decides_a_proposal`). `accept_many` must keep the same refusal when it is added.
10. **A hub has no approver.** `fleet-hub serve` builds its guards `.without_approver()` (`serve.rs:903`), so every confirm forced on the operator answers `E_FORBIDDEN` on a hub. Operator-driven starts there need a hub confirm queue (§4.4).
11. **Stop does not stop.** `cancel_task` only marks the task cancelled, and the worker keeps going. There is no primitive to pause one agent either.
12. **The operator is not excluded from detection.** `work/detect*` never checks `operator_ref`, so every key the operator mentions becomes a link suggestion. **Fixed 2026-10-06 (A0):** `detect::subject_state` answers `None` for the operator, so its prompts, its `agent_inferred` links and re-resolves suggest nothing; the classification nudge skips it too (`operator::is_operator_session`; tests `the_operator_is_never_a_detection_subject`, `the_operator_is_never_nudged_to_classify`).

---

## 1. Shared primitives (used by all three modes)

Each primitive is built once and called by the desktop, the phone, the operator and the run executor.

**P-1. Start preview: `work_link { action: preview_start }`** (desktop: `preview_start_work`). It answers without creating anything. It is its own action rather than a `dry_run` flag on `start` (changed at implementation): an older hub ignores an unknown field and would *start* the session it was asked to preview, as `wire_contract.rs` records for `move_session`; an unknown action is refused instead (`E_INVALID`), and the desktop then starts as before.
- the resolved `StartPlan`: project, host, branch, and whether the worktree is new or reused;
- `candidates { projects[], hosts[] }`, sorted by recent use and reachability;
- `checkout { exists, dirty?, busy_by? }`. The dirty probe is best-effort: 3 s, and `null` when unknown;
- the text of the brief;
- `siblings[]`, for a multi-repo start;
- `conflicts[]`, as **data** rather than `E_*` errors (§2.3).

The commit is the existing `start_work`, given the chosen values explicitly. A hub older than this falls back to a direct start, and its `E_AMBIGUOUS` candidates fill the same popover.

**P-2. Switch: `work_link { action: switch, session_id, link_id, key | item_id, expected_primary }`** (desktop: `switch_session_work`; `link_id` is *from*, the key or item *to*, as `link` names a target). A single compare-and-set that:
- ends the old link, keeping its snapshot;
- makes the new task primary.

Today's link → set_primary → unlink sequence can half-fail; this replaces it.

**P-3. Live-elsewhere warning.** `link` returns `details.live_elsewhere[]` when the task already has another live session. A one-shot `ack_live: true` goes ahead anyway.

**P-4. Own tasks in search: `work_tickets { include_local: true }`.** Own tasks appear in the session-side "Work on task…" and in ⌘K.

**P-5. Start progress events.** The new session's timeline gains `start_spawned`, `worktree_ready`, `repl_ready` and `brief_sent`, beside today's `handover_waiting`. They ride `session:updated` (ids only), so the hub forwards them unchanged.

**P-6. Abandon a start: `abandon_start`.** It safe-kills the session and removes the worktree, but only when fleet created the worktree and it holds no commits and no changes. Otherwise it answers `E_DIRTY`.

**P-7. Link windows.** A Switch without `/clear` keeps the same Claude conversation id. Today `KEY_CONVERSATIONS` (`store/work_journal.rs`) gives a live link every conversation of its session, and `end_link` snapshots all of them, so after A→B task B's journal would show A's work. Two changes fix this:
- **Journal and retention for a link are windowed** by `work_links.created_at` / `ended_at`.
- **The stamps choose what Continue resumes.** A link records the conversation current when it began (`began_claude_id`) and ended (`ended_claude_id`), so Continue on the old task resumes *its* conversation.

P-7 ships with Switch (A2). Mode B's threads (§3) are the full per-conversation binding, and they are only for the operator.

**P-8. Parallel start: `StartArgs.parallel: bool`.** Today `resolve_start` / `plan_resolved` answer `E_EXISTS` for any live session on the key. `parallel` skips that key guard, keeps the branch guard, and forces a `-N` worktree.

> **Naming:** the word *plan* means only a brainstorm plan (§4). The start
> dry run is a **start preview**, and the start path's internal
> `StartPlan` type keeps its name.

## 2. Mode A: the manual desktop flow

### 2.1 Journeys

| # | Situation | What happens |
|---|---|---|
| J1 | Own task, no project known | Clicking **Start** asks for a preview, which comes back with no project. The popover opens with **Repo** focused, sorted by recent use, and *Remember for this task* is ticked. ↓ then ⏎ starts; the row turns into the progress strip; the session opens when Claude's REPL is ready. |
| J2 | Jira or Asana ticket whose prefix has run before | The preview resolves everything, so a plain click starts at once. The brief is **on by default**, which closes gap 3. ▾ or Alt-click opens the popover first. |
| J3 | Attach a running session, from the task | ▾ → *Attach running session…* opens a picker. It lists sessions in the same repo first, then sessions with no task, then idle ones. A session that is on another task shows "on PAY-139 ★" and offers **Switch** or **Add**. The default is **Add** while that session is mid-turn, or when its branch names the other key (detection would re-suggest the old task); otherwise it is **Switch**. The new task's brief waits for the next prompt and never interrupts a running turn. |
| J4 | Attach, from the session | `SessionTasks`' "Add task…" becomes **Work on task…** and also searches own tasks (P-4). If the task already has a live session, an inline line offers **Attach anyway**, Open that one, or Cancel (P-3). |
| J5 | Continue a task whose session ended | The primary label reads **Continue** (`resume_work last`, using P-7's conversation). ▾ lists past sessions to pick a different one. |
| J6 | Task already has a live session | The label reads **Open**. ▾ → *Start new…* opens the popover with the warning "PAY-150 is already open in …; this starts a parallel session in its own worktree `-2`" (P-8). *Attach another session…* is also offered. |
| J7 | One task in two repos | In the popover, **+ also in:** shows sibling chips. Ticking one turns the action into a multi-repo start, and the strip shows one line per repo. |
| J8 | "Discuss with agent" | The button opens or creates the task's operator thread (Mode B). |

### 2.2 The Work button and its popover

Every task row in the List, the Grouped tree and the task detail gets one **split button**. Its primary label is the first that applies:

1. **Open**: a live session exists that I may drive.
2. **Accept & start**: the task is a proposal not yet accepted.
3. **Continue**: a resumable past session exists and no live one.
4. **Start**: otherwise.

The **▾ menu** is kept short: Start new…, Continue ‹session›…, Attach running session…, Discuss with agent, and Copy key.
- **There is one way to start.** *Start new…* always opens the popover. A parallel start is the same popover with a warning, and several repos are its *Also in* row, not separate menu entries.

```
[jira]  PAY-142 Refund rounding  In Dev · pay-api · 0 active · 2 past  [Continue|▾]
[local] TASK-31 Release notes    fleet · 0 active                      [   Start|▾]
[jira]  PAY-150 Webhook retries  1 active ●                            [    Open|▾]
```

The popover is anchored to the button and is not a modal. It is one component, used in the List and in the task detail's action bar, where it replaces today's three buttons.

```
┌ Start PAY-142 · Refund rounding ──────────────────┐
│ Repo    [pay-api       ▾]  last used for PAY-*    │
│ Host    [oci           ▾]  last ran pay-api       │
│ Branch  pay-142-refund-rounding  new         ✎    │
│ Also in ☐ pay-web  ☐ ledger                       │
│ Brief   ☑ Send ticket brief          Preview ▸    │
│ ⚠ Done in Jira; the task moves to Doing here      │
│                            Cancel   [Start ⏎]     │
└───────────────────────────────────────────────────┘

┌ Attach a running session to PAY-142 ──────────────┐
│ ⌕ filter…                                         │
│ ● pay-api--main   oci  idle 4m   no task          │
│ ● pay-api--fix-x  oci  working   PAY-139 ★        │
│ It is on PAY-139:  (•) Switch to PAY-142           │
│                    ( ) Add (PAY-139 stays primary) │
│                            Cancel   [Attach ⏎]    │
└───────────────────────────────────────────────────┘
```

**Rules:**
- **A plain click acts at once** only when the preview is clean: everything resolved and no conflicts. Otherwise the click opens the popover with the unresolved field focused. The common case takes 1 click and the worst case 2 (click, then ⏎).
- **Keyboard in the List:**
  - `j` / `k` move between rows and `⏎` opens the task;
  - `s` runs the primary action, `⇧S` opens the popover and `a` opens Attach;
  - the keys follow the Assets M5 gating: never inside a field or a dialog.
- **Keyboard in the popover:** Tab moves through the fields, `⏎` commits, and `Esc` closes and returns focus to the button.
- **⌘K.** ⌘⏎ on a ticket keeps today's behaviour. When the preview is not clean, it opens this popover instead of NewSessionDialog.
- **Stability.** The popover re-reads the preview on `work:changed`, and on `session:updated` only for sessions on the same project or host, debounced to 1 s. The 3 s dirty probe runs at most once per opening; this prevents an SSH storm. A change underneath, such as someone else starting the same task, is flagged without the layout jumping.

### 2.3 Conflict matrix

| Conflict | What the UI says | Choices (**default**) |
|---|---|---|
| Live session exists, mine | The label becomes Open | **Open** · Start parallel (`-2`) · Attach another |
| Live session exists, someone else's | "Someone is working on this". No name, unless the session is visible, as with `already_running` | **Start parallel** · Cancel |
| Session is already on another task (attach) | "On PAY-139 since 2 h" | **Switch** · Add as secondary · Add as primary |
| Cross-org | `crossOrgSentence` | **Cancel** · Across orgs: explicit, never the default |
| Branch checkout exists, idle | "Existing checkout · 3 uncommitted · last commit 2 d ago" | **Reuse** · Fresh branch `-2` |
| Branch checked out by a live session | "Checked out by pay-api--x" | **Fresh branch `-2`** · Open that session |
| Dirty checkout | Shown in the line above. Fleet never stashes on its own | **Reuse as is** · Fresh branch |
| Task is Done (tracker or native) | Inline ⚠ | **Start** (a native task moves to Doing; nothing is written back, C3) · Cancel |
| Proposal not accepted | The label is "Accept & start" | **Accept & start** · Reject |
| No project or host known | The field is empty, focused, and its candidates are sorted | Pick one. *Remember* is ticked |
| Host unreachable | The host chip is greyed: "oci offline" | **The next reachable host with the repo** · Pick another. Disabled if there is none |
| Hub client, hub down | `hubActionBlocked` on a disabled button | none |
| Lost race (`E_CONFLICT`) | `WorkConflictNotice`; the popover stays open with a fresh preview | **Retry** · Open the winner |

### 2.4 Polish

- **Progress in place, from P-5.** The row becomes a strip:

  ```
  PAY-142  ✓ queued  ✓ worktree  ◐ Claude starting  ○ brief      [Cancel]
  ```

  - A trust prompt reads "Waiting for you: trust this folder", with an **Open** button.
  - The session opens when the REPL is ready. If the person has moved to another row in the meantime, a toast appears instead.
  - `aria-live` announces each step.
- **Undo.**
  - During a start: the strip offers **Cancel start** (`abandon_start`, P-6) only until `brief_sent`. After that, Claude may already have edited files, and the cancel would fail with `E_DIRTY`.
  - After a switch or attach: Undo restores the previous primary through the compare-and-set.
- **Empty states.**
  - No tracker: "Your tasks live here. Type one above, or connect Jira, Asana, Linear or GitHub in Settings."
  - A task with no sessions: one big **Start**, with the preview's one-line summary.
- **Errors** show inline, under the field they concern, never as toasts. Focus always returns to the button that opened the popover.

## 3. Mode B: operator threads (automatic conversation management)

### 3.1 Model: one pane, many threads

A **thread** is a named lane of conversation, tied to a task or a topic. It owns one or more Claude conversation ids, and its **head** is the one that gets resumed. The operator keeps **one pane** and runs one thread at a time.

- **Switch to thread T:**
  1. Check that the pane is quiet, with Rewind's live pane probe.
  2. Check that T's head transcript exists.
  3. Run `rebind_conversation(op, head, Resume)` and `restart`.
  4. On failure, `revert_rebind`.

  This is proven by Rewind and costs about 3–6 s. Phase 2 would type `/resume <id>` into the REPL instead, so MCP stays connected, but only after it is verified on the pinned Claude Code version.
- **New thread:** a fresh launch. The thread's brief (the task brief, or the prompt carried over) is queued as a `handover` row and delivered through SessionStart `additionalContext`.
- **`/clear` typed by hand** acts as Refresh: a fresh conversation in the **current** thread, because most `/clear`s mean "fresh context, same topic". Only `+`, *Discuss with agent* and an accepted suggestion start a new thread. **Compaction** stays inside the thread.
- **Refresh:** past 70% of context, the panel offers a new conversation inside the same thread, built from the journal's brief, instead of compacting. The old conversation id stays readable.
- **Nothing is ever deleted.** Threads are parked, not cleared.

Rejected alternatives:
- **One endless conversation with `/clear`.** Compaction blurs topics together.
- **One background operator per topic.** It breaks the singleton guard and the token model, and duplicates what workers already do.

### 3.2 Detecting a change of topic

1. **Explicit (always available):** pick a thread, press `+`, use *Discuss with agent* on a task, or say "switch to TASK-12". For the spoken form, the operator calls `operator_thread {suggest}`. The operator never switches itself.
2. **Suggested (the recommended default):**
   - The composer checks before sending, but only when the **prompt itself** names a task key (or a task title found by search) that belongs to another thread. It then asks: "This is about TASK-9. Send in its thread?" The prompt is moved, not lost, and *Not for this thread* silences the check for that key. The context chip alone never triggers it, since it changes whenever the person browses the sidebar.
   - The operator's CLAUDE.md gains a rule to call `suggest` when a request belongs to another topic. The panel then shows a banner.
3. **Automatic (opt-in):** fleet, not the model, switches to an existing thread or opens a new one. It does so only while the pane is quiet and before the prompt is sent, and every such switch is followed by a 10 s Undo toast.

**Never mid-turn.** While the operator is in a turn, the panel offers "Switch when done" (queued and run on the Stop hook) or "Interrupt and switch".

### 3.3 Binding conversation ↔ task

```sql
-- Threads belong to the operator singleton. They take no participant or
-- session FK: the operator's identity is (host, tmux) because session row
-- ids churn, and a SET NULL would also defeat the unique index below.
CREATE TABLE conversation_threads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id INTEGER REFERENCES orgs(id) ON DELETE SET NULL,    -- isolation for list/view
  person_id INTEGER,                                        -- multi-user hub (migration 098)
  title TEXT NOT NULL,
  item_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL,
  ref_key TEXT,
  head_claude_id TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'open',   -- open | parked | done | archived
  origin TEXT NOT NULL,                 -- manual|suggested|auto|clear|task|migrated
  created_at INTEGER NOT NULL, last_active_at INTEGER NOT NULL, archived_at INTEGER);
CREATE UNIQUE INDEX ux_thread_open_item ON conversation_threads(item_id)
  WHERE item_id IS NOT NULL AND state IN ('open','parked');
CREATE TABLE thread_conversations (
  thread_id INTEGER NOT NULL REFERENCES conversation_threads(id) ON DELETE CASCADE,
  claude_session_id TEXT NOT NULL UNIQUE,
  joined_at INTEGER NOT NULL, via TEXT NOT NULL);   -- start|clear|refresh|resume|fork
```

- **Attribution.** A journal or step row resolves `claude_session_id` → thread → `item_id` first, and falls back to the session's links only when no thread matches. This matters because the operator is linked to many tasks: without the thread, every step would land on every task. Workers have no threads, so they behave exactly as today.
- **The operator gets no `work_links`.** A task page lists "Discussed in operator thread …" separately from its working sessions.
- **The operator is excluded from detection.** `detect.rs` skips the operator's session, and the keys it mentions feed thread suggestions only (gap 12).
- **Delivery is per thread.** Handover rows and inbox rows (`task_result`, attention) for the operator carry `thread_id`. Today they are queued per session row, and SessionStart delivery is off by default (D5), so a brief or a result meant for thread T would land in whichever thread is current. A row is delivered only into its thread's head, or folded into that thread's return digest.
- **Dispatching from a thread.** A dispatch or start made from a thread gets the thread's task as its parent and primary link.
- **Backfill.** Each existing operator conversation becomes an untitled thread (`migrated`). The newest 20 stay open and the rest are archived.
- **Old transcripts.** Claude Code's `cleanupPeriodDays` (30 by default) deletes them. **On park, fleet copies the head transcript into its data dir**, and a resume restores it when the original is gone. Raising `cleanupPeriodDays` is possible, since provisioning already merges `~/.claude/settings.json`, but it is global to the host, so it is optional and needs consent. A thread whose copy is also gone offers *Revive with brief* from the journal.

### 3.4 UI

```
┌ Agent ── ◆ TASK-12 Login redesign ▾ ─── ctx 41% ↻ ── ⤢ ✕ ┐
├─────────────┬───────────────────────────────────────────┤
│ THREADS   + │ …conversation (ConversationPanel)…        │
│▶◆ TASK-12   │ ┌ Workers on this thread ───────────────┐ │
│   2 workers │ │ ● dev-web--login   working     Open ↗ │ │
│ ⚑ TASK-9    │ │ ⚑ dev-api--auth    needs you   Open ↗ │ │
│   API auth  │ └───────────────────────────────────────┘ │
│ ○ Fleet ops │ ┌ New topic: "Release notes 0.9"? ──────┐ │
│ ○ Untitled  │ │ [Start thread] [Keep here] [Not this]  │ │
│ ─ 7 parked  │ └───────────────────────────────────────┘ │
│ ─ archived  │ [◆ TASK-12] [dev-web · local ✕] Tidy…     │
│             │ > _                                       │
└─────────────┴───────────────────────────────────────────┘
```

- **Narrow panel and phone:** the rail collapses into the header dropdown, which is a bottom sheet on the phone.
- **Parked threads.** Clicking one opens it read-only first; this is ConversationPanel's existing `viewing` path and needs no restart. *Continue here* then does the actual switch.
- **Chips.**
  - The thread chip ◆ cannot be removed.
  - The view-context chip stays removable.
  - When the view's task differs from the thread's, the view chip turns amber: "TASK-9 · switch thread?".
- **Worker attention.** "Needs you", failed, done and PR events roll up into a ⚑ on the thread, a count on the FAB, and an OS notification with two actions: *Open thread* and *Open worker*.
- **Returning to a thread.** A one-line digest is sent with the next prompt, the way the context chip already works: "[since last here: dev-api--auth finished; PR #41]".
- **Stepping into a worker.** *Open ↗* selects the worker in the main view, and the panel shrinks to a pill "◆ TASK-12 ◂ back". ⌘E or the pill returns to the thread.

### 3.5 Operator vs workers

- **The operator** brainstorms, creates tasks, splits work, dispatches, monitors and summarises. **It never edits code.**
- **Hand-off rule** (in its CLAUDE.md): once a thread needs a repository, file edits, tests, or more than a few minutes of autonomous work, the operator proposes a start or a run (§4.3) with a brief built from the thread.
- **Self-guard.** When the operator calls `operator_thread`, only `list`, `view`, `suggest` and `bind` are allowed. `switch`, `new` and `refresh` restart its own pane, so they are refused with `E_FORBIDDEN`, for the same reason it cannot rename itself.
- **Proposals.** The operator may propose. It never accepts or rejects (gap 9).

## 4. Mode C: brainstorm → plan → tasks → agents

### 4.1 Brainstorming as a feature (**must not be forgotten**)

There are two pieces:
- **The skill `fleet-brainstorm`,** seeded in `catalog-seed/skills/` like `fleet-guides`. The operator always has it. The Assets catalog can ship it to worker hosts, so a worker can rethink its own task.
- **A brainstorm thread.** This is a Mode B thread with a pinned plan and a stage bar.

Each stage ends at a checkpoint that a person passes. The agent never advances a stage on its own.

| Stage | The agent | The person steers with |
|---|---|---|
| Diverge | 3–7 options, each with a one-line trade-off and its unknowns | *More*, *Pin* or *Drop* an option; constraint chips (deadline, repo, "no new deps") |
| Converge | Scores the pinned options against the constraints and names the risks | *Merge A+C*, *Research X first* (starts a read-only spike) |
| Decide | One decision record per choice: what, why, and what was rejected | *Accept* or edit |
| Plan | A plan document and a proposed task tree | The review screen (§4.2) |

**C0, the minimum that must not be forgotten:** the skill alone, before threads or plan tables exist. The operator brainstorms in its ordinary conversation, and the result is written to the root task's `notes` and the journal. The skill is written into the operator's own directory at `ensure_operator`, so it does not depend on catalog sync.

**What it produces (C1 onwards):**
- **A plan row** (`work_plans`) on a root task, which is a new `TASK-<id>` or the tracker item. Its body holds the goal, non-goals, the options considered at Diverge and Converge (pinned, dropped and merged, so a later "why not X?" has an answer), decisions, risks and the task tree. It is versioned and rendered as text, never as markup.
- **Decision rows.** Each decision is also a journal row of `kind = 'decision'`, which is raw material for the part-2 summaries.
- **Proposed subtasks.** The task tree consists of real `origin = 'proposed'` subtasks that carry `plan_id`. There is no new kind of task.

### 4.2 Plan → tasks: review and bulk approval

```
┌ TASK-412 Offline sync for POS ─ Plan v3 ─ stage: PLAN ───────────────┐
│ Goal: queue sales offline, replay on reconnect.  Non-goals: conflicts │
│ Decisions (3) ▸ D1 IndexedDB queue (rejected: SQLite-wasm, size)      │
├ Proposed tasks (5) ─ waves computed ─────────────────────── [Edit md] ┤
│ ☑ W1 a Queue schema        pos-web   S  done-when: unit tests green  │
│ ☑ W1 b Replay endpoint     pos-api   M  done-when: e2e replay passes │
│ ☑ W2 c Wire UI → queue     pos-web   M  needs a   ⚠ same repo as a   │
│ ☐ W2 d Telemetry           pos-api   S  needs b   "why: optional"    │
│ ☑ W3 e Docs + runbook      docs      S  needs c,d                    │
├──────────────────────────────────────────────────────────────────────┤
│ Autonomy for this plan: (•)L0 ( )L1 ( )L2 ( )L3   budget ▸ 3 agents, $8│
│ [Accept 4 selected]  [Reject plan]  [Back to Decide]   undo 10 min   │
└──────────────────────────────────────────────────────────────────────┘
```

**`work_link { propose_tree, parent, plan_id, items[] }`** creates the whole tree atomically. Each item carries:
- `title`, `notes`, `why`;
- `done_when`: a checkable list, **required**;
- `depends_on`;
- `project_id`, `host_hint`;
- `size`: S, M or L;
- `touches`: path globs, optional.

**Brakes:**
- **`PLAN_ITEMS_CAP = 10` per `plan_id`,** counting every item of the plan, accepted ones included. The existing `PROPOSALS_OPEN_CAP` counts only *open* proposals per parent, so it does not limit a plan's size. Worker proposals and `agent` mirror items also use its slots, which would make an atomic tree fail.
- **`propose_tree` answers per item.** A title already rejected skips that item; it does not fail the whole tree.
- **Subtask depth stays at one.**

A larger effort becomes a second plan.

**Waves** (groups that can run in parallel) are derived by a pure topological sort, `plan_waves`, and never stored.

**Approval** follows the ChangesetCard precedent: one sentence, a table and one primary verb.
- **Bulk:** `accept_many { item_ids }` in one transaction. Only a person may call it.
- **One by one:** the existing Accept and Reject on the task page.
- **Undo:** for 10 minutes, as long as nothing has started.

### 4.3 Execution: one agent or several

A pure planner, testable on its own like `playbooks.rs`, plus an executor.

- **One sequential agent** when any of these holds:
  - the plan is small: S/M in total;
  - tasks overlap in `touches`;
  - everything is in one repo and forms a chain.

  That one session then gets the tasks in order (one `run_prompt` each), which keeps context and costs less. **A sequential chain shares one branch and produces one PR.** "One item, one worktree" applies to parallel items only.
- **Parallel agents** only for tasks that are in the same wave, are size ≥ M, and are either in different projects or in the same project with disjoint `touches`.
- **Parallel work in one repo always gets separate worktrees**, which comes free with the start path (one item, one worktree, one branch). Combining them is a dedicated final-wave **integrate** task.
- **Choosing the host.**
  - Filter: the project is there (or can be cloned), the host is healthy (`fleet_health`: not `disk_low`, `hooks_silent` or `claude_behind`), and the host is inside the grant.
  - Rank: by fewest live sessions, then by account-usage headroom.
  - `host_hint` wins when it passes the filter.
- **Launch: `work_link { run, item_id }`.**
  - It starts through P-1 and the start path: project, host, worktree, and a brief made of the parent brief, the subtask, its `done_when`, and the `FLEET_TASK_DONE_<nonce>` instruction (`tasks::with_instruction`). The start path does not add that instruction today.
  - It writes a `tasks` row with the operator as requester, and links it through the **existing** `work_items.task_id` (migration 086, unique). There is no new `tasks.item_id`.
  - It **skips `mirror_dispatched`**, since the plan item already exists; the mirror would create a second `agent` item. Retries go in a `task_attempts` table.
  - The worker's claude id is stamped at its first SessionStart.
  - **Idempotent:** at most one open task per item. A second `run` returns the existing one.
  - So `wait_for_task`, the inbox `task_result` and `FLEET_TASK_DONE` work unchanged, and `dispatch_task {new_worker}`'s missing worktree is no longer in the way.
  - When the requester is the operator, `mirror_dispatched` (for plain dispatches) takes its parent from the **thread's** task, because the operator has no primary link.
- **Evidence.** A worker's report means only "done per the agent". Each `done_when` line is **typed**, and only these count as proof:
  - `ci:<check>`: a named CI check that passed on the item's PR head;
  - `review`: the parsed `FLEET_TASK_DONE` result of a reviewer *task* (`spawn_review` itself returns only a session, and `evidence.rs` "explains, it does not decide");
  - `person`: a person ticks it.

  Each check is recorded as a journal row of `kind = 'verify'`. The subtask is shown as **Verified** only when every line has proof; otherwise it reads **Unverified: N checks missing**.
- **Retries.** `max_retries` (default 1). A retry is a fresh prompt that carries the failure and the unmet lines, never a blind resend.
- **No progress means stop.** Any of these moves the item to **Needs you**, together with what it tried:
  - no commit, step or turn for `no_progress_secs`;
  - the same error twice;
  - a context or OOM loop;
  - the task's wall clock or cost runs out.

### 4.4 The autonomy dial

| Level | Name | The operator may, without asking |
|---|---|---|
| **L0** | Manual | Brainstorm, write plans and propose. A person accepts and presses Start. |
| **L1** | Assisted | Also *prepare* runs as **Ready** cards, with project, host and worktree filled in. A person presses Start, or *Start wave*. |
| **L2** | Supervised auto | Start and retry inside a **run grant**. Anything outside the grant, plus merges and PRs leaving draft, needs confirmation. |
| **L3** | Full auto | L2, plus marking items Verified from evidence and opening the next wave. Still bounded by the grant and the budget. |

**The effective level is the lowest of four:**
- the global ceiling `orchestrator.max_level` (default **0**);
- the org's `orgs.max_autonomy`;
- the level chosen for the plan;
- a per-task override, which can only lower it.

**The run grant is how the person keeps full control.** Approving a plan at L2 or L3 signs one grant, which holds:
- the plan's item ids;
- the hosts and projects it may use;
- the budgets;
- an expiry (default 24 h).

`confirm_gate_with` consults `run_grants` for operator callers only. A `run` or `retry` that matches an active grant passes without a dialog; everything else stays behind D12 exactly as today. Because the grant is signed ahead of time, this also works on a hub with no live approver.

**Enforcement reaches the workers.** Workers run with permission prompts off, so merge, a push to the default branch and tracker writes go through Bash `git` / `gh` and never reach `confirm_gate_with`. A run's worker therefore gets a fleet **PreToolUse hook** that refuses, outside the grant:
- `git push` to the default branch;
- `gh pr merge` and `gh pr ready`;
- tracker CLIs.

Branch protection on the remote is documented as the backstop.

**On a hub, a confirm needs a queue (gap 10).** Before C5, operator-driven starts on a hub are offered as Ready cards that a person presses, never as confirm requests. C5 depends on a **hub confirm queue**: pending confirms are listed and decided by a person's device (desktop or phone).

**Always confirmed, at every level:**
- kill, delete or move of a session or worktree;
- merge, or a push to the default branch;
- any write to an external tracker;
- `add_project`, or cloning a new remote;
- anything outside the grant;
- raising a budget or the level;
- crossing an org;
- `broadcast_prompt`;
- accepting a proposal made by a worker.

**Budgets** (settings defaults, which a grant can only lower):

| Setting | Default |
|---|---|
| `max_concurrent` | 3 |
| `max_agents_per_plan` | 6 |
| `max_cost_usd_per_plan` | 10 (from the `usage.rs` estimate) |
| `task_wall_secs` | 2 h |
| `plan_wall_secs` | 8 h |
| `max_retries` | 1 |
| `account_stop_pct` | 80 (no new starts above this share of account usage) |

**The stop controls:**
- **Kill switch:** turning `orchestrator.enabled` off revokes every grant.
- **Pause all:** one press on the desktop header, the phone or the AgentPanel. No new starts happen, every worker gets "finish your current step, then stop", and grants are suspended.
- **Stop:** sends Escape to each worker's pane, then runs `cancel_task`, which on its own only marks the task cancelled (gap 11). Sessions are kept, never killed.
- **Pause one agent (⏸):** sends Escape and suspends the grant for that item. **Resume** re-sends the item's remaining `done_when` lines.
- **The effective level is always explained.** When levels are capped, the dial shows the cap, e.g. "effective L1, capped by org Papaya POS · raise in Settings → Orchestrator", rather than greying them out without a reason.

**Run board** (on the plan's page while a run is active):

```
TASK-412 run · L2 · 2/3 agents · $3.10/$8 · 1h12/8h   [Pause all][Stop]
W1 a Queue schema    gpu-1 task-412-a ●working 14m  steps 4/6  [⏸][↪][Take over]
W1 b Replay endpoint hel-2 task-412-b ✓ Verified    PR #88 ready, e2e ✓
W2 c Wire UI         —     queued (needs a)                     [Start now]
W2 d Telemetry       ⚠ Needs you: same error ×2 "pnpm: ENOSPC"  [Retry][Skip]
```

- **⏸ pauses** one agent.
- **↪ redirects** it: a `send_prompt` with the person's text, recorded in the journal.
- **Take over** opens the pane and suspends the grant for that item until the person releases it.

## 5. Data model (additive; migration numbers are assigned at implementation, after 103)

| Migration | Contents | Phase |
|---|---|---|
| ~~`NNN_link_conversation_stamps`~~ | Not needed (A2): `work_links.claude_session_id` is the begin stamp, the switched link's windowed snapshot ends at the end stamp (P-7) | A2 |
| `NNN_conversation_threads` | `conversation_threads`, `thread_conversations` (§3.3); `thread_id` on handover and inbox rows | B1 |
| `NNN_work_plans` | `work_plans` (state brainstorm\|proposed\|accepted\|running\|paused\|done\|abandoned, stage, body, version, level, author); `work_items.plan_id`, `done_when` (JSON), `size`, `host_hint`, `touches`; `work_item_deps` | C1–C2 |
| `NNN_run_grants` | `run_grants` (plan, granted_by, level, item_ids, hosts, project_ids, budgets, `expires_at`, `suspended_at`, `revoked_at`); `task_attempts` (task, attempt, grant, reason); `orgs.max_autonomy`. **No `tasks.item_id`**: `work_items.task_id` already links them | C4–C5 |
| `NNN_hub_confirms` | the hub confirm queue: pending confirm, caller, summary, nonce, decided_by, decided_at | before C5 |

New journal kinds: `decision` and `verify`. New settings live on an **Orchestrator** page in `SPECS`:
- `orchestrator.enabled`, `max_level`;
- the budgets in §4.4;
- `no_progress_secs`;
- `operator.thread_detection` (`explicit` | `suggested` | `automatic`).

## 6. API, commands and hub parity

| Surface | Additions |
|---|---|
| `work_link` | `preview_start`, `start {parallel}`, `switch`, `abandon_start`, `propose_tree`, `accept_many`*, `plan_put`, `run`, `retry`, `pause`, `resume`, `stop`, `verify`, `grant`*, `revoke`*, `level`* (* person-only: refused to per-host tokens **and** to the operator) |
| `link` | `details.live_elsewhere[]`, `ack_live` |
| `work { tickets }` (the desktop's `work_tickets` command) | `include_local` |
| `work` | `plan { plan_id }`: plan, waves, board and budget burn |
| New tool `operator_thread` | `list \| view \| switch \| new \| refresh \| rename \| bind \| park \| archive \| suggest` |
| Desktop commands | `preview_start_work`, `switch_session_work`, `abandon_start`, `operator_threads`, `operator_thread_switch`, `operator_thread_new`, `operator_thread_update`, and plan / run commands. Every one is `Verdict::Routed`; nothing is `LocalOnly` |
| Events | `operator:threads`; start progress on `session:updated` |

**Each new action needs:**
- a row in `backend/verdicts.rs`, followed by `REGEN_HUB_VERDICTS=1`;
- a row in the isolation matrix (`mcp/tools/tests_isolation.rs`);
- for actions on a **session** (`switch`, `abandon_start`), the `share.ts` tier, which covers session actions only;
- the regenerated `docs/control-api-reference.md`.

**Tool surface budget.** About 15 new `work_link` actions and a new tool will trip the `BUDGET_BYTES` tool-surface test in `mcp/tools/tests.rs`. Each phase pays back its own descriptions or raises the budget deliberately, as M11 did.

**Isolation.**
- Grants bind only to the operator's `ux-agent` client and a person.
- A per-host token never matches a grant.
- A worker may only `propose` or `propose_tree` under its own task, and its proposals always wait for a person, **even at L3**.
- A worker's text never edits a plan, a grant or a budget.

## 7. Phasing (each slice ships alone)

The order follows the owner's stated pain first: *"chýba mi to hlavne na desktope"*.

| # | Slice | Contents | Roadmap part |
|---|---|---|---|
| **A0** | Fix now | **Done 2026-10-06.** The operator may not accept or reject proposals (gap 9). The operator is excluded from detection and the classification nudge (gap 12). | 1 |
| **A1** | Start preview + Work button | **Done 2026-10-06.** P-1 (`work_link { preview_start }`, `preview_start_work`), P-8 (`parallel`), the Work split button in TaskList (Open / Continue / Start, ▾ Start new… / Continue ‹session› / Copy key, `j`/`k`/`s`/⇧S) and the start popover (repo, host, branch, brief preview, conflicts with their choices), the task page's Start new through the same preview, the brief on by default for tickets, J1/J2/J5/J6. **Left for later:** the task page keeps its own Open / Continue buttons (their share gating is pinned by `resume_gates.test.ts`); *Remember for this task* (no write path for a native item's project yet); the dirty-checkout probe (`checkout.dirty`, A3); "Accept & start" as the row's label (proposals are not listed rows; the popover offers it). | 1 |
| **A2** | Attach and switch | **Done 2026-10-06.** P-2 (`work_link { switch }`, `switch_session_work`), P-3 (`ack_live` on `link` / `switch`, opt-in so older clients link as before), P-4 (`work { tickets, include_local }`, used by Work on task…), P-7 link windows, J3 (▾ *Attach running session…* in the Work button: the attach picker, Switch / Add with the mid-turn and branch-named defaults, Undo for 10 s) and J4 (the session's *Work on task…* with the live-elsewhere line). **Changed at implementation:** P-7 needs no migration. `began_claude_id` is `work_links.claude_session_id`, already stamped at the decision, and `ended_claude_id` is the last id of the switched link's windowed snapshot; the journal window is derived from the `switched` ends around a link, so a link no switch touched reads exactly as before (linking a session after the work still files that work under the task). **Left for later:** the new task's brief waiting for the next prompt (J3); own tasks in ⌘K (its ⌘⏎ starts by key, gap 8); Undo after an *Add* (the primary does not move; Remove in the session's Tasks undoes it). | 1 |
| **C0** | Brainstorm skill | `fleet-brainstorm` in the operator's directory; the plan goes to the root task's notes and journal. No new tables. **The owner asked that this not be forgotten, so it comes early.** | 3 / 4 |
| **A3** | Polish | P-5 progress strip, P-6 Cancel start, J7 multi-repo in the popover, empty states | 1 |
| **A4** | Phone parity | Own tasks on the phone (create, list, subtasks), the by-key start bug, the same Work menu | 1 |
| **B1** | Operator threads | Schema, explicit switching, thread rail, Discuss with agent (J8), the worker card, attribution through the thread | 1 / 4 |
| **B2** | Suggested switching | Pre-send check, `suggest`, digest on return, Refresh, the transcript retention fix | 4 |
| **C1** | Brainstorm at L0 | The `fleet-brainstorm` skill, the brainstorm thread with stages, `work_plans`, `decision` rows, the plan on the task page | 3 / 4 |
| **C2** | Task tree | `propose_tree`, deps, `done_when`, the plan review card, `accept_many`, Undo | 3 |
| **C3** | Evidence | `verify` rows, Verified / Unverified, `done_when` in the brief. **Lands before any autonomy.** | 2 |
| **C4** | L1 Assisted | `plan_waves`, host choice, `work_link run`, Ready cards, *Start wave*, a read-only run board | 4 |
| **C5** | L2 Supervised | **Prerequisite: the hub confirm queue.** Run grants, the gate change, the worker PreToolUse hook, budgets, the no-progress stop, Pause all, Take over, approval on the phone | 5 |
| **C6** | L3 Full auto | Verify from evidence, opening the next wave, retries with context. Ships behind `max_level = 0`. | 5 |
| **B3** | Automatic switching | Opt-in, with Undo; only after B2 has been used for a while | 5 |

## 8. Counter-review and countermeasures

These come from the counter-review in `2026-09-29-ai-task-system-brainstorming.md`, plus the new risks of this design.

| Risk | Countermeasure |
|---|---|
| **More process than value.** Brainstorming every small task becomes ceremony. | Brainstorming is opt-in per piece of work. A plain task still goes through Mode A with one click. Plans are capped at 10 tasks. Success is measured as time to a useful result, not as the number of tasks or agents. |
| **A convincing "done" without proof.** | `done_when` is required. Only Verified counts. C3 comes before C4. Missing checks are shown plainly. |
| **Runaway cost and fan-out.** | The global default is L0. Budgets and `account_stop_pct` apply. Grants expire. The no-progress stop exists. There is one Pause all and a kill switch. |
| **Losing context on a thread switch.** | Nothing is deleted. Switches never happen mid-turn. Every automatic switch has Undo. A parked thread can always be read. Transcript retention is raised, with Revive with brief as the fallback. |
| **Two Claudes in one tree.** | Start parallel always gets its own worktree. A branch checked out by a live session defaults to a fresh branch. |
| **The operator acting on itself.** | The existing self-guard is extended: the operator cannot switch, refresh or create its own threads. Fleet does it, or a person. |
| **Escalation through a worker.** | Worker proposals always wait for a person. A per-host token never matches a grant. Worker text is untrusted data. |
| **Hub parity drift.** | Every new command is Routed, with verdict rows and generated docs. There are no `LocalOnly` exceptions. |
| **"Always confirmed" bypassed through Bash** (review). | A PreToolUse hook on run workers, with branch protection as the backstop (§4.4). |
| **A thread's brief or result lands in the wrong thread** (review). | Handover and inbox rows carry `thread_id` (§3.3). |
| **Switch files a running turn's work under the new task** (review). | Add is the default while mid-turn; links are windowed (P-7). |
| **The popover's preview causes an SSH storm** (review). | The re-read is debounced and filtered; the dirty probe runs once per opening (§2.2). |

## 9. Open decisions for the owner

Each has a recommendation. Nothing is built until the owner says yes.

| # | Question | Recommendation |
|---|---|---|
| TS1 | Should a plain click on the Work button start without a popover when the preview is clean? | **Yes.** Alt-click or ▾ always opens the popover. |
| TS2 | When attaching a session that is on another task: Switch or Add by default? | **Switch.** Add is one radio button away. |
| TS3 | Should the brief be on by default for Jira and Asana tickets? | **Yes**, with the preview one click away. |
| TS4 | Start parallel: its own worktree, or the same checkout? | **Its own worktree, always.** |
| TS5 | How should topic changes in the operator be detected by default? | **Suggested.** Automatic switching is opt-in, with Undo (B3). |
| TS6 | How should the operator switch threads? | **Respawn** now (proven by Rewind). In-REPL `/resume` later, after verification. |
| TS7 | One live thread per task, or several? | **One.** Refresh adds conversations inside it. |
| TS8 | Where should brainstorming live first? | **In the operator (C1)**, then as a catalog skill for workers in the same release. |
| TS9 | Should the plan cap stay at 10 tasks? | **Yes.** A larger effort becomes a second plan. |
| TS10 | What autonomy should apply after release? | **L0 globally**, raised per org deliberately. L3 only for orgs whose CI produces evidence. |
| TS11 | May L3 merge PRs? | **No.** "Merge on green" could later be a grant flag the person ticks. |
| TS12 | Should the budget be in USD, or as a share of the 5-hour account limit? | **Both.** USD caps a plan, and the account share stops new starts. |
| TS13 | Who combines the branches of parallel workers? | **A dedicated final integrate task**, with the merge confirmed by a person. |
| TS14 | "Konflikty": which conflicts did you mean? Task ↔ session conflicts (§2.3), or also git and file conflicts between two manual sessions in one repo? | §2.3 covers the first. For the second, the recommendation is a warning when two live sessions on one repo touch the same files (from `touches` and the steps), and never automatic merging. |
| TS15 | Should the fix for the operator accepting proposals (gap 9) ship now, on its own, before this spec is decided? | **Decided: yes** (owner, 2026-10-06). Done. |

## 10. Not in scope

- Writing back to external trackers beyond today's Jira PR link (C3 of the shared-context spec).
- Threads on worker sessions. The schema allows them, but v1 is the operator only.
- Several operators.
- Sprints, releases and epics (their own spec).

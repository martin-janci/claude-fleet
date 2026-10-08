# Autonómne orchestračné projekty — návrh a zosúladenie s kódom

**Dátum:** 2026-10-07
**Stav:** odporúčania O1–O12 (§11) vlastník prijal 2026-10-07 („go on"). O0 je rozpracované: `work_link { run }` a migrácia 110 (§10).
**Vstup:** handover „Cloud Fleet — Autonomous Orchestration Projects" (vlastník, 2026-10-07; ďalej *handover*).
**Nadväzuje na:**
- `2026-09-29-ai-task-system-brainstorming.md` — vízia a roadmapa častí 1–5.
- `2026-09-29-shared-work-context-design.md` — časť 1, postavená (migrácia 086).
- `2026-10-06-task-to-session-flow-design.md` — **Mode C** (brainstorm → plán → agenti, autonomy dial L0–L3, run grants). Tento dokument je jeho pokračovaním, nie paralelným návrhom (§1).
- `2026-09-28-sprints-releases-epics-design.md` — epiky ako `work_items` s deťmi (E2), buckety (migrácia 108).
- Lokálny workspace sync, fáza 1 (rozpracované, berie migráciu **109**).

## 0. Zhrnutie v piatich vetách

1. Handover má správny smer: tenká orchestračná vrstva nad `work_items`, `tasks`, `work_links`, sessions a worktrees, žiadny druhý Fleet vo Fleete.
2. Repo však už včera dostalo návrh tej istej vrstvy (`task-to-session-flow` Mode C: `work_plans`, `work_item_deps`, `run_grants`, `task_attempts`, autonomy dial, evidencia). Handover ho nespomína, takže by vznikli dva návrhy jednej veci. **Odporúčanie: `orchestration_projects` nahradí `work_plans`** a zvyšok Mode C sa prevezme (§1).
3. Tri tvrdenia handoveru o kóde neplatia: `dispatch_task` nevytvára worktree, jeden WorkItem dnes nemôže mať viac executions (`work_items.task_id` je unikátny 1:1) a policy typu `allow_push` / `allow_merge` Fleet sám nevynúti, lebo workery pushujú cez Bash (§2).
4. Orchestrátor nemá byť dlho žijúca session, ale **bezstavové volanie** cez existujúci uzamknutý `claude -p` (`service/claude_print.rs`: bez nástrojov, MCP aj hookov). Dostane snapshot a vráti JSON rozhodnutia, ktoré Fleet overí proti policy a vykoná (§5).
5. Chýba multi-user a org rozmer (owner, org, kto podpisuje grant) a hub nemá komu poslať potvrdenie (`serve.rs: .without_approver()`). Bez hub confirm queue nemôže autonómia na hube nič schvaľovať (§7).

## 1. Vzťah k existujúcemu Mode C

`task-to-session-flow` §4–§5 už navrhuje takmer všetko, čo handover nazýva orchestráciou:

| Handover | Mode C (2026-10-06) | Odporúčanie |
|---|---|---|
| `orchestration_projects` (goal, mode, state, policy) | `work_plans` na koreňovej úlohe (goal, non-goals, rozhodnutia, stav, level) | **Jedna tabuľka**: `orchestration_projects`. `work_plans` sa nestavia. Telo plánu (goal, non-goals, decisions) sú jej stĺpce. |
| `work_item_dependencies` | `work_item_deps` | Rovnaká vec. Jedno meno: `work_item_deps` (§4.3). |
| policy layer | run grant + autonomy dial L0–L3 + budgety + „vždy potvrdiť" | Policy projektu je **žiadosť**. Efektívne právo je grant podpísaný človekom, orezaný globálnym a org stropom (§7). |
| `orchestration_events` | journal kinds `decision`, `verify` | `orchestration_events` ako audit projektu. `verify` riadky ostávajú v journale, lebo patria konverzácii, ktorá dôkaz vyrobila (§6). |
| orchestrator session | operátor + jeho threads (B1) | Rozhodovanie ide cez bezstavový planner. Ľudská konverzácia o projekte je operátorovo vlákno naviazané na koreňovú úlohu (§5). |
| Phase 3 manual orchestration | C4 (L1 Assisted, Ready karty, `work_link run`) | Zhodné. |
| Phase 5 policy | C5 (grants, PreToolUse hook, hub confirm queue) | Zhodné, s doplnením hub queue ako tvrdej podmienky. |

Handover pridáva tri veci, ktoré v Mode C chýbajú, a tie sú jeho skutočný prínos:
- **multi-repo kontajner** (`orchestration_project_repos`),
- **continuous mode** (projekt, ktorý nekončí a budia ho udalosti),
- **durable backend loop** s obnovou po reštarte, ktorý nezávisí od kontextu jedného modelu.

## 2. Overenie tvrdení handoveru voči kódu

Overené na `main` (v0.5.1, posledná migrácia 108; 109 berie lokálny sync).

| # | Tvrdenie | Skutočnosť | Dôsledok |
|---|---|---|---|
| 1 | `projects` = repo checkout, viažu sa naň sessions, worktrees, work_items | **Platí.** `work_items.project_id` pridala 086. | Nové meno je nutné. Pozor aj na UI: „Project" už v produkte znamená repo (project picker, `add_project`, `list_projects`). |
| 2 | `work_items` má `title, status, parent_id, origin, project_id, notes, task_id, proposal_*` | **Takmer.** Stĺpec `status` neexistuje: je to `status_category` (`todo` / `in_progress` / `done`) s precedenciou `status_set_by` (084). `blocked` nie je hodnota. | READY a BLOCKED sa **odvodzujú**, neukladajú (§4.3). |
| 3 | `parent_id` dáva ľubovoľnú hierarchiu | **Neplatí pre natívne položky.** `parent_id` (048) existuje, ale shared-context spec obmedzuje natívnu hĺbku na jednu úroveň a `create`/`propose` odmietnu rodiča, ktorý má sám rodiča. Epiky (E2) pridajú druhú úroveň. | Štruktúru nesú závislosti, nie hĺbka stromu (§4.2). |
| 4 | `tasks` (020) = execution job, queued → running → done/failed/cancelled | **Platí.** Plus `detached_at` (101), TTL sweep a fail pri strate alebo výmene workera (`tasks::sweep_open_tasks`). | Dobrý základ pre WAIT. |
| 5 | Jeden WorkItem → viac executions | **Neplatí.** `work_items.task_id` + `ux_work_items_task` je 1:1 a znamená „táto položka zrkadlí tento job". | Treba `tasks.work_item_id` (§4.4). |
| 6 | DISPATCH cez `dispatch_task` použije worktree | **Neplatí.** `dispatch_task { new_worker }` volá `new_session` s `worktree_id: None, new_worktree: None`, takže worker beží v hlavnom checkoute repa. Logika je navyše v MCP handleri (`mcp/tools/orchestration.rs`), nie v service. `mirror_dispatched` by k už existujúcej položke vytvoril druhú `agent` položku. | Dispatch ide cez **start path** (`tickets::start_work`: worktree podľa `branch_slug(key+title)`, `parallel` → `-N`, brief cez handover) + nonce inštrukciu. To je `work_link { run }` z Mode C. |
| 7 | `work_links` N:M session ↔ item, primary, ended, suggested, história | **Platí.** Má aj `role` (`work` / `review` / `worker`, 047), čo pokrýva časť „worker roles". | Roly netreba vymýšľať nanovo (§4.4). |
| 8 | Worktree self-repair | **Platí.** `service/repair.rs` (probe → plan → apply → verify) a opt-in `repair_tick.rs`. | Orchestrácia nikdy nevymýšľa cesty, súhlas s handoverom. |
| 9 | `tasks.result` = execution result | **Čiastočne.** Je to jeden odsek po markeri `FLEET_TASK_DONE_<nonce>`, voľný text. | Štruktúrovaný výsledok treba doplniť, a commity či zmenené súbory má zistiť Fleet z gitu, nie z hlásenia workera (§6). |
| 10 | Existujúce confirmation mechanizmy | **Na desktope áno** (`confirm_gate_with`, `OPERATOR_CONFIRMS`). **Na hube nie**: `fleet-hub serve` stavia guardy `.without_approver()`, takže každé vynútené potvrdenie tam skončí `E_FORBIDDEN`. | Hub confirm queue je podmienka každej autonómie na hube (§7). |
| 11 | Agent-created tasks cez proposals | **Platí**, a operátor ich od 2026-10-06 nesmie akceptovať (TS15). | Planner navrhuje, rozhoduje človek, alebo grant (§7). |

**Čo handover vynechal:**
- **Org a vlastník.** Od 098–107 má každá session vlastníka (`owner_person_id`), položky majú `org_id` a izoláciu stráži `view_scope`. Projekt musí mať `org_id` a `owner_person_id`. Repozitáre aj workery musia byť v jeho orgu, a cross-org prechod je vždy explicitný.
- **Hub vs desktop.** Loop beží tam, kde je store: na hube, ak beží, inak v desktop appke. Tick už beží na oboch miestach (`spawn_reconcile_tick` v `serve.rs`).
- **Tool surface budget.** Test `BUDGET_BYTES` v `mcp/tools/tests.rs` pri desiatkach nových akcií zlyhá. Každá fáza si ho musí zaplatiť.
- **Hub parity.** Každý nový desktop príkaz potrebuje riadok vo `backend/verdicts.rs` a `REGEN_HUB_VERDICTS`, riadok v izolačnej matici a pregenerovanú `control-api-reference.md`.
- **Evidencia.** Agentovo „done" je len „done podľa agenta" (counter-review 2026-09-29). Hotovo je až po overení `done_when` (C3).

## 3. Model zodpovedností

```
OrchestrationProject   prečo a kam (goal, done_when, mode, policy, stav)
  ├─ root WorkItem     kind='epic', natívny alebo tracker ticket, z ktorého cieľ vznikol
  ├─ repos             podmnožina existujúcich projects (repo checkouty)
  └─ WorkItems         čo treba urobiť (orchestration_project_id = X)
       ├─ deps         poradie (work_item_deps)
       └─ Tasks        jednotlivé pokusy (tasks.work_item_id, attempt, role)
            └─ Session ── WorkLink(role) ── Worktree (start path)
Planner run            bezstavové rozhodnutie: snapshot → JSON príkazy
orchestration_events   append-only audit, nie zdroj pravdy
```

Súhlasím s handoverom, že sa **nevytvára** nová Task, Session, Worktree, Conversation, Thread ani Repository tabuľka. Operátorove `conversation_threads` z B1 nie sú „Thread" z pôvodného návrhu. Sú to jazdné pruhy konverzácie jedného operátora, a projekt ich len použije (§5.3).

## 4. Dátový model (aditívny, čísla až pri implementácii z `origin/main`, ≥ 110)

### 4.1 `orchestration_projects`

```sql
CREATE TABLE orchestration_projects (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id           INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  owner_person_id  INTEGER,                 -- kto ho vlastní a podpisuje granty
  root_item_id     INTEGER REFERENCES work_items(id) ON DELETE SET NULL,
  name             TEXT    NOT NULL,
  goal             TEXT    NOT NULL,
  non_goals        TEXT,
  done_when        TEXT,                    -- JSON, typované riadky ako pri položkách (§6)
  mode             TEXT    NOT NULL,        -- finite | continuous
  state            TEXT    NOT NULL,        -- §4.5
  level            INTEGER NOT NULL DEFAULT 0, -- požadovaná autonómia L0–L3
  policy_json      TEXT,                    -- žiadosť; platí len to, čo pokryje grant
  plan_version     INTEGER NOT NULL DEFAULT 1,
  next_wake_at     INTEGER,                 -- loop: najbližšie zobudenie (tick)
  lease_until      INTEGER,                 -- loop: jeden planner beh naraz
  last_snapshot_at INTEGER,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  started_at INTEGER, finished_at INTEGER,
  version          INTEGER NOT NULL DEFAULT 1   -- expected_version, ako buckety
);
```

- **`root_item_id`** je hlavný rozdiel oproti handoveru. Projekt má koreňovú úlohu (epik podľa E2, alebo Jira ticket, z ktorého cieľ prišiel). Zadarmo tak dostane TaskDetail, sessions, sprint a release (108), stav, Work view a operátorovo vlákno (`conversation_threads.item_id`).
- `orchestrator_session_id` z handoveru vypúšťam (§5). Ak vlastník chce interaktívny planner, vlákno operátora sa nájde cez `root_item_id`.
- `metadata_json` vypúšťam. Neštruktúrovaný „sem daj čokoľvek" stĺpec sa zvyčajne stane druhou schémou bez testov.

### 4.2 `orchestration_project_repos` a členstvo položiek

```sql
CREATE TABLE orchestration_project_repos (
  orchestration_project_id INTEGER NOT NULL REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role       TEXT,               -- voľný text: primary, mobile, backend…
  created_at INTEGER NOT NULL,
  PRIMARY KEY (orchestration_project_id, project_id)
);
ALTER TABLE work_items ADD COLUMN orchestration_project_id INTEGER
  REFERENCES orchestration_projects(id) ON DELETE SET NULL;
CREATE INDEX idx_work_items_orch ON work_items(orchestration_project_id)
  WHERE orchestration_project_id IS NOT NULL;
```

- Repos sú **povolená množina**: `run` mimo nej odmietne a grant ich preberie ako `project_ids`.
- Repo musí byť viditeľné v orgu projektu, inak `E_FORBIDDEN`.
- Každá položka už má `project_id` (086), teda v ktorom repe sa robí. Multi-repo položka sú dve položky so závislosťou alebo J7 siblings start.
- **Hĺbka:** koreň (epik) → úlohy → voliteľne podúlohy. Členstvo je plochý stĺpec, nie odvodené zo stromu, takže follow-up vytvorený hocikde patrí projektu bez prehadzovania rodičov.
- Tracker item (Jira) môže byť členom projektu. Fleet mu nikdy nemení status (C3); závislosť naň je splnená, keď ho tracker hlási `done`.

### 4.3 `work_item_deps` a READY

```sql
CREATE TABLE work_item_deps (
  item_id    INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  depends_on INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL DEFAULT 'blocks',
  source     TEXT NOT NULL,      -- person | planner | proposal
  created_at INTEGER NOT NULL,
  PRIMARY KEY (item_id, depends_on),
  CHECK (item_id <> depends_on)
);
CREATE INDEX idx_work_item_deps_rev ON work_item_deps(depends_on);
```

- Cykly odmietne store v transakcii (DFS nad grafom projektu, max ~100 uzlov), nie trigger.
- Hrana len medzi položkami toho istého orgu. Hrana z projektu na položku mimo projektu je povolená, ale len na čítanie (čakám na cudzí ticket).
- **READY**, odvodené, nikdy uložené:
  `status_category = 'todo'` a nie je to neakceptovaný alebo zamietnutý proposal, všetky `blocks` závislosti sú `done`, nebeží pre ňu otvorený task a nemá `hold` (osoba ju zastavila).
- **BLOCKED** = má nesplnenú závislosť na položke, ktorá je `failed` alebo `needs_input`, alebo na niečom mimo projektu. Je to odvodený stav pre UI.
- Vlny (`plan_waves`) sú čistý topologický sort, ako v Mode C.

### 4.4 Viac pokusov na jednu položku a roly

```sql
ALTER TABLE tasks ADD COLUMN work_item_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN attempt      INTEGER;  -- 1, 2, … v rámci (work_item_id, role)
ALTER TABLE tasks ADD COLUMN role         TEXT;     -- implement | review | test | research | integrate
ALTER TABLE tasks ADD COLUMN result_json  TEXT;     -- §6
CREATE INDEX idx_tasks_item ON tasks(work_item_id, created_at DESC) WHERE work_item_id IS NOT NULL;
```

- To je odchýlka od Mode C, ktoré chcelo samostatnú tabuľku `task_attempts`. Stĺpec na `tasks` je jednoduchší, lebo pokus *je* task. Dôvod opakovania sa zapíše do `orchestration_events`.
- `work_items.task_id` si ponechá dnešný význam (položka *zrkadlí* dispatch, `origin = 'agent'`). Orchestrované behy ho nepoužívajú a `mirror_dispatched` preskočia.
- Rola sa zapisuje aj do `work_links.role` (existuje), takže TaskDetail ukáže „reviewer" bez novej logiky. `work_links.role` dnes pozná `work | review | worker`; `test` / `research` pribudnú ako hodnoty, nie ako stĺpec.

### 4.5 Stavy projektu: lifecycle oddelený od fázy loopu

Handover má desať stavov, ktoré miešajú dve veci: čo vidí človek a kde je stroj. Odporúčam:

- **`state`** (uložený, pre človeka): `draft` → `active` ⇄ `paused` → `completed` | `failed` | `cancelled`.
- **Fáza loopu** (odvodená, pre UI ako podtitul): `planning` (planner beží), `running` (aspoň jeden task), `waiting` (nič nebeží, čaká na udalosť), `needs_input` (existuje otvorená otázka alebo potvrdenie), `blocked` (nič nie je ready, nič nebeží, niečo je failed).

Uložené fázy by sa rozchádzali so skutočnosťou po každom páde. Odvodené sa nerozídu nikdy.

### 4.6 `orchestration_events`

```sql
CREATE TABLE orchestration_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,      -- started, planned, created_item, dep_added, dispatched, task_done,
                           -- task_failed, verified, retry, review_failed, refused, asked, answered…
  actor TEXT NOT NULL,     -- person:<id> | planner:<run> | fleet | worker:<task>
  work_item_id INTEGER, task_id INTEGER,
  decision_id TEXT,        -- idempotencia: jeden príkaz plannera sa vykoná raz
  payload TEXT             -- JSON, capped
);
CREATE INDEX idx_orch_events ON orchestration_events(orchestration_project_id, at DESC);
CREATE UNIQUE INDEX ux_orch_events_decision ON orchestration_events(decision_id) WHERE decision_id IS NOT NULL;
```

- Súhlas s handoverom: audit, nie zdroj pravdy. Timeline v UI je čítanie tejto tabuľky.
- `refused` riadky (policy povedala nie) sú najcennejšie pre ladenie. „Prečo to agent nespravil" je rovnako dôležité ako „prečo to spravil".
- Retencia: continuous projekt beží mesiace, preto cap na projekt (napr. 5 000 riadkov). Staršie sa zhrnú do jedného `digest` riadku, rovnako ako `STEP_CAP` / `PROGRESS_CAP`.

## 5. Orchestrátor a loop

### 5.1 Planner je bezstavové volanie, nie session

Tri možnosti:

| Možnosť | Za | Proti |
|---|---|---|
| A. Dlho žijúca orchestrator session (handover) | modelový kontext medzi krokmi | pane, tmux, kompakcia, reštart; „pamätá si projekt" je presne to, čomu sa handover chce vyhnúť; potrebuje MCP a nástroje, teda širokú útočnú plochu |
| B. Operátor (singleton) robí planning vo vláknach | existuje, má UI | jeden operátor pre všetky projekty; switch vlákna stojí 3–6 s reštartu; operátor je konverzačný, nie dávkový |
| **C. Uzamknutý `claude -p` na každé rozhodnutie** | `claude_print.rs` už existuje (bez hookov, nástrojov, MCP aj transcriptu; capped výstup; tag line, ktorú model nevie podvrhnúť); obnova po páde je triviálna; cena je predvídateľná | každé volanie platí snapshot znova (preto §5.4) |

**Odporúčanie: C.** Planner nemá žiadne nástroje, takže ani prompt injection z výsledku workera nevie nič vykonať, vie len navrhnúť zlý príkaz, a ten prejde policy (§7). „Model rozhoduje, Fleet vlastní stav" je tu doslova. Ľudská diskusia o projekte („prečo si to rozdelil takto?") ide do operátorovho vlákna na koreňovej úlohe, ktoré dostane rovnaký snapshot.

Planner beží na hoste, ktorý zvolí Fleet (host s účtom a headroomom, `account_usage`). Model je nastavenie projektu (`policy.planner_model`), predvolene silnejší model; lacné rozhodnutia (`classify_result`) môžu ísť na Haiku cez existujúci `decide::haiku`.

### 5.2 Loop

Čistý planner a vymeniteľný executor, rovnaký vzor ako `playbooks.rs`:

```
wake(project)                    ← udalosť alebo tick (next_wake_at)
  take lease (lease_until)       ← nikdy dva behy naraz, ani hub + desktop
  observe  → ProjectSnapshot     (čisté čítanie store)
  if nič sa nezmenilo od last_snapshot a nič nie je ready → sleep
  decide   → planner run → [Command]   (alebo deterministicky, §5.3)
  validate → každý Command proti schéme, grafu a policy → Allowed | NeedsPerson | Refused
  apply    → Allowed vykoná service vrstva (decision_id = idempotencia)
             NeedsPerson → proposal / Ready karta / confirm queue
             Refused → event
  release lease, nastav next_wake_at
```

**Zobudenia** (všetky už existujú ako signály, treba ich len nasmerovať na projekt):
- task prešiel do `done` / `failed` / `cancelled` (`complete_task` na Stop hooku, `sweep_open_tasks`),
- zmena PR alebo CI (`outcome.rs` probe → `pr_evidence`),
- tracker sync zmenil člena projektu,
- človek: accept/reject, odpoveď, zmena plánu, Pause, Resume,
- tick ako poistka (`next_wake_at`).

**Obnova po reštarte:** všetko, čo loop potrebuje, je v store. Po štarte stačí `SELECT … WHERE state = 'active'`, nastaviť `next_wake_at = now` a nechať expirovať staré leasy. Bežiace tasky dobehnú cez existujúci sweep. Polovične vykonaný príkaz sa nezopakuje vďaka `decision_id`. Nie je čo obnovovať v modeli.

### 5.3 Nie každé rozhodnutie potrebuje model

Väčšina krokov je mechanická a Fleet ich spraví sám, bez tokenov:
- dispatch ready položiek až do `max_concurrent`,
- prvý retry s chybou v prompte, keď je ešte v limite,
- spustenie review po `implement` tasku, ak `require_review`,
- uzavretie projektu, keď sú všetky `done_when` overené.

Planner sa volá len na **úsudok**: prvotný rozklad cieľa, vyhodnotenie zlyhania (retry, rozdeliť, vzdať, opýtať sa), follow-upy z review, a otázka „je cieľ splnený?". Toto je hlavná páka na cenu a na to, aby autonómia nevytvárala viac procesu než úžitku (counter-review).

**Jev (K3, test map §5).** Výsledok workera bez JSON hlásenia a ďalší krok po zlyhaní (`retry / split / give_up / ask`) sa pýtajú Jev ako lacný predfilter pred plannerom, od O2 v shadow režime. Jev nikdy nenavrhuje `complete` ani neoznačí `done_when` za overené.

### 5.4 ProjectSnapshot

Súhlas s handoverom, s dvoma pravidlami navyše:
- **Pevný rozpočet znakov** (napr. 24 kB) s prioritou: cieľ a `done_when` → otvorená otázka rozhodnutia → zlyhania a blokované → bežiace → ready → posledných N hotových (len `summary`) → zhrnutie staršej histórie (digest z `orchestration_events`).
- **Text od workerov je v snapshote označený ako nedôveryhodný blok** (rovnaký vzor ako `untrusted` pri `dispatch_task`). Planner ho číta ako dáta.

### 5.5 Príkazy plannera

Výstup je JSON pole s pevnou schémou. Neznámy príkaz alebo neplatné pole znamená odmietnutie celého behu a `refused` event, nie čiastočné vykonanie.

| Príkaz | Vykoná | Poznámka |
|---|---|---|
| `create_item {title, notes, done_when[], project_id, depends_on[], size, touches[]}` | `work_link create` / `propose` | `allow_create_tasks = auto` → položka, `propose` → proposal (dnešné UX) |
| `add_dep`, `remove_dep` | store | kontrola cyklov |
| `run {item_id, role}` | `work_link run` (start path) | worktree, brief, nonce |
| `retry {item_id, note}` | `run` s chybou a nesplnenými riadkami | `max_retries` |
| `cancel {item_id}`, `hold {item_id}` | `cancel_task` + Escape | Stop zatiaľ nezastaví workera (gap 11 Mode C) |
| `ask {question, options[]}` | Needs-you položka, karta na desktope a telefóne | projekt prejde do fázy `needs_input` |
| `complete {evidence}` | stav `completed` | len ak sú všetky `done_when` overené, inak `refused` |
| `note {text}` | event | vysvetlenie pre timeline |

`change_priority` z handoveru vynechávam, kým `work_items` nemá prioritu (nemá). Poradie dáva graf.

## 6. Výsledok tasku a evidencia

- **Hlásenie workera:** inštrukcia pri `run` žiada za markerom `FLEET_TASK_DONE_<nonce>` jeden fenced JSON blok: `summary`, `outcome` (`done | partial | blocked | failed`), `tests_run[]`, `warnings[]`, `blockers[]`, `followups[]`, `confidence`. Parser je tolerantný: bez JSON ostane dnešný odsek v `result` a `result_json = null`. Uloží sa do `tasks.result_json` (cap).
- **Čo zistí Fleet sám** a nikdy neberie z hlásenia: commity a zmenené súbory (git na hoste vo worktree tasku: `merge-base` voči base, `diff --stat`), PR a CI (`outcome.rs`, `evidence.rs`). Toto je `evidence` časť snapshotu.
- **`done_when` riadky sú typované**, ako v Mode C C3: `ci:<check>`, `review` (výsledok reviewer tasku), `test:<command>` (spustí *tester* task, nie planner), `person`. Overenie = journal riadok `verify`. Položka je `Verified` až po všetkých riadkoch. Planner nemôže položku označiť ako overenú, môže len navrhnúť `complete`, ktoré Fleet overí.
- Orchestrátor nikdy nečíta celé transcripty, súhlas s handoverom. Potrebné detaily pre zlyhanie (posledná chyba, posledných pár krokov z `work_journal` `step`) sú v snapshote capped.

## 7. Policy, granty a ľudské brány

### 7.1 Vrstvy

```
efektívne právo = min( orchestrator.max_level (global, default 0),
                       orgs.max_autonomy,
                       project.level,
                       per-item override )
                  ∩ aktívny run grant (podpísaný človekom, expiruje)
                  − „vždy potvrdiť" zoznam
```

- `policy_json` projektu je **čo projekt žiada**: `max_parallel_workers`, `max_retries_per_task`, `max_total_tasks`, `task_creation: auto | propose`, `require_review`, `allow_push_branch`, `planner_model`, budgety.
- **Grant** je čo človek podpísal: projekt, repos, hosty, budgety a expirácia. Mode C ho viazal na plán, teraz sa viaže na projekt a `plan_version`. Zmena plánu, ktorá pridá repo alebo zvýši budget, vyžaduje nový podpis.
- Granty podpisuje `owner_person_id` alebo admin orgu (107). Nikdy operátor, per-host token ani planner.

### 7.2 Vynútenie aj na workeroch

`allow_push` alebo `allow_merge` ako príznak v DB nič nezastaví: workery bežia bez permission promptov a `git push` či `gh pr merge` volajú cez Bash, mimo `confirm_gate_with`. Preto, rovnako ako Mode C:
- worker spustený projektom dostane **fleet PreToolUse hook**, ktorý mimo grantu odmietne push na default branch, `gh pr merge`, `gh pr ready` a tracker CLI,
- záloha je branch protection na remote (dokumentovaná, nie vynútená Fleetom).

### 7.3 Brány

| Akcia | Predvolene |
|---|---|
| implement, test, review v izolovanom worktree | auto v rámci grantu (L2+) |
| commit vo worktree tasku | auto |
| push vetvy tasku (nie default) | podľa `allow_push_branch` v grante |
| otvoriť draft PR | auto pri L2+ |
| PR z draftu na ready, merge, push na default | **vždy človek** (TS11) |
| zmazať branch alebo worktree, kill session | **vždy človek** |
| `add_project`, clone nového remote | **vždy človek** |
| zápis do Jira/Asana | **vždy človek** (C3) |
| zvýšiť budget alebo level, cross-org | **vždy človek** |
| akceptovať proposal od workera | **vždy človek**, aj pri L3 |

### 7.4 Hub confirm queue je tvrdá podmienka

Dnes hub každé potvrdenie od operátora odmietne. Bez fronty, v ktorej čaká „planner chce X" a človek ho schváli z desktopu alebo telefónu, môže projekt na hube bežať len do L1 (Ready karty, ktoré človek stlačí). **fleet-mobile** je tu podstatný: schvaľovanie a Pause all z telefónu sú presne to, čo dlho bežiaci projekt potrebuje.

### 7.5 Brzdy

Prevzaté z Mode C: budgety (USD z `usage.rs` + `org_spend.rs`, `account_stop_pct`), `no_progress_secs`, rovnaká chyba dvakrát → Needs you, Pause all, kill switch `orchestrator.enabled`, Pause jedného agenta a Take over. Navyše pre projekt:
- **strop počtu planner behov za hodinu** (bráni slučke „replan → fail → replan"),
- **strop položiek na projekt** (namiesto `PLAN_ITEMS_CAP = 10` napríklad 30, s tým, že continuous projekt počíta len otvorené).

## 8. Worktrees a integrácia

- Jedna paralelná položka = jeden worktree a jedna vetva cez start path. Sekvenčná reťaz v jednom repe zdieľa jednu vetvu a jedno PR (Mode C §4.3). Orchestrácia nikdy nepíše cesty.
- **Stav integrácie** pred záverečným krokom: Fleet spustí na hoste `git merge-tree --write-tree <base> <a> <b>` (bez zásahu do worktree; vyžaduje git ≥ 2.38 na hoste, inak `unknown`) a určí `clean | conflict | stale` (base sa pohol) `| needs_rebase`. Výsledok ide do snapshotu.
- **Konflikt = nová položka** „Resolve integration conflict TASK-101 × TASK-103" so závislosťou na oboch. Súhlas s handoverom. Integráciu robí dedikovaný `integrate` task (TS13), merge do main potvrdzuje človek.
- **Predchádzanie konfliktom** je lacnejšie ako ich riešenie: `touches` (glob cesty) pri položke a pravidlo, že položky s prekrývajúcimi sa `touches` v tom istom repe nebežia paralelne.
- Lokálny workspace sync (109) je ortogonálny. Orchestrovaný worktree sa dá prepojiť na lokálny adresár ako hocijaký iný, ale sync dnes beží len v desktop appke.

## 9. UI

Súhlas s handoverom: nie ďalší chat. Konkrétne:

- **Kde:** Work tab dostane skupinu „Projekty" (meno v UI viď O2). Projekt je stránka nad koreňovou úlohou, takže TaskDetail koreňa *je* jej spodná časť.
- **Hlavička:** cieľ, stav a fáza, level s vysvetlením stropu („efektívne L1, strop orgu"), budget ($3.10 / $8, 2/3 agentov), repos, `[Pause all] [Stop]`.
- **Needs your attention** navrchu, nie dole: otázky plannera, proposals, potvrdenia, zlyhania.
- **Graf:** začať zoznamom vĺn (W1, W2…) s ✓ / ● / ⚠ a „čaká na a". Ide o run board z Mode C §4.4. Skutočný DAG nakreslený ako graf je až druhý krok. Pri 10–30 uzloch je zoznam vĺn čitateľnejší.
- **Timeline** z `orchestration_events` s filtrom „len rozhodnutia / len zlyhania".
- **TaskDetail:** doplniť závislosti a závislé, projekt, pokusy (task attempts s rolou), a „rozhodnutia o tejto položke" (eventy filtrované na `work_item_id`). Druhý TaskDetail sa nestavia.
- **Telefón:** čítanie projektu, Needs you, schvaľovanie a Pause all. Editácia plánu nie.

## 10. Fázy (každá sa dá vydať samostatne)

Mapované na Mode C, aby sa nestaval rovnaký kus dvakrát.

| # | Fáza | Obsah | Mode C | Autonómia |
|---|---|---|---|---|
| **O0** | Pripravenosť | `work_link { run, item_id, role? }` cez start path + nonce (`service/work/run.rs`), `tasks.work_item_id/attempt/role` (migrácia 110). **Zmenené pri implementácii:** presun `dispatch_task` zo MCP handlera do service sa odkladá do O4. `run` ho nepotrebuje a loop bude spúšťať cez `run`, nie cez dispatch do existujúcej session. | časť C4 | žiadna |
| **O1** | Kontajner | `orchestration_projects`, `_repos`, `work_items.orchestration_project_id`, `orchestration_events` (len zápis). CRUD, desktop stránka, verdikty, izolácia, telefón len čítanie. | nahrádza C1 `work_plans` | žiadna |
| **O2** | Graf | `work_item_deps`, READY/BLOCKED, vlny, `propose_tree`, `accept_many`, Undo | C2 | L0 |
| **O3** | Evidencia | `result_json`, git-zistené commity a súbory, typované `done_when`, `verify`, Verified/Unverified. **Pred akoukoľvek autonómiou.** | C3 | L0 |
| **O4** | Manuálna orchestrácia | Ready karty, *Start wave*, run board, retry tlačidlo; deterministický loop (§5.3) bez modelu | C4 | L1 |
| **O5** | Planner | ProjectSnapshot, uzamknutý `claude -p`, príkazy §5.5, lease, zobudenia, obnova po reštarte. Všetky príkazy plannera idú ako proposals alebo Ready karty. | nové | L1 |
| **O6** | Granty | **hub confirm queue**, run grants na projekt, PreToolUse hook, budgety, no-progress stop, Pause all, Take over, schvaľovanie na telefóne | C5 | L2 |
| **O7** | Review loop | role review/test/integrate, follow-upy z review, `merge-tree` stav integrácie, retries s kontextom | C6 | L2–L3 |
| **O8** | Continuous | `mode = continuous`, zobudenia z trackera, CI, PR a časovača, digest histórie, strop otvorených položiek | nové | podľa grantu |

**Definition of Done** z handoveru (§32) sa splní na konci O7. Body 1–8 a 13 (vytvorí projekt, graf, ready, N workerov v izolovaných worktrees, výsledky, obnova po reštarte) už na konci O5 pri L1, kde človek stláča Start.

## 11. Otvorené rozhodnutia

Každé má odporúčanie. Nič sa nestavia, kým vlastník nepovie áno.

| # | Otázka | Odporúčanie |
|---|---|---|
| O1 | Nahradí `orchestration_projects` `work_plans` z Mode C? | **Áno.** Jedna entita, Mode C sa upraví odkazom sem. |
| O2 | Ako sa to volá v UI, keď „Project" už znamená repo? | **„Misia" / „Mission"** v UI, `orchestration_projects` v kóde. Alternatíva: „Cieľ" / „Goal". |
| O3 | Má projekt koreňovú work položku? | **Áno**, epik alebo tracker ticket. Dostane TaskDetail, sessions, sprint a vlákno zadarmo. |
| O4 | Orchestrátor: session alebo bezstavové volanie? | **Bezstavový uzamknutý `claude -p`** na rozhodnutie. Diskusia s človekom ide do operátorovho vlákna. |
| O5 | Viac pokusov: `tasks.work_item_id` alebo tabuľka `task_attempts`? | **Stĺpce na `tasks`.** |
| O6 | Uložené stavy alebo odvodené fázy? | **7 uložených lifecycle stavov + odvodené fázy.** |
| O7 | Smie planner vytvárať položky priamo? | **Do O6 nie**, všetko ide cez proposals. Potom podľa grantu `task_creation = auto`. Proposal od workera vždy čaká na človeka. |
| O8 | Kde beží loop? | **Vo vlastníkovi store** (hub, inak desktop), na ticku, s leasom. |
| O9 | Hĺbka stromu | **Koreň → úloha → podúloha, nie viac.** Štruktúru nesie graf závislostí. |
| O10 | Strop položiek na projekt | **30**, continuous počíta len otvorené. |
| O11 | Smie L3 mergovať? | **Nie** (TS11). „Merge on green" môže byť neskôr políčko v grante. |
| O12 | Začať O0/O1 hneď, alebo najprv dokončiť A3/A4/B1 z task→session? | **O0 hneď** (refaktor dispatch je užitočný sám osebe), O1 po A3. B1 (vlákna) nie je podmienka. |

## 12. Mimo rozsahu

- Viac plannerov na jeden projekt (multi-agent planning).
- Automatické deploye.
- Cross-org projekty.
- Zápis do trackerov nad rámec dnešného Jira PR linku.
- Plánovanie naprieč projektmi (zdieľané workery, globálna fronta). Prvá verzia: každý projekt má vlastné `max_concurrent`, globálny strop dáva `orchestrator.max_concurrent`.

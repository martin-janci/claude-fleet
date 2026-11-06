# Misie po líniách: jedna vedúca session na líniu, subagenti vnútri

**Dátum:** 2026-10-23
**Stav:** rozhodnutia D1–D9 (§9) vlastník prijal podľa odporúčaní 2026-10-24. **L0 je postavené** (2026-10-23): oprava P2 a metriky misie (§5.2, §5.5). **L1 je postavené** (2026-10-23): subagenti v prompte behu, plánovač, ktorý číta repo, a pravidlo o spoločných súboroch (§5.1, §5.4). **L2 je postavené** (2026-10-24): línie v dátach, bez zmeny behu (§2.2). **L3 je postavené** (2026-10-24): vedúca session na líniu za `policy.execution = lane` (§3, „Ako je to postavené v L3“). **L4 je postavené** (2026-10-24): review nad snapshotom, test ako brána, štafeta aj podľa kontextu, headless checks predvolene zapnuté (§3.10). **L5 je postavené** (2026-10-25): `touches` a pravidlo prekrytia (§5.4, „Ako je to postavené v L5“).
**Nadväzuje na:** `2026-10-07-autonomous-orchestration-projects-design.md` (ďalej *O-návrh*), najmä §5.3 (deterministický loop), §6 (evidencia), §8 (worktrees a integrácia).
**Vstup:** analýza „ako zlepšiť misie: jeden Claude so subagentmi alebo veľa sessions" (2026-10-23).

## 0. Zhrnutie

1. Dnes je jednotkou práce misie **úloha**: každý beh (`implement`, `retry`, `review`, `test`) je vlastná interaktívna session s vlastným worktree a vetvou (`work/run.rs` → `tickets::start_work_unprompted`). Každý beh platí studený štart (kontext, CLAUDE.md, prieskum kódu, build) a paralelné vetvy sa na konci integrujú.
2. Návrh mení jednotku na **líniu** (lane): usporiadanú množinu úloh misie v jednom repe, ktorú robí **jedna vedúca session na jednej vetve** postupne. Fleet jej úlohy podáva po jednej ako samostatné `tasks` riadky, takže sledovanie, evidencia, `done_when`, granty a brány ostávajú po úlohách ako dnes.
3. Vedúca session používa **subagentov** (Claude Code `Agent`) na prieskum, review pred hlásením a hľadanie príčin. Nezávislé overenie ostáva Fleetu (headless review a test), nie vedúcemu.
4. Paralelizmus sa presúva z úloh na línie: línie v rôznych repách alebo s disjunktnými `touches` bežia naraz, línie, ktoré by sa zrazili, nie. Integrácia je potrebná len medzi líniami v jednom repe.
5. Zavádza sa za prepínačom misie `policy.execution = lane`. Dnešný režim (`item`) ostáva pre nezávislé úlohy a ako záloha. Predvolený režim sa prepne až po A/B porovnaní na skutočných misiách (L6).

## 1. Problém

### 1.1 Čo vieme z kódu

| # | Zistenie | Kde | Dôsledok |
|---|---|---|---|
| P1 | Každý beh je nová session, nový worktree a nová vetva `branch_slug(key+title)` | `run.rs::run_item_with`, `tickets.rs::branch_slug` | studený štart na každý beh: kontext, prieskum, v Rust repe build workspace od nuly |
| P2 | Ak na položke ešte žije session (hotový implementátor beží ďalej), ďalší beh dostane `parallel = true`, teda **nový `-N` checkout z default vetvy** | `run.rs` (`has_live_work`), `StartArgs::parallel` | **potvrdené a opravené v L0:** nový checkout vznikal z default vetvy (`start_one`: `base_branch: None`) a brief menoval len vlastnú prázdnu vetvu, takže review nevidel prácu a retry začínal od nuly. |
| P3 | Plánovač je `claude -p` bez nástrojov | `orchestrate/planner.rs` | rozkladá cieľ bez znalosti súborov, teda bez ohľadu na to, čo sa zrazí |
| P4 | `touches` a pravidlo „prekrývajúce sa úlohy nebežia paralelne" z O-návrhu §8 nie sú postavené | grep `touches` v `service/work/` | konflikty sa riešia až po fakte |
| P5 | Sekvenčná reťaz v jednom repe nezdieľa vetvu (O-návrh §8 to predpokladal) | `run.rs`, `branch_slug` | reťaz A → B → C sú tri vetvy, B a C sa navzájom integrujú |
| P6 | Integrácia je najčastejšie opravovaná časť misií | `a7d18b9a`, `c56727a1`, `d7046121`, `06f76cea` | merge-wave pravidlá a integračné úlohy sú réžia, nie progres |
| P7 | Hotspoty repa, kde sa dve paralelné vetvy zrazia takmer vždy: číslo migrácie a `MIGRATIONS`, `control-api-reference.md`, `hub_verdicts.generated.json`, `BUDGET_BYTES`, `docs/status.md` | CLAUDE.md | v claude-fleet sú „nezávislé" úlohy v jednom repe zriedkavé |
| P8 | `orchestrator.headless_checks` je predvolene vypnuté | `settings.rs:383` | review a test sú ďalšie celé sessions |

### 1.2 Čo nevieme

Koľko z času misie je práca a koľko čakanie (tick, štart, brief, človek, integrácia), koľko behov treba na jednu overenú úlohu a koľko stojí jedna úloha. Dáta na to sú v `tasks` a `orchestration_events`, ale nikto ich neagreguje. Preto je L0 meranie (§7), nie kód.

## 2. Model

```
Mission
  ├─ Lane  (repo, vetva, worktree, vedúca session, stav)
  │    ├─ WorkItem 1 ─ Task (implement, attempt 1)  ─┐
  │    ├─ WorkItem 2 ─ Task (implement, attempt 1)   ├─ jedna vedúca session, po jednej
  │    └─ WorkItem 3 ─ Task (implement, attempt 2)  ─┘
  │                      └─ Task (review, headless, snapshot na commite úlohy)
  └─ Lane  (iné repo alebo disjunktné touches) … paralelne
```

### 2.1 Línia

**Línia** je usporiadaná množina úloh misie v jednom repe s jednou vetvou a nanajvýš jednou živou vedúcou session.

- **Odkiaľ sa berie:**
  - z importu plánu: stĺpec `Lane` už existuje (`plan_import.rs`), dnes sa ukladá ako assignee,
  - od plánovača: `create_item { …, lane }` (§5.4),
  - od človeka: presun úlohy medzi líniami v UI,
  - predvolene **jedna línia na repo** misie.
- **Poradie v línii** dáva graf závislostí (vlny, `graph.rs`). Úlohy jednej línie bez vzájomnej závislosti sa robia v poradí vĺn a potom podľa id. Nič sa neukladá navyše.
- **Závislosť medzi líniami** (úloha v línii B čaká na úlohu v línii A) funguje ako dnes. B čaká, kým A nie je `done`. Ak sú v tom istom repe, B pred štartom úlohy zmerguje vetvu A do svojej (§3.6).

### 2.2 Dátový model (aditívny; číslo migrácie pri implementácii z `origin/main`)

```sql
CREATE TABLE mission_lanes (
  id                       INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  name                     TEXT    NOT NULL,          -- „backend", „mobile", z plánu
  project_id               INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  branch                   TEXT,                      -- slug(mission + lane), raz zvolený, nemení sa
  worktree_id              INTEGER REFERENCES worktrees(id) ON DELETE SET NULL,
  lead_session_id          INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
  relay                    INTEGER NOT NULL DEFAULT 0, -- koľká vedúca session v poradí (§3.5)
  state                    TEXT    NOT NULL DEFAULT 'idle', -- idle | working | waiting | done | held
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (orchestration_project_id, name)
);
ALTER TABLE work_items ADD COLUMN lane_id INTEGER REFERENCES mission_lanes(id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN start_head TEXT;  -- HEAD vetvy línie pri doručení úlohy (§3.3)
ALTER TABLE tasks ADD COLUMN end_head   TEXT;  -- HEAD pri hlásení; evidencia je start_head..end_head
```

- `state` línie je **odvodený pre UI** rovnako ako fáza misie (O-návrh §4.5). Uložené sú len `held` (človek ju zastavil) a `done`. Ostatné sa počítajú z taskov a session.
- `lane_id` je `NULL` pre úlohy misie v režime `item` a pre koreň misie.
- Assignee z importu ostáva assignee. Import v režime `lane` navyše založí alebo nájde líniu podľa mena.

**Ako je to postavené v L2** (migrácia 209, `store/mission_lanes.rs`), odchýlky od SQL vyššie:
- `mission_lanes.project_id` je **nepovinné** (`ON DELETE SET NULL`): línia z plánu nemusí mať vlastné repo. Prvé repo, ktoré niekto pri línii uvedie, si ponechá.
- `tasks.start_head` / `end_head` sa presúvajú do **L3**, kde ich prvý raz niečo číta. L2 nemení `tasks` ani `WorkItemRow`: `lane_id` sa číta samostatným dotazom (`Store::mission_lanes`, `item_lane`).
- Línia vzniká pri prvom použití mena a zaniká, keď v nej neostane úloha a nemá vedúcu session. Úloha, ktorá opustí misiu, opustí aj líniu. Koreň misie v línii nebýva.
- Kto úlohu do línie dáva: import plánu (stĺpec `Lane`, meno overené pred zápisom), plánovač (`create_item { lane? }`, nesené v karte `create` ako `done_when`) a človek (`work_link { mission_item, lane }`; `""` vyberie z línie; desktop `set_mission_item { lane? }`). Novú akciu `mission_lane` som nepridal: rozšírenie `mission_item` nepotrebuje nový verdikt, riadok izolačnej matice ani desktop príkaz.
- `work { mission }` vracia `lanes` (každá s viditeľnými úlohami), Graph view má zoskupenie **Lane** (predvolené, keď línia drží úlohu) a riadok úlohy má výber línie s „New lane…“. Zmena sa zapíše do logu misie (`lane_set`).
- `policy.execution` (§2.3) je **L3**. fleet-mobile línie zatiaľ neukazuje; nové pole odpovede ignoruje.

### 2.3 Prepínač

`policy.execution: item | lane`, predvolene `item`. Mení sa len na misii v stave `draft` alebo `paused` bez otvorených behov. Zmena z `lane` na `item` nechá vetvy línií tak, ako sú, a ďalšie behy idú po úlohách.

## 3. Vedúca session

### 3.1 Štart

Prvá úloha línie, ktorá je `ready` a na ktorú má misia slot (§4), naštartuje líniu cez existujúcu štartovaciu cestu:

- `StartArgs { worktree: Some(lane.branch), parallel: false, origin: mission, profile: grant.profile, … }`. Vetva sa volá `slug(mission.name + " " + lane.name)`. Existujúci worktree s rovnakým menom sa použije znova, takže štafeta (§3.5) aj reštart nájdu tú istú vetvu.
- **Prvý prompt** je brief línie, nie úlohy:
  - cieľ misie a non-goals,
  - zoznam úloh línie v poradí s `done_when` (len názvy a jedna veta, nie celé poznámky),
  - pravidlá línie: jedna úloha naraz, po každej `task_report`, nepúšťať sa do ďalšej, kým ju Fleet nepodá,
  - pokyny pre subagentov (§3.4).
- Za briefom ide hneď prvá úloha ako task (§3.2).

### 3.2 Podávanie úloh

Úloha = `tasks` riadok (`work_item_id`, `attempt`, `role = implement`, `worker_session_id = lead`) a prompt s `FLEET_TASK_DONE_<nonce>` inštrukciou, rovnako ako dnes (`report::with_run_instruction`). Rozdiel je, že sa doručí **do existujúcej session**:

- Loop podá ďalšiu úlohu, keď predošlý task línie je terminálny, session je nečinná (stav REPL, ako pri `dispatch_task` do existujúcej session) a ďalšia úloha je `ready`.
- Prompt obsahuje kľúč, názov, poznámky a `done_when` úlohy, plus „čo ostalo z predošlých pokusov" pri retry (dnešný `context`).
- `start_head` sa zapíše pri doručení (`git rev-parse HEAD` vo worktree línie, už tak či tak číta evidenciu).
- Kým vedúci pracuje, na línii nie je otvorený žiadny iný implement task. Jedna línia nikdy nemá dve úlohy naraz.

### 3.3 Evidencia po úlohách

Dnes `report::evidence_script` počíta commity a súbory voči merge-base s default vetvou. V línii by úloha 3 „videla" aj commity úloh 1 a 2. Preto:

- evidencia úlohy = `start_head..end_head`, kde `end_head` sa prečíta pri `task_report` alebo Stop,
- brána dokončenia (`gate.rs`, „implement bez evidencie") platí bez zmeny, len na tomto rozsahu,
- prázdny rozsah (vedúci nič necommitol) je `missing` rovnako ako dnes.

Pravidlo pre vedúceho v brief: **na konci každej úlohy commit** (nie squash, nie amend predošlej úlohy). Rebase alebo amend, ktorý prepíše `start_head`, Fleet zistí (`start_head` už nie je predok `HEAD`) a úlohu označí `ask` s vysvetlením namiesto toho, aby počítal zlú evidenciu.

### 3.4 Subagenti

Vedúci smie a má používať Claude Code subagentov. Fleet od nich nič nevyžaduje a nič na nich nestavia. Sú to len lacnejšie a paralelné ruky vedúceho:

| Na čo | Ako | Prečo |
|---|---|---|
| prieskum pred úlohou | read-only subagent (Explore) | hlavný kontext ostane čistý, viac miest naraz |
| review pred `task_report` | subagent s čerstvým kontextom nad `git diff start_head` | zachytí chyby skôr, než ich nájde headless review a stojí to retry |
| hľadanie príčiny po neúspešnom teste | subagent | vedúci nečíta stovky riadkov logu |
| písanie kódu | **nie** paralelne | subagenti zdieľajú worktree aj cargo lock, súbežné zápisy sa bijú |

- Definície subagentov (`mission-explorer`, `mission-reviewer`) dodá Fleet cez katalóg (druh `agent` je v katalógu Claude Code subagent, `2026-10-12-configurable-agents-design.md`). Model subagenta môže byť lacnejší než model vedúceho.
- Hook udalosti `SubagentStart` / `SubagentStop` sa už dajú prijímať (hook-events návrh). V L4 ich timeline úlohy môže ukázať ako „prieskum: 3 subagenti". Nič na nich nezávisí.
- PreToolUse guard (`orchestrate/guard.rs`) sa vzťahuje na session, takže subagenti pod ním bežia tiež.

### 3.5 Kontext a štafeta

Jedna session na celú líniu narazí na veľkosť kontextu. Claude Code kompaktuje sám, ale kompakcia v strede úlohy stráca detaily. Preto:

- **Štafeta medzi úlohami:** po `policy.lane_relay_after` úlohách (predvolene 4) alebo keď Fleet z hookov či usage vidí kontext nad prahom, loop **nepodá** ďalšiu úlohu starej session, ukončí ju (`kill_session`, ako pri retry dnes) a naštartuje novú vedúcu session v **tom istom worktree** (`relay += 1`).
- Brief štafety je brief línie plus „hotové v tejto línii" zo store: názvy úloh, `summary` z `result_json` a rozsah commitov. Nič sa neprenáša z modelu, zdrojom je store (rovnaký princíp ako O-návrh §5.2).
- Štafeta je lacná: worktree a build cache ostávajú, mení sa len kontext.

### 3.6 Retry, zlyhanie, pád

| Situácia | Dnes | V línii |
|---|---|---|
| prvé zlyhanie úlohy | nová session v `-N` checkoute | **retry v tej istej session**: vedúci má kontext zlyhania, prompt nesie chybu ako dnes |
| rovnaká chyba druhýkrát | `ask` | `ask` (bez zmeny) |
| ďalší retry po review | nová session | štafeta (čerstvá session, ten istý worktree), lebo „čerstvé oči" sú tu cieľ |
| vedúca session spadne alebo je stratená | sweep zlyhá task | sweep zlyhá task, línia ostane `idle`, ďalší krok je štafeta s retry |
| usage limit | beh čaká (`limit.rs`) | línia čaká, ostatné línie na iných loginoch idú ďalej |
| závislosť na línii v tom istom repe | nová vetva z default | pred doručením úlohy loop vo worktree línie spraví `git merge --no-edit <vetva línie A>`. Konflikt → `ask` alebo integračná úloha podľa §4.2 |

Zahodenie práce: retry v línii pokračuje z `HEAD` línie, nie z default vetvy. Ak vedúci chce začať odznova, spraví to vo vetve sám (`git revert`), a evidencia to ukáže.

### 3.7 Review a test

`done_when` riadky a ich význam sa nemenia. Mení sa, kde bežia.

- **`review`**: headless (`checks.rs`) nad **snapshotom úlohy**, teda dočasný `git worktree add --detach <end_head>` (vzor `agents/workspace.rs:107`), diff `start_head..end_head`. Vedúci medzitým môže robiť ďalšiu úlohu, lebo snapshot sa s jeho checkoutom nehýbe. Toto rieši aj dnešnú podmienku `checks.rs` „checkout sa pohol → session cesta".
- **`test:<cmd>`**: beží v **checkoute línie**, kým vedúci čaká. Loop nepodá ďalšiu úlohu, kým test úlohy nedobehne, aby test meral presne tú úlohu a build bol teplý. `policy.lane_test_at: item | lane` (predvolene `item`) dovolí hromadiť testy na koniec línie, keď je testov veľa a sú pomalé.
- **`ci`**: z PR línie (§3.8). CI sa viaže na commit, takže `end_head` úlohy sa overí, keď CI dobehne na commite, ktorý ho obsahuje.

### 3.8 PR a vetva

- Jedna línia = jedna vetva = jeden PR. Vedúci otvorí draft PR po prvej úlohe, ak to grant dovolí (`allow_push_branch`, O-návrh §7.3). Ďalšie úlohy ho len dopĺňajú.
- Ready a merge zostávajú človeku (TS11, guard).
- „Merge wave" položky (`check_merge_waves`) v režime `lane` nie sú potrebné na integráciu vetiev úloh jednej línie. Ostávajú len medzi líniami jedného repa (§4.2).

### 3.9 Ako je to postavené v L3

- `policy.execution` (`item` | `lane`, predvolene `item`) a `policy.lane_relay_after` (1–20, predvolene 4). Režim sa mení len na misii, ktorá nebeží a nemá otvorené behy; v UI je to „Tasks run“ v úprave misie.
- `tasks.start_head` (migrácia 210) a evidencia úlohy `start_head..HEAD` (`report::evidence_script_since`). `end_head` netreba: je to `TaskEvidence.head`. Prepísaná história (štart už nie je predok `HEAD`) je chyba evidencie, takže brána dokončenia úlohu nezavrie.
- `orchestrate/lanes.rs`:
  - `gate`: v režime `lane` jeden implement beh na líniu, prvý v poradí grafu, a žiadny, kým je línia `busy_lanes` (jej úloha má otvorený implement beh, alebo jej živá vedúca session nie je pri prompte či čaká na dialóg). Úloha bez línie, review, test a integrácia idú ako doteraz.
  - `run_in_lane`: živému vedúcemu úlohu doručí (`deliver`: nový `tasks` riadok so `start_head`, napísaný cez typing lane ako direct send). Bez živého vedúceho, po `lane_relay_after` úlohách alebo pri treťom pokuse o tú istú úlohu na tom istom vedúcom (`needs_relay`) ukončí starého vedúceho misie a naštartuje nového v tom istom checkoute línie (`start_lead`: brief línie, vetva `slug(M<id> <misia> <línia>)`, `set_lane_lead` zvýši `relay`). Tá istá cesta obnoví líniu po páde vedúceho.
  - Vedúci je vždy session misie (`SessionOrigin::mission`), takže guard, grant a brzdy platia ako pre ktorýkoľvek beh. Id vedúceho sa overuje voči misii a checkoutu línie, lebo id session sa recykluje.
- **Ostáva na L4:** review nad snapshotom úlohy (dnes review línie ide session cestou z vetvy línie vďaka P2, alebo headless, keď sa checkout nepohol), test ako brána pred ďalšou úlohou, PR na líniu ako pravidlo Fleetu (dnes len pokyn v briefe), štafeta podľa zaplnenia kontextu (dnes len podľa počtu úloh) a stav línie `done`.

### 3.10 Ako je to postavené v L4

- **Review úlohy línie nad snapshotom** (`orchestrate/checks.rs`): `implementer_checkout` pozná úlohu línie podľa `start_head`, porovnáva od neho (diff je len tej úlohy) a označí ho `snapshot`. Headless review potom v checkoute línie overí, že commit existuje, spraví `git worktree add --detach` do dočasného priečinka, beží v ňom a EXIT trap ho odstráni. Vedúci medzitým môže robiť ďalšiu úlohu a checkout línie môže byť pohnutý aj špinavý.
- **Test ako brána** (`lanes::awaiting_test`, `busy_lanes`): línia je obsadená, kým jej úloha vo `verifying` má `test` riadok, ktorý ešte neprešiel, alebo kým beží jej test. Test tak beží v checkoute línie na commite úlohy (headless preflight vyžaduje nepohnutý a čistý checkout). `policy.lane_test_at = lane` sa nestavalo.
- **Štafeta aj podľa kontextu**: `needs_relay` odovzdá ďalšiu úlohu novému vedúcemu aj pri `context_pct` ≥ 70 (`RELAY_CONTEXT_PCT`).
- **Línia `done`** sa odvodzuje pri čítaní (`work { mission }`): všetky jej úlohy sú hotové a nie je `held`. Vedúci sa automaticky neukončuje; ukončí ho Clean up pri dokončení misie ako ostatné sessions.
- **`orchestrator.headless_checks` je predvolene zapnuté** (§5.2): P2 aj snapshot sú postavené. Čo headless nevezme (pohnutý checkout pri teste, príkaz so shell syntaxou, host mimo grantu), ide ako doteraz session cestou.
- **Nestavalo sa:** PR na líniu ako pravidlo Fleetu (ostáva pokyn v briefe vedúceho) a definície subagentov v katalógu (pokyn v prompte stačí; definície by pridali katalógovú synchronizáciu na každý host).

## 4. Paralelizmus a integrácia

### 4.1 Sloty

`policy.max_parallel` v režime `lane` počíta **živé vedúce session** (línie v stave `working`), nie tasky. Headless review a test sa počítajú zvlášť (`policy.max_parallel_checks`, predvolene 2), lebo sú krátke a nepíšu.

Dve línie v **tom istom repe** bežia naraz, len ak:
- obe majú vyplnené `touches` (§5.4) a nemajú prienik, alebo
- človek to výslovne povolil na misii (`policy.lanes_same_repo_parallel = true`).

Inak druhá čaká. V claude-fleet pre P7 predvolene čaká.

### 4.2 Integrácia

- V jednom repe s jednou líniou integrácia neexistuje.
- Medzi líniami jedného repa ostáva dnešný mechanizmus (`integrate.rs`: `merge-tree`, jedna integračná úloha na repo), len nad vetvami línií namiesto vetiev úloh. Integračnú úlohu robí vedúci línie, ktorá na tú druhú čaká (§3.6), takže nevzniká nová session.

## 5. Ďalšie body (z analýzy 2–6)

### 5.1 (bod 2) Subagenti aj v dnešnom režime `item`

Pokyny z §3.4 (prieskum, review pred hlásením) sa pridajú aj do promptu behu v režime `item` (`run_prompt`). Je to jedna zmena textu, lacné a hneď merateľné na počte retry.

**Postavené v L1:** `run::subagent_note` podľa roly (`implement` a `integrate`: prieskum a review vlastného diffu cez subagentov, ktorí len čítajú; `review` a `research`: paralelné čítanie; `test`: nič). Pridá sa len Claude workerovi (nie Codex) a len pri zapnutom `orchestrator.run_subagents` (predvolene zapnuté, aby sa dalo porovnať s vypnutým cez metriky z L0).

### 5.2 (bod 3) Headless checks predvolene a P2

- **P2, postavené v L0.** `run::prior_work` nájde vetvu a host najnovšieho implement pokusu s vlastným checkoutom. Beh na tej istej položke (retry, review, test) potom:
  - dostane nový checkout **z tejto vetvy** (`StartArgs::base_branch` → `StartPlan` → `NewSessionArgs::base_branch`, ktoré `worktree_add_script` už vedel),
  - beží na **jej hoste**, ak pri poslednej sonde odpovedal a host nikto nezvolil alebo ho grant misie povoľuje (`orchestrate::start_for`, vtedy s loginom grantu); inak host vyberie placement a skript nájde vetvu ako `origin/<vetva>`, ak bola pushnutá,
  - má v prompte vetu, ktorá vetvu menuje a hovorí, ako ju stiahnuť, keby na hoste chýbala (`prior_work_note`).
  Vetva, ktorú prompt nevie bezpečne niesť (hlásená hostom, s inými znakmi než `[A-Za-z0-9._/-]`), sa nepoužije. Klient `base_branch` do plánu nevie poslať (`skip_deserializing`).
- `orchestrator.headless_checks` predvolene zapnúť, až keď je P2 vyriešené a snapshot z §3.7 hotový (bez neho headless review padá na session cestu vždy, keď sa checkout pohne).

### 5.3 (bod 5) Teplý build

- Režim `lane` rieši väčšinu sám: jeden worktree na líniu, štafeta ho nemení, build je studený raz na líniu, nie raz na beh.
- Pre režim `item` a prvý build línie: experiment so zdieľaným cache na hoste (sccache alebo spoločný `CARGO_TARGET_DIR` na repo cez session env, `2026-10-11-session-env-and-secrets-design.md`). Spoločný target dir serializuje súbežné buildy na cargo locku, preto len pre línie, ktoré aj tak nebežia naraz (§4.1). Rozhodnutie až po meraní z L0.

### 5.4 (bod 4) Plánovač s read-only nástrojmi a `touches`

- Plánovač dostane rovnakú sadu ako headless review: `Read`, `Grep`, `Glob` v checkoute koreňového repa misie, `--permission-mode dontAsk`, nič iné. Výstup ostáva JSON príkazov, validácia a „všetko alebo nič" bez zmeny. Text workerov v snapshote ostáva označený ako nedôveryhodný. Riziko prompt injection rastie len o to, že plánovač môže čítať súbory repa, ktoré misia aj tak smie meniť.
- `create_item` dostane `lane` a `touches[]` (globy ciest). `touches` sa uloží na položku (stĺpec alebo `notes` metadáta; rozhodnutie pri implementácii, preferovaný stĺpec `touches_json`).
- Pravidlo: v režime `item` dve položky s prekrývajúcimi sa `touches` v tom istom repe nebežia naraz. V režime `lane` to isté platí pre línie (§4.1). Bez `touches` sa predpokladá prekrytie.
- Plánovač má v prompte pravidlo z P7: zmena migrácie, MCP nástroja, verdiktu alebo `status.md` patrí do jednej línie na repo.

**Postavené v L1:**
- `orchestrator.planner_reads` (predvolene vypnuté). Keď je zapnuté, `run_planner` pred každým pokusom zaistí checkout projektu misie na hoste plánovača (`sessions::project_checkout_on`; projekt koreňovej úlohy, inak prvé repo misie) a plánovač beží v ňom s `Read`, `Grep`, `Glob` (`planner::planner_script_as` s checkoutom, `claude_print::tool_flags`) a s dlhším limitom (`PLANNER_READS_TIMEOUT_SECS`, 600 s). Prompt (`planner_prompt(true)`) mu povie prečítať CLAUDE.md alebo README, považovať prečítané za dáta a pri `create_item` uviesť v `notes` súbory, ktoré úloha zmení. Checkout, ktorý sa nedá zaistiť, zapíše `note` do logu misie a plánovač plánuje bez čítania.
- Pravidlo o spoločných miestach je v prompte vždy, aj bez čítania, a všeobecne, nie len pre claude-fleet: úlohy, ktoré menia tie isté súbory alebo ten istý spoločný register (zoznam migrácií, generovaný súbor, tabuľka nástrojov či príkazov, stavový dokument), na seba nadväzujú.
- `touches` ako pole a pravidlo prekrytia pri spúšťaní ostávajú pre L5.

**Ako je to postavené v L5:**
- Migrácia 211: `work_items.touches_json`, čítané mimo `WorkItemRow` (`store/item_touches.rs`). Cesty sú relatívne k repu: absolútna cesta, `..`, riadiaci znak, viac ako 20 ciest alebo cesta dlhšia než 200 znakov sú odmietnuté.
- Prekrytie (`touches_overlap`) je konzervatívne: berie sa istá časť cesty pred prvým wildcardom a dve cesty sa stretnú, keď jedna obsahuje druhú po segmentoch (`src/store/**` stretne `src/store/mod.rs`, `src/store` nestretne `src/storefront`).
- Plánovač: `create_item { touches? }`, nesené v karte `create` ako `lane` a `done_when`; prompt čítajúceho plánovača chce `touches` namiesto ciest v `notes`.
- `orchestrate/touches.rs`:
  - režim `item` (`gate_items`): implement beh, ktorého `touches` stretnú otvorený beh (alebo beh vybraný skôr v tom istom priechode) v tom istom repe, čaká. **Odchýlka od návrhu:** úloha bez `touches` sa nedrží, lebo by to dnešným misiám vypol paralelizmus.
  - režim `lane` (`gate_lane_starts`): rozbehnutá línia pokračuje; nová línia čaká, kým v tom istom repe beží iná, pokiaľ obe neuvádzajú `touches` všetkých úloh a nestretnú sa (§4.1). Úlohy bez línie idú cez `gate_items`. `policy.lanes_same_repo_parallel` sa nestavalo.
  - Neznámy repo (`None`) sa berie ako rovnaký s každým; misia s jedným repom ho dosadí úlohe bez projektu.
- `work { mission }` vracia `touches` po úlohách a riadok úlohy ukazuje „changes …“.

### 5.5 (bod 6) Metriky

**Postavené v L0** ako pole `stats` odpovede `work { mission }` (`service/work/missions/stats.rs`, čistá funkcia nad `tasks` členov misie; žiadna nová akcia, teda žiadny nový riadok verdiktov ani tool budget) a riadok v hlavičke miestnosti misie (`mission_room.ts::statsLine`): behy a behy na hotovú úlohu, retry, studené štarty (worker sessions), medián času úlohy a podiel času bez otvoreného behu. Čakanie podľa príčiny (človek, limit, slot, tick) a integračné konflikty z eventov sú ďalší krok, keď eventy budú niesť príčinu. Pôvodný zámer:

| Metrika | Z čoho |
|---|---|
| čas od `ready` po `verified` na úlohu (medián, p90) | eventy `dispatched`, `verified`, `task_done` |
| čas čakania: na slot, na človeka (karta, ask), na limit, na tick | eventy `asked` / `answered`, `limit_wait`, `account_limit` |
| behy na overenú úlohu (implement, retry, review, test) | `tasks` podľa `role`, `attempt` |
| integračné úlohy a konflikty na misiu | eventy integrácie |
| cena na overenú úlohu | `tasks` cost, `headless_check` náklady |
| studené štarty (nové worktrees) na misiu | `start_spawned` z timeline |

Tie isté čísla porovnajú režimy `item` a `lane` v L6.

## 6. Bezpečnosť a brány

Nič sa neuvoľňuje:
- Vedúca session je session misie (`SessionOrigin::mission`), teda pod guardom, grantom (hosty, login, budget) a brzdami ako každý worker dnes.
- Grant na misiu kryje línie. Nová línia v repe, ktoré nie je v allow-liste misie, je odmietnutá ako dnes run.
- Ukončenie vedúcej session pri štafete robí Fleet nad vlastnou session misie (rovnaká podmienka ako `orchestrate/mod.rs:~800`: `origin = mission`, `origin_ref = misia`). Session človeka sa nikdy neukončí.
- Merge, ready, push na default, tracker CLI ostávajú človeku.
- Subagenti nedostávajú nič navyše, bežia v tej istej session pod tým istým guardom.

## 7. Roadmap

Každá fáza sa dá vydať samostatne a každá končí meraním proti L0.

| Fáza | Obsah | Správanie | Veľkosť | Výstupné kritérium |
|---|---|---|---|---|
| **L0 Meranie a overenie** | `work { mission_stats }` a panel (§5.5); test pre P2 a oprava, ak sa potvrdí (§5.2) | žiadna zmena okrem opravy P2 | S | baseline z doterajších misií; P2 potvrdené alebo vyvrátené testom |
| **L1 Rýchle výhry** | pokyny pre subagentov v `run_prompt` (§5.1); plánovač s read-only nástrojmi za nastavením `orchestrator.planner_reads` (§5.4); pravidlo P7 v prompte plánovača | opt-in nastavenie, text promptu | S–M | menej retry na úlohu, menej integračných úloh na nových misiách |
| **L2 Línie v dátach** (postavené) | migrácia (§2.2), `lane` pri importe a v `create_item`, presun úlohy medzi líniami (`work_link { mission_item, lane }`), Graph view kreslí línie z `lane_id`; verdikty, izolačná matica, `REGEN_DOCS`, telefón len čítanie | žiadna zmena behu | M | misia sa dá naplánovať po líniách, nič sa nespúšťa inak |
| **L3 Vedúca session** (postavené) | `policy.execution = lane`: štart línie, podávanie úloh do session (§3.2), `start_head`/`end_head` a evidencia (§3.3), retry v session, štafeta (§3.5), pád a limit (§3.6) | opt-in na misii | L | jedna skutočná misia v jednom repe dokončená v režime `lane` bez ručného zásahu mimo brán |
| **L4 Overenie v línii** (postavené) | review nad snapshotom (§3.7), test v checkoute línie a `lane_test_at`, PR na líniu (§3.8), `headless_checks` predvolene zapnuté (§5.2), subagent definície v katalógu (§3.4) | opt-in na misii, `headless_checks` globálne | M | `done_when` overené bez session ciest pre review |
| **L5 `touches` a plánovanie línií** (postavené) | `touches_json`, pravidlo prekrytia pre `item` aj `lane` (§4.1, §5.4), plánovač priraďuje línie; integrácia len medzi líniami (§4.2) | platí v oboch režimoch | M | v misii s dvoma líniami v jednom repe nevznikne konflikt, ktorý `touches` mohli predpovedať |
| **L6 Porovnanie a prepnutie** | A/B: ten istý typ misie v `item` a `lane`, porovnanie metrík z L0; build cache experiment (§5.3) | rozhodnutie vlastníka | S | vlastník rozhodne o predvolenom režime podľa čísel |

Závislosti: L0 → všetko (baseline). L2 → L3 → L4. L1 a L5 sú nezávislé od L3, L5 ťaží z L2. Build cache experiment môže bežať popri L3.

**Odhad prínosu (hypotéza na overenie v L6, nie meranie):** v misii s 5 previazanými úlohami v jednom repe ubudnú 4 z 5 studených štartov implementácie, review a test bez vlastnej session, a integrácia medzi úlohami zmizne. Stratí sa paralelizmus vnútri repa, ktorý v claude-fleet pre P7 aj dnes končí integračnými úlohami.

### 7.1 L6: postup porovnania

Nástroj: **Misie → Compare** (`MissionCompare.svelte`, `mission_compare.ts`). Ukazuje misie, ktoré bežali, a medián na režim (`item` / `lane`): behy na hotovú úlohu, studené štarty (spustené sessions) na úlohu, medián času úlohy, podiel nečinnosti a cenu na úlohu.

**Príprava (rovnaké pre obe ramená, aby sa menil len režim):**
- Hub s líniami (PR martin-janci/claude-fleet#1025). Nastavenia: `orchestrator.headless_checks` zapnuté (predvolené), `orchestrator.run_subagents` zapnuté (predvolené), `orchestrator.planner_reads` rovnako v oboch ramenách (odporúčam zapnuté).
- Ten istý host a login, rovnaká úroveň a grant (L2), rovnaké `max_parallel` (2) a `require_review`.

**Misie:** 2–3 páry porovnateľných misií v jednom repe (napr. claude-fleet), každá so 4–8 úlohami a podobnou veľkosťou úloh. Najľahšie je importovať plán so stĺpcom `Lane` a každý pár rozdeliť: jedna kópia s „Tasks run: Each in its own session“, druhá s „By lane“. Ramená sa striedajú, aby žiadne nemalo vždy čerstvejší `main`.

**Čo sledovať počas behu (zlyhania, ktoré unit testy nepokryjú):**
- krok `refused` s textom „lane … lead is not at its prompt yet“, ktorý sa opakuje dlhšie než pár minút: vedúci nedostáva úlohy;
- úloha, ktorá ostane `running`, hoci vedúci skončil (chýba Stop hook alebo marker);
- štafeta, ktorá sa točí (nový vedúci pri každej úlohe);
- po review ostane worktree (`git worktree list` v checkoute línie);
- evidencia „the task began on …, which the branch no longer holds“ (vedúci prepísal históriu).

**Rozhodnutie** (medián „By lane“ voči „Each in its own session“):
- **Prepnúť predvolený režim na `lane` pre misie s jedným repom**, ak naraz platí: cena na úlohu ≤ 0,8×, čas úlohy ≤ 1,0×, behy na úlohu nie sú vyššie a v ramene `lane` sa neobjavilo žiadne zo zlyhaní vyššie.
- **Ponechať `item` ako predvolený a `lane` odporúčať po misiách**, ak je `lane` lacnejší, ale pomalší alebo sa objavilo zlyhanie, ktoré má známu opravu.
- **Ponechať `item`**, ak `lane` nie je lacnejší.

Výsledky (tabuľku z Compare a čo sa pokazilo) mi stačí poslať; zmena predvoleného režimu (`MissionPolicy::default().execution`) a prípadné opravy pôjdu samostatným PR.

## 8. Riziká

| Riziko | Zmiernenie |
|---|---|
| vedúci ignoruje „jedna úloha naraz" a urobí aj ďalšie | evidencia je rozsah `start_head..end_head`, takže práca navyše sa pripíše úlohe, kde vznikla; review ju vidí. Ďalšia úloha bez commitov je `missing` → `ask` |
| kompakcia v strede veľkej úlohy | štafeta medzi úlohami (§3.5); veľké úlohy má plánovač rozdeliť (existujúce pravidlo veľkosti) |
| jedna línia = jeden bod zlyhania | štafeta pri páde; ostatné línie bežia ďalej |
| menej paralelizmu | paralelizmus sa presúva na línie a headless checks; pre nezávislé úlohy ostáva režim `item` |
| prepísaná história (`rebase`, `amend`) | detekcia cez predka `start_head` → `ask` (§3.3) |
| cena jednej session cez viac úloh sa ťažko rozpočíta | cena úlohy = rozdiel kumulatívnej ceny session pri doručení a pri hlásení (otázka D6) |

## 9. Rozhodnutia

Vlastník prijal všetky odporúčania 2026-10-24.

| # | Otázka | Odporúčanie |
|---|---|---|
| D1 | Je jednotkou vedúcej session línia (viac úloh) alebo celá misia? | **Línia.** Celá misia v jednej session stratí paralelizmus medzi repami a kontext nestačí. |
| D2 | Línie ako tabuľka, alebo odvodené z assignee? | **Tabuľka `mission_lanes`.** Línia nesie vetvu, worktree a session, teda trvalý stav, ktorý sa nedá odvodiť. |
| D3 | Predvolený režim pre nové misie | **`item`, kým L6 neukáže čísla.** Potom `lane` pre misie s jedným repom. |
| D4 | Retry v tej istej session alebo nová? | **Prvý retry v tej istej**, ďalší cez štafetu. |
| D5 | Štafeta: po koľkých úlohách? | **4**, a skôr pri prahu kontextu. Nastaviteľné na misii. |
| D6 | Cena úlohy v zdieľanej session | **Rozdiel kumulatívnej ceny session** medzi doručením a hlásením. |
| D7 | Testy po každej úlohe alebo na konci línie? | **Po každej** (`lane_test_at = item`), na konci len na žiadosť. |
| D8 | Smie plánovač čítať repo (L1)? | **Áno, read-only a za nastavením**, rovnaká sada ako headless review. |
| D9 | `touches` ako stĺpec alebo v poznámkach? | **Stĺpec `touches_json`**, lebo ho číta plánovanie, nie človek. |

## 10. Mimo rozsahu

- Paralelné zápisy viacerých subagentov do jedného worktree.
- Viac plánovačov na misiu (O-návrh §12).
- Automatický merge PR línie.
- Zdieľané línie naprieč misiami.

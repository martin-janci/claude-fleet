# Zlepšenia misií po líniách: cena, čakanie, závislosti, PR, testy, build, subagenti

Stav: návrh, 2026-10-26. Nadväzuje na `2026-10-23-mission-lanes-design.md` (L0–L6 postavené) a `2026-10-07-autonomous-orchestration-projects-design.md` (O0–O8).

## 0. Zhrnutie

Línie sú postavené a merané (Missions → Compare). Z návrhu línií ostali nepostavené body a pri ich overovaní v kóde sa ukázali dve chyby, ktoré skresľujú meranie alebo spomaľujú misie:

1. **Cena úlohy v línii je nadhodnotená** (M1). Úloha počíta celú cenu každej session, s ktorou súvisí; vedúci línie súvisí so všetkými jej úlohami.
2. **Závislá úloha nevidí prácu svojej závislosti** (M2), a to **v oboch režimoch**. Beh štartuje z default vetvy (alebo z vetvy vlastného skoršieho pokusu, P2), nie z vetvy úlohy, od ktorej závisí. Kým človek nemergne PR závislosti, závislá úloha prácu robí bez nej, a konflikty potom končia integračnými úlohami.

Ďalej návrh dopĺňa príčiny čakania do metrík (M3), PR na líniu ako pravidlo Fleetu (M4), testy na konci línie (M5), teplý build (M6) a subagentov na timeline (M7).

## 1. Čo vieme z kódu

| Tvrdenie | Kde |
|---|---|
| `WorkTask::cost_micros` je súčet `usage_cost_micros` každej odlišnej session na aktívnom alebo ukončenom linku úlohy; „session, ktorá pracovala na dvoch úlohách, sa počíta v oboch“ | `service/work/view.rs` (pole `cost_micros`, výpočet `spent`) |
| Cena misie počíta každú session raz (`s.id IN (...)`) plus `aux_usage` plánovača | `store/mission_loop.rs::mission_cost_micros` |
| `run_cost_estimate` delí cenu worker sessions počtom behov; sám píše, že session môže niesť viac behov | `store/mission_loop.rs` |
| Beh dostane `base_branch` len z `prior_work`, teda z vlastného implement pokusu tej istej položky | `service/work/run.rs` (`StartArgs { base_branch: prior… }`) |
| Vedúci línie štartuje s `base_branch: None`; doručenie úlohy do živého vedúceho nič nemerguje | `orchestrate/lanes.rs::start_lead`, `deliver` |
| Integrácia porovnáva hotové vetvy až pred uzavretím misie (`merge-tree`) a konflikty jedného repa dá do jednej karty | `orchestrate/integrate.rs` |
| Mission eventy majú strop 5000 na misiu a 4096 B na payload; `limit_wait` už existuje | `store/orchestration.rs`, `orchestrate/limit.rs` |
| Fleet inštaluje hooky `Stop`, `StopFailure`, `Notification`, `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`, … ale nie `SubagentStart`/`SubagentStop`; handler ich ignoruje | `service/hooks_install.rs`, `service/hooks.rs::apply` |
| Grant nemá nič o pushi; guard odmieta len push na `main`/`master`, `gh pr merge`/`ready` a tracker CLI | `store/mission_loop.rs::GrantRow`, `orchestrate/guard.rs` |
| Session PR Fleet zisťuje sám (`prs.rs`) | `service/prs.rs` |

## 2. M1: cena úlohy v zdieľanej session (D6)

**Model.** `tasks.cost_start_micros` (nová migrácia): `usage_cost_micros` session v okamihu, keď úlohu dostala. Cena úlohy v session = `cost_start` nasledujúcej úlohy v tej istej session − vlastný `cost_start`; posledná úloha session = aktuálna cena session − vlastný `cost_start`. Tak sa cena session rozdelí na úlohy bez zvyšku, a oneskorené dočítanie usage (polling) pripadne úlohe, počas ktorej vzniklo, nie tej, ktorá práve hlási.

- Zapisuje sa pri každom vzniku `tasks` riadku s `worker_session_id` (štart behu: 0; `deliver` do živého vedúceho: jeho cena v tej chvíli).
- Starší riadok bez hodnoty: ak je session jedinou úlohou, celá cena session; inak rovnaký podiel (označený ako odhad).
- **Kde sa to prejaví:** `WorkTask::cost_micros` pre úlohy misie (súčet ich úloh namiesto celých sessions), stĺpec „Cost / task“ v Compare zostáva cena misie / hotové úlohy (už správne), riadok úlohy v miestnosti misie dostane vlastnú cenu.
- Úloha mimo misie (človekova session na dvoch úlohách) sa nemení: pravidlo „počíta sa v oboch“ ostáva tam, kde Fleet nevie, ktorá práca patrila ktorej úlohe.

**Veľkosť** S–M. **Test:** dve úlohy v jednom vedúcom, cena session 10 → 30 → 45; úlohy 30 a 15 (nie 45 a 45; brief vedúceho patrí prvej úlohe).

**Ako je to postavené v M1:**
- Migrácia 213: `tasks.cost_start_micros` a partial index `idx_tasks_cost_start` (len riadky s hodnotou). Úlohy línie spred migrácie (`start_head IS NOT NULL`) dostanú `-1` a ich session sa delí rovným dielom.
- Hodnotu zapisuje len `lanes::begin_task` (prvá úloha nového vedúceho aj doručenie do živého), nie `insert_task`: viac úloh rôznych položiek nesie iba vedúci línie, takže beh vo vlastnej session ostáva `NULL` a počíta celú session ako doteraz. Work view tak číta len úlohy línií cez partial index, bez full scanu (`scale_tests`).
- `store/task_cost.rs`: `split_session_spend` (čistá funkcia; časti dávajú súčet ceny session, pokles ceny je nula) a `shared_session_spend`. `Graph::shared_spend` vo Work view nahradí celú cenu vedúceho podielom položky.
- **Odchýlka od návrhu:** `run_cost_estimate` sa nemení. Už počíta každú session raz (`IN`) a delí počtom hotových behov, takže zdieľaná session v ňom nie je dvakrát. Presun session (`repoint_open_tasks`) `cost_start` neprepisuje; je to zriedkavá cesta a úloha v nej dostane nanajvýš nulu namiesto podielu.

## 3. M3: prečo úloha čaká

`stats.idle_secs` hovorí, koľko času misia nemala otvorený beh, nie prečo. Loop to pritom vie v každom ticku: pripravená úloha nedostala beh kvôli slotu, limitu, grantu, `touches`, obsadenej línii, otvorenej karte alebo brzde.

**Model.** Tabuľka `mission_waits (mission_id, cause, secs, PRIMARY KEY (mission_id, cause))`. Na konci ticku loop pre každú príčinu, ktorá držala aspoň jednu pripravenú úlohu, pripočíta čas od predošlého ticku (orezaný na 2× interval, aby uspaný hub nenafúkol čísla). Je to jeden `UPSERT` na príčinu a tick, bez eventov, teda strop 5000 eventov sa nemíňa.

| `cause` | Kedy |
|---|---|
| `slot` | `max_parallel` grantu alebo misie je plný |
| `limit` | login čaká na usage limit (`limit.rs`) |
| `grant` | žiadny živý grant, alebo host mimo grantu |
| `person` | otvorená karta, `ask`, misia pozastavená človekom, úloha `held` |
| `touches` | `gate_items` / `gate_lane_starts` (L5) |
| `lane` | línia je obsadená (`busy_lanes`: beh, test, vedúci nie pri prompte) |
| `brake` | brzda bez progresu alebo budget |

Jedna úloha sa pripíše prvej príčine v tomto poradí (príčina bližšie k človeku vyhráva), misia pripočíta čas každej príčine, ktorá v ticku niečo držala. Súčet teda môže byť väčší než `span_secs`; UI ukazuje podiel z `span_secs` po príčinách, nie koláč.

**Kde sa to prejaví:** `stats.waits` (`{cause: secs}`), riadok štatistík v miestnosti misie („waited: person 40 %, slot 12 %“), dva stĺpce v Compare (najväčšia príčina a jej podiel) a medián po režimoch.

**Veľkosť** M.

**Ako je to postavené v M3:**
- Migrácia 214: `mission_waits (orchestration_project_id, cause, secs, open)` a `orchestration_projects.waits_at`. `Store::account_mission_waits(id, now, causes)` pripočíta čas od poslednej pozície príčinám, ktoré boli vtedy otvorené (najviac `WAIT_STEP_MAX_SECS`, 2 h), a otvorí tie, ktoré vidí teraz. Príčina sa teda počíta od chvíle, keď ju loop videl, nie spätne. `mission_waits(id, now)` k otvoreným pridá čas do `now`.
- Tick (`orchestrate::steps_and_waits`, `orchestrate/waits.rs`): `slot` z porovnania plánu bez stropu behov s plánom pod stropom (minuté pokusy misie sú `person`, lebo ich rieši ask), `lane` z `lanes::gate`, `touches` z `gate_items` a `gate_lane_starts`, `person` z askov a `held` úloh, `limit` z `hold_runs_over_limit` a z plánovača čakajúceho na reset, `grant` keď sú pripravené behy a autonómia pod `AUTO_LEVEL`.
- Pauza: aktívne misie sa tickujú, pozastavené nie. Preto `set_mission_state` účtuje pri každej zmene stavu: pauza od loopu (brzda) otvorí `brake`, iná pauza `person`, každý iný stav príčiny zavrie.
- `stats.waits` v `work { mission }`, riadok štatistík („waited on a person 50%, for a slot 17%“, najviac tri príčiny s aspoň 1 % rozpätia) a stĺpec „Waited most“ v Compare (pri misii jej najväčšia príčina, v súhrne príčina s najväčším mediánom podielu).
- **Odchýlka od návrhu:** jedna úloha sa nepripisuje len prvej príčine. Misia pripočíta čas každej príčine, ktorá v ticku niečo držala, takže poradie v `WAIT_CAUSES` je len poradie zobrazenia.

## 4. M2: závislá úloha začína z práce svojej závislosti

**Pravidlo** (oba režimy). Keď sa spúšťa implement úloha X v repe R a X závisí (priamo) od hotových úloh D₁…Dₙ v tom istom repe, ktorých vetva ešte nie je v default vetve:

- **jedna takáto závislosť:** checkout X začne z vetvy D (`StartArgs::base_branch`, rovnaká cesta ako P2; ak X má vlastný skorší pokus, prednosť má ten, lebo už obsahuje D),
- **viac závislostí:** checkout začne z prvej (poradie grafu) a pred briefom sa v ňom spraví `git merge --no-edit` ostatných. Konflikt → `git merge --abort`, beh sa nespustí a loop vytvorí integračnú kartu pre tieto vetvy (`integrate.rs`, rovnaká karta ako pred uzavretím misie), X čaká na ňu,
- **v režime `lane`:** pred doručením úlohy do vedúceho línie B sa vo worktree línie spraví `git merge --no-edit` vetiev línií závislostí (vedúci je pri prompte a checkout čistý, inak sa doručenie odloží o tick). Konflikt → `--abort` a rovnaká integračná karta. Vetva línie, ktorú už obsahuje, sa preskočí (`merge-base --is-ancestor`),
- **„nie je v default vetve“:** `git merge-base --is-ancestor <head D> origin/<default>` po `git fetch` (to už robí evidencia). Mergnutá závislosť sa nepridáva.

**Prompt** povie, z čoho checkout začal („obsahuje prácu úloh … z vetiev …, PR týchto úloh ešte nemusia byť mergnuté“), aby worker nerobil prácu znova a aby PR uviedol, na čom stojí.

**Evidencia** sa nemení: `start_head` sa zapíše až po merge, takže evidencia X sú len jej commity.

**Dôsledok pre PR:** PR úlohy X obsahuje commity D, kým sa D nemergne (stacked PR). Je to správne: dnes PR X neobsahuje D, ale kód X na D závisí, a teda sa nedá mergnuť samostatne. Postup mergovania (najprv D) uvádza prompt a telo PR.

**Veľkosť** M. **Test:** e2e scenár v `hub-e2e.sh`: A → B v režime `item`, B vidí súbor z A; a dve línie, kde úloha línie B závisí od úlohy línie A.

**Ako je to postavené v M2** (`service/work/dep_work.rs`):
- `unmerged_deps`: priame závislosti úlohy, ktoré sú hotové, v tom istom repe (úloha bez projektu sa berie ako v ľubovoľnom), majú vetvu z implement behu (`run::prior_work`) a ich PR nie je mergnutý (`Store::branch_pull_request_merged`, platí aj pre squash merge, kde git spoločný commit nevidí). Najviac 8.
- `merge_script` v checkoute úlohy: `git fetch`, potom pre každú vetvu (lokálnu, inak `origin/`) `had` (checkout ju obsahuje), `landed` (default vetva ju obsahuje), `took` (`git merge --no-edit`, s identitou repa, inak fleet), `conflict` (`git merge --abort`) alebo `missing`, a nakoniec `HEAD`.
- Režim `item` (`run::run_item_with`): len prvá implementácia (bez vlastného skoršieho pokusu, ten už závislosti obsahuje). Checkout začne z vetvy prvej závislosti (`base_branch`, na jej hoste, ak je hore a nikto host nezvolil), ostatné sa mergnú po štarte a pred promptom.
- Režim `lane` (`lanes::begin_task`): pred každou úlohou sa do checkoutu línie mergnú závislosti z iných línií; závislosti z tej istej línie sa preskočia.
- `start_head` sa zapíše až po merge, takže evidencia aj review sú len vlastné commity úlohy (v režime `item` je to nové: úloha so závislosťami dostane `start_head` a review nad snapshotom ako úloha línie).
- Prompt (`dep_work::note`) povie, čo checkout obsahuje a že PR stojí na PR závislostí; vetvu, ktorá sa nemergla čisto alebo chýba, má worker zlúčiť sám ako prvý krok.
- **Odchýlka od E4:** konflikt pri merge závislostí nevytvára integračnú kartu a úloha nečaká. Merge prebieha až v checkoute novej session a integračná úloha by bola ďalšia session (studený štart navyše), hoci worker úlohy má kontext na vyriešenie konfliktu. Konflikt medzi hotovými vetvami misie aj tak nájde integračná kontrola (`integrate.rs`) pred uzavretím misie.
- e2e: `hub-e2e.sh` §12 (dve úlohy v režime `item`, druhá závisí od prvej). Lane variant pokrývajú unit testy, e2e nie.

## 5. M7: subagenti na timeline

- `hooks_install` pridá `SubagentStart` a `SubagentStop` (HTTP hook ako ostatné). Hosty ich dostanú pri re-provisioningu (`provision_stale`), rovnako ako guard v L-fázach.
- Handler zapíše riadok timeline session (`subagent_started` s `agent_type`, `subagent_stopped`), nič iné. Na nich nič nezávisí.
- Úloha misie zráta subagentov v okne svojho behu (od vzniku `tasks` riadku po jeho koniec) a riadok úlohy ukáže „subagents: 3 (Explore 2, general 1)“. Compare dostane „subagents / task“.
- Platí pre všetky sessions, nielen misie; je to len timeline.

**Veľkosť** S.

**Ako je to postavené v M7:**
- `hooks_install::FLEET_HOOK_EVENTS` má `SubagentStart` a `SubagentStop` (HTTP, bez matchera). Mení to odtlačok provisioningu, takže každý host je `provision_stale` a hooky dostane pri re-provisioningu.
- `hooks::apply_subagent_hook`: na aktuálnej konverzácii zapíše tichú udalosť timeline (`insert_session_event_quietly`, bez živého rámca klientom) `subagent_started` / `subagent_stopped` s `agent_type` ako detailom, ak je to meno (písmená, číslice, `-_:.`, najviac 64 znakov).
- `Store::task_subagents(task, now)`: `subagent_started` udalosti session workera od štartu úlohy po jej koniec, podľa typu (bez typu `other`). Číta ich len detail misie: `graph.nodes[].attempt.subagents` a `stats.subagents` (súčet cez pokusy misie), nie tick.
- Riadok úlohy ich ukazuje v riadku pokusu („3 subagents (Explore 2, general-purpose 1)“), Compare má stĺpec „Subagents / task“.
- Vedúci línie robí viac úloh v jednej session; okno úlohy (od jej štartu po koniec) rozdelí subagentov medzi úlohy.

## 6. M5: testy na konci línie

`policy.lane_test_at: item | lane` (predvolene `item`, D7 sa nemení).

- `lane`: `awaiting_test` línie nedrží, úlohy po review čakajú vo `verifying` s neprejdeným `test` riadkom. Keď sú všetky implement úlohy línie hotové (alebo línia nemá ďalšiu pripravenú úlohu), loop spustí test raz v checkoute línie na `HEAD` (headless, ako dnes). Testovacie riadky sa zjednotia (každý príkaz raz).
- Úspech zavrie test riadky všetkých čakajúcich úloh.
- Neúspech: vedúci línie dostane opravnú úlohu („test zlyhal na konci línie“, výstup testu) ako nový pokus o **poslednú** úlohu línie; po oprave sa test spustí znova. Druhé zlyhanie toho istého testu → `ask`, ako dnes.
- Mení sa len na misii, ktorá nebeží (rovnako ako `execution`); v UI vedľa „Tasks run“.

**Veľkosť** M.

**Ako je to postavené v M5:**
- `policy.lane_test_at` (`item` | `lane`, predvolene `item`), mení sa len na misii, ktorá nebeží a nemá otvorené behy (rovnako ako `execution`). V úprave misie „Tests run“, keď sa úlohy robia po líniách.
- `lane`: `steps_and_waits` neposiela `busy_lanes` úlohy čakajúce na test, takže línia ide ďalej; `lanes::gate_lane_tests` nechá z testových krokov línie len posledný v poradí grafu, a len keď sú všetky úlohy línie `done` alebo `verifying` a línia nie je obsadená. Test beží headless v checkoute línie na jej `HEAD` ako doteraz.
- **Odchýlka od návrhu (bez kópií a zlučovania príkazov):** úspešný beh sa nekopíruje do ostatných úloh. `verify::check_line` pri riadku `test:` úlohy v takej línii počíta aj úspešné testové behy ostatných úloh línie, ktoré hlásili spustenie toho príkazu (a staršie než posledná implementácia úlohy sa nerátajú). Úloha s iným príkazom dostane svoj vlastný beh, jeden po druhom. Neúspech ostáva úlohe, ktorej test bežal, takže retry (s výstupom testu) ide len jej, cez vedúceho línie; druhé rovnaké zlyhanie je `ask` ako doteraz. Počty behov v štatistikách sa tak nenafukujú.

## 7. M4: PR na líniu

`policy.lane_pr: off | draft` (predvolene `off`, lebo push a PR sú navonok).

- `draft`: keď je prvá úloha línie overená, Fleet v checkoute línie na hoste spustí `git push -u origin <vetva línie>` a `gh pr create --draft --title "<misia>: <línia>" --body <úlohy línie a ich done_when>`. URL zapíše do `mission_lanes.pr_url` (migrácia). Ďalšie úlohy PR dopĺňajú pushom po overení, telo sa aktualizuje (`gh pr edit --body`).
- Vetva na default vetvu a `gh pr ready`/`merge` ostávajú zakázané (guard), merge je vždy človeka.
- Bez `gh` alebo bez prihlásenia na hoste: event `note` do logu misie a ostáva dnešný pokyn v briefe vedúceho. PR, ktorý otvoril vedúci sám, Fleet zistí cez `prs.rs` a zapíše ho do `pr_url` tiež.
- Riadok línie v miestnosti misie a v Graph view ukáže odkaz na PR a stav CI (už ho číta `prs.rs`).

**Veľkosť** M.

## 8. M6: teplý build (experiment)

Línia už build ohrieva sama (jeden worktree na líniu, štafeta ho nemení). Najviac studených buildov má režim `item` (checkout na beh).

- Nastavenie `orchestrator.build_cache` (predvolene vypnuté). Zapnuté: session misie dostane `RUSTC_WRAPPER=sccache` (a `SCCACHE_DIR` na hoste), ak sonda hosta našla `sccache`. Inak nič. Spoločný `CARGO_TARGET_DIR` sa nepoužije, lebo paralelné behy by sa serializovali na cargo locku.
- Ide cez existujúci session env (`2026-10-11-session-env-and-secrets-design.md`), nie cez prompt.
- Meranie: Compare (čas úlohy, `item` so zapnutým a vypnutým), rovnaký postup ako §7.1 návrhu línií. Rozhodnutie o predvolenom zapnutí až po čísle.

**Veľkosť** S–M.

## 9. Roadmap

| Fáza | Obsah | Správanie | Veľkosť |
|---|---|---|---|
| **M1** (postavené) | `tasks.cost_start_micros`, cena úlohy z rozdielov | oprava čísel | S–M |
| **M3** (postavené) | `mission_waits`, príčiny v ticku, `stats.waits`, riadok štatistík, Compare | len meranie | M |
| **M2** (postavené) | závislá úloha z vetiev závislostí (`item`), merge závislých línií (`lane`), konflikt vyrieši worker (odchýlka od E4), prompt, e2e | mení, odkiaľ beh začína | M |
| **M7** (postavené) | `SubagentStart`/`Stop` hooky, timeline, počet na úlohe a v Compare | len zobrazenie | S |
| **M5** (postavené) | `policy.lane_test_at = lane` | opt-in na misii | M |
| **M4** | `policy.lane_pr = draft`, `mission_lanes.pr_url` | opt-in na misii | M |
| **M6** | `orchestrator.build_cache` (sccache) | opt-in nastavenie | S–M |

M1 a M3 pred meraním L6, aby Compare ukazoval správnu cenu a dôvod čakania. M2 hneď potom, lebo najviac mení progres (menej práce naviac a integračných úloh). Ostatné nezávisle.

## 10. Rozhodnutia

| # | Otázka | Odporúčanie |
|---|---|---|
| E1 | Cena úlohy: rozdiel pri doručení nasledujúcej úlohy, alebo pri hlásení? | **Pri doručení nasledujúcej** (a cena session pre poslednú). Usage sa dočítava oneskorene, hlásenie by časť ceny posunulo ďalšej úlohe. |
| E2 | Príčiny čakania: súčty na misiu, alebo na úlohu? | **Na misiu** (jedna tabuľka, 7 riadkov). Na úlohu by to bol riadok na úlohu a tick. |
| E3 | M2 platí aj v režime `item`? | **Áno.** Chyba je v oboch režimoch a v `item` je častejšia. |
| E4 | Konflikt pri merge závislostí: `ask`, alebo integračná karta? | **Integračná karta** (existujúci mechanizmus, človek ju schváli), závislá úloha na ňu čaká. |
| E5 | PR na líniu otvára Fleet, alebo len zapíše PR vedúceho? | **Fleet, za `policy.lane_pr = draft`** (predvolene vypnuté); PR vedúceho sa zapíše vždy. |
| E6 | Zlyhaný test na konci línie: komu? | **Vedúcemu línie** ako nový pokus o poslednú úlohu, s výstupom testu. |
| E7 | Build cache: sccache, alebo spoločný target dir? | **sccache, len keď je na hoste**; spoločný target dir serializuje paralelné behy. |
| E8 | Subagent hooky pre všetky sessions? | **Áno**, je to len timeline. |

## 11. Mimo rozsahu

- Automatický merge PR závislostí alebo línie (TS11).
- Rebase závislých vetiev po merge závislosti (worker to spraví sám, keď PR zastará).
- Predvolené prepnutie na `lane` (čaká na meranie podľa §7.1 návrhu línií).

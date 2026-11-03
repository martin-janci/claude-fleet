# Misie: rýchlejší progres s menej čakania na človeka

Stav: návrh, 2026-11-03. Nadväzuje na `2026-10-26-mission-improvements-design.md` (M1–M7 postavené, PR #1055) a návrh línií (L0–L6).

## 0. Zhrnutie

M3 ukazuje, prečo misia čaká. Najväčšie čakania sú na človeka (karty, grant, merge PR) a na to, čo misia opakuje, lebo sa nepoučila. N-séria ide po nich:

| Fáza | Čo | Prečo |
|---|---|---|
| **N2** | Vedúci línie na dialógu (povolenie, otázka) je čakanie na človeka, nie na líniu | M3 ho dnes počíta ako `lane`, hoci rieši ho človek |
| **N1** | Poučenia misie: čo skoršie behy zlyhali a čo review vrátilo, ako citované poznámky pre ďalšie behy a plánovač | menej opakovaných chýb a retry |
| **N4** | Grant jedným ťuknutím („Dokonči misiu“) s rozpočtom z odhadu | `grant` a `person` čakania |
| **N5** | Odhad dokončenia: zostávajúci čas a cena misie | rozhodnutie, či pustiť grant, a koľko |
| **N3** | Merge vlak: overené PR misie v poradí závislostí, jedna karta, Fleet merguje po schválení | PR čakajú na merge a závislé úlohy na stacked PR |
| **N6** | Integračná karta pri konflikte závislostí (E4), zistená pred štartom | odchýlka M2 od E4 |

## 1. Čo vieme z kódu

| Tvrdenie | Kde |
|---|---|
| Línia je `busy`, keď jej živý vedúci nie je pri prompte alebo je na dialógu (`on_a_dialog`); M3 to počíta ako `lane` | `orchestrate/lanes.rs::busy_lanes`, `orchestrate/mod.rs::steps_and_waits` |
| Retry nesie text chyby predošlého pokusu (`failure_text`) len pre tú istú úlohu; iné úlohy misie sa ho nedozvedia | `orchestrate/steps.rs` |
| Karta `create` vytvorí úlohy cez `propose_tree` s `done_when`, líniou a `touches`; `depends_on` ide cez strom | `orchestrate/mod.rs::apply_card` |
| Grant: úroveň, hodiny alebo `until_done` (len s rozpočtom), hosty, `max_parallel`, login | `orchestrate/mod.rs::grant`, `store/mission_loop.rs::GrantRow` |
| Odhad ceny behu (`run_cost_estimate`) existuje; času nie | `orchestrate/mod.rs::run_estimate` |
| Guard odmieta `gh pr merge` workerovi; Fleet sám nikdy nemerguje | `orchestrate/guard.rs`, O11 |
| Integračná kontrola (`merge-tree`) a karta jednej integrácie na repo existujú; karta sa tvorí až pred uzavretím misie | `orchestrate/integrate.rs` |

## 2. N2: vedúci na dialógu čaká na človeka

- `busy_lanes` vráti aj dôvod: `run` (otvorený beh alebo test), `prompt` (vedúci pracuje) alebo `dialog` (vedúci čaká na povolenie či odpoveď).
- `steps_and_waits`: línia držaná dialógom pridá `person`, ostatné `lane`.
- Riadok línie v miestnosti misie povie „lead waits for you“ s odkazom na session.

**Veľkosť** S.

**Ako je to postavené v N2:**
- `lanes::lead_on_dialog`: živý vedúci línie je na dialógu (`stuck_kind` alebo `claude_status = blocked`, rovnaká podmienka ako v `busy_lanes`).
- `steps_and_waits`: pre každý implement beh, ktorý línia zadržala, zistí, či jej vedúci je na dialógu: áno → `person`, inak `lane`. `busy_lanes` sa nemení (dôvod sa nevracia, odvodí sa pri počítaní príčin).
- `work { mission }`: línia nesie `lead_waits` (odvodené pri čítaní, neuložené); riadok bežiacej úlohy takej línie ukáže „lead waits for you“ vedľa tlačidla Session.

## 3. N1: poučenia misie

**Zdroj** (bez modelu, len zo store):
- zlyhaný implement pokus, po ktorom ďalší pokus tej istej úlohy prešiel: chyba (`failure_text`, skrátená) a súhrn úspešného pokusu,
- review s výsledkom iným než `done`: súhrn recenzenta,
- test, ktorý zlyhal: príkaz a súhrn.

**Model.** Tabuľka `mission_lessons (mission_id, item_id, kind, text, at)`, najviac 20 na misiu (najstaršie idú preč), text najviac 300 znakov, deduplikované podľa textu. Zapisuje sa v ticku, keď loop uzavrie úlohu (`close`) alebo spustí retry.

**Použitie:**
- prompt behu (`run_prompt`, `lanes::task_prompt`): sekcia „What earlier runs of this mission ran into (their words, quoted)“, najviac 5 najnovších, citované ako nedôveryhodný text,
- snapshot plánovača: rovnaká sekcia,
- miestnosť misie: zoznam poučení (len čítanie, človek môže jedno zmazať).

**Veľkosť** M.

**Ako je to postavené v N1** (`orchestrate/lessons.rs`):
- **Odchýlka od návrhu: bez tabuľky.** Poučenia sa pri každom použití odvodia z `tasks` členov misie (implement pokus, ktorý zlyhal alebo hlásil menej než `done`; review a test s iným výsledkom než `done`), text je `failure_text` (chyba, blokery, inak súhrn) v jednom riadku, najviac 300 znakov, rovnaké texty raz, najnovšie prvé. Odpadá zápis v ticku a migrácia; zmazanie jedného poučenia človekom sa nestavalo.
- Prompt implementácie (`run::run_item_with` aj `lanes::begin_task`) dostane 5 najnovších poučení iných úloh misie (`mission_lessons_note`) ako citovaný text; vlastné zlyhania úlohy nesie už jej retry.
- Snapshot plánovača má sekciu „What earlier runs ran into“ (10 najnovších, každé v `<untrusted>`).
- `work { mission }` vracia `lessons` (najviac 20, len úlohy, ktoré volajúci vidí); zoznam úloh misie ich ukáže v rozbaľovacom „What its runs ran into“.

## 4. N4: grant jedným ťuknutím

- `work_link { mission_grant, preset: "finish" }`: úroveň 2, `until_done`, hosty z placementu misie (hosty jej doterajších behov, inak host plánovača), rozpočet = zostávajúce úlohy × cena úlohy z N5 × 1,5 (najmenej $5), login z posledného grantu.
- Je to stále grant, ktorý podpisuje človek (`person_decides`), so všetkými brzdami.
- Tlačidlo „Finish it“ v miestnosti misie a na telefóne s textom, čo podpíše („level 2 until done, up to $12, on h1“).

**Veľkosť** S–M.

## 5. N5: odhad dokončenia

Čistá funkcia nad `stats` a grafom: zostávajúce úlohy (nie `done`), medián času úlohy (`item_secs_p50`), `max_parallel` a vlny grafu (kritická cesta) → čas; zostávajúce úlohy × cena na hotovú úlohu (M1) → cena. Bez hotovej úlohy sa použije priemer misií vo flotile (`run_cost_estimate(None)`) a odhad je označený ako hrubý.

`stats.forecast { secs, micros, basis }`, riadok v hlavičke miestnosti („about 2h 10m and $8 left“) a N4.

**Veľkosť** S.

## 6. N3: merge vlak

- Karta `merge_train`, keď má misia aspoň jeden overený PR (úloha `done`, jej PR otvorený, CI zelené alebo bez CI, nie draft): PR v poradí závislostí (najprv tie, od ktorých iné závisia), najviac 10.
- Kartu schvaľuje človek (ako každú kartu; `loop_applies_card` ju nikdy nepustí loopu, ani pri L3). Po schválení Fleet na hoste vedúceho alebo implementéra spustí pre každý PR `gh pr merge <url> --merge` (alebo metóda podľa repa, `gh repo view --json …`), pred každým overí, že CI je stále zelené a PR je mergeable; pri prvom probléme zastaví a zvyšok nechá v karte.
- Draft PR línie (M4) sa pred merge neoznačí za ready: draft v karte nie je.
- **Toto mení O11/TS11** („merge je vždy človeka“): merge zostáva rozhodnutím človeka, ale vykoná ho Fleet. Preto je za nastavením `orchestrator.merge_train` (predvolene vypnuté) a karta sa nedá schváliť operátorom ani agentom.

**Veľkosť** M–L.

## 7. N6: integračná karta pri konflikte závislostí (E4)

- Pred štartom prvej implementácie úlohy X s ≥ 2 nemergnutými závislosťami (M2) loop na hoste prvej závislosti spustí `merge-tree` nad ich hlavami (`integrate::merge_tree_script`).
- Konflikt: X nedostane beh (príčina čakania `person`), vznikne integračná karta pre tieto vetvy (`integrate::integration_card`) s `dependents: [X]`; jej aplikovanie pridá hranu X → integračná úloha, takže X začne z integrovanej vetvy (M2 ju berie ako závislosť).
- Bez odpovede (`unknown`, git < 2.38, iný host) ostáva M2: merge v checkoute a pokyn workerovi.

**Veľkosť** M.

## 8. Roadmap

| Fáza | Obsah | Správanie | Veľkosť |
|---|---|---|---|
| **N2** (postavené) | dôvod obsadenej línie, `person` pri dialógu | len meranie a zobrazenie | S |
| **N1** (postavené) | poučenia zo `tasks`, prompt a plánovač, zoznam v miestnosti | mení prompty | M |
| **N5** | `stats.forecast` | len zobrazenie | S |
| **N4** | `preset: finish`, tlačidlo | nový spôsob grantu | S–M |
| **N6** | `merge-tree` pred štartom, karta s `dependents` | mení, kedy závislá úloha štartuje | M |
| **N3** | karta `merge_train`, `orchestrator.merge_train` | opt-in, mení O11 | M–L |

N5 pred N4 (rozpočet z odhadu). N3 posledné, lebo mení pravidlo o merge.

## 9. Rozhodnutia

| # | Otázka | Odporúčanie |
|---|---|---|
| F1 | Poučenia: zo store, alebo zhrnuté modelom? | **Zo store**, citované; model by pridal cenu a riziko. |
| F2 | Koľko poučení do promptu? | **5 najnovších**, 300 znakov každé. |
| F3 | Grant „Finish it“: rozpočet? | **Odhad × 1,5, najmenej $5**, človek ho vidí pred podpisom. |
| F4 | Odhad času: z mediánu úlohy a kritickej cesty? | **Áno**, s označením „hrubý“ bez hotovej úlohy. |
| F5 | Merge vlak: smie Fleet mergovať po schválení karty? | **Áno, za nastavením (vypnuté)**, len karta schválená človekom, stop pri prvom probléme. |
| F6 | Merge metóda | **Podľa repa** (`gh repo view`: povolený squash/merge/rebase, prednosť merge commit). |
| F7 | N6: kam s úlohou pri konflikte? | **Čaká na integračnú kartu**, ktorá jej pridá závislosť na integračnú úlohu. |

## 10. Mimo rozsahu

- Automatický merge bez karty (aj s nastavením).
- Poučenia naprieč misiami.
- Prepnutie predvoleného režimu na `lane` (čaká na meranie).

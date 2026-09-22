# Iterácia 02 — Pomenovanie sessions a identita

**Šošovka:** pomenovanie sessions a identita · **Zasahuje:** UX-05, UX-06, UX-07, UX-15 ·
**Vstup:** `docs/ux/2026-09-21-audit/README.md`, screenshoty 01, 03 a 05, kód na
`c1d5f481`, lokálna `state.db` (read-only `SELECT`), skill
`~/.claude/skills/fleet-friendly-name/SKILL.md` · **Režim:** read-only review, žiadne
zmeny kódu.

## Zhrnutie

1. Meno session má **jeden stĺpec a štyroch zapisovateľov** (`sessions.friendly_name`,
   migrácia 016): default z vetvy pri vytvorení, odvodenie z promptu pri každom
   fleet-om poslanom prompte, skill cez MCP `set_friendly_name`, človek cez 🏷 /
   dvojklik. Nikde sa neukladá, **kto** meno nastavil. Jediná ochrana zvoleného mena je
   porovnanie s vetvovým defaultom (`prompt.rs:177-182`) — čo je zároveň príčina, prečo
   sa `yes` už nikdy neopraví: `yes` ≠ default, takže ďalší prompt ho nechá tak.
2. Všetky štyri nálezy sú **potvrdené**; UX-06 s korekciou: `bg:<uuid>` riadky **nie sú
   operátor**. Sú to `kind='external'` riadky — interaktívne Claude sessions, ktoré
   `claude agents --json` na hoste vidí, ale žiadna tmux session si ich nenárokovala
   (`reconcile.rs:1036-1064`). Operátor je bežná `work` session `fleet-operator` s menom
   „fleet operator“ (`operator.rs:125,322`) a na screenshote 03 na `claude-fleet-trn` ani
   nebeží (Agent panel na screenshote 01 to správne hlási).
3. UX-05 je širšie, než README uvádza: sessions pomenúvajú aj **systémové prompty** —
   safe-kill inštrukcia, hlavička `[msg #N from …]` pri `send_message`, seed review
   promptu a `broadcast_prompt` (N sessions dostane to isté meno). Naopak prompt napísaný
   **priamo v tmux paneli** session nikdy nepomenuje — hook `UserPromptSubmit` ho vidí,
   ale meno nenastavuje (`hooks.rs:596-618`).
4. Sedem nových nálezov UX-35…UX-41 (systémové prompty ako mená, závislosť od vstupnej
   cesty, chýbajúca proveniencia, junk default z názvu worktree, hub bez backfillu, tri
   rôzne fallback výrazy pre zobrazené meno, MCP popisy sľubujúce dnešné správanie).
5. Návrh: (a) filter promptu — slash príkazy, hlavičky, ≤ 2 slová a stop-list nikdy
   nepomenujú; (b) **iba prvý skutočný prompt konverzácie** pomenúva, cez nový stĺpec
   `friendly_name_source ∈ {default, prompt, chosen}`; (c) zvolené meno sa **nikdy**
   neprepíše; (d) UI odlíši auto meno tlmenou farbou + tooltipom; (e) `external` riadky
   dostanú default `<repo> · <id8>` namiesto `bg:<uuid>`; (f) systémové prompty idú
   explicitnou cestou bez pomenovania.
6. Backfill: migrácia 040 pridá stĺpec, jednorazový Rust prechod po `migrate()` (spoločný
   pre desktop aj hub) doplní zdroj a **zresetuje** mená, ktoré by dnešný filter odmietol.
7. Odhad: PR-A (Rust, pravidlá + proveniencia + backfill) **M**, PR-B (TS, zobrazenie) **S**.
   PR-A mení tri MCP popisy → `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
   a nový wire field → `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`.

## Ako vzniká meno dnes

### Zapisovatelia v poradí, v akom sa dostanú k slovu

| # | Kedy | Kód | Čo zapíše | Podmienka |
|---|---|---|---|---|
| 1a | `new_session` (dialóg, MCP, operátor) | `crates/fleet-core/src/service/sessions/lifecycle.rs:646-655` → `derive_friendly_name` `:703-734` | explicitná hodnota (`args.friendly_name`, orezaná), inak `humanize_branch(new_worktree ∣ worktree.branch ∣ worktree.name ∣ tmux_name)` (`humanize.rs:24-42`) — „Fix login“, „Main“, „Qqq“ | vždy pri vytvorení; validácia `validate.rs:281-286` (≤ 80 znakov, bez control chars) |
| 1b | štart desktopu | `src-tauri/src/lib.rs:158-164` → `Store::backfill_friendly_names` `store/sessions.rs:617-655` | ten istý humanizovaný default pre riadky s `friendly_name IS NULL` | preskočí `kind IN ('bg','external')` (`:626-627`); **hub ho nevolá** (grep `backfill` v `crates/fleet-hub/src` = 0) |
| 1c | reconcile pane-less agentov | `service/sessions/reconcile.rs:1036-1064` → `agent_row_name` `:931-935` → `Store::upsert_bg_session` `store/sessions.rs:101-141` | **nič** — `tmux_name = "bg:<claude_session_id>"`, `friendly_name` sa nedotkne; z `ClaudeAgentRow` (`claude_agents.rs:37-61`) sa použije iba `cwd` → `project_id` (`:1044-1046`), `name` a `started_at` sa zahodia | `default_friendly_name` pre `has_no_pane` vracia `None` (`store/sessions.rs:688-690`, `rows.rs:98-100`) |
| 2a | každý prompt poslaný **cez fleet** | `service/sessions/prompt.rs:91-103` `send_prompt_inner` → `record_prompt_outcome` `:133-195` → `friendly_name_from_prompt` `:110-127` | prvých 5 alfanumerických slov, lowercase, ≤ 80 znakov | `replaceable` = `friendly_name IS NULL` **alebo** `== default_friendly_name(id)` (`:177-182`) |
| 2b | `new_bg_session` | `service/bg_sessions.rs:254-281` `stamp_bg_row` (`:265-269`) | tých istých 5 slov zo štartovacieho promptu | iba ak `friendly_name.is_none()` — u `bg` riadku vždy |
| 3 | skill `fleet-friendly-name` | MCP `set_friendly_name` `mcp/tools/lifecycle.rs:125-156` → `set_session_friendly_name` `lifecycle.rs:1019-1046` → `Store::set_friendly_name` `store/sessions.rs:699-718` | 3–6 slov podľa skillu (SKILL.md `:55-60`); prázdny reťazec = `NULL` (`:84-87`) | fire iba na deterministické signály: prvý prompt, prvý prompt po `/clear`, heartbeat ~10 promptov, explicitná žiadosť (SKILL.md `:15-31`) |
| 4 | človek | `src/lib/session_rename.ts:24-50` `applySessionRename('label')` → `setFriendlyName` (`sessions.ts:282-293`) → ten istý Tauri príkaz `set_session_friendly_name` | ľubovoľný text, prázdny = zmazať | 🏷 (`SessionRowItem.svelte:322`), dvojklik na riadok / na `h2` v detaile (`SessionDetails.svelte:420-424`) |

Kto všetko volá cestu 2a (a teda pomenúva):

| Volajúci | Kód | Text, ktorý sa stane menom |
|---|---|---|
| Composer, Enter | `src/lib/ConversationPanel.svelte:876-` `send()` → `sendPrompt` | čo napíše človek |
| Quick-action chip, Shift+klik | `ConversationPanel.svelte:858-867` `usePreset(p, sendNow)` → `sendText`, `:1665` `onclick={(e) => usePreset(p, e.shiftKey)}`; presety `composer_presets.ts:14-20` | `/clear` → **„clear“**, `/compact` → „compact“, `/status` → „status“, „Continue where you left off.“ → „continue where you left off“ |
| MCP `send_prompt`, `run_prompt` | `mcp/tools/messaging.rs:8-19`, `mcp/tools/support.rs:854` | prompt bez untrusted markera (`prompt.rs:143`) |
| `broadcast_prompt` | `prompt.rs:342-378` (`:378` volá `send_prompt_inner` pre každý riadok) | ten istý text pre **každú** cieľovú session |
| `send_message` s `deliver` | `service/messages.rs:175-186`, hlavička `pane_header` `:68-75` | `[msg #42 from dev-foo@hetzner]: hello there` → **„msg 42 from devfoohetzner hello“** |
| Safe remove | `service/safe_kill.rs:186-199`, text `build_safe_kill_prompt` `:108-` | „I want to safely remove this session…“ → **„i want to safely remove“** |
| `spawn_review` seed | `service/sessions/review.rs:102-110` `send_prompt_inner` | `DEFAULT_REVIEW_PROMPT` (`src/lib/sessions.ts:483`) → **„review the work in this“** |

Kto **nevolá** cestu 2a: prompt napísaný priamo v tmux paneli. Ten prejde iba hookom
`UserPromptSubmit` (`service/hooks.rs:596-618` `apply_prompt_submit_hook`), ktorý zapíše
`conversations.first_prompt` (`store/conversations.rs:383-396`, len keď je `NULL`) a
`working` stav — meno nikdy.

### Ako sa meno zobrazuje

| Plocha | Kód | Primárne | Sekundárne |
|---|---|---|---|
| Riadok v sidebare (živý) | `src/lib/SessionRowItem.svelte:92-101` `primaryName`/`secondaryName`, render `:274`, `:358` | `$showFriendlyNames && friendly_name ? friendly_name : tmux_name` | pod ním `tmux_name` (ak je friendly), inak `worktree_key` |
| Riadok „Outside fleet“ (readOnly) | `SessionRowItem.svelte:207` | to isté `primaryName` → u `external` vždy `tmux_name` = `bg:<uuid>` | žiadny badge druhu (bg má 🤖, review 🔍, shell ▶ — external nič) |
| Ghost riadok | `SessionRowItem.svelte:226-227` | tretí inline výraz toho istého pravidla | — |
| Hosts detail | `src/lib/HostDetail.svelte:94-96` `sessionName`, `:229-230` | `friendly_name?.trim() ∣∣ tmux_name` — **ignoruje** prepínač `friendly` | `sessionState` `:98-102`: stuck › ghost › `claude_status` › `status` (preto „running“ pri `bg:` riadku) |
| Session detail | `src/lib/SessionDetails.svelte:420-428` | `<h2>` = **`tmux_name`** | `<p class="friendly">` = friendly — hierarchia **opačná** než v sidebare |
| Hlavička terminálu | `src/lib/TerminalView.svelte:1100` | iba `tmux_name` | — |
| tmux status bar | tmux sám | `tmux_name` | — |
| Agent panel chip a prefix | `src/lib/agent_context.ts:21-34` | `friendly_name ∣∣ tmux_name` → chip „yes · claude-fleet-trn“, prefix `the person is looking at session "yes"` | `tmux_name` sa agentovi **nepovie** |
| Quick switcher | `src/lib/quick_switcher.ts:76` | `friendly_name ∣∣ tmux_name` | — |

Prepínač `showFriendlyNames` (`src/lib/sessions.ts:182-185`) je default `true`, takže auto
meno je to prvé, čo používateľ vidí.

### Kde sa to láme

1. **Prvý junk prompt vyhrá navždy.** `replaceable` porovnáva s *vetvovým* defaultom
   (`prompt.rs:180`). Po prvom prompte „yes“ je meno „yes“ ≠ „Main“, takže druhý prompt
   „Fix the login redirect loop“ ho už nenahradí. Dôkaz: hub (screenshot 03, 11 sessions
   na `claude-fleet-trn`): `yes` ×2 (`kuk-agent--main`, `claude-fleet--violet-mars`),
   `clear` ×2 (`claude-fleet--rustic-jupiter`, `pos-frontend--qqq`). Lokálna `state.db`
   (staršie, standalone riadky): `clear` ← last_prompt `/clear`, `yes` ← `yes`.
2. **Zvolené meno chráni iba nerovnosť s defaultom.** Dialógom vygenerované meno
   „indigo cosmos“ (`nameWords`, `names.ts:72-74`) prežilo prompt „push“ (screenshot 01)
   *iba preto*, že humanizér vracia „Indigo cosmos“ s veľkým I. „Ember pulsar“ (veľké E =
   presne default) by prompt „push“ premenoval. Človek, ktorý nastaví label rovný defaultu,
   ho stratí pri ďalšom prompte.
3. **Skill `/clear` nevidí.** `/clear` je príkaz harnessu; agent ho nedostane ako prompt,
   takže pravidlo 2 skillu („prvý prompt po `/clear`“) po chipe nevystrelí a „clear“
   ostane. Backend pritom prepnutie konverzácie **vie** (`hooks.rs:281`
   `StartSource::Clear`, `:657-665` `SessionEnd clear|resume`).
4. **Bez proveniencie nie je čo zobraziť.** Jedno pole → UI nevie odlíšiť „yes“
   (auto) od „design better chat UX“ (skill), takže obe vyzerajú ako vedomý label.
5. **Hub = ten istý kód, bez backfillu.** `fleet-hub` otvára store cez
   `crates/fleet-hub/src/serve.rs:36-40` bez `backfill_friendly_names`; telefón číta
   `friendly_name` doslovne (`store/rows.rs:151-155`, `hub_contract.golden.json:227`),
   takže každé pravidlo musí žiť v `service/`/`store/`, nie vo frontende.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-05 | **Potvrdené, rozšírené.** Nie je to iba chip: rovnakou cestou pomenúvajú `send_message` (`msg 42 from devfoohetzner hello`), safe-kill (`i want to safely remove`), review seed (`review the work in this`) a `broadcast_prompt` (N riadkov, jedno meno). Filter nemá žiadnu výnimku pre `/`-príkazy ani dĺžku (`prompt.rs:110-127`). Existujúci test `tests.rs:3332-3348` pinuje iba pozitívne prípady + prázdny/interpunkčný vstup. | screenshoty 01/03; lokálna DB: `clear`←`/clear`, `yes`←`yes`; riadky s menom od skillu („design better chat UX“, last_prompt „Go on“; „merge terminal and conversation tabs“, last_prompt „ok“) prežili — ochrana funguje, keď skill stihne byť prvý. |
| UX-06 | **Potvrdené s korekciou.** (a) `bg:<uuid>` riadky sú `kind='external'`: sekcia „Outside fleet“ filtruje presne `kind === 'external'` (`src/lib/sidebar_index.ts:53-61`, `Sidebar.svelte:340,755-772`). Vznikajú v `reconcile_agent_rows` z agentov, ktorých si nenárokovala žiadna tmux session (`unmatched_bg_agents` `reconcile.rs:903-928`; `AgentKind::Interactive → "external"` `:1040-1043`). (b) **Nie je to operátor:** operátor je `work` tmux session `fleet-operator` s explicitným menom „fleet operator“ (`operator.rs:125,316-322`); v zozname 11 sessions na trn (screenshot 03) taká nie je, a „working“ patrí riadku `yes` (kuk-agent). (c) Prečo bez mena: `upsert_bg_session` `friendly_name` nezapisuje, `backfill_friendly_names` a `default_friendly_name` pane-less riadky zámerne preskakujú („humanise poorly“, `store/sessions.rs:613-615`), a žiadna prompt cesta k `external` riadku nevedie. (d) „running“/„ghost“ je `status`, lebo `claude_status` je `NULL` (`HostDetail.svelte:98-102`). (e) Riziko duplikátu: `find_by_unique_cwd` vracia `None`, keď v jednom cwd bežia dva Claude (`claude_agents.rs:311-315`) a na vzdialenom hoste sa cwd nikdy nekanonizuje (`:315`) — fleet-ová tmux session sa potom môže objaviť **dvakrát**: ako svoj riadok aj ako `bg:<uuid>` v Outside fleet. Zo screenshotu to nedokážem; overenie nižšie v otázkach. | screenshot 03 (Outside fleet (2), Sessions 11: `bg:62e7… running`, `bg:8df9… ghost`); lokálna DB: 6 `external` riadkov `bg:…` na `local`, všetky bez mena a bez `last_prompt`. |
| UX-07 | **Potvrdené, rozšírené.** `dev-martin-janci-kuk-agent--main` je na screenshote 01 tri razy (riadok linka 2 `SessionRowItem.svelte:358`, hlavička `TerminalView.svelte:1100`, tmux bar). Navyše detail session má hierarchiu **naopak** (`h2` = tmux, friendly pod ním, `SessionDetails.svelte:420-428`), Hosts detail ignoruje prepínač `friendly` (`HostDetail.svelte:94-96`). Štyri plochy, tri rôzne poradia. | screenshot 01 |
| UX-15 | **Potvrdené.** `agent_context.ts:23` `friendly_name ∣∣ tmux_name` → chip „yes · claude-fleet-trn“ a prefix `looking at session "yes" on host claude-fleet-trn`; `tmux_name` (skutočná identita, ktorou agent volá tools) v prefixe nie je. Test `agent_context.test.ts:29` toto správanie pinuje („prefers the friendly name“). | screenshot 05 |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-35 | H | **Systémové prompty pomenúvajú sessions.** Safe remove, `send_message` hlavička, review seed a `broadcast_prompt` idú cez `send_prompt`/`send_prompt_inner`, ktorý bezpodmienečne volá `record_prompt_outcome`. Nepomenovaná (default) session po safe-kill sa volá „i want to safely remove“; príjemca správy „msg 42 from devfoohetzner hello“; každá review session „review the work in this“; broadcast dá N riadkom jedno meno. | `safe_kill.rs:186-199`, `messages.rs:68-75,175-186`, `review.rs:102-110`, `prompt.rs:378` |
| UX-36 | M | **Pomenovanie závisí od vstupnej cesty.** Prompt cez fleet (composer, MCP, telefón) pomenúva; prompt napísaný v tmux paneli nie. Hook `UserPromptSubmit` pritom dostane každý prompt a už ukladá `conversations.first_prompt` iba raz na konverzáciu — presne signál „prvý skutočný prompt“, ktorý meno potrebuje, nevyužitý. | `hooks.rs:596-618`, `store/conversations.rs:383-396` |
| UX-37 | M | **Žiadna proveniencia mena.** Jeden stĺpec, štyria zapisovatelia; ochrana = `friendly_name == default_friendly_name` (`prompt.rs:177-182`). Dôsledky: (a) label rovný defaultu sa prepíše; (b) generované meno z dialógu („indigo cosmos“) prežíva iba vďaka rozdielu vo veľkosti písmen oproti „Indigo cosmos“; (c) UI nevie označiť auto meno; (d) skill a prompt-derivácia sa nedajú rozlíšiť ani v `session_events` (žiadny `label_*` event — `lifecycle.rs` emituje iba `killed`, `recreated`). | screenshot 01 (`indigo cosmos` · `push`, `Ember pulsar`), `humanize.rs:40` `sentence_case` |
| UX-38 | M | **Vetvový default dedí junk názvy worktree.** Humanizér urobí z `dev-…--qqq` „Qqq“, z `lkkmkm` „Lkkmkm“; v lokálnej DB sú „Qqq“, „Lkkmkm“, „Lllihiuh“, „Sssdsd“, „Eeeee“, „Dasdasd“ a sidebar ich ukazuje ako vedomé labely. Kocka dialógu dáva čitateľné „misty saturn“ — problém je ručne napísaný názov bez validácie (mimo šošovky, ale default meno je jeho obeť). | lokálna `state.db`; screenshot 03 (`pos-frontend--qqq` sa volá „clear“, jeho default by bol „Qqq“) |
| UX-39 | L | **`fleet-hub` nikdy nespustí `backfill_friendly_names`.** Volá sa iba z `src-tauri/src/lib.rs:158-164`. Riadky, ktoré hub adoptuje z tmux cez reconcile (nie cez `new_session`), ostanú `NULL` → sidebar ukáže `tmux_name` → prvý prompt ich pomenuje cestou UX-05. | `grep backfill crates/fleet-hub/src` = 0; `serve.rs:36-40` |
| UX-40 | L | **Tri fallback výrazy pre zobrazené meno.** `$showFriendlyNames && friendly ? friendly : tmux` (`SessionRowItem.svelte:94,227`), `friendly?.trim() ∣∣ tmux` (`HostDetail.svelte:95`), `friendly ∣∣ tmux` (`agent_context.ts:23`, `quick_switcher.ts:76`). Hosts detail a switcher ignorujú prepínač. Patrí sem jedna funkcia `displayName(sess, showFriendly)` v `session_view.ts`. | uvedené riadky |
| UX-41 | L | **MCP popisy sľubujú dnešné správanie.** `send_prompt`: „The first prompt to a still-unnamed session also becomes its friendly name“ (`messaging.rs:12`, `docs/control-api-reference.md:356`); `new_bg_session`: „The prompt becomes the row's default friendly name“ (`session_ops.rs:350`, ref `:164`); `set_friendly_name` nehovorí, že label je chránený (`lifecycle.rs:125-129`, ref `:398`). Zmena pravidiel = zmena kontraktu → `REGEN_DOCS`; pozor na budgetový test popisov (jedna krátka klauzula, próza do ADR/skillu). | uvedené riadky |

## Pravidlá pomenovania

Návrh je formulovaný tak, aby žil celý v `crates/fleet-core` (`service/` + `store/`), lebo
ten istý kód beží v `fleet-hub` a telefón číta `friendly_name` doslovne. Frontend iba
zobrazuje.

### P1 — Proveniencia: `friendly_name_source`

Nový stĺpec `sessions.friendly_name_source TEXT` s hodnotami:

| Hodnota | Kto ju zapíše | Prepíše ju prompt? |
|---|---|---|
| `default` | `new_session` bez explicitného mena (`derive_friendly_name` z vetvy), `backfill_friendly_names`, `upsert_bg_session` pri **inserte** (P5), reset po `/clear` (P3) | **áno** |
| `prompt` | `record_prompt_outcome` / `stamp_bg_row`, keď prompt prejde filtrom P2 | **nie** (iba prvý skutočný prompt) |
| `chosen` | `set_session_friendly_name` (skill cez MCP, človek cez UI, telefón), explicitné `friendly_name` v `new_session` (dialóg s kockou, operátor „fleet operator“) | **nikdy** |
| `NULL` | legacy riadok pred migráciou | rieši backfill (nižšie) |

`replaceable` v `record_prompt_outcome` sa zmení z „`== default_friendly_name`“ na
„`friendly_name IS NULL` alebo `source == 'default'`“. Tým padá aj krehkosť UX-37(a,b):
„Ember pulsar“ z dialógu je `chosen`, aj keď sa zhoduje s humanizovaným defaultom.
Agentov vs. ľudský label sa **nerozlišuje** — pre ochranu ani pre UI to netreba, a
telefón/klient token by tak či tak spadli do jednej kategórie.

### P2 — Filter promptu: čo session nikdy nepomenuje

`friendly_name_from_prompt` → premenovať na `label_from_prompt(prompt) -> Option<String>`
(alebo ponechať názov a zmeniť telo; testy volajú funkciu priamo) s pravidlami v tomto
poradí:

1. **Marker a riadok.** `strip_marker` (už je), potom **prvý neprázdny riadok**, orezaný.
   Viacriadkový prompt sa hodnotí podľa prvého riadku (ten je aj v `promptPreview`,
   `attention.ts:335-340`).
2. **Príkazy a hlavičky nikdy.** Riadok začínajúci `/` (`/clear`, `/compact`, `/status`,
   `/resume`, `/model`), `!` (bash mód), `#` (memory), `[` (fleet hlavičky `[msg #…]`,
   `[context]`, `[claude-fleet: …]`), `<` (harness tagy) → `None`.
3. **Tokenizácia ako dnes** (alfanumerické znaky, lowercase), plus: ak žiadne slovo
   neobsahuje písmeno („1“, „2“, „👍“) → `None`.
4. **Stop-list na prvom slove.** Ak je prvé slovo v stop-liste → `None`. Nie „odstráň a
   pokračuj“ — „yes, and also push“ → „and also push“ je horšie než nič, a follow-up po
   prvom skutočnom prompte aj tak nepomenúva (P3). Stop-list (EN + SK, lowercase):
   `yes yeah yep y no nope n ok okay k kk sure go continue proceed next done push commit
   merge ship retry again stop wait thanks thank hi hello please fine good great correct
   right áno ano hej nie dobre pokračuj pokracuj ďakujem dakujem`. Pokrýva všetky štyri
   default chipy (`Continue where you left off.` → prvé slovo `continue`).
5. **Dĺžka.** Menej ako **3** slová → `None`; inak prvých 5 slov (ako dnes), ≤ 80 znakov.
   Skill radí 3–6, auto ostáva na 5 — kratšie než skill, aby bolo vidno, že je to náhrada.
6. **Veľkosť písmen sa nepoužíva ako signál.** Mobilná klávesnica automaticky kapitalizuje,
   slovenčina a angličtina sa miešajú; „fix the login redirect“ je rovnako dobrý prompt ako
   „Fix the login redirect“. (Odpoveď na otázku zo zadania: lowercase sloveso nie je
   dôvod odmietnuť.)
7. **Systémové prompty explicitne, nie heuristikou.** `send_prompt_inner` dostane parameter
   `label: bool`; verejné `send_prompt(SendPromptArgs)` posiela `true` (wire struct
   `SendPromptArgs` sa **nemení**, takže hub routing/`tests_routing.rs` ostávajú), nová
   `pub async fn send_system_prompt(...)` posiela `false` a použijú ju `safe_kill.rs:189`
   a `messages.rs:176`; `review.rs:102` a `broadcast_prompt` (`prompt.rs:378`) volajú
   `send_prompt_inner(..., false)`. Broadcast nikdy nepomenúva — jedno meno na N riadkov
   ničí skenovateľnosť sidebaru.

Príklady:

| Prompt | Dnes | Podľa P2 |
|---|---|---|
| `yes` | „yes“ | — |
| `/clear` (chip) | „clear“ | — |
| `Continue where you left off.` (chip) | „continue where you left off“ | — |
| `go on` | „go on“ | — |
| `ok, ship it` | „ok ship it“ | — |
| `[msg #42 from dev-foo@hetzner]: hello there` | „msg 42 from devfoohetzner hello“ | — |
| `fix the login redirect` | „fix the login redirect“ | „fix the login redirect“ |
| `Oprav chybu v prihlásení (rýchlo)` | „oprav chybu v prihlásení rýchlo“ | nezmenené |
| `Fix the login bug, then add tests for it!` | „fix the login bug then“ | nezmenené |

### P3 — Iba prvý skutočný prompt konverzácie pomenúva

- `source == 'default'` → prvý prompt, ktorý prejde P2, nastaví meno a `source = 'prompt'`.
  Ďalšie prompty ho nemenia. „yes“ ako prvý prompt filter odmietne, `source` ostane
  `default` a pomenuje až „Fix the login redirect“ — dnešná pasca (láme sa 1) zaniká.
- **Reset po `/clear`:** v `apply_session_end_hook` (`hooks.rs:657-665`), vetva
  `reason == "clear"` (nie `resume` — obnovená konverzácia si meno zaslúži): ak
  `source == 'prompt'`, vrátiť `friendly_name` na `default_friendly_name(id)` a
  `source = 'default'`. Zrkadlí pravidlo 2 skillu, ale funguje aj pre `/clear` z chipu,
  ktorý agent nikdy nevidí. `chosen` sa nedotýka.
- **Terminálový prompt (UX-36):** `apply_prompt_submit_hook` (`hooks.rs:596-618`) po
  `conversation_set_first_prompt` zavolá tú istú `record_prompt_outcome`-logiku
  (`label = true`) — payload má `prompt` aj riadok. Tým pomenúva každá vstupná cesta
  rovnako a `last_prompt` sa aktualizuje aj pre terminálové prompty. Toto je jediná
  časť návrhu s rizikom duplicity (`send_prompt` → hook → dvakrát): druhý zápis je
  no-op, lebo `source` je už `prompt`; `last_prompt` sa zapíše dvakrát tou istou hodnotou.
- `new_bg_session`: štartovací prompt je úloha samotná → pomenúva cez P2; ak filter
  odmietne, ostane default z P5.

### P4 — Ako UI odlíši auto meno od zvoleného

- `SessionRowItem.svelte` `.sess-name` dostane `data-name-source={sess.friendly_name_source ?? 'default'}`
  (jsdom-testovateľné) a CSS: `default`/`prompt` → `color: var(--fg-muted)`; `chosen` →
  `var(--fg)`. **Bez kurzívy** — kurzíva v monospace sidebare je zle čitateľná a
  emoji/badge okolo ňou nerotujú; tlmená farba stačí, tooltip dopovie.
- `title` na mene (dnes iba `tmux_name`) → `"<tmux_name> — auto-named from the first
  prompt · double-click to set a label"` / `"— default from branch"` / `"— label"`.
  Pri `friendly off` sa nič nemení (primárne je `tmux_name`).
- Jedna funkcia `displayName(sess, showFriendly)` + `nameSource(sess)` v
  `src/lib/session_view.ts`, použitá v `SessionRowItem` (3 miesta), `HostDetail.svelte:95`,
  `quick_switcher.ts:76`, `agent_context.ts:23` (UX-40).
- **UX-07:** `SessionDetails.svelte:420-428` otočiť — `h2` = `displayName`, pod ním
  `tmux_name` v mono s CopyButton; `TerminalView.svelte:1100` → `displayName` + `tmux_name`
  tlmene za ním (`.name` + `.name-tmux`). tmux status bar ostáva (je tmux-ov), riadok linka 2
  ostáva (v sidebare je `tmux_name` užitočný pri vypnutom `friendly`). Z troch výskytov na
  screenshote 01 ostanú dva, z toho jeden tlmený.
- **UX-15:** `agent_context.ts` prefix vždy nesie obe identity:
  `looking at session "yes" (tmux dev-martin-janci-kuk-agent--main) on host …`; chip
  ostáva `displayName · host`. Agent volá tools cez `tmux_name`, label je dekorácia.

### P5 — `bg:<uuid>` (`external`) riadky

- Pri **inserte** v `upsert_bg_session` (nie pri update — človek môže label zmeniť) nastaviť
  `friendly_name = "<repo> · <id8>"`, `source = 'default'`, kde `<repo>` je
  `projects.repo` z `project_id`, inak basename `agent.cwd`, inak `claude`; `<id8>` =
  prvých 8 znakov `claude_session_id`. Príklad zo screenshotu 03: `claude-fleet · 62e738aa`.
  `default_friendly_name` pre `has_no_pane` vracia tú istú hodnotu (dnes `None`), aby
  ostal jeden zdroj pravdy. `reconcile_agent_rows` musí `cwd` odovzdať (nový parameter
  alebo `Option<&str>` v `upsert_bg_session`).
- Alternatíva zvážená a odmietnutá: Claude-ovo vlastné `agent.name` (`<dir>-XX`, napr.
  `jhkljh-f2`, `claude_agents.rs:41-45`) — nesie adresár, ale sufix je šum a pre `bg`
  riadky z fleetu sa rovná `tmux_name`.
- Riadok v Outside fleet dostane badge druhu (dnes žiadny, `SessionRowItem.svelte:207`):
  po PR-1 z iterácie 01 `IconExternal` (`monitor` alebo `external-link`, doplniť do
  `icons.ts`), dovtedy textový chip `outside`. Hosts detail zobrazí „claude-fleet · 62e738aa ·
  running“.
- Telefón dostane čitateľný default zadarmo (číta `friendly_name`).

### P6 — Operátor

- Meno „fleet operator“ je explicitné → `chosen` → žiadny prompt ho neprepíše (dnes ho
  chráni len rozdiel „fleet“ vs. „Fleet“). Zjednotiť na „Fleet operator“ (sentence case
  ako humanizér) alebo nechať — kozmetika.
- Riadok operátora je `work` session v projekte, takže sedí v skupine projektu ako každá
  iná. Jeho UI je FAB; v sidebare je skôr šum. Návrh: badge `IconAgent` (sparkles z
  iterácie 01) pred menom a `readOnly`-ish akcie (žiadne Kill bez potvrdenia) — či ho z
  projektu vytiahnuť do vlastnej sekcie, je otázka pre vlastníka (mimo šošovky 11).

## Backfill

**Odporúčanie: migrácia 040 (iba stĺpec) + jednorazový Rust prechod po `migrate()`,
spoločný pre desktop aj hub.** Nie čistý SQL (SQLite bez rozšírení nevie vyhodnotiť
slovný filter), nie reconcile (reconcile je o tmux pravde, nie o labeloch), nie „nechať
a nech to opraví ďalší prompt“ (pri dnešnej logike sa „yes“ neopraví nikdy — láme sa 1).

1. `crates/fleet-core/migrations/040_friendly_name_source.sql`:
   `ALTER TABLE sessions ADD COLUMN friendly_name_source TEXT;` + `schema_version 40`;
   v `store/schema.rs` `MIGRATIONS` položka s `already_applied:
   Some(sessions_has_friendly_name_source)` (vzor 038/039, `schema.rs:306-317`).
2. `Store::backfill_friendly_name_sources()` volaná zo `Store::open*` hneď po `migrate()`
   (`store/mod.rs`, aby ju dostal aj `fleet-hub/src/serve.rs:36-40`), bez event busu (ako
   `backfill_friendly_names`, `store/sessions.rs:614-616`). Presunúť tam aj volanie
   `backfill_friendly_names` z `src-tauri/src/lib.rs:158-164` (UX-39). Beží iba nad
   riadkami so `source IS NULL`, takže je idempotentná a druhý štart nič nerobí.
3. Rozhodovací strom pre riadok so `source IS NULL`:

   | Stav | Zdroj | Akcia |
   |---|---|---|
   | `friendly_name IS NULL` | `default` | pane-less riadok dostane label z P5, ostatné nechať `NULL` (doplní `backfill_friendly_names`) |
   | `lower(friendly_name) == lower(default_friendly_name(id))` | `default` | nič (case-insensitive, takže „indigo cosmos“ z kocky = default — správne, kocka je náhoda, nie voľba) |
   | `friendly_name == friendly_name_from_prompt_legacy(last_prompt)` (dnešných 5 slov) **alebo** `label_from_prompt(friendly_name).is_none()` (t. j. ≤ 2 slová, stop-slovo, číslo) | `prompt` → **reset** | `friendly_name = default_friendly_name(id)`, `source = 'default'` |
   | inak | `chosen` | nič |

   Na hube to zresetuje `yes`, `yes`, `clear`, `clear` (a na mojej lokálnej DB `clear`,
   `yes`); „design better chat UX“, „merge terminal and conversation tabs“, „verify menu
   fallback fix e2e“ ostanú `chosen`. **Vedomé riziko:** ručný label s ≤ 2 slovami
   („Dasdasd“ je humanizovaný default, ten ostane; ale napr. ručné „PR review“ by padlo
   na default) — nedá sa rozlíšiť bez proveniencie; je to jednorazové, vratné jedným
   dvojklikom a týka sa iba riadkov spred migrácie. Do CHANGELOG-u jedna veta.
4. Po backfille prvý reconcile tick emituje riadky normálne; sidebar sa neprekreslí
   skôr, než sa frontend pripojí (rovnaký dôvod ako dnes).

## Návrh riešenia (PR plán)

Dva PR-y, aby backend mohol ísť na hub skôr než frontend (telefón a hub profitujú hneď).

### PR-A — Rust: pravidlá, proveniencia, backfill (**M**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `crates/fleet-core/migrations/040_friendly_name_source.sql`, `crates/fleet-core/src/store/schema.rs:306-317` | stĺpec + registrácia s `already_applied` guardom |
| 2 | `crates/fleet-core/src/store/rows.rs:151-155,218-230` + `map_session_row` | `pub friendly_name_source: Option<String>` s `#[serde(default)]` (**bez toho je to výpadok proti staršiemu hubu**); `SESSION_COLUMNS`; všetky struct literály `SessionRow { … friendly_name: None, … }` v testoch (`store/reconcile.rs:810`, ďalšie cez `grep -rn "friendly_name: None"`) |
| 3 | `crates/fleet-core/src/store/sessions.rs:101-141,617-655,661-695,699-718` | `upsert_bg_session(…, cwd)` insert nastaví P5 label + `source`; `backfill_friendly_names` zapíše `source='default'`; `default_friendly_name` pre pane-less vráti P5 label; `set_friendly_name(…, source: NameSource)`; nová `backfill_friendly_name_sources()` |
| 4 | `crates/fleet-core/src/store/mod.rs` `open*` | volať obe backfill funkcie po `migrate()`; zmazať volanie v `src-tauri/src/lib.rs:158-164` |
| 5 | `crates/fleet-core/src/service/sessions/prompt.rs:110-127,133-195,342-378` | `label_from_prompt` + `STOP_WORDS`; `record_prompt_outcome(…, label: bool)` s `source`; `send_prompt_inner(…, label)`; `send_system_prompt`; broadcast → `false` |
| 6 | `crates/fleet-core/src/service/safe_kill.rs:189`, `service/messages.rs:176`, `service/sessions/review.rs:102` | systémová cesta bez pomenovania |
| 7 | `crates/fleet-core/src/service/sessions/lifecycle.rs:646-655,703-734,1019-1046` | `derive_friendly_name` vráti `(label, source)`; `set_session_friendly_name` → `chosen` (prázdny → `NULL`, `default`) |
| 8 | `crates/fleet-core/src/service/bg_sessions.rs:265-269` | filter P2 + `source='prompt'` |
| 9 | `crates/fleet-core/src/service/hooks.rs:596-618,657-665` | prompt hook pomenúva (P3); `clear` resetuje `prompt` label |
| 10 | `crates/fleet-core/src/service/sessions/reconcile.rs:1044-1065` | odovzdať `agent.cwd` do `upsert_bg_session` |
| 11 | `crates/fleet-core/src/mcp/tools/messaging.rs:12`, `session_ops.rs:350`, `lifecycle.rs:125-129` | „The first task-like prompt (≥3 words, no slash command) names a still-default session“ / „a label set here is never overwritten by prompts“ — jedna klauzula, budget popisov |
| 12 | `docs/control-api-reference.md` | **generované**: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (nespúšťané v tejto iterácii) |
| 13 | `src-tauri/src/backend/hub_contract.golden.json` | nový wire field: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (podľa memory beh s regen hlási FAILED — spustiť dvakrát) |
| 14 | `~/.claude/skills/fleet-friendly-name/SKILL.md` / `service/provision.rs` (zdroj skillu) | **bez zmeny správania**; voliteľne jedna veta: „a label you set is never overwritten by prompts; clearing it (empty string) hands naming back to the first task-like prompt“ |

Poradie: 1–4 (schéma + store, zelené samy so store testami) → 5–10 (service) → 11–13
(kontrakty). Ak treba PR zmenšiť, položka 9 (hook naming + `/clear` reset) sa dá odložiť
bez nekonzistencie — P2/P3 fungujú aj bez nej, iba terminálové prompty ostanú nepomenované.

### PR-B — TS: zobrazenie (**S**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `src/lib/sessions.ts:25-112` | `friendly_name_source?: 'default' ∣ 'prompt' ∣ 'chosen' ∣ null` (**voliteľné**, aby fixtures `Sidebar.test.ts:44,981` a starší hub prešli) |
| 2 | `src/lib/session_view.ts` | `displayName(sess, showFriendly)`, `nameSource(sess)`, `nameTitle(sess)` |
| 3 | `src/lib/SessionRowItem.svelte:92-101,207,226-227,274` + CSS | `data-name-source`, tlmená farba pre auto, tooltip; badge `outside` pre `external` |
| 4 | `src/lib/SessionDetails.svelte:420-428` | otočená hierarchia (UX-07) |
| 5 | `src/lib/TerminalView.svelte:1100` | `displayName` + tlmený `tmux_name` |
| 6 | `src/lib/HostDetail.svelte:94-96`, `src/lib/quick_switcher.ts:76` | cez `displayName` (UX-40) |
| 7 | `src/lib/agent_context.ts:21-34` | prefix nesie `tmux_name` (UX-15) |

## Akceptačné testy

### Rust — `crates/fleet-core/src/service/sessions/tests.rs` (pri `:3332`)

1. `label_from_prompt_rejects_slash_commands`: `/clear`, `/compact`, `/status`, `  /resume`,
   `!ls`, `#note`, `<command-name>/clear</command-name>` → `None`.
2. `label_from_prompt_rejects_confirmations_and_short_prompts`: `yes`, `y`, `ok`, `go`,
   `go on`, `push`, `done.`, `sure thing`, `Continue where you left off.`, `yes, do it`,
   `1`, `👍`, `ok ship it` → `None`; `fix it now` (3 slová, „fix“ nie je stop-slovo) →
   `Some("fix it now")`.
3. `label_from_prompt_rejects_fleet_headers`: `[msg #42 from dev-foo@hetzner]: hello there`,
   `[context] the person is looking …`, `[claude-fleet: message from me] ship it` → `None`.
4. `label_from_prompt_keeps_real_tasks_regardless_of_case`: existujúce prípady z
   `:3334-3347` + `fix the login redirect` → `Some("fix the login redirect")`; viacriadkový
   prompt hodnotí prvý riadok.
5. `first_real_prompt_names_once`: default „Fix login“ → `yes` (meno nezmenené, `source`
   `default`) → `Rewrite the auth flow!` (→ „rewrite the auth flow“, `prompt`) → `Now add
   tests for it` (nezmenené). Nahrádza asserty z `prompt_derived_name_replaces_only_the_branch_default`
   (`:3242-3305`), ktorý ostáva ako ochrana `chosen`.
6. `chosen_label_survives_even_when_equal_to_default`: `set_session_friendly_name` na
   presný default → `source='chosen'` → prompt ho nenahradí.
7. `system_prompts_never_label`: `send_system_prompt` so safe-kill textom / `pane_header`
   a `broadcast_prompt` nad dvoma default riadkami → mená nezmenené, `last_prompt` a
   `prompt_sent` event zapísané.
8. `clear_resets_prompt_label_but_keeps_chosen` (`hooks.rs` testy, vzor `:1885-1960`):
   `SessionEnd reason=clear` → `prompt` riadok späť na default; `chosen` riadok nezmenený;
   `reason=resume` nemení nič.
9. `prompt_submit_hook_names_the_session` (UX-36): payload s `prompt: "Fix the login
   redirect"` na default riadku → meno + `last_prompt`.
10. `external_rows_get_project_and_short_id_default` (`reconcile_tests.rs`, vzor `:875-941`):
    agent s `cwd` v projekte `o/r`, id `62e738aa-…` → `friendly_name == "r · 62e738aa"`,
    `source='default'`; druhý reconcile po ručnom `set_friendly_name` label nezmení.
11. `backfill_sources_resets_junk_and_keeps_chosen` (`store/sessions.rs` testy, vzor
    `:2450-2510`): seed `yes`/last_prompt `yes` → default; `Fix login` == default → `default`;
    `indigo cosmos` vs default `Indigo cosmos` → `default`; `design better chat UX` →
    `chosen`; `bg:…` external `NULL` → P5 label; druhý beh = 0 zmien.
12. `marked_prompt_records_the_body_not_the_marker` (`:3214-3239`) — upraviť očakávanie:
    `Rewrite the auth flow!` stále pomenúva (4 slová), lookalike `[claude-fleet: …] ship it`
    už **nie** (hlavička), `last_prompt` ostáva doslovný.
13. `store/sessions.rs:2425` `friendly_name_defaults_skip_pane_less_rows` → premenovať a
    otočiť: pane-less riadky dostanú P5 default.

### Frontend — Vitest

1. `src/lib/session_view.test.ts`: matica `displayName` (toggle on/off × friendly null/set ×
   kind work/external) a `nameTitle` pre tri zdroje.
2. `src/lib/Sidebar.test.ts` (vzor `:1189-1199`): riadok so `friendly_name_source: 'prompt'`
   má `.sess-name[data-name-source="prompt"]` a `title` obsahuje „auto“; `chosen` má
   `data-name-source="chosen"`; external riadok v `outside-fleet-section` **neobsahuje**
   text začínajúci `bg:`.
3. `src/lib/agent_context.test.ts:29`: prefix obsahuje aj `tmux_name`; chip ostáva
   `friendly · host`.
4. `SessionDetails` test: `h2` = friendly, `tmux_name` v podriadku; dvojklik na `h2` stále
   otvára label editor (`details-label`).
5. `src/lib/HostDetail.test.ts`: s `showFriendlyNames=false` zobrazí `tmux_name`.
6. `npx svelte-check` = 0 chýb; `npx vitest run` celé (memory: filtrované behy schovali
   červený test).

### Manuálne (jeden screenshot podľa README §1)

Sidebar po backfille: žiadne `yes`/`clear`, auto mená tlmené, „design better chat UX“
plné; Outside fleet: `claude-fleet · 62e738aa` s badge; detail session s friendly v `h2`;
Agent panel chip s tým istým menom ako riadok.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| PR-A položky 1–4 (schéma, store, backfill) | S | ~120 riadkov + store testy ~120 |
| PR-A položky 5–10 (service, hooky, reconcile) | M | ~180 riadkov + testy ~200 |
| PR-A položky 11–13 (popisy, regen docs, golden) | S | ~20 riadkov + generované |
| **PR-A spolu** | **M** | ≈320 riadkov bez testov a generovaných súborov — na hrane limitu ~300 z README §5; položka 9 je prirodzený odštep (PR-A2) |
| **PR-B** | **S** | ~120 riadkov + testy ~80 |

## Otázky pre vlastníka

1. **Reset po `/clear`** (P3): má sa auto meno z promptu po `/clear` vrátiť na default, aby
   ho pomenoval prvý prompt novej konverzácie? Zrkadlí pravidlo skillu; `resume` navrhujem
   nechať.
2. **Backfill reset ≤ 2-slovných mien:** akceptuješ, že jednorazovo padnú na default aj
   ručné labely s ≤ 2 slovami spred migrácie (bez proveniencie sa nedajú odlíšiť od
   „yes“)? Alternatíva: resetovať iba presné zhody so stop-listom a slash príkazmi
   (konzervatívnejšie, nechá „go on“).
3. **Default pre `external` riadok:** `<repo> · <id8>` (návrh) alebo Claude-ovo `agent.name`
   (`jhkljh-f2`)? A má ísť `cwd` do samostatného stĺpca (užitočné aj pre Hosts detail),
   alebo stačí odovzdať ho do `upsert_bg_session`?
4. **Overenie duplikátu (UX-06e):** na hube porovnať `claude_session_id` tmux riadkov na
   `claude-fleet-trn` s uuid v `bg:62e738aa…` a `bg:8df9227c…`. Ak sa zhodujú, Outside fleet
   ukazuje fleet-ové sessions druhý raz a treba fix v `find_by_unique_cwd` pre vzdialené
   hosty, nie len meno.
5. **Broadcast nikdy nepomenúva** — súhlas? (Alternatíva: pomenovať iba ak je cieľ jeden.)
6. **Stop-list jazyky:** EN + SK stačí? Ďalšie pridať ako konfiguráciu, alebo pevne?
7. **Operátor v sidebare:** nechať ako bežný riadok projektu s agent ikonou, alebo
   presunúť do vlastnej sekcie / skryť (jeho UI je FAB)? Patrí do šošovky 11, tu len
   rozhodnutie o mene „Fleet operator“ a `chosen`.
8. **Validácia názvu worktree v dialógu** (UX-38, mimo šošovky): odmietnuť názvy bez
   samohlásky / < 3 znaky, alebo ponechať a spoľahnúť sa na pomenovanie z promptu?

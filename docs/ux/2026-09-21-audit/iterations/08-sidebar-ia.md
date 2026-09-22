# Iterácia 08 — Sidebar: filter chrome a informačná architektúra

**Šošovka:** štyri riadky chipov nad zoznamom, vyhľadávanie, pätička (`+ New session` · `⚡` ·
`theme: dark`), project picker · **Zasahuje:** UX-08, UX-12 (+ UX-11 pätička, UX-04/31 kôš,
položka „`Needs you (0)` svieti aj pri 0“ z konsolidácie-01 §5) · **Vstup:** README, screenshoty
01 a 03, `iterations/01-icon-system.md` (mapa `icons.ts`), `02-session-naming.md`
(`displayName()`, P4), `consolidation-01.md` (D1–D6, §4 „default“), kód na `5952a0aa` ·
**Režim:** read-only, žiadne zmeny kódu, žiadny `cargo`; `npx vitest run src/lib/Sidebar.test.ts`
spustený na overenie východiskového stavu. Nálezy od **UX-87**, PR od **UXPR-28**.

## Zhrnutie

1. **Chrome sidebaru dnes zaberá ≈ 200 px** (hlavička ≈ 134 px + pätička ≈ 65 px) z ≈ 977 px
   výšky panelu pri okne 1027 px, t. j. **≈ 20 %**; pri predvolenej šírke 280 px
   (`App.svelte:66`) sa riadok hostov s piatimi hostami zalomí a hlavička rastie o 22 px na
   riadok. README pri UX-08 uvádza „~90 px“ — to sú iba štyri riadky chipov (22 + 18 + 18 + 18 px
   + 3 medzery); riadok vyhľadávania, paddingy a okraj pridávajú ďalších ≈ 43 px. Návrh nižšie
   ide na **≈ 110 px** (hlavička 68 + pätička 42), čo je −90 px ≈ 2,5 dvojriadkových session.
2. Všetky tri nálezy šošovky sú **potvrdené**; UX-08 s korekciou výšky a s upresnením, že
   `Needs you (0)` nesvieti načerveno (`class:hot={needsYouCount > 0}`,
   `SidebarFilters.svelte:142`) — pri nule ostáva výstražný glyf `⚠` a text, čo je ten „svit“.
   UX-12 rozšírené: picker zahadzuje poradie podľa poslednej aktivity, ktoré backend už dodáva
   (`projects.rs:118-119` vs `Sidebar.svelte:346-356`).
3. **Dve premisy zadania sú vyvrátené:** `bg off` **nie je** default (`showBgAgents` je `true`,
   `sessions.ts:175`, spec 2026-05-23 §1) — na screenshote ho operátor vypol; a vypnutý prepínač
   **neskrýva** riadky `bg:<uuid>` v *Outside fleet* (`buildOutsideFleet` ho ignoruje,
   `sidebar_index.ts:53-61`) — skrýva iba supervidované `kind='bg'` agenty. Hosts view má chord
   **⌘I**, nie ⌘1 (`app_views.ts:76,92`; README §1 treba opraviť).
4. Desať nových nálezov **UX-87…UX-96**. Dva sú M a menia sémantiku: časový filter je
   **projektový**, nie session-ový (`matchesRecency(p, …)` nad `project.last_session_at`,
   `session_status.ts:22-28`), a vyhľadávanie tiež — zhoda jednej session zobrazí **všetky**
   sessions projektu (`Sidebar.svelte:254-265`, riadky žiadny search predikát nedostanú). Tretí M:
   `byTriage` (`attention.ts:246-251`) sa **nikde nevolá** — v projekte platí poradie zo store,
   `blocked` riadok môže sedieť pod `idle` riadkami (FE-4 „status sort“ nie je landed).
5. Navrhovaná IA: **dva riadky hlavičky** (vyhľadávanie + jeden riadok filtrov: host `<select>`,
   čas `<select>`, chip pozornosti skrytý pri nule, ikona Select, tlačidlo **View ▾**), **View
   menu** so sekciami Show / Sort / Theme, **bulk lišta nahrádza riadok filtrov** namiesto
   vkladania medzi riadky, pätička = **jedno split tlačidlo** `+ New session ▾` (hlavná časť →
   picker s vyhľadávaním, hostom a sekciou *Recent*; šípka → *Background session…*, *Add
   project…*), Tasks von z riadku filtrov do horného pásu záložiek vedľa `Hosts ⌘I`.
6. Štyri Svelte PR: **UXPR-28** (riadok filtrov + View menu, M), **UXPR-29** (split tlačidlo +
   picker, M), **UXPR-30** (chip pozornosti skrytý pri nule + status sort, S), **UXPR-31**
   (session-ová sémantika času a vyhľadávania + chord `/`, S). Žiadny kľúč v `prefs.ts` sa
   nepremenúva, migrácia nie je potrebná. `UXPR-02` (ikony v `SidebarFilters.svelte`) musí
   ísť **pred** UXPR-28, inak sa jeho diff zahodí.

## Inventár chrome

Hlavička (`SidebarFilters.svelte:66-222`) a pätička (`Sidebar.svelte:775-823`). „Typ“: F = filter
(mení množinu riadkov), V = prepínač zobrazenia (mení vzhľad, nie množinu), M = mód, A = akcia,
N = navigácia. „Perzist.“: kľúč v `localStorage` (`prefs.ts:12-33`, prefix `cf:pref:`).

| # | Ovládač | Riadok | Typ | Čo robí | Perzist. | Klávesnica | Hint | Test |
|---|---|---|---|---|---|---|---|---|
| 1 | `Search sessions, projects…` | `SidebarFilters.svelte:68-73` | F | substring nad owner/repo, `tmux_name`, `host_alias`, `friendly_name` (`Sidebar.svelte:254-265`), debounce 150 ms (`:88-99`); **filtruje projekty, nie riadky** (UX-88) | nie | žiadny chord na fokus (`grep` `.focus()` nad `sidebar-search` → 0); Hosts view má `/` (`HostsView.svelte:326,368`), switcher ⌘K/⌘P (`QuickSwitcher.svelte:2`) | — | `Sidebar.test.ts:558,745` |
| 2 | `↻` Refresh | `:74-76` | A | `refreshProjects` + `loadSessions({force:true})` (`Sidebar.svelte:238-251`); pri `loading` glyf `…`, `disabled` + `cursor: progress` (`:266`) | — | tab | — | nepriamo |
| 3 | `‹` Hide sidebar | `:78-84` | V (layout) | `onCollapse` → `App.svelte` `layout.sidebar-collapsed` | `layout.sidebar-collapsed` (App) | tab | — | `Sidebar.test.ts:726-743` |
| 4 | `all` + host pills | `:88-104` | F | `hostFilter.set(alias)`; iba `!h.hidden`; bodka reachability; `title` = tmux/claude verzia + účet (`:99`, `:57-63`) | `host-filter` (`hosts.ts:42-46`) | tab per pill, bez ← → | `host-filter` (≥ 2 hosty, `hints.ts:25-30`) | `Sidebar.test.ts:756-870` |
| 5 | `☑` Tasks | `:105-112` | N | otvorí `Modal` s `TasksPanel` (`Sidebar.svelte:892-896`) | — | tab; **bez chordu** | — | `tasks-open` (nepriamo) |
| 6 | `⚙` Settings | `:113-120` | N | `settingsOpen.set(true)` (`app_views.ts:26`) | — | tab; **⌘,** (`app_views.ts:79`) | — | `settings-open` |
| 7 | `all 8h 1d 3d 7d 30d` | `:123-133` | F | `recency` → `matchesRecency(p, r)` nad `project.last_session_at` (`session_status.ts:13-28`) — **projektový filter** (UX-87) | `recency` (`Sidebar.svelte:66-72`) | tab per pill | `recency-filter` (`hints.ts:46-50`) | `Sidebar.test.ts:700-724` |
| 8 | `⚠ Needs you (N)` | `:139-149` | F | `needsYouOnly` → predikát `needsYou` (`Sidebar.svelte:148-152`); N = `countNeedsYou` nad host-viditeľnými riadkami (`:270-276`); `.hot` iba pri N > 0 (`:142`), glyf vždy | **nie** (zámerne, `Sidebar.svelte:131-136`) | tab, `aria-pressed` | — | `Sidebar.test.ts:1086-1145` |
| 9 | `☑ select` | `:150-159` | M | `toggleSelectMode` → checkboxy v riadkoch; bulk lišta až pri `selectedCount > 0` (`:163-182`) | nie | tab, `aria-pressed`; shift/⌘-klik bez módu (`Sidebar.svelte:461-466`) | — | `Sidebar.test.ts:1148-1188` |
| 10 | `<Attention />` | `:161` | — | `sr-only` `aria-live` (stuck oznamy); nemá výšku (`position: absolute`) | — | — | — | `Attention.test.ts` |
| 11 | `🤖 bg on/off` | `:185-194` | F | `showBgAgents` → `sessionVisible` vyradí `kind === 'bg'` (`sidebar_index.ts:22`); *Outside fleet* ignoruje | `show-bg-agents` default **true** (`sessions.ts:175`) | tab, `aria-pressed` | — | `Sidebar.test.ts:934-950` |
| 12 | `🏷 friendly on/off` | `:195-206` | V | `showFriendlyNames` → `primaryIsFriendly` (`SessionRowItem.svelte:93-94`) | `show-friendly-names` default true (`sessions.ts:182-185`) | tab | — | `Sidebar.test.ts:1190+` |
| 13 | `≡ details on/off` | `:207-216` | V | `showRowDetails` → línia 2 (`SessionRowItem.svelte:354-355`) | `rows.details` default true (`sessions.ts:189-190`) | tab | — | `Sidebar.test.ts:1220+` |
| 14 | `+ New session` | `Sidebar.svelte:777-782` | A | `toggleProjectPicker` (`:393-396`) → `.picker` `role="listbox"` (`:801-822`), abecedne (`:346-356`), bez vyhľadávania, bez hosta (iba `pickerHost` z Hosts view, `:384-411`) | — | tab; Escape zavrie (`:828-830`); Enter na položke | — | `Sidebar.test.ts:539-600`, `hub_disabled.test.ts:243,431-442` |
| 15 | `⚡` bg session | `:783-790` | A | `NewBgSessionDialog` (host `<select>` nad **všetkými** hostami vrátane skrytých, `NewBgSessionDialog.svelte:53-56`) | — | tab; bez `aria-label` (UX-34) | `bg-session` (`hints.ts:31-35`) | `Sidebar.test.ts:1438` |
| 16 | `theme: dark` | `:792-799` | V | `cycleTheme` auto→light→dark (`theme.ts:24-28`) | `cf:theme` — **mimo `prefs.ts`** (`theme.ts:4-12`) | tab | — | `Sidebar.test.ts:745-753` |
| 17 | `＋ Add project…` (v pickeri) | `:802-809` | A | `AddProjectDialog`; `disabled` cez `hubBlock('add_project')` (`:367`) | — | tab | — | `hub_disabled.test.ts:431-442` |
| 18 | `🗑️` kôš (riadok projektu, nie chrome) | `:716-724` | A | `ConfirmDialog` → `purgeProject`; v hub režime `disabled` + trvalo viditeľný (UX-04/31) | — | tab | — | `Sidebar.test.ts:382` |

Súvisiace, čo chrome nemá: **status sort** (`byTriage`, `attention.ts:246-251`, nevolaný),
**počet skrytých riadkov** pri `bg off` alebo aktívnom hostovi (nikde), **chord na vyhľadávanie**.

### Výška chrome (výpočet z CSS, `1rem = 14px`, `src/app.css:161`)

Hlavička `.sidebar-header` (`SidebarFilters.svelte:225-233`): padding 7 + 5,6 px, gap 4,9 px,
border 1 px. Riadky (line-height `normal` ≈ 1,2 pre pills, `1` pre `.icon-btn`):

| Riadok | Výpočet | px |
|---|---|---|
| search + `↻` + `‹` | input `0.85rem` × 1,2 + 2 × 4,2 padding + 2 border ≈ 24,7; `.icon-btn` 12,6 + 7 + 2 = 21,6 | **25** |
| hosty (`.pill` 9,8 × 1,2 + 4,2 + 2 = 18; `.icon-btn` 21,6 → `align-items: center`) | 1 riadok pri ≈ 600 px; **2–3 riadky pri 280 px** | **22** (+22/riadok) |
| čas | 6 pills | **18** |
| triage | 2 pills | **18** |
| bg/friendly/details | 3 pills | **18** |
| **spolu** | 7 + 25 + 4,9 + 22 + 4,9 + 18 + 4,9 + 18 + 4,9 + 18 + 5,6 + 1 | **≈ 134** |

Pätička `.sidebar-footer` (`Sidebar.svelte:1030-1043`): 1 border + 5,6 + `new-btn` 27,5
(`0.85rem` × 1,2 + 2 × 5,6 + 2) + 4,2 gap + `theme-toggle` 21,6 + 7 ≈ **67 px**.

Overenie na screenshote 01 (mierka 1800/1728 = 1,042): hlavička 58 → 192 px obrazu ≈ 129 px
skutočných; pätička 965 → 1027 ≈ 60 px. Rozdiel ±5 px je rendering fontu. **Chrome ≈ 195–200 px.**
Panel sidebaru = 1027 − 28 (titulok) − 22 (stavový riadok appky) ≈ **977 px** → zoznam
**≈ 780 px = 80 %**, chrome **20 %**; pri okne 800 px (13" notebook) je to 27 %. Dvojriadková
session má ≈ 36 px (screenshot: 230 → 267 px obrazu), takže chrome = **≈ 5,5 riadkov session**.

Pills majú 18 px, `.icon-btn` 21,6 px, `theme-toggle` 21,6 px — všetko **pod 24 px
`--control-h`** (`app.css:26-29`, WCAG 2.5.8), ktoré `controls.css` `.btn--chip` dodržiava.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-08 | **Potvrdené, s korekciou čísla a upresnením.** Štyri riadky chipov = 91 px; celá hlavička = 134 px, chrome so pätičkou ≈ 200 px (20 % panelu). Mieša **päť** druhov ovládačov (F/V/M/A/N v inventári) v jednom bloku: riadok hostov (`nav aria-label="host filter"`, `:88`) obsahuje aj Tasks a Settings (`:105-120`) — čítačka ich ohlási ako súčasť host filtra. `Needs you (0)`: červený tón iba pri N > 0 (`:142,271`), ale glyf `⚠` a text sú bezpodmienečné (`:148`) → pri nule „svieti“ ikonou, nie farbou. Počítadlo zámerne vynecháva `idle_long` (`attention.ts:140-150`, „Do not reconcile these two sets“) — skrytie pri nule preto berie jedinú cestu k idle nudge (rieši status sort, UXPR-30). | `SidebarFilters.svelte:66-222`; screenshot 01 (hlavička 58–192 px obrazu) |
| UX-12 | **Potvrdené, rozšírené.** (a) Picker je `role="listbox"` z `<button>`ov bez `option` role (`:801-822`), abecedne (`allProjectsSorted`, `:346-356`) — **zahadzuje** poradie „naposledy aktívne prvé“, ktoré `list_projects` už vracia (`store/projects.rs:118-119`); bez vyhľadávania, bez hosta (host ide iba cez `pickerHost` z Hosts view, `:384-411`, alebo `last-host` v dialógu, `NewSessionDialog.svelte:107`); *Add project…* je prvá položka. (b) `⚡` (`:783-790`) vedie do `NewBgSessionDialog` s vlastným `<select>` hostov **vrátane skrytých** (`NewBgSessionDialog.svelte:53-56`, UX-96), kým pills aj `NewSessionDialog` skryté vyraďujú (`SidebarFilters.svelte:94`, `NewSessionDialog.svelte:102`). (c) Tri tvary plusu (`+`, `+ New session`, `＋ Add project…`, iter. 01 tabuľka A). (d) Hint `bg-session` visí na `⚡` (`:790`, `hints.ts:31-35`). | `Sidebar.svelte:346-356,383-411,775-823` |
| UX-11 | **Potvrdené** (iter. 01) + doplnok: tlačidlo je 100 % šírky, 21,6 px — **celý riadok pätičky pre jedno nastavenie**; kľúč `cf:theme` žije mimo `prefs.ts` (`theme.ts:4-12`), takže sa ho žiadna prefs migrácia nedotkne; cyklus bez náhľadu, `auto` neukáže, na čo sa rozhodlo. | `Sidebar.svelte:792-799,1064-1073` |
| UX-04 / UX-31 | **Potvrdené** (iter. 01, 06, 07 — skryť v hub režime). Kôš je v riadku projektu (`:716-724`), nie v chrome; táto šošovka ho nemení, iba upozorňuje, že UXPR-23 aj UXPR-28/29 editujú `Sidebar.svelte` (viď PR plán). | `Sidebar.svelte:716-724,1056-1063` |
| `Needs you (0)` (kons. §5) | **Potvrdené** — viď UX-08. | `SidebarFilters.svelte:139-149` |
| Premisa „`bg off` je default“ | **Vyvrátené.** `showBgAgents = readPref('show-bg-agents', true, …)` (`sessions.ts:175`); spec 2026-05-23 §1 „default `true`“. Screenshot ukazuje stav po vypnutí operátorom. | `sessions.ts:175-176` |
| Premisa „`bg off` skryje operátora/agentov v Outside fleet“ | **Vyvrátené.** `buildOutsideFleet` neberie `showBgAgents` (`sidebar_index.ts:53-61`); `bg:<uuid>` sú `kind='external'` (iter. 02). Prepínač skrýva iba `kind === 'bg'` (`:22`). | screenshot 03 (`bg off` + *Outside fleet (2)* viditeľné) |
| README §1 „Hosts view (⌘1)“ | **Nepresné.** Chord je **⌘I** / Ctrl+Shift+H (`app_views.ts:76,82,92`; `App.svelte:737-739` `Hosts <kbd>⌘I</kbd>`). Oprava README, bez ID. | `SettingsDialog.test.ts:78` |
| UX-10 (šošovka 10, iba poznámka) | Bulk lišta **existuje** (`SidebarFilters.svelte:163-182`, testid `bulk-bar`, `Sidebar.test.ts:1160,1182`), ale iba pri `selectedCount > 0`; so zapnutým select módom a nulovým výberom nie je hint. Zvyšok šošovke 10. | — |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz | Návrh |
|---|---|---|---|---|
| **UX-87** | **M** | **Časový filter je projektový, ale tvári sa session-ovo.** `matchesRecency(p, r)` porovnáva `project.last_session_at` (= MAX `last_activity_at` sessions projektu, prepisované pri každom reconcile, `reconcile.rs:461-478,693-698`). Pod `8h` sa zobrazí projekt, ktorého najnovšia session je mladšia ako 8 h, **so všetkými** jeho sessions vrátane 30-dňových; projekt s jedinou 9-hodinovou session zmizne celý. Chipy `all 8h 1d…` nemajú viditeľnú jednotku ani predmet (iba `aria-label="recency filter"`); hint sľubuje „Narrow the list to recent activity“ (`hints.ts:46-50`). | `session_status.ts:13-28`; `Sidebar.svelte:282-291`; `Sidebar.test.ts:700-724` | Session-ová sémantika: predikát `last_activity_at ≥ now − window` do `buildSessionsByProject`; projekt viditeľný, ak má ≥ 1 viditeľnú session (rovnaké pravidlo ako host filter); label `Active: 8h` (UXPR-31, otázka 2) |
| **UX-88** | **M** | **Vyhľadávanie filtruje projekty, nie riadky.** `matchesSearch` vráti `true`, ak sa zhoduje repo/owner **alebo ktorákoľvek** session (`Sidebar.svelte:254-265`); `filteredSessionsByProject` search predikát nedostane (`:313-315`) → dotaz `violet` zobrazí `claude-fleet` s oboma sessions (`yes`, `clear`). Substring, bez vetvy a stavu; QuickSwitcher hľadá fuzzy nad 8 fazetami s MRU (`quick_switcher.ts:65-96`, `fuzzy.ts`); tri vyhľadávania, tri správania (sidebar / ⌘K / Hosts `/`). Bez chordu na fokus. | `Sidebar.svelte:88-99,254-265,313-315`; `QuickSwitcher.svelte:2,174`; `HostsView.svelte:326,368` | `fuzzyMatchFields` nad tými istými poľami ako switcher; zhoda projektu = všetky riadky, inak iba zhodné riadky; `/` fokusuje search mimo editovateľného cieľa (UXPR-31, otázka 3) |
| **UX-89** | L | **Host pills sa neškálujú a duplikujú Hosts view.** N + 1 pills + 2 ikony v jednom `flex-wrap` riadku (`:297`): pri 280 px (default `layout.sidebar`, `App.svelte:66`) a 5 hostoch dva až tri riadky; bez počtu sessions per host (Hosts view ho má: `⚡9 ↑20`, screenshot 03); `title` nesie verziu tmux/claude a účet (`:99`) — fakty Hosts view. Bez ← → navigácie (nie je to `SegmentedControl`). Aktívny host mimo `all` a automatické rozšírenie pri výbere (`Sidebar.svelte:208-225`) je správne, ostáva. | `SidebarFilters.svelte:88-104,297-307`; `hosts.ts:42-46` | Jeden `<select class="btn btn--chip">` „Host: all (12)“ s možnosťami `alias (n) · offline`; skryté hosty vyradené ako dnes (UXPR-28, otázka 1) |
| **UX-90** | L | **Navigácia v riadku filtrov.** `☑` Tasks (Modal) a `⚙` Settings sú posledné deti `<nav aria-label="host filter">` (`:88-121`); Tasks nemá chord, hoci je to fleet-wide pohľad ako Hosts (⌘I v páse záložiek, `App.svelte:737-739`); `☑` = Tasks aj Select (UX-33). | `SidebarFilters.svelte:105-120`; `Sidebar.svelte:892-896` | Settings → riadok vyhľadávania (`IconSettings`); Tasks → pás záložiek vedľa `Hosts ⌘I` cez `tasksOpen` v `app_views.ts` (vzor `settingsOpen`), fallback View menu → *Open* (otázka 4) |
| **UX-91** | M | **Prepínače zobrazenia ako „stav on/off“.** Tri pills s textom `bg off` / `friendly on` / `details on` — stav je zakódovaný dvakrát (text + `.pill.active`), text sa pri kliku mení, takže tlačidlo číta ako informačný `.tag`; ikona `🏷` zdieľaná s *Edit label* (UX-33). Pri `bg off` nikde nie je „(3 hidden)“; prepínač skrýva `kind='bg'`, ale *Outside fleet* nie (`sidebar_index.ts:22,53-61`) — používateľ vidí `bg off` a pod ním `bg:62e7…` (screenshot 03). Sú to jediné ovládače, ktoré **nefiltrujú** (friendly, details) v bloku, ktorý inak filtruje. | `SidebarFilters.svelte:184-217`; `sessions.ts:175-190` | View menu: `[x] Background agents (3 hidden)` / `[x] Friendly names` / `[x] Details line` — checkbox = stav, label stály (UXPR-28); Outside fleet ostáva nezávislé (otázka 7) |
| **UX-92** | **M** | **Žiadny status sort v zozname.** `byTriage()` je exportovaný a nepoužitý; poradie v projekte = poradie zo store (`list_sessions … ORDER BY last_activity_at DESC`, `store/sessions.rs:528`) + nové riadky na koniec (`row_store.ts:60-65`, `sessions.ts` bez `normalize`) → `blocked` riadok pod tromi `idle`. Podľa závažnosti sa radia iba **projekty** (`sortProjectsBySeverity`, `sidebar_index.ts:83-91`). FE-4 „status sort“ (plán 2026-09-10) nie je landed; D2 hovorí iba o projektoch. | `attention.ts:244-251`; `Sidebar.svelte:313-315`; `sidebar_index.ts:29-44` | `buildSessionsByProject` zoradí každý projekt `byTriage` (jeden prechod + sort, mimo per-row cesty); pref `rows.sort ∈ {status, recent}`; radio vo View menu (UXPR-30) |
| **UX-93** | L | **Bulk lišta sa vkladá medzi riadky.** `{#if selectedCount > 0}` blok (`:163-182`) sedí medzi triage a bg riadkom → pri prvom zaškrtnutí zoznam skočí o ≈ 28 px dole a pri `clear` späť. | `SidebarFilters.svelte:163-182,275-285` | Lišta **nahrádza** riadok filtrov v tej istej výške (24 px) kým `selectedCount > 0` (UXPR-28); obsah lišty šošovka 10 |
| **UX-94** | L | **Tri zo siedmich hintov visia na chrome, ktorý sa mení** (`host-filter`, `recency-filter`, `bg-session`, `hints.ts:25-61`); `hints-seen` ukladá id (`:72-73`) — premenovanie id by hinty znovu ukázalo; texty predpokladajú pills a `⚡`. | `hints.ts:25-61`; `SidebarFilters.svelte:88,123`; `Sidebar.svelte:790` | Id ponechať, presunúť kotvy: `host-filter` → host select, `recency-filter` → čas select, `bg-session` → šípka split tlačidla; texty upraviť (UXPR-28/29) |
| **UX-95** | L | **Lokálne CSS pod cieľovou výškou.** `.pill` 18 px, `.search` 24,7, `.icon-btn` 21,6, `.new-btn` 27,5, `.theme-toggle` 21,6 (`SidebarFilters.svelte:239-296`; `Sidebar.svelte:934-958,1044-1073`) — `controls.css` má `.btn--chip` (24 px, `--control-h`), `.btn--icon`, `.btn--primary` (28 px) a `.tag`; UX-29 rieši `.icon-btn` kópie, pills/search/new-btn nie. `SegmentedControl.svelte` (← →, `aria-pressed`) existuje a sidebar ho nepoužíva. | `controls.css:14-38,99-146`; `app.css:26-41`; `SegmentedControl.svelte:1-65` | Všetko v chrome na `controls.css` triedy; zrušiť `.pill`, `.new-btn`, `.theme-toggle` (UXPR-28/29) |
| **UX-96** | L | **`NewBgSessionDialog` ponúka skryté hosty** (`$hosts` bez filtra, `:53-56`), kým pills (`SidebarFilters.svelte:94`) a `NewSessionDialog` (`:102`) ich vyraďujú; dva „vytvor“ toky, dve pravidlá pre „hidden“. | `NewBgSessionDialog.svelte:51-58` | Rovnaký `usableHost` filter; host predvolený z pickera (UXPR-29) |

## Návrh IA

### Princípy

1. **Filter mení množinu, View mení vzhľad, mód mení interakciu** — každý druh má vlastné miesto:
   filtre v jednom riadku, vzhľad v jednom menu, mód ako jedna ikona, navigácia mimo filtrov.
2. **Výška chrome nezávisí od šírky sidebaru ani od počtu hostov** — žiadny `flex-wrap` v hlavičke;
   host aj čas sú `<select>`, ktoré sa skracujú, nie zalamujú.
3. **Nič nesvieti bez dôvodu** — chip pozornosti existuje iba pri N > 0 (alebo kým je filter
   zapnutý, aby sa dal vypnúť).
4. **Stav sa kóduje raz** — checkbox/`aria-pressed`, nie text „on/off“.
5. **Všetko na `controls.css`** (`.btn`, `.btn--chip`, `.btn--toggle`, `.btn--icon`,
   `.btn--primary`, `.tag`), ikony z `icons.ts` (iter. 01): `IconRefresh` (`refresh-cw`),
   `IconSettings` (`settings`), `IconSidebarHide` (`panel-left-close`), `IconAttention`
   (`triangle-alert`), `IconSelect` (`square-check`), `IconCaret` (`chevron-down`), `IconNew`
   (`plus`), `IconTasks` (`list-checks`, v páse záložiek). Riadky View menu sú textové checkboxy
   — bez ikon, čím odpadajú kolízie `🏷`/`☑`/`⚡` z UX-33.

### Hlavička v pokoji (280 px, `1rem = 14px`)

```
┌──────────────────────────────────────────────────────────────┐ ─┐ 7 px padding
│ [ Search sessions, projects…              / ]  (↻) (⚙) (‹)   │  │ 24 px  riadok 1
│                                                              │  │ 6 px gap
│ [Host: all ▾] [Active: all ▾] [⚠ 3]            (☐) [View ▾] │  │ 24 px  riadok 2
└──────────────────────────────────────────────────────────────┘ ─┘ 5,6 px + 1 px border
   ▲ <select>       ▲ <select>    ▲ chip, iba N>0  ▲ Select  ▲ View menu
```

- Riadok 1: `<input class="search">` s `--control-h`, placeholder s `/` vpravo ako v Hosts view
  (`Filter hosts /`); `IconRefresh` (`.btn.btn--icon`, pri `loading` `aria-busy` + spinner z
  UXPR-01, nie `…`); `IconSettings` (⌘, v `title`); `IconSidebarHide`.
- Riadok 2 vľavo: `Host` `<select class="btn btn--chip">` — `all (12)` + `alias (n)` (`n` z jednej
  mapy `alias → count` nad `$sessions`, derived raz), suffix ` · offline` pre `!reachable`;
  `Active` `<select>` — `all · 8h · 1d · 3d · 7d · 30d`; **chip pozornosti** `IconAttention N`
  (`.btn--chip.btn--toggle.btn--crit`, `aria-pressed`, testid `needs-you-filter`) iba ak
  `N > 0 || needsYouOnly`.
- Riadok 2 vpravo: `IconSelect` (`.btn--icon.btn--toggle`, testid `select-mode`); **`View ▾`**
  (`.btn--chip`, `aria-haspopup="true"`, `aria-expanded`).
- Pri `selectedCount > 0` **riadok 2 nahradí bulk lišta** (`N selected · Send · Kill · clear`,
  rovnaká výška, testid `bulk-bar`; obsah = šošovka 10).
- `loadError` ostáva pod riadkom 2 (jediný podmienený rast).

Výška: 7 + 24 + 6 + 24 + 5,6 + 1 = **≈ 68 px** (dnes 134).

### View menu otvorené

```
│ [Host: all ▾] [Active: all ▾] [⚠ 3]            (☐) [View ▾] │
│                                   ┌────────────────────────────┐
│  papayapos-backend         4      │ SHOW                       │
│   ● check PD-2939 vat …           │ [x] Background agents      │
│     claude-fleet-trn · …          │     (3 hidden)             │
│   ● indigo cosmos                 │ [x] Friendly names         │
│     …                             │ [x] Details line           │
│  kuk-agent                 1      │ SORT                       │
│   ● yes                           │ (•) Status   ( ) Recent    │
│                                   │ THEME                      │
│                                   │ [ Auto ][ Light ][ Dark ]  │
│                                   └────────────────────────────┘
```

- `ViewMenu.svelte` (nový): absolútne umiestnený panel ako `.picker` (`Sidebar.svelte:1075-1086`
  — overený vzor vo WKWebView; žiadne `popover`/anchor API), `role="group"`, zatvára Escape a
  klik mimo; fokus po otvorení na prvý checkbox, Tab cykluje vnútri.
- SHOW: tri natívne `<input type="checkbox">` viazané na `showBgAgents`, `showFriendlyNames`,
  `showRowDetails` (testidy `bg-toggle`, `friendly-name-toggle`, `toggle-row-details` ostávajú);
  „(n hidden)“ = počet `kind === 'bg'` riadkov pod host filtrom (derived raz).
- SORT: radio `rows.sort` (`status` | `recent`), nový pref (UXPR-30).
- THEME: `SegmentedControl` (`options: auto/light/dark`, `testidPrefix="theme-"`, `onchange →
  applyTheme`); `Auto` label ukáže rozhodnutie: `Auto (dark)` z
  `matchMedia('(prefers-color-scheme: dark)')`. Testid `theme-toggle` ostáva na segmente.
- Menu nič nefiltruje — nikdy nie je „skrytý filter“, ktorý by prežil reštart bez povšimnutia
  (dôvod, prečo `needsYouOnly` nie je perzistovaný, `Sidebar.svelte:131-136`, ostáva).

### Pätička

```
│ [ + New session                                  ][ ▾ ]      │ 28 px (.btn--primary)
```

- Split tlačidlo: hlavná časť (`.btn.btn--primary`, testid `new-session-footer`) otvorí picker;
  šípka (`.btn--icon.btn--primary`, `IconCaret`, testid `new-session-menu`,
  `aria-haspopup="menu"`) otvorí menu **druhov**: `Background session…` (testid
  `new-bg-session-btn` → `NewBgSessionDialog` s hostom z pickera), `Add project…` (testid
  `add-project-row`, v hub režime `disabled` s krátkym `title`). Druhy v šípke, ciele v pickeri —
  nič sa nedubluje.
- `theme:` riadok zmizne (→ View menu). Výška: 1 + 5,6 + 28 + 7 = **≈ 42 px** (dnes 67).

### Picker (nad pätičkou, `ProjectPicker.svelte`)

```
┌────────────────────────────────────────────┐
│ [ Find project…            ] [claude-fleet-trn ▾] │
│ RECENT                                     │
│  papayapos-backend      4 · 12 min ago     │
│  kuk-agent              1 · 1 h ago        │
│ ALL                                        │
│  claude-fleet                              │
│  phone-manager                             │
│  pos-frontend                              │
└────────────────────────────────────────────┘
```

- Vyhľadávanie `fuzzyMatchFields` nad `owner/repo`; fokus po otvorení; ↑ ↓ Enter Escape.
- Host `<select>` predvolený z `pickerHost` → `last-host` (`NewSessionDialog.svelte:107`) → `local`;
  posiela sa ako `initialHost` (plumbing `dialogHost` už existuje, `Sidebar.svelte:413-422`).
- RECENT = prvých 5 podľa `last_session_at` (poradie zo store, `projects.rs:118-119`), s počtom
  živých sessions a `timeAgo`; ALL abecedne (dnešný `allProjectsSorted`). Systémový projekt
  vyradený ako dnes (`:347-352`, test `Sidebar.test.ts:572`).
- `role="listbox"` + `role="option"` + `aria-activedescendant` (dnes položky nemajú `option`).

### Vyhľadávanie (UXPR-31)

`searchQuery` ide ako predikát do `buildSessionsByProject` (fuzzy nad `friendly_name`,
`tmux_name`, `host_alias`, `worktree_key`, `claude_status`); projekt je viditeľný, ak (a) sa zhoduje
menom — vtedy so všetkými riadkami — alebo (b) má ≥ 1 zhodný riadok. Chord `/` v `App.svelte`
`onChordKeydown` vzore, iba keď cieľ nie je `input/textarea/[contenteditable]` ani terminál a Hosts
view nie je otvorené (to má vlastné `/`). QuickSwitcher ostáva na ⌘K/⌘P (skok), sidebar search
je filter — dve funkcie, jeden matcher.

### Rozpočet výšky po zmene

| | Dnes | Návrh | Δ |
|---|---|---|---|
| Hlavička | ≈ 134 px (280 px šírka: až 178) | ≈ 68 px (konštantne) | −66 … −110 |
| Pätička | ≈ 67 px | ≈ 42 px | −25 |
| Bulk lišta | +28 px vložených | 0 (swap) | −28 pri výbere |
| **Chrome** | **≈ 200 px** | **≈ 110 px** | **−90 px ≈ 2,5 riadku session** |
| Zoznam (panel 977 px) | ≈ 777 px | ≈ 867 px | +11,6 % |

### Perzistencia a výkon

- Kľúče **bez zmeny**: `recency`, `host-filter`, `show-bg-agents`, `show-friendly-names`,
  `rows.details`, `hints-seen`, `layout.sidebar*`, `cf:theme` (ostáva v `theme.ts`; nesťahovať do
  `prefs.ts` v tomto bloku). Nový: `rows.sort`. **Žiadna migrácia** — test v `prefs.test.ts`
  iba pripne, že päť existujúcich kľúčov číta rovnaké hodnoty.
- Tripwire `Sidebar.test.ts:952-1010` (`buildSessionsByProject` ≤ 2 volania po mounte): sort
  a search predikát idú **dovnútra** builderov (jeden prechod), `alias → count` a „n hidden“ sú
  samostatné `$derived` nad `$sessions` (O(n), raz), View menu je zatvorené (nič per-row), `<select>`
  má N + 1 možností, nie N riadkov. `rows.sort = status` číta `nowSec` (30 s tick) — `byTriage`
  posúva vek všetkých riadkov rovnako, poradie sa mení iba pri zmene bucketu, nie pod kurzorom.

## Hub režim

Chrome sidebaru nemá vlastnú „odmietnutú sekciu“, takže **žiadny `HubScopeNote`** (D5 hovorí
jeden na pohľad; spojenie už hlási `connectionBanner`, `Sidebar.svelte:376-381`). Per ovládač:

| Ovládač | Príkaz | Verdikt (`hub_verdicts.generated.json`) | V novej IA |
|---|---|---|---|
| `IconRefresh` | `refresh_projects`, `list_sessions` | Routed | bez zmeny |
| Host `<select>` | `list_hosts` | Routed | bez zmeny; skryté hosty vyradené |
| `IconSettings` | otvorí dialóg | — (obsah = UXPR-14/15) | bez zmeny; ⌘, |
| Tasks (pás záložiek) | `list_tasks`, `cancel_task` | Routed | `hubActionBlocked` na Cancel ako dnes (`TasksPanel.svelte:10-12`) |
| Bulk Send / Kill | `send_prompt`, `kill_session` | Routed | `hubActionBlocked` ako dnes (`SidebarFilters.svelte:12-15`) |
| `+ New session` | `new_session` | Routed | bez zmeny |
| `Background session…` | `new_bg_session` | Routed (`:89`) | bez zmeny; overiť, že `NewBgSessionDialog` používa `hubActionBlocked`, nie `hubBlock` (spojenie vs verdikt) |
| `Add project…` | `add_project` (+ `list_github_repos`) | **LocalOnly** (`:4,47`; UXPR-24 odložené) | položka v šípke `disabled`, `title` = `REASONS.add_project` (`hub.ts:193-194`) — jedna krátka veta, bez „on the hub“ (iter. 06 §tabuľka `:229`); `hub_disabled.test.ts:431-442` sa prepíše na klik na `new-session-menu` |
| Kôš (riadok projektu) | `purge_project` | **LocalOnly** (`:53`), T3 bez toolu (07) | **skryť** `{#if purgeProjectBlocked === null}` — už rozhodnuté (06 tabuľka `:94`, 07 `:113`, UXPR-23); UXPR-28/29 ho **nedotýkajú**, iba upozornenie na spoločný súbor |
| View menu, Theme, Select, `‹` | lokálne | — | bez rozdielu |
| OnboardingCard | `check_local_prereqs` LocalOnly | gate `setupBlocked` (`OnboardingCard.svelte:37`) | šošovka 19 |

`hub_inline_state` (UXPR-07) sa v chrome nepoužije; `E_FORBIDDEN` pre `readonly` klienta pri
`new_session` rieši `NewSessionDialog` (`:180-205` vzor) a UXPR-21 (`client_mode` vopred).

## PR plán

Všetko Svelte/TS, bez `cargo`, bez regen. Nová **lane F (sidebar)**: sekvenčná vnútri, paralelná
s lane B, C, D; závisí od lane A (`UXPR-01` ikony, **`UXPR-02` musí landnúť pred UXPR-28**, lebo
oba prepisujú `SidebarFilters.svelte` — 02 mení glyfy na `IconRefresh`/`.btn--icon`, 28 súbor
zásadne prestavia; v opačnom poradí sa diff 02 zahodí). Ak 02 mešká, alternatíva: 02 vypustí svoju
`SidebarFilters.svelte` časť (položky 6–8 iterácie 01 pre tento súbor) a 28 ju absorbuje — musí to
rozhodnúť kontrolór pri plánovaní lane A.

| UXPR | Názov | Súbory | Veľkosť | Závisí od | Paralelnosť |
|---|---|---|---|---|---|
| **28** | Sidebar chrome — dva riadky: host/čas `<select>`, chip pozornosti (`.btn--crit`, zatiaľ vždy), `IconSelect`, bulk swap (UX-93), `ViewMenu.svelte` (Show + Theme), Settings do riadku 1, Tasks → pás záložiek (`tasksOpen` v `app_views.ts`), theme riadok von z pätičky, `controls.css` triedy namiesto `.pill`/`.icon-btn`/`.theme-toggle` (UX-95), hint kotvy `host-filter`/`recency-filter` (UX-94), `title` hosta bez verzií (UX-89) | `SidebarFilters.svelte` (prepis ~200 → ~170), `ViewMenu.svelte` (nový ~110), `Sidebar.svelte` (−20: theme, Tasks modal von), `App.svelte` (+25: Tasks záložka + modal), `app_views.ts` (+5), `hints.ts` (+0, texty ±2) | **M ~250** (+~220 testov) | 01, **02 pred**; 23 (spoločný `Sidebar.svelte`, iný región — poradie 23 → 28 alebo 28 → 23, nie súbežne) | lane F začiatok; ∥ s B, C, D |
| **29** | Pätička — split `+ New session ▾`, `ProjectPicker.svelte` (search, host, Recent/All, `option` role), šípka: `Background session…` / `Add project…`, `⚡` preč, hint `bg-session` na šípku, `NewBgSessionDialog` filter skrytých hostov + `initialHost` (UX-96) | `Sidebar.svelte` (footer + picker von, −80/+40), `ProjectPicker.svelte` (nový ~140), `NewBgSessionDialog.svelte` (+10), `hints.ts` (text) | **M ~220** (+~200 testov) | 28 (spoločný `Sidebar.svelte`, `hints.ts`); mäkko 06 nie (picker ukazuje projekty, nie mená sessions) | → 28 |
| **30** | Pozornosť a poradie — chip skrytý pri `N === 0 && !needsYouOnly`, `byTriage` v `buildSessionsByProject` (per projekt), pref `rows.sort`, SORT radio vo View menu | `SidebarFilters.svelte` (+6), `sidebar_index.ts` (+20), `sessions.ts` (+6 pref), `ViewMenu.svelte` (+25), `Sidebar.svelte` (+4) | **S ~70** (+~120 testov) | 28 | → 28; ∥ s 29 (iné súbory okrem 4 riadkov `Sidebar.svelte` — nechať po 29) |
| **31** | Sémantika filtrov — čas na session (`last_activity_at`), search ako riadkový predikát cez `fuzzyMatchFields` (UX-87, 88), label `Active`, chord `/`, projekt viditeľný podľa riadkov | `sidebar_index.ts` (+30), `session_status.ts` (±15: `matchesRecency(s, r, now)`), `Sidebar.svelte` (−15/+15), `App.svelte` (+12 chord) | **S ~100** (+~130 testov) | 28, 30 (spoločný `sidebar_index.ts`) | → 30 |

Súčet lane F ≈ **640 riadkov** (+≈ 670 testov). Ak 28 po rebase na 02 prekročí 300, odštep
**28b** = `ViewMenu.svelte` + Theme + Tasks presun (~120), 28a = riadky 1–2 + bulk swap (~130).

**Testy, ktoré sa musia zmeniť** (pinnuté selektory): `Sidebar.test.ts:705,711,723` (`getByText('1d'/'7d')`,
`.recency .pill.active` → `select` hodnota, `fireEvent.change`), `:558` (`getByText('1d')`),
`:745-753` (`theme-toggle` je v otvorenom View menu), `:756-870` host pills → `select` options + `title`,
`:934-950` (`bg-toggle` v menu; `queryByText('🤖')` už mení UXPR-02), `:1086-1145` (chip
`needs-you-filter` po UXPR-30 iba pri N > 0 — test s `fine` a bez `stuck` očakáva `null`),
`:1148-1188` (`select-mode` ostáva; `bulk-bar` nahrádza riadok 2), `:539-620` (picker: `role="listbox"`
ostáva, `add-project-row` je v `new-session-menu`), `hub_disabled.test.ts:431-442` (klik na šípku),
`App.hosts.test.ts` (Tasks záložka, ak sa presunie). Testidy **nepremenúvať**.

**Poradie v lane F:** 02 → 28 → 29 → 30 → 31 (30 a 31 by mohli ísť pred 29, ak 29 čaká na rozhodnutie
otázky 4).

## Akceptačné testy

Nový `src/lib/SidebarFilters.test.ts` (dnes neexistuje) + `ViewMenu.test.ts`, `ProjectPicker.test.ts`,
doplnky v `Sidebar.test.ts`, `sidebar_index.test.ts`, `session_status.test.ts`, nový `prefs.test.ts` (dnes neexistuje):

1. **Výška je konštantná** (`SidebarFilters.test.ts`): hlavička renderuje presne dva `.row` deti pri
   1, 5 a 12 hostoch a pri 0 aj 7 „needs you“; host `<select>` má `hosts.filter(!hidden).length + 1`
   možností; žiadny element s `flex-wrap` (jsdom: `getComputedStyle` nie, tak asserovať štruktúru —
   `querySelectorAll('.sidebar-header > *').length === 2` + `loadError`).
2. **Host select** ukazuje počet a offline: `all (3)`, `mefistos (2)`, `mac (1) · offline`; zmena
   `change` → `hostFilter` = alias; hodnota z `localStorage` `host-filter` sa hydratuje (prepis testu
   `:777-800`).
3. **Chip pozornosti** (UXPR-30): `N === 0 && !needsYouOnly` → `queryByTestId('needs-you-filter')`
   je `null`; `N === 1` → text `1`, `aria-pressed=false`; po kliku filter; keď počas filtra klesne N
   na 0, chip ostáva (`aria-pressed=true`) a druhý klik ho odstráni.
4. **Bulk swap** (UX-93): pri `selectedCount > 0` chýbajú `host`/`active` selecty a je `bulk-bar`;
   po `clear` sú späť; `select-mode` `aria-pressed` ostáva.
5. **View menu**: zatvorené po mounte (`queryByTestId('view-menu') === null`); klik `View` →
   otvorené, fokus na prvom checkboxe; Escape zatvorí a vráti fokus na tlačidlo; checkbox
   `bg-toggle` prepne `showBgAgents` a zapíše `cf:pref:show-bg-agents`; „(n hidden)“ = počet bg pod
   host filtrom; Theme segment `theme-dark` → `applyTheme('dark')`, `documentElement[data-theme]`.
6. **Status sort** (`sidebar_index.test.ts`): `buildSessionsByProject(rows, 'all', true, null,
   { sort: 'status', opts })` vráti v projekte `waiting, stuck, failed, lifecycle, idle_long,
   working, idle`, tie by id; `sort: 'recent'` = poradie vstupu; tripwire `:952-1010` ostáva
   ≤ 2 volaní.
7. **Split tlačidlo a picker** (`ProjectPicker.test.ts`, `Sidebar.test.ts`): `new-session-footer`
   otvorí picker s fokusom v `picker-search`; písanie `pap` zúži na `papayapos-backend`; RECENT má
   ≤ 5 položiek zoradených podľa `last_session_at` desc (fixture `fakeProjects`), ALL abecedne, bez
   `system` projektu (`:572` ostáva); host select predvolený z `pickerHost` a odovzdaný ako
   `initialHost` (rozšíriť `:652`); `new-session-menu` → `new-bg-session-btn` otvorí
   `NewBgSessionDialog` s hostom z pickera, `add-project-row` `disabled` v hub režime s `title`
   (prepis `hub_disabled.test.ts:431-442`); `⚡` (`new-bg-session-btn` mimo menu) neexistuje.
8. **Hidden hosty** (UX-96): `NewBgSessionDialog` neponúka `hidden: true` host.
9. **Sémantika času** (`session_status.test.ts`, UXPR-31): `matchesRecency(session, '8h', now)`
   podľa `last_activity_at`; projekt s 9 h a 1 h session pod `8h` zobrazí **iba** 1 h riadok; pod
   `all` oba; picker ďalej ukazuje všetky projekty (`:550` ostáva).
10. **Search po riadkoch** (UX-88): `violet` → `claude-fleet` s **jedným** riadkom; `claude-fleet` →
    projekt so všetkými; `blue mef` fuzzy nad hostom + menom; `/` mimo inputu fokusuje
    `sidebar-search`, `/` v textarea nie.
11. **Hinty** (UX-94): `hint-bubble[data-hint-id="host-filter"]` sa ukotví na host select pri ≥ 2
    hostoch; `bg-session` na `new-session-menu`; `hints-seen` id nezmenené.
12. **Prefs bez migrácie** (nový `prefs.test.ts`): starý `localStorage` so šiestimi kľúčmi → rovnaké
    hodnoty po mounte; nový `rows.sort` default `status`.
13. **AX**: `View` má `aria-haspopup`/`aria-expanded`; šípka `aria-haspopup="menu"`; picker položky
    `role="option"`, `aria-selected`; Tasks/Settings nie sú vnútri `nav[aria-label="host filter"]`.
14. `svelte-check` bez nových varovaní; celý `npx vitest run` (nie iba filtrované súbory — memory
    „full suite per task“).

Východiskový stav: `npx vitest run src/lib/Sidebar.test.ts` spustený počas tejto iterácie — **81 / 81 passed**,
1 súbor (log v scratchpade); suite je pred zmenou zelená, v súlade s CLAUDE.md „no known pre-existing frontend test
failures“).

## Odhad

| Položka | Veľkosť | Riadky (bez testov) | Testy |
|---|---|---|---|
| UXPR-28 | M | ~250 (odštep 28a/28b pripravený) | ~220 |
| UXPR-29 | M | ~220 | ~200 |
| UXPR-30 | S | ~70 | ~120 |
| UXPR-31 | S | ~100 | ~130 |
| **Lane F spolu** | | **~640** | **~670** |
| Výška chrome | | 200 → 110 px (−45 %) | |
| Nové komponenty | | `ViewMenu.svelte`, `ProjectPicker.svelte` | |
| Nové prefs | | `rows.sort` | |
| Zmenené pinnuté testy | | ~14 miest v `Sidebar.test.ts`, 2 v `hub_disabled.test.ts` | |

## Otázky pre vlastníka

Iba nové; každá s odporúčaním („default“ = prijať odporúčania).

1. **Host filter — vždy `<select>`, alebo `SegmentedControl` pri ≤ 3 hostoch?** Segment je krajší
   pri širokom sidebari, ale dve správania = dva testy a výška závislá od počtu hostov.
   *Odporúčanie: vždy `<select>` — jedno pravidlo, natívna klávesnica a AX vo WKWebView.*
2. **Časový filter na session (`last_activity_at`) namiesto projektu (`last_session_at`)?** Mení, čo
   `8h` zobrazí (UX-87); `all` je identické. *Odporúčanie: session; label `Active: 8h`.*
3. **Chord `/` na fokus vyhľadávania** (rovnako ako Hosts view), keď fokus nie je v editovateľnom
   prvku ani termináli? ⌘F patrí Conversation find (`ConversationPanel.svelte:611`).
   *Odporúčanie: áno, `/`.*
4. **Tasks do pásu záložiek vedľa `Hosts ⌘I`** (zasahuje `App.svelte`, šošovka 13), alebo do View
   menu ako sekcia *Open*? *Odporúčanie: pás záložiek — Tasks je fleet-wide pohľad ako Hosts;
   chord neprideľovať (šošovka 16).*
5. **Chip pozornosti skrytý pri nule** odoberá jediný prepínač, ktorý pri N = 0 ukazoval `idle_long`
   riadky (počítadlo ich zámerne nezapočítava). Status sort (UXPR-30) ich zdvihne nad `idle` v
   každom projekte; vek idle v chipe riadku patrí šošovke 9. *Odporúčanie: skryť; prijať.*
6. **Theme do View menu** (a neskôr aj do Settings › Appearance, ak ju šošovka 3/18 zavedie — jeden
   store `theme.ts`)? *Odporúčanie: View menu; `Auto (dark)` ukáže rozhodnutie.*
7. **Má `Background agents` off skrývať aj `bg:<uuid>` riadky v *Outside fleet*?** Dnes nie
   (`sidebar_index.ts:53-61`), čo mätie (UX-91). *Odporúčanie: nie — Outside fleet je vlastná
   zbaliteľná sekcia; iba label „Background agents“ + „(n hidden)“ a default `outside-fleet-open`
   ostáva `false`.*
8. **Default `rows.sort`: `status` alebo `recent`?** `recent` = dnešné správanie. *Odporúčanie:
   `status` — FE-4 a P13 tak boli zamýšľané; `recent` ostáva voľbou v menu.*

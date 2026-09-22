# UX audit claude-fleet — analýza a príprava na iterácie

**Dátum:** 2026-09-21 · **Verzia:** v0.2.33 (`46276c28`) · **Režim:** desktop
spárovaný s hubom (`hub: https://fleet.rlt.sk (as mac-desktop)`), téma dark,
okno 1728×1027.

Tento dokument je vstup pre sériu ~20 iterácií „UX expertov“ (sekcia 5). Každá
iterácia si má prečítať tento súbor, pozrieť screenshoty v tomto adresári a
priložené `file:line` odkazy, a až potom navrhovať zmeny.

## 1. Screenshoty

Screenshoty nie sú v repozitári (zachytávajú živú fleetu); ostali lokálne vo
worktree `feature/ux-errors-analysis-prep-f5c1e7`. Tabuľka nižšie popisuje, čo
zachytávali, a dá sa zopakovať postupom pod ňou.

| Súbor | Čo ukazuje |
|---|---|
| `01-session-terminal.jpg` | Hlavný pohľad: sidebar + Session/Terminal, otvorený Agent panel „The agent is not running“ |
| `02-files-tab.jpg` | Tab Files (Changed / All files / History / Branches), commit box |
| `03-hosts-view.jpg` | Hosts view (**⌘I** / Ctrl+Shift+H; iter. 08): účty, hosty, detail hosta `claude-fleet-trn` |
| `04-settings.jpg` | Settings dialóg v hub režime (odscrollované na spodok) |
| `05-agent-panel.jpg` | Agent panel po kliku na FAB — composer s kontextovým chipom |
| `06-conversation.jpg` | Conversation pohľad s hlavičkou a quick-action chipmi |

Postup zachytenia (opakovateľný, appka nemusí byť v popredí):

```bash
# window id z computer-use app_list_windows (tu 900), potom:
screencapture -x -l 900 out.png
sips -s format jpeg -s formatOptions 70 --resampleWidth 1800 out.png --out out.jpg
```

## 2. Nájdené chyby a slabé miesta

Závažnosť: **C** kritické (blokuje/zavádza), **H** vysoká, **M** stredná, **L** nízka.
Odkazy sú relatívne ku koreňu repa.

### Ikony a vizuálny jazyk

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-01 | H | **Refresh ikona je textový glyf `↻`** (Unicode), pri načítaní sa mení na `…`. Ten istý glyf `↻` znamená v appke tri rôzne veci: *Refresh* (sidebar, Files), *Restart claude* (riadok session, detail) a *reconnect* (terminál). | `src/lib/SidebarFilters.svelte:74-76`, `src/lib/FilesPanel.svelte:301`, `src/lib/SessionRowItem.svelte:312-314`, `src/lib/TerminalView.svelte:1112`, `src/lib/SessionDetails.svelte:627` |
| UX-02 | H | **Recreate = `♻` (recyklačný emoji)**, Edit label = `🏷`, Rename tmux = `✎`. Hover akcie riadku sú `↻ 🏷 ✎ ♻ ×` — päť glyfov z rôznych fontov, žiadny nie je čitateľný bez tooltipu. Screenshot 01, vybraný riadok `yes`. | `src/lib/SessionRowItem.svelte:312-350`, `src/lib/SessionDetails.svelte:655` |
| UX-03 | M | Emoji ako ikonografia naprieč appkou: `🤖` bg, `🔍` review, `▶` shell, `⚡` nová bg session, `✦` FAB, `⚠ Needs you`, `🏷 friendly on`, `≡ details on`. Render závisí od fontu OS, farby sa nedajú tematizovať. | `src/lib/SessionRowItem.svelte:266-272`, `src/lib/Sidebar.svelte:790`, `src/lib/AgentFab.svelte` |
| UX-04 | L | Kôš (purge project) otvára `ConfirmDialog`, nie je na jeden klik; trvalo viditeľný je iba v hub režime, lebo `disabled` prebije `opacity: 0` (iter. 01, ↔ UX-31). | `src/lib/Sidebar.svelte` (hlavička skupiny projektu) |

### Pomenovanie sessions

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-05 | C | **Sessions sa volajú `yes`, `clear`, `go`, `push`.** Po každom prompte sa nepomenovanej session (alebo session s default menom) nastaví friendly name z prvých 5 slov promptu — bez ohľadu na to, či je to odpoveď („yes“), slash príkaz („/clear“ → „clear“) alebo jednoslovný povel. Quick-action chipy v Conversation (Shift+klik pošle `/clear`) tak premenujú session na „clear“. V Hosts detaile je 11 sessions, z toho 5 sa volá `yes`/`clear`. | `crates/fleet-core/src/service/sessions/prompt.rs:110-127` (derivácia), `:176-190` (kedy sa prepíše), `src/lib/composer_presets.ts:14` |
| UX-06 | H | **Sekcia „Outside fleet“ a Hosts detail ukazujú surové `bg:<uuid>`** (`bg:de870cc6-009a-46d1-…`) ako názov riadku. Sú to `kind='external'` riadky (agenti bez tmux nároku), nie operátor — ten je session „fleet operator“ (iter. 02). | screenshot 03 a 01; `src/lib/Sidebar.svelte:755-770` |
| UX-07 | M | Meno tmux session (`dev-martin-janci-kuk-agent--main`) sa opakuje 3× na jednej obrazovke: riadok v sidebare, hlavička terminálu, tmux status bar. | screenshot 01 |

### Sidebar a riadok session

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-08 | H | **Štyri riadky filter-chipov** nad zoznamom (hosty · čas `all 8h 1d 3d 7d 30d` · `Needs you (0)`/`select` · `bg off`/`friendly on`/`details on`) zaberajú 91 px, celá hlavička 134 px, chrome s pätičkou ≈ 200 px (20 % panelu); miešajú filtre s prepínačmi zobrazenia. `Needs you (0)` svieti glyfom `⚠`, nie farbou, aj keď je 0 (iter. 08). | `src/lib/SidebarFilters.svelte` |
| UX-09 | M | Riadok session nesie 11 dát bez vizuálnej hierarchie, z toho 3 prázdne alebo mŕtve (effort, elapsed pre tmux-objavené, `status` bodka — iter. 09, stavový model D9): host chip, tmux name, čas `8h 24m` (nie je jasné čo — vek? posledná aktivita?), `53 %` chip (kontext, bez legendy; 100 % červené), `$4.28`, `PR↗`, `✓ CI`, posledný prompt. | `src/lib/SessionRowItem.svelte:360-420` |
| UX-10 | M | **Select mode**: bulk lišta existuje (`bulk-bar`), ale iba pri `selectedCount > 0`; zapnutý mód bez výberu nemá hint (UX-112). Checkbox je `<input>`, auditový AX klik zlyhal na WKWebView bridgingu (iter. 10). | screenshot 01 vs. `select` chip; `src/lib/Sidebar.svelte` |
| UX-11 | L | `theme: dark` je textové tlačidlo v pätičke sidebaru; cyklovanie auto/light/dark bez ikony a bez indikácie, že je to klikateľné. | `src/lib/Sidebar.svelte:791-799` |
| UX-12 | L | Nová session: `+ New session` otvorí popover s plochým zoznamom projektov (bez hosta, bez vyhľadávania); `⚡` vedľa neho spúšťa bg session — dve tlačidlá pre „vytvor“ bez spoločnej logiky. | `src/lib/Sidebar.svelte:775-830` |

### Agent (FAB + panel)

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-13 | H | **Panel tvrdí „The agent is not running“ / „Wake the agent“**, hoci operátor beží (v Outside fleet svieti `bg:… working`). Po kliku na FAB sa ukáže composer — stav sa teda medzi dvoma otvoreniami zmenil bez vysvetlenia. Iter. 02: operátor na screenshote 01/03 nebežal, panel hlásil pravdu; závažnosť prehodnotí šošovka 11; iter. 07 sa panelu nedotkla. | screenshot 01 vs. 05; `src/lib/operator.ts:84-90`, `refreshOperator` |
| UX-14 | H | Panel je ~300×110 px v pravom dolnom rohu, `position: fixed`, prekrýva terminál a tmux status bar; FAB prekrýva stavový riadok appky (`usage … resets Fri 11:00`). Konverzácia s agentom sa v ňom nedá čítať. | `src/lib/AgentPanel.svelte:173-175`, `src/lib/AgentFab.svelte:50-52` |
| UX-15 | M | Kontextový chip `yes · claude-fleet-trn ×` preberá auto-meno z UX-05, takže agent dostane kontext „yes“. | screenshot 05 |

### Hub-client režim: chýbajúca parita s desktopom (to je „viac funkcií, ktoré má desktopová aplikácia“)

70 zo 130 príkazov je v hub režime `LocalOnly` (podľa `src/lib/hub_verdicts.generated.json`; po konsolidácii-01 cieľ 51, po konsolidácii-02 **42**, 40 s odloženým UXPR-24). UI to rieši tak, že na miesto funkcie vloží vetu odmietnutia.

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-16 | C | **Settings v hub režime** opakuje 3× ten istý odsek („these settings drive the reconcile tick, the GC sweeper and the playbooks, which the hub runs and this app does not. Do it on the hub“) pod *Projects*, *Automation*, *Limits*; *Control API (MCP)* má dve ďalšie vety odmietnutia. Používateľ nedostane ani odkaz, ani read-only hodnoty. | screenshot 04; `src/lib/SettingsDialog.svelte:85`, verdikty `get_fleet_settings`, `set_fleet_setting`, `mcp_status`, `mcp_configure`, `provision_hosts` |
| UX-17 | H | **Assets tab** je jedna strana prózy („the asset catalog is a git checkout on the machine that owns the fleet…“) — hub servíruje iba definície toolov, katalóg nikdy nenačíta (UX-49, iter. 04); 5 príkazov je routovateľných na existujúce tools. | `src/lib/AssetsPanel.svelte:72-80,216`; verdikty `catalog_*` |
| UX-18 | H | **Files tab**: git zápisy (`repo_checkout`, `repo_create_branch`, `repo_stage`, `repo_commit_create`, `repo_fetch/pull/push`, 10 príkazov) sú LocalOnly; `instead` text (`NO_GIT_WRITE_TOOL`) sa zobrazí iba v tooltipe, nič nezlyhá, gate je pre-emptívny (UX-59, iter. 05). Commit box „Commit 0 files“ bez vysvetlenia. | screenshot 02; `verdicts.rs` (repo_* bez `instead`) |
| UX-19 | M | **Hosts detail**: sekcia *Integration → Token* je jedna veta odmietnutia inline; *Danger* tlačidlá sú disabled s dôvodom iba v `title` (dôvod `remove_host` sa použil aj pre token-mode a Rotate — UX-72); *Token* riadok vypíše vetu odmietnutia ako hodnotu. Koreň chýbajúceho usage je UX-69 (H): hub má `UsageCache`, chýba iba tool `list_account_usage` (iter. 06). | screenshot 03; `src/lib/HostsView.svelte:339-343` |
| UX-20 | M | **Projekty**: `add_project`, `list_github_repos` LocalOnly → „＋ Add project…“ v pickeri je disabled iba s tooltipom. | `src/lib/Sidebar.svelte:366,809-813` |
| UX-21 | H | Sessions: Safe remove je v hub režime nedostupný celý, aj routovaná cesta `safe_kill_session` (UX-77); `session_tool_detail` padá per riadok; živý indikátor ticho vypnutý; `discard_kill_session`, `session_activity`, `dismiss_agent_session`, `purge_project` LocalOnly (iter. 07). | `verdicts.rs` |

### Ostatné pohľady

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-22 | M | **Conversation hlavička** má 10 prvkov v jednom riadku: `Current · /resume · 11 turns · 239k/1M · 24% · opus-4-8 · working · /resume 22 · 🔍 · 6 turns ▾ · 4 background ▾`. | screenshot 06 |
| UX-23 | M | **Settings dialóg** je jedna dlhá scrollovaná strana bez záložiek, s dlhými odsekmi prózy pri každom nastavení (natívny `<dialog>` + `showModal()` je centrovaný — iter. 03). | screenshot 04; `src/lib/SettingsDialog.svelte`, `src/lib/settings_dialog.css` |
| UX-24 | M | **Update toast** `✓ Update installed · Restart to update` sa vykreslí ako text nad terminálom, bez tlačidla Restart, a zostane visieť. | screenshot 05 (vpravo dole nad tmux barom) |
| UX-25 | L | Pravý horný roh: `Hosts ⌘1` a `↻ reconnect` sú nalepené na okraj okna/scrollbar (`center-wrap` má `overflow:auto; padding-right:1rem`). | `src/App.svelte:934-939` |
| UX-26 | L | Files: `Loading…` bez skeletu, prázdny stav „Select a file to view it.“ bez CTA. | screenshot 02 |
| UX-27 | L | Prázdny stav Hosts: `Filter hosts /` – lomka ako hint na skratku bez vysvetlenia. Legenda skratiek je skrytá za `?`; `?` v stĺpci účtov znamená „usage neznáme“ — dva významy jedného glyfu (iter. 06, UX-68). | `src/lib/HostsView.svelte:357+` |
| UX-28 | M | **Prístupnosť**: AX strom okna vracia 4 elementy (3 tlačidlá okna + WebArea). Klik cez accessibility na taby/riadky nefunguje, treba raw input; prvý klik po zatvorení modalu sa stratí. Overiť s VoiceOver; pravdepodobne chýba `role`/`aria-*` na vlastných komponentoch a WKWebView AX bridging. | pozorované počas auditu (computer-use `app_click` → „accessibility action unavailable“) |

## 3. Root cause klastre (na čo mieriť fixy)

1. **Žiadny ikonový systém.** Glyfy a emoji ad hoc → UX-01…04, 11. Fix: jeden SVG
   sprite (napr. Lucide), komponent `<Icon name>`, mapa významov
   *refresh ≠ restart ≠ recreate ≠ reconnect*.
2. **Auto-pomenovanie z promptu bez filtra.** UX-05, 06, 15. Fix v
   `prompt.rs`: ignorovať slash príkazy, stop-list (`yes|no|ok|go|y|n|continue|push|…`),
   minimálne 3 slová, pomenovať len z *prvého* promptu; operátor a bg sessions
   dostanú default z `kind`/projektu, nie `bg:<uuid>`.
3. **Hub režim rieši paritu textom, nie funkciou.** UX-16…21. Buď hub dostane
   tool (parita), alebo UI prvok zmizne/skryje sa za read-only hodnotu — nikdy
   opakovaný odsek. Kandidáti na routing: `get_fleet_settings` (read-only),
   `list_account_usage`, `catalog_list_*`/`plan_sync`/`scan_assets` (hub má tools,
   ale katalóg musí najprv načítať — UXPR-08), `repo_*` čítania už idú; zápisy
   idú podľa prístupovej politiky T0–T3 v `iterations/consolidation-01.md` §2 D3;
   hub tools pre sessions a hosty v `consolidation-02.md` §2 D11
   (`discard_kill_session` `confirm: true` je jediná výnimka).
4. **Hustota bez hierarchie.** UX-08, 09, 22, 23. Fix: primárne/sekundárne
   metadáta, stavový model `StatusChip` (consolidation-02 D9), `rows.details`
   off = 2 linky / on = + prompt (D14), Settings s ľavou navigáciou.
5. **Plávajúce vrstvy bez layout kontraktu.** UX-13, 14, 24, 25. Fix: agent ako
   pravý drawer/split, toasty v jednom stacku, update toast s akciou.

## 4. Čo už je zdokumentované inde (neduplikovať)

- `docs/plans/2026-09-10-fleet-improvement-plan.md` FE-4 (triage, bulk
  actions — bulk kill/prompt landed, status sort nie (UX-92, UXPR-30);
  klávesnica a potvrdenie s rizikom v UXPR-36/37), FE-8 (modaly bez focus
  trapu), PROD-4 (friendly names) — UX-05 je
  regresný dôsledok fixu PROD-4/PROD-5.
- `docs/superpowers/specs/2026-09-20-ux-agent-fab-design.md` — FAB a panel sú
  „slice 1“; UX-13/14 sú pripomienky k implementácii, nie k dizajnu.
- `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md` — telefónny klient
  má rovnaký problém s paritou (UX-16…21) a zdedí fix na hub strane pre Settings
  a git; Assets UI v mobile spece nie je (UX-55).

## 5. Príprava: 20 iterácií UX expertov

**Protokol jednej iterácie** (spúšťa sa ako samostatný subagent, read-only voči
appke, môže čítať kód a screenshoty):

1. Vstup: tento README, screenshoty, jedna *šošovka* (nižšie), ID nálezov, ktoré
   ju zasahujú.
2. Výstup: `docs/ux/2026-09-21-audit/iterations/NN-<slug>.md` s: (a) potvrdené /
   vyvrátené nálezy, (b) nové nálezy s ID `UX-NN+`, (c) návrh riešenia s
   `file:line`, (d) návrh akceptačného testu (Vitest/`svelte-check`), (e) odhad
   S/M/L. Žiadne zmeny kódu.
3. Po každých 4–5 iteráciách jeden **konsolidačný krok**: dedup, priorita,
   rozpad na PR (jeden klaster = jeden PR, max ~300 riadkov diffu).
4. Nový screenshot sa robí iba pri zmene UI (postup v sekcii 1).

**Šošovky (poradie podľa hodnoty):**

| # | Šošovka | Zasahuje | Očakávaný výstup |
|---|---|---|---|
| 1 | Ikonový systém a vizuálny jazyk | UX-01…04, 11 | mapa glyf → význam → ikona; výber knižnice |
| 2 | Pomenovanie sessions a identita | UX-05, 06, 07, 15 | pravidlá auto-mena, testy pre `friendly_name_from_prompt` |
| 3 | Hub parita: Settings | UX-16, 23 | ktoré nastavenia read-only zo hubu, ktoré skryť |
| 4 | Hub parita: Assets/Catalog | UX-17 | read-only katalóg z `list_assets`/`list_layers` |
| 5 | Hub parita: Files/git | UX-18 | tabuľka repo_* → route/hide, návrh hub tools |
| 6 | Hub parita: Hosts, účty, projekty | UX-19, 20 | **hotové** → `iterations/06-hub-parity-hosts-accounts.md` (UXPR-21–24) |
| 7 | Hub parita: sessions (safe-kill, tool detail) | UX-21 | **hotové** → `07-hub-parity-sessions.md` (UXPR-25–27, ADR 0003) |
| 8 | Sidebar filter chrome / IA | UX-08, UX-12 | **hotové** → `08-sidebar-ia.md` (UXPR-28–31) |
| 9 | Riadok session: hierarchia metadát | UX-09 | **hotové** → `09-session-row.md` (UXPR-32–35; `StatusChip` D9 nahrádza UXPR-19) |
| 10 | Select mode a hromadné akcie | UX-10 | **hotové** → `10-select-mode-bulk-actions.md` (UXPR-36–38) |
| 11 | Agent panel a FAB | UX-13, 14, 15 | drawer layout, stavový model panelu, slice 2 confirmation channel (UX-84/120), operátor v sidebare |
| 12 | Conversation hlavička a composer | UX-22 | zoskupenie, overflow menu |
| 13 | Terminál chrome a tmux bar | UX-07, 25 | čo skryť, čo zdvojené |
| 14 | Prázdne, loading a chybové stavy | UX-26, 27, 18 | skeletony, CTA; prevziať `HubScopeNote` + `hub_inline_state` (consolidation-01 D5) |
| 15 | Toasty, notifikácie, update flow | UX-24 | stack, akcie, trvanie, bulk súhrnný toast a zoskupenie N hub toastov (UXPR-37) |
| 16 | Klávesnica a discoverability skratiek | UX-27 | cheat-sheet, tooltipy so skratkou, `/`, ⌘A, Space, dvojkrokový Escape (iter. 08/10), `?` legenda vs. `?` stĺpec |
| 17 | Prístupnosť (AX strom, fokus, kontrast) | UX-28 | audit `role`/`aria`, focus trap modalov |
| 18 | Light a auto téma | – | screenshoty vo light, kontrast chipov |
| 19 | Onboarding a prvé spustenie | – | Welcome/Onboarding card vs. hub režim |
| 20 | Parita telefónneho klienta | UX-16…21 | čo z hub parity zdedí `fleet-mobile` |

**Rozhodnutia (schválené 2026-09-21):** (a) ikonová knižnica **Lucide**
(`@lucide/svelte` — `lucide-svelte` je deprecated; subpath importy, sémantický
`src/lib/icons.ts`, žiadny `<Icon name>`), (b) hub parita = **nové hub tools**
(route, nie skrývať), (c) iterácie bežia **postupne v tomto worktree**, jeden
subagent naraz, výstup do `iterations/`, (d) rozpočet MCP popisov
má **dva stropy** — `AGENT_BUDGET_BYTES` tvrdý 51 000 (plocha per-host tokenu)
a `MASTER_BUDGET_BYTES` mäkký 65 500 — nastavené **raz v `UXPR-25`** (ADR 0003
`Visibility::ClientOnly`, pred UXPR-08/09); parity tools sú `ClientOnly`;
ďalšie PR konštanty nemenia (jediná známa výnimka `UXPR-24` → 66 800)
(consolidation-02 D7), (e) prístup hub tools: T0 readonly · T1 full · T2
full+trusted (`set_fleet_setting`, `repo_push`) · T3 master; `confirm: false`
na hube s jednou výnimkou: skladaný tool nesmie obísť `confirm` bránu svojich
súčastí (`discard_kill_session`, consolidation-02 D11).

**Konsolidácia po iteráciách 1–5:** `iterations/consolidation-01.md` (register
UX-01…67, fronta UXPR-01…20, 28 otázok — vlastník 2026-09-22 prijal všetky
odporúčania („default“)).

**Konsolidácia po iteráciách 6–10:** `iterations/consolidation-02.md` (register
UX-68…123, D7–D14, zlúčená fronta UXPR-01…38 — 35 hlavná + 1 odložený, 23
otázok Q-II s odporúčaniami).

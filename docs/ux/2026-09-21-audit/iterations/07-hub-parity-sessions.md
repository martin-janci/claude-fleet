# Iterácia 07 — Hub parita: operácie so session a viditeľnosť toolov pre agenta

**Šošovka:** hub-client parita pri operáciách so session — Safe remove (náhľad + tri cesty),
Discard & kill, detail tool-callu v Conversation tabe, živý indikátor (`session_activity`),
Remove from list neaktívneho agenta, Repair workspace, ghost riadky, Transfer sheet — a otázka
`Visibility::ClientOnly` (konsolidácia-01 §5: „ADR po šošovke 7“) · **Zasahuje:** UX-21,
root-cause klaster 3 („hub režim odpovedá prózou namiesto funkcie“), rodina UX-42 (`instead`
ukazuje do prázdna) · **Vstup:** `docs/ux/2026-09-21-audit/README.md`,
`iterations/consolidation-01.md` (záväzné D1–D6, §4 „default“, §5 položka `Visibility::ClientOnly`),
`03-hub-parity-settings.md` §Recept (kroky 1–14, tu iba odkazované), `05-…` a `06-…` (formát
matice, stav rozpočtu), screenshoty 01 a 06, `docs/hub.md`, `docs/control-api.md`,
`docs/specs/2026-09-20-mcp-token-efficiency.md`, ADR 0001/0002, kód na `5975afa3` · **Režim:**
read-only review, žiadne zmeny kódu, žiadny `cargo` beh, `npx vitest` nespúšťaný (čísla testov sú z
čítania). Nálezy od **UX-77**, PR od **UXPR-25**.

Platia rozhodnutia konsolidácie-01: D3 rebrík T0 readonly · T1 full · T2 full+trusted · T3
master, `confirm: false` na hube (tu jedna odôvodnená výnimka — otázka 1); D4 `BUDGET_BYTES` sa
zdvihol raz v UXPR-09 na 63 800 a iterácia 06 už spotrebovala oba rezervné trimy (meranie ≈ 63 430);
D5 `HubScopeNote` + `hub_inline_state` z UXPR-07; D6 pravidlá routing PR.

## Zhrnutie

1. **UX-21 potvrdené a ostrejšie: Safe remove je v hub režime nedostupný celý — aj tá jeho
   cesta, ktorá už routuje.** Tlačidlo *⏏ Safe remove* je disabled cez `hubBlock('inspect_safe_kill')`
   (`src/lib/SessionDetails.svelte:695-704`), dialóg sa nikdy neotvorí, a preto sa nedá kliknúť ani
   na *Let Claude commit it* (`safe_kill_session`, verdikt `Routed`, `verdicts.rs:165-170`,
   policy T1 `guard.rs:314-320`). Test `hub_disabled.test.ts:379-388` tento stav pinuje ako
   správny. Používateľ na hube má iba *Kill session* — naslepo, bez náhľadu špinavých súborov a
   nepushnutých commitov (UX-77, **H**).
2. **Premisa zadania „hubov `safe_kill_session` už počíta tie isté fakty“ neplatí.**
   `safe_kill::safe_kill_session` (`service/safe_kill.rs:135-219`) iba pošle prompt s nonce a
   nastaví `safe_kill_state=requested`; git kontrola beží až po markeri READY v
   `finalize_safe_kill` (`:616-660`: `git status --porcelain` → `git worktree remove`). Dry-run
   z toho vytiahnuť nejde — `inspect_safe_kill` je samostatná funkcia (`:221-338`, jeden SSH
   round-trip: porcelain, branch, upstream, ahead) a na hube potrebuje **vlastný T0 tool**.
3. **Discard & kill je T1, nie T2/T3:** skladá `git worktree remove --force` + `kill_session`
   (`safe_kill.rs:362-449`), teda presne to, čo už dnes klient smie cez `delete_worktree`
   (`Access::Client, confirm: true`, `guard.rs:543-548`) a `kill_session` (`:308-313`). Dosah je
   jedna session a jej worktree — D3 kritérium T1. Jediná odchýlka od D3: **`confirm: true`**,
   lebo skladaný tool nesmie obísť `mcp.confirm_destructive` bránu svojich súčastí (otázka 1).
4. **`session_tool_detail` potrebuje ID-adresované čítanie:** `Conversation` (`transcript.rs:495-508`)
   vstupy a výsledky nikdy nenesie („tool inputs and results are never included“,
   `orchestration.rs:77-88`); `ToolDetail` sa grepuje z transkriptu na požiadanie
   (`fetch_tool_detail`, `:1416-1439`, ≤ 2 MiB, každé pole ≤ 8 000 znakov). Nový T0 tool
   `session_tool_detail { session_id, tool_use_id, claude_session_id? }`; `ToolDetail` a
   `EditDetail` derivujú iba `Serialize` (`:1289-1310`) → `Deserialize` + kontrakt.
5. **`session_activity` je RE za 0 B:** `probe_from_tail` je čistá funkcia nad 12 riadkami pane
   (`activity.rs:16,34-43`), hub má `capture_session` (T0, `guard.rs:265-270`, vracia text —
   `route_text` už používa `delete_worktree`, `commands/worktrees.rs:89`). Verdikt
   `Routed { tool: "capture_session" }`, `routed::session_activity` = `route_text` + lokálny
   `probe_from_tail`. Veta „the hub's pane reads answer a different shape“ (`verdicts.rs:260-267`)
   je pravdivá o tvare, nie o obsahu — tvar sa prevedie na desktope.
6. **`dismiss_agent_session` ≡ `kill_session` na neaktívnom agentovi — overené:**
   `bg_kill_action` vráti `Dismiss` pre `claude_status == "stopped"` (`service/sessions/lifecycle.rs:816-818`)
   a kill vykoná `s.dismiss_agent(...)` (`:917-925`) — tú istú store funkciu ako
   `bg_sessions::dismiss_agent_session` (`:292-322`). Rozdiel je iba na *working* agentovi
   (dismiss odmietne `E_INVALID_STATE "stop the agent first"`, kill ho zastaví). UI ale Remove
   ponúka **iba** pre `isInactiveAgent` (`sessions.ts:564-566`) a Kill pre neaktívneho **skrýva**
   (`SessionRowItem.svelte:341-350`) — takže `instead` „use Kill instead“ ukazuje na tlačidlo, ktoré
   pre ten riadok neexistuje (UX-82). Návrh: RN `dismiss_agent_session { session_id }` T1 (~280 B,
   pod ADR `ClientOnly` 0 B pre agenta), alternatíva RE cez `kill_session` — otázka 2.
7. **Repair je routovaný správne, ale UI nie je úprimné** (UX-79): *Repair workspace* beží bez
   potvrdenia v oboch režimoch (`SessionDetails.svelte:192-215`, žiadny `ConfirmDialog`), hoci hub
   ho vedie ako `confirm: true`/`destructiveHint` (`guard.rs:365-374`) a tooltip menuje iba
   neškodnú polovicu („Recreate a deleted worktree directory, re-register it…“, `:633`); v hub
   režime sa automatická pre-attach kontrola **ticho preskočí** (`TerminalView.svelte:455-464`),
   takže hint „needs Repair workspace“ sa na hube nikdy neukáže. `RoutedUnless` ostáva — je to
   správny verdikt, chýba mu UI.
8. **Matica: 30 príkazov — 18 Routed bez zmeny, 1 RoutedUnless (ostáva), 5 SameInBoth
   (`fix/attachments-hub-parity` c2b5eb42 už zjednotil upload + 4 attachment príkazy), 4 RN, 1 RE,
   1 R** (`purge_project`, T3, `instead` bez „from the hub“). LocalOnly po UXPR-26: 47 (po 06) − 5 =
   **42**; Routed 62 + 5 = **67**.
9. **ADR `Visibility::ClientOnly` — odporúčam (a) os v `TOOL_POLICIES`,** ale s korekciou toho,
   čo rieši: skrýva tools pred **per-host (agent) tokenom**, nie pred masterom ani pred paired
   klientom. Operátorov agent je paired *klient* (`service/operator.rs:156-168`), telefón tiež —
   obaja parity tools vidieť **majú**. Rozpočtový test dnes meria **master** plochu
   (`tests.rs:2325-2400`, `Caller::master()`), takže `ClientOnly` sám o sebe D4 nerieši; rieši ho
   až **zmena toho, čo test stráži**: tvrdý strop na *agentovu* plochu (host `full`, dnes ≈ 50 300 B),
   mäkký strop na master (65 500 po všetkých tranžiach 03–07). Potom UXPR-09 nemusí nič dvíhať pre
   agenta a UXPR-22/25/26 sa vojdú bez trimov.
10. **Rozpočet tejto iterácie: +≈1 970 B** na master/klient ploche (4 tools), **0 B** na agentovej
    ploche pod ADR. Bez ADR by meranie po UXPR-22 (≈ 63 430) skončilo na ≈ 65 400 → výnimka D4
    (zdvih na 65 500) alebo trimy, ktoré už nie sú (06 ich spotrebovalo). Desať nových nálezov
    UX-77…UX-86 (H 1 · M 5 · L 4); tri PR: UXPR-25 (S/M, ADR + os), UXPR-26 (M, odštep a/b),
    UXPR-27 (M, Svelte).

## Matica parity

Príkazy v `generate_handler!` poradí (`src-tauri/src/backend/verdicts.rs`). Akcia: **OK** už
routované · **RE** route na existujúci tool · **RN** route na nový tool · **RU** `RoutedUnless`
ostáva · **SIB** `SameInBoth` · **R** ostáva refused (s opraveným `instead` a tým, čo UI ukáže).
Tier podľa D3; policy v `crates/fleet-core/src/mcp/guard.rs`.

### Kill, Safe remove, Discard

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia · tier |
|---|---|---|---|---|
| `kill_session` | `×` v riadku (`SessionRowItem.svelte:341-350`), *Kill session* v detaile (`SessionDetails.svelte:706-714`), bulk kill (`Sidebar.svelte:855-866`); `ConfirmDialog` s rovnakou kópiou v riadku aj detaile (`Sidebar.svelte:841-851`, `SessionDetails.svelte:744-756`) | Routed (`verdicts.rs:159-164`) | `kill_session` Client/mut/**confirm: true**/Lifecycle (`guard.rs:308-313`); s `mcp.confirm_destructive` na hube → `E_CONFIRM_REQUIRED` → `hubNextStep` (`hub.ts:336-365`) | **OK** — ale kópia dialógu nehovorí, že worktree so zmenami ostáva na disku (UX-78) |
| `safe_kill_session` | *Let Claude commit it* / *Ask Claude to commit + push* **iba vnútri** dialógu Safe remove (`SessionDetails.svelte:766-771,846-851`) | Routed (`:165-170`) | `safe_kill_session` Client/mut/Lifecycle (`guard.rs:314-320`); pošle prompt s nonce, delete až po markeri (`safe_kill.rs:135-219`, `:616-660`) | **OK na drôte, nedostupné v UI** — dialóg otvára `inspect_safe_kill` (UX-77) |
| `inspect_safe_kill` | `askSafeKill` → *Inspecting worktree…* → tri stavy dialógu (`SessionDetails.svelte:247-257,758-865`) | LocalOnly „…the hub exposes no tool for it; retire the session from the hub“ (`:171-177`) | žiadny; `safe_kill::inspect_safe_kill` (`safe_kill.rs:221-338`) = 1 SSH bash: porcelain · branch · upstream · ahead; `SafeKillInspection` (`:51-70`, iba `Serialize`) | **RN** `inspect_safe_kill` · **T0** (`readonly: true`, precedens `capture_session` — pane obsahuje viac než zoznam ciest) · Quick. Params = `SafeKillSessionParams` (session_id **alebo** host+tmux, `params.rs:153-165`) — pole za poľom na to, čo desktop posiela (`InspectSafeKillArgs { host_alias, tmux_name }`, `:72-76`) |
| `discard_kill_session` | *Remove worktree + kill* (`force=false`, clean fast path, `:277-289`) a *Discard & kill* (`force=true`, `:293-306`) v tom istom dialógu | LocalOnly „…use safe_kill_session, or do it from the hub“ (`:178-184`) | žiadny; `safe_kill::discard_kill_session(args, force)` (`:362-449`) = `git worktree remove[ --force]` + `delete_worktree` row + `kill_session { force: true }` | **RN** `discard_kill_session` · **T1** (dosah = jedna session + jej worktree; `delete_worktree` aj `kill_session` sú T1) · Lifecycle · **`confirm: true`** (otázka 1). Params `SafeKillSessionParams` + `force: bool` (`#[serde(default)]`; Tauri ho dnes berie ako samostatný arg, `commands/sessions.rs:124-133`) |

### Detail, aktivita, agenti

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia · tier |
|---|---|---|---|---|
| `session_conversation` | Conversation tab (screenshot 06) | Routed (`:247-252`) | `session_conversation` Client/ro/Lifecycle (`guard.rs:459-464`); `remote.rs:725-743` posiela `claude_session_id` iba keď je | **OK** |
| `session_tool_detail` | rozbalenie riadka `› Run …` (`ToolLine.svelte:95-108`), `toolDetail()` (`conversation.ts:147`) | LocalOnly „…the Conversation tab's tool lines still come from session_conversation“ (`:253-259`) | žiadny; `transcript::fetch_tool_detail` (`transcript.rs:1416-1439`), `ToolDetail` (`:1289-1303`, iba `Serialize`), `SessionToolDetailArgs { session_id, tool_use_id, claude_session_id? }` (`commands/sessions.rs:386-394`) | **RN** `session_tool_detail` · **T0** (čítanie transkriptu ako `session_transcript`, `guard.rs:450-455`) · Lifecycle (SSH grep ≤ 2 MiB). Pole za poľom |
| `session_activity` | živý indikátor v hlavičke Conversation, poll 2 s (`ConversationPanel.svelte:692-722`, `ACTIVITY_POLL_MS` `conversation.ts:791`); gate `ownsTheFleet` → v hub režime nikdy nebeží | LocalOnly „…the hub's pane reads answer a different shape, so the live indicator is off in remote mode“ (`:260-267`) | **existuje** `capture_session` Client/ro/Quick (`guard.rs:265-270`, `session_ops.rs:244-291`, text, `max_lines`); `probe_from_tail(tail)` je PURE (`activity.rs:34-43`) nad `ACTIVITY_TAIL_LINES = 12` (`:16`) | **RE** `Routed { tool: "capture_session" }` — `routed::session_activity` = `hub.route_text("session_activity", json!({session_id, max_lines: 12}))` → `probe_from_tail`; sentinel „(session pane is empty — nothing to capture)“ (`session_ops.rs:280-284`) mapovať na prázdny tail. 0 B |
| `dismiss_agent_session` | `×` *Remove from list* pri `inactive` chipe (`SessionRowItem.svelte:298-306`), *Remove from list* v detaile (`SessionDetails.svelte:681-687`) | LocalOnly „use Kill instead: …“ (`:317-325`) | **ekvivalent existuje:** `kill_session` na `bg:` riadku so `stopped` → `BgKillAction::Dismiss` → `s.dismiss_agent` (`service/sessions/lifecycle.rs:807-823,903-925`); `DismissAgentArgs { session_id }` (`bg_sessions.rs:284-286`); rozdiel iba na working agentovi | **RN** `dismiss_agent_session { session_id }` · **T1** · Quick (~280 B) — presná parita vrátane odmietnutia working agenta; alternatíva **RE** cez `kill_session` (0 B, UI gate `isInactiveAgent` je jediná ochrana) — otázka 2 |
| `dismiss_ghost_session` | `↺` na ghost riadku | Routed (`:305-310`) | `dismiss_ghost_session` Client/mut/Quick (`guard.rs:279-284`); `routed::` zahodí `{"dismissed": id}` (`commands/sessions.rs:608-623`) | **OK** — ghost riadky v hub režime fungujú (klik blokovaný iba `status === 'ghost'`, `SessionRowItem.svelte:167-170`) |
| `new_bg_session` | `⚡` v sidebare | Routed (`:326-331`) | `new_bg_session` Client/mut/Lifecycle | **OK** |
| `purge_project` | kôš (šošovka 6 → skryť v hub režime, UXPR-23) | LocalOnly „…purge from the hub“ (`:332-338`) | žiadny; nevratné, na každom hoste | **R · T3**. `instead` → „…the hub exposes no tool for it (a standalone app can)“ (06 už navrhlo). Master tool **nenavrhujem** (otázka 5) |

### Lifecycle, mená, presun

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia · tier |
|---|---|---|---|---|
| `list_sessions`, `related_sessions`, `new_session` | sidebar, detail, New session | Routed (`:149-158`) | Client (`guard.rs:223-235,251-256`) | **OK** |
| `repair_session` | *🩹 Repair workspace* (`explicit: true`, `SessionDetails.svelte:192-215,629-638`); automatická pre-attach kontrola (`explicit: false`) v `TerminalView.svelte:447-485` — v hub režime **preskočená** | **RoutedUnless** (`:212-221`) — routuje iba `explicit: true` (`commands/sessions.rs:625-641`, test `tests_routing.rs:1620-1670`) | `repair_session` Client/mut/**confirm: true**/Lifecycle (`guard.rs:365-374`); tool je vždy explicitný (`lifecycle.rs:275-288`) | **RU ostáva** — verdikt je správny („routing would quietly become the hub's always-explicit repair“). UI: potvrdenie + úprimný text (UX-79, UXPR-27) |
| `rename_session`, `set_session_friendly_name` | `✎`, `🏷` v riadku a detaile | Routed (`:222-233`) | Client/mut/Quick (`guard.rs:322-336`) | **OK** |
| `session_history`, `session_conversations` | Conversation hlavička, timeline | Routed (`:234-246`) | Client/ro/Quick (`guard.rs:405-418`) | **OK** |
| `restart_session`, `recreate_session` | `↻`, `♻` v riadku; detail | Routed (`:268-273,289-294`) | Client/mut/Lifecycle (`guard.rs:272-277,338-343`) | **OK** — aj na **agent-transport hoste**: hubov `SshClient` routuje cez pripojeného agenta (`ssh.rs:105-148`), takže kill/restart/recreate/inspect na `transport: agent` fungujú; iba desktopov PTY attach sa odmietne (`TerminalView.svelte:1076-1083,1226-1232`) |
| `send_prompt`, `spawn_review` | composer, 🔍 | Routed (`:275-286`) | Client (`guard.rs:345-350,392-397`) | **OK** |
| `move_session` | *Move to host…* → Transfer sheet (`TransferSheet.svelte:20` `moveBlockedReason`, `moveEligibility.ts:23-25`) | Routed (`:295-298`) | `move_session` Client/mut/**confirm: true** (`guard.rs:376-382`); `move:progress` premostené (`backend/events.rs:589`) | **OK** — pozn.: `docs/control-api.md:266-272` tvrdí „master token only“, policy hovorí Client (UX-83) |
| `resolve_move` | *Finish the move* / *Undo the move* (`SessionDetails.svelte:670-679`, Transfer sheet) | Routed (`:299-304`) | `resolve_move` Client/mut/confirm/Lifecycle (`guard.rs:384-390`) | **OK na drôte**, ale **chýba v `ROUTED_ACTIONS`** (`hub.ts:257-274`) → Finish/Undo nie sú gate-ované na stav spojenia (UX-86) |

### Upload, prílohy, terminál (bez zmeny)

| Príkaz | Verdikt | Poznámka |
|---|---|---|
| `upload_to_session` | SameInBoth (`verdicts.rs:366-374`) | drop na pane = vlastné `ssh` tohto stroja |
| `pick_attachments`, `attachment_preview`, `attachment_describe`, `upload_attachments` | SameInBoth (`:80`, `:375-388`) | **`fix/attachments-hub-parity` (c2b5eb42) landed:** 4× LocalOnly → SameInBoth, `REASONS` kľúč zmazaný, `ConversationPanel` gate preč, `local_only.golden.json` −4. Nič neostáva |
| `pty_open/write/resize/close/drain`, `cancel_command` | SameInBoth (`:908-948`) | mimo šošovky |

**Bez Tauri príkazu (iba MCP, N/A pre paritu):** `capture_session`, `session_transcript`,
`wait_for_session`, `set_session_tags`, `new_shell_session` (desktop ide cez `new_session` s
rozšírenými poľami, `hub.md:1032-1036`).

**Súčty:** 30 príkazov · OK 18 · RU 1 · SIB 5 · **RN 4** (`inspect_safe_kill` T0,
`discard_kill_session` T1, `session_tool_detail` T0, `dismiss_agent_session` T1) · **RE 1**
(`session_activity` → `capture_session`) · **R 1** (`purge_project` T3). LocalOnly po UXPR-26:
**42**, Routed **67** (zo 130; po 06: 47/62).

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-21 | **potvrdené, spresnené, závažnosť M → H pre Safe remove** | Safe-kill náhľad: tlačidlo disabled celé, vrátane routovanej cesty `safe_kill_session` (`SessionDetails.svelte:695-704` vs `:766-771,846-851`; `hub_disabled.test.ts:379-388`). Tool detail: dialóg funguje, rozbalenie riadka padne per klik s vetou `E_LOCAL_ONLY` (`ToolLine.svelte:104-108`). `session_activity`: indikátor ticho nie je živý (`ConversationPanel.svelte:709-722`). `dismiss_agent_session`: má ekvivalent, ale rada mieri na skryté tlačidlo. `purge_project`: patrí šošovke 6 (UXPR-23 skryje kôš) — tu iba `instead`. Podstata „v hub režime nefungujú“ platí pre 5 zo 6 |
| verdikt `inspect_safe_kill` „retire the session from the hub“ | **vyvrátené ako pokyn** | hub nemá kam „retire“ okrem `safe_kill_session`/`kill_session` cez MCP klienta — rodina UX-42; po RN veta zaniká |
| verdikt `discard_kill_session` „use safe_kill_session, or do it from the hub“ | **čiastočne vyvrátené** | `safe_kill_session` je v UI nedostupný (UX-77); „from the hub“ nemá cieľ. `hub.ts:194-195` radí „Safe remove's Ask Claude path“ — tú istú nedostupnú cestu |
| verdikt `session_activity` „the hub's pane reads answer a different shape“ | **potvrdené o tvare, vyvrátené ako dôvod refusal** | tvar sa prevedie lokálne čistou funkciou (`probe_from_tail`); refusal nebol nutný |
| verdikt `dismiss_agent_session` „kill_session removes an inactive agent exactly as this would“ | **potvrdené** | `bg_kill_action` → `Dismiss` → `dismiss_agent` (`lifecycle.rs:816-818,917-925`); obe cesty mažú cez tú istú store funkciu, ktorá emituje `session:removed` (`store/sessions.rs:435-438`) |
| zadanie „hubov `safe_kill_session` už počíta tie isté fakty“ | **vyvrátené** | `safe_kill_session` fakty nepočíta (`safe_kill.rs:135-219`); clean check je až v `finalize_safe_kill` po READY (`:616-660`) — a je to *iný* check (iba porcelain, bez ahead/upstream) |
| README §5 šošovka 7 „návrh `inspect_safe_kill` na hube“ | **potvrdené ako správny cieľ** | jeden T0 tool, ~510 B, `SafeKillInspection` + `Deserialize` |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz | Návrh |
|---|---|---|---|---|
| **UX-77** | **H** | **Safe remove v hub režime je slepá ulička aj pre routovanú cestu.** Jediné tlačidlo je disabled cez `inspect_safe_kill`; *Let Claude commit it* (`safe_kill_session`, routuje) a *Kill* existujú iba za tým dialógom resp. vedľa neho. Používateľ hubu tak nemá **žiadnu** bezpečnú cestu na ukončenie session — iba blind Kill, ktorý nechá worktree so zmenami na disku hosta bez slova | `SessionDetails.svelte:59,695-704,758-865`; `hub_disabled.test.ts:373-403`; `verdicts.rs:165-177` | RN `inspect_safe_kill` (UXPR-26) + dialóg, ktorý sa otvorí vždy a pri `E_FORBIDDEN`/`E_HUB_PROTOCOL` (starší hub) ponúkne *Ask Claude* + *Kill* s jedným riadkom „this hub cannot inspect worktrees yet“ (UXPR-27) |
| **UX-78** | M | **Tri spôsoby ukončenia bez vysvetlenia rozdielu.** Riadok má iba `×` Kill; detail má *⏏ Safe remove* s prázdnym tooltipom (`title={inspectSafeKillBlocked ?? ''}`) a *Kill session*; Kill dialóg hovorí „lose any running claude state“ a **mlčí o worktree** (uncommitted files, unpushed commits ostávajú na hoste; ghost riadok potom ponúka iba Recreate). Discard dialóg to hovorí správne, Kill nie. Rovnaká kópia v riadku aj detaile (`Sidebar.svelte:841-851` = `SessionDetails.svelte:744-756`) — lokál vs hub sa **nelíši**, čo je v poriadku; líši sa iba to, čo je dostupné | `SessionDetails.svelte:695-714,744-756`; `Sidebar.svelte:841-851`; `SessionRowItem.svelte:341-350` | Kill dialóg: veta „The worktree and any uncommitted or unpushed work stay on `<host>`; use **Safe remove** to check first.“ + sekundárne tlačidlo *Safe remove instead…* (prepne na Safe dialóg); tooltip Safe remove: „Check for uncommitted or unpushed work before removing“ (UXPR-27). Ikona podľa A3 (`shield-check`, UXPR-03) |
| **UX-79** | M | **Repair workspace nie je úprimný o tom, čo robí — v oboch režimoch, na hube viac.** Beží bez potvrdenia (`onRepair` → hneď `repairSession(explicit: true)`), hoci hub ho vedie `confirm: true` s `destructiveHint` a popis toolu menuje „unregister … adopt … recreate the branch … respawn a pane“; tooltip v detaile menuje iba neškodnú polovicu. V hub režime sa automatická kontrola pred attachom preskočí bez stopy, takže „needs Repair workspace“ hint sa na hube nikdy neukáže a stav worktree je neznámy až do kliku. Kill/Recreate/Purge dialóg majú, Repair nie (↔ UX-62) | `SessionDetails.svelte:192-215,629-638`; `TerminalView.svelte:455-485`; `guard.rs:365-374`; `lifecycle.rs:275-288`; `verdicts.rs:212-221`; `hub.ts:340-347` (E_CONFIRM zoznam menuje repair), `hub.md:996-1002` (nemenuje) | `ConfirmDialog` „Repair workspace?“ s vetami z popisu toolu; v hub režime dodatok „On a hub this is always the full (explicit) repair; the automatic pre-attach check does not run here.“; po attachu jednorazový riadok pod hlavičkou terminálu „workspace not checked (hub client) — Repair workspace checks it“; `hub.md:996-1002` doplniť `repair_session` (UXPR-27, docs v 26b) |
| **UX-80** | M | **Detail tool-callu v hub režime zlyháva per riadok, tou istou vetou.** `ToolLine` nemá pre-gate; každé rozbalenie zavolá `session_tool_detail`, dostane `E_LOCAL_ONLY` a zobrazí celú backendovú vetu (`…; the Conversation tab's tool lines still come from session_conversation`) ako `role="alert"` bez Retry — N riadkov, N alertov. Allowlist `handledInlinePerClickNotPreGated` to pinuje ako „deliberate“ | `ToolLine.svelte:44-50,95-108,164-170`; `hub_verdicts.test.ts:169-176`; `verdicts.rs:253-259` | RN `session_tool_detail` (UXPR-26); pre starší hub `hubInlineState(err)` jedna veta, jedna na **panel** (nie na riadok): `ConversationPanel` si pamätá `toolDetailUnsupported` po prvom `E_FORBIDDEN`/`E_HUB_PROTOCOL` a riadky sa nerozbaľujú (UXPR-27) |
| **UX-81** | L | **Živý indikátor je na hube ticho vypnutý.** `probeLive` je `false`, hlavička ukazuje `claude_status` z riadku (čerstvý po reconcile ticku hubu), spinner/`waiting_for` nikdy; nič nehovorí, že indikátor nie je živý. Refusal je zbytočný — tvar sa dá previesť lokálne | `ConversationPanel.svelte:709-722`; `activity.rs:16,34-43`; `session_ops.rs:244-291` | RE `session_activity` → `capture_session` + `probe_from_tail` (0 B); `probeLive` bez `ownsTheFleet`, s `hubActionBlocked`-free logikou (routované čítanie); pri `E_FORBIDDEN` (readonly telefón nie — `capture_session` je ro; iba hub bez toolu) prestať pollovať a v hlavičke `· not live` (UXPR-27) |
| **UX-82** | L | **`instead` radí tlačidlo, ktoré pre daný riadok neexistuje.** Remove from list je disabled s „use Kill instead“, no Kill je pre neaktívneho agenta skrytý (`{#if !isInactiveAgent(sess)}`) v riadku aj v detaile; komentár v kóde to priznáva („this row's Kill button is hidden for an inactive one“) | `SessionRowItem.svelte:139-143,298-306,341-350`; `SessionDetails.svelte:681-687,693-694`; `verdicts.rs:317-325` | RN/RE (otázka 2); do vtedy ukázať Kill pre neaktívneho agenta v hub režime (alebo `instead` bez „use Kill instead“) |
| **UX-83** | L (docs) | **Doc drift okolo policy:** `control-api.md` menuje `move_session` „master token only“, policy je `Access::Client` a desktop ho pre klienta routuje; `hub.md` zoznam `E_CONFIRM_REQUIRED` toolov vynecháva `repair_session` (`confirm: true`), `hub.ts` ho menuje; `hub_verdicts.test.ts:79` „the five non-command REASONS keys“ pri štyroch (D6 bod 1 už rieši ako „the three“) | `docs/control-api.md:266-272`; `guard.rs:376-382`; `docs/hub.md:996-1002`; `hub.ts:340-347`; `hub_verdicts.test.ts:79-86` | opraviť v UXPR-26b (docs krok 13); `control-api.md` po ADR aj tak prepisuje odsek *The served tool surface* |
| **UX-84** | L | **Dvojité potvrdenie, druhé až po kliku.** S `mcp.confirm_destructive` na hube prejde desktopov `ConfirmDialog` (Kill/Recreate/Move), potom hub vráti `E_CONFIRM_REQUIRED` a `hubNextStep` to vysvetlí toastom. Používateľ potvrdil, nič sa nestalo, a čaká na operátora hubu. Nič pred klikom nehovorí, že hub potvrdenia vyžaduje | `hub.ts:336-365`; `hub.md:996-1002`; `guard.rs:308-313,365-390` | po UXPR-09 (`get_fleet_settings` T0, derived `mcp.confirm_destructive` — konsolidácia C4) desktop vie hodnotu vopred: dialóg dostane riadok „the hub will also ask its operator to approve this“; slice 2 FAB specu presunie potvrdenie za hub — nie tu |
| **UX-85** | L | **`instead`/`REASONS` vety ukazujú do prázdna** (rodina UX-42): `inspect_safe_kill` „retire the session from the hub“, `discard_kill_session` „or do it from the hub“, `purge_project` „purge from the hub“; `hub.ts` navyše radí cestu, ktorá je disabled (UX-77) | `verdicts.rs:171-184,332-338`; `hub.ts:192-197` | RN zmaže dve; `purge_project` text podľa 06; `REASONS` kľúče `inspect_safe_kill`, `discard_kill_session`, `dismiss_agent_session` von (D6 bod 1) |
| **UX-86** | L | **`resolve_move` routuje, ale nie je v `ROUTED_ACTIONS`** — *Finish the move* / *Undo the move* nie sú gate-ované na stav spojenia; offline klik skončí surovou chybou namiesto vety pred klikom (na rozdiel od `move_session`, ktorý cez `moveBlockedReason` gate má) | `hub.ts:257-274`; `moveEligibility.ts:23-25`; `TransferSheet.svelte:20,157-159`; `SessionDetails.svelte:670-679`; `verdicts.rs:299-304` | `ROUTED_ACTIONS` + `'resolve_move'`, `hubActionBlocked('resolve_move')` na oboch tlačidlách (UXPR-26b min. TS / UXPR-27) |

Číslovanie: iterácia 06 končí UX-76; táto používa **UX-77…UX-86**. Ghost riadky a Transfer sheet
v hub režime **bez nálezu** okrem UX-86: `dismiss_ghost_session`/`recreate_session` routujú,
`move:progress` je premostený, `moves.ts` nemá žiadnu hub vetvu a nepotrebuje ju.

## Návrh hub tools / rozšírení

Zásady z iterácií 03 a 06: meno toolu = meno príkazu, jedna klauzula popisu, próza do
`docs/hub.md`/`control-api.md`, `Deadline` podľa SSH. Všetky štyri nové tools dostanú
`visibility: Visibility::ClientOnly` (ADR nižšie) — agent v session ich nepotrebuje (má shell a
`session_transcript`), operátorov agent a telefón ich vidia.

### Tool 1 — `inspect_safe_kill` (T0)

| | |
|---|---|
| Súbor | `crates/fleet-core/src/mcp/tools/lifecycle.rs` (za `safe_kill_session`) |
| Popis (~150 B) | `"Dry run for safe_kill_session: the worktree's dirty files, branch, upstream and unpushed-commit count, and whether it is safe to remove without asking Claude. Read-only."` |
| Params | `SafeKillSessionParams` (existujúci struct, `params.rs:153-165`: `session_id` alebo `host_alias`+`tmux_name`) → `resolve_target(..., "the session to inspect")` ako `safe_kill_session` (`lifecycle.rs:75-81`) |
| Výsledok | `SafeKillInspection` (`safe_kill.rs:51-70`) → pridať `Deserialize` (+ `DirtyFile` už má) ; `ok_json` |
| Policy | `access: Client, readonly: true, confirm: false, deadline: Quick, visibility: ClientOnly` |
| Prečo T0 | čisté čítanie jedného SSH round-tripu; obsah = cesty súborov a názov vetvy — menej než `capture_session` (T0) ukáže z pane; `readonly` telefón má vidieť, či je bezpečné odísť (otázka 4) |
| Wire | `InspectSafeKillArgs` ostáva desktopový; `remote.rs` posiela `json!({host_alias, tmux_name})` (doc-komentár: tool berie aj `session_id`, desktop ho neposiela — príkaz ho nemá); sample `sample_safe_kill_inspection()` (jeden `DirtyFile`, `unpushed_commits: 2`, `error: None`) do `tests_contract.rs` → `REGEN_HUB_CONTRACT` |

### Tool 2 — `discard_kill_session` (T1, `confirm: true`)

| | |
|---|---|
| Súbor | `lifecycle.rs`; params `params.rs` nový `DiscardKillSessionParams { session_id?, host_alias?, tmux_name?, force: bool /// Remove the worktree even when it has uncommitted or unpushed work (Discard & kill). Default false: refuse a dirty worktree. , confirm_nonce? }` |
| Popis (~170 B) | `"Remove a session's worktree and kill it in one step, without asking Claude: force=false only when clean, force=true discards local-only work. Prefer safe_kill_session when unsure."` |
| Výsledok | `i64` (killed session id) ako `kill_session` |
| Policy | `access: Client, readonly: false, confirm: true, deadline: Lifecycle, visibility: ClientOnly` |
| Prečo T1 | skladá `delete_worktree` (T1, confirm) + `kill_session` (T1, confirm) na **jednej** session; nemení politiku fleetu, nekoná mimo fleetu; `readonly` odmietnutý gate-om; `refuse_if_operator` ako `safe_kill_session` (`safe_kill.rs:144-152`) doplniť aj sem (dnes chýba — discard operátora by prešiel) |
| Prečo `confirm: true` (odchýlka od D3) | `confirm_gate` (`lifecycle.rs:40-45`) chráni `kill_session`; tool, ktorý kill **obsahuje**, ho nesmie obísť. `E_CONFIRM_REQUIRED` desktop už vysvetľuje (`hubNextStep`) — otázka 1 |
| Wire | `remote.rs`: `json!({host_alias, tmux_name, force})` — `force` je dnes samostatný Tauri arg (`commands/sessions.rs:124-133`), do wire structu ide ako pole; Case s `force: true` (nedefaultná hodnota, recept krok 6) |

### Tool 3 — `session_tool_detail` (T0)

| | |
|---|---|
| Súbor | `crates/fleet-core/src/mcp/tools/orchestration.rs` (za `session_conversation`); params `SessionToolDetailParams { session_id /// Fleet session id. , tool_use_id /// The tool call's id from session_conversation. , claude_session_id? /// An earlier conversation of the session (from session_conversations). }` |
| Popis (~150 B) | `"One tool call's input (edit before/after, Bash command, or pretty JSON) and result, read from the session's transcript; each text ≤ 8000 chars. Read-only. Errors: E_NOTFOUND, E_INVALID, E_NO_TRANSCRIPT."` |
| Výsledok | `ToolDetail` (`transcript.rs:1290-1303`) + `EditDetail` (`:1306-1310`) → `Deserialize`; `ok_json_compact` |
| Policy | `access: Client, readonly: true, confirm: false, deadline: Lifecycle, visibility: ClientOnly` |
| Prečo T0 | rovnaké čítanie ako `session_transcript` (T0); per-host binding cez `resolve_target_row` (`orchestration.rs:66-67`) ako transkript |
| Prečo nie rozšírenie `session_conversation` | popis toolu výslovne sľubuje „tool inputs and results are never included“ — poll by narástol o 2× 8 000 znakov na každý riadok; ID-adresované čítanie je to, čo desktop robí dnes (`commands/sessions.rs:396-431`) |
| Wire | `remote.rs`: `claude_session_id` iba keď je (vzor `session_conversation`, `:725-743`); sample `sample_tool_detail()` (`edit: Some`, `command: None`, `result: Some`, `is_error: false`) → kontrakt |

### Tool 4 — `dismiss_agent_session` (T1) — alebo RE (otázka 2)

| | |
|---|---|
| Súbor | `session_ops.rs` (za `dismiss_ghost_session`); params `DismissAgentSessionParams { session_id /// Fleet session id of a bg agent that is not working. }` |
| Popis (~120 B) | `"Remove an inactive background agent (kind bg, not working) from the list without touching its process; a working agent is refused — kill_session stops it."` |
| Výsledok | `{"dismissed": id}` ako `dismiss_ghost_session`; `routed::` zahodí telo (`commands/sessions.rs:612-620`) |
| Policy | `access: Client, readonly: false, confirm: false, deadline: Quick, visibility: ClientOnly` |
| Alternatíva RE | `Routed { tool: "kill_session" }`, `routed::dismiss_agent_session` = `hub.route("kill_session", json!({session_id}))` — 0 B, ale na working agentovi by hub agenta **zastavil**, kde standalone odmietne; jediná ochrana je `isInactiveAgent` v UI. Rovnaká sémantická medzera, pre ktorú `repair_session` refused ostal → **odporúčam RN** |

### Rozšírenie — `session_activity` → `capture_session` (RE, 0 B)

`routed::session_activity(backend, store, ssh, id)`: `Some(hub) => { let text =
hub.route_text("session_activity", &json!({"session_id": id, "max_lines": ACTIVITY_TAIL_LINES})).await?;
Ok(activity::probe_from_tail(strip_capture_sentinel(&text))) }`. `probe_from_tail` a
`ACTIVITY_TAIL_LINES` sú `pub` (`activity.rs:16,34`). Sentinel „(session pane is empty — nothing
to capture)“ a prípadná úvodná „truncation note“ (`session_ops.rs:280-290`) → prázdny tail (all-`None`
probe, ako tick). `capture_session` je `readonly: true` → readonly telefón dostane živý indikátor.
Test `every_routed_tool_is_a_tool_the_hub_serves` prejde (`capture_session` je v `TOOL_POLICIES`).
Pozn.: `capture_session` berie iba `session_id` — desktopov `SessionActivityArgs { session_id }`
mapuje 1:1.

### Čo sa mení vo `VERDICTS` (recept krok 3)

| Príkaz | Dnes | Po UXPR-26 |
|---|---|---|
| `inspect_safe_kill` | LocalOnly | `Routed { tool: "inspect_safe_kill" }` |
| `discard_kill_session` | LocalOnly | `Routed { tool: "discard_kill_session" }` |
| `session_tool_detail` | LocalOnly | `Routed { tool: "session_tool_detail" }` |
| `session_activity` | LocalOnly | `Routed { tool: "capture_session" }` (doc-komentár: tvar sa prevádza lokálne, `probe_from_tail`) |
| `dismiss_agent_session` | LocalOnly | `Routed { tool: "dismiss_agent_session" }` (RN) alebo `Routed { tool: "kill_session" }` (RE) |
| `purge_project` | „…purge from the hub“ | text podľa 06 (bez cieľa, ktorý neexistuje) |
| `repair_session` | RoutedUnless | **bez zmeny** |

`local_only.golden.json` stratí 5 záznamov (`REGEN_LOCAL_ONLY`), tabuľka v `hub.md` sa prepíše
(`REGEN_HUB_VERDICTS`), `hub_verdicts.generated.json` `local_only` 70 → 65 v tomto PR (relatívne k
dnešku; po celej fronte 42).

### Starší hub a readonly klient (D5 `hub_inline_state`)

- Hub bez `inspect_safe_kill`: `E_FORBIDDEN` (gate padá closed na neznámom mene,
  `tests.rs:1471`) alebo `E_HUB_PROTOCOL` → dialóg Safe remove **sa otvorí** a v stave
  `inspectError` ukáže jeden riadok „this hub cannot inspect worktrees yet (update the hub) — you
  can still ask Claude, or kill“ + tlačidlá *Ask Claude to commit + push* a *Kill*; nikdy toast.
- Hub bez `discard_kill_session`: *Remove worktree + kill* / *Discard & kill* disabled s tou istou
  vetou; *Ask Claude* ostáva.
- Hub bez `session_tool_detail`: jeden riadok na panel (UX-80), riadky sa nerozbaľujú.
- `readonly` klient: `inspect_safe_kill`, `session_tool_detail`, `capture_session` prejdú;
  `discard_kill_session`, `dismiss_agent_session`, `safe_kill_session`, `kill_session` →
  `E_FORBIDDEN` — po UXPR-21 disabled **vopred** („this client is readonly on the hub“), bez
  UXPR-21 prvý `E_FORBIDDEN` → `hubInlineState(err).text` v dialógu.
- `E_CONFIRM_REQUIRED` (`kill_session`, `discard_kill_session`, `repair_session`, `move_session`):
  `hubNextStep` ako dnes; UX-84 riadok v dialógu po UXPR-09.

## Návrh UI (Safe remove, Discard, Tool detail, Repair) v hub režime

### Safe remove (`SessionDetails.svelte:695-704,758-865`)

- Tlačidlo *Safe remove* gate-ovať `hubActionBlocked('safe_kill_session')` (spojenie), nie
  `hubBlock('inspect_safe_kill')`; tooltip v oboch režimoch „Check for uncommitted or unpushed
  work before removing“ (UX-78).
- Dialóg má dnes 4 stavy (loading · inspectError · safe_to_remove · inspection). Pridať piaty
  **`unsupported`** (starší hub, podľa kódu z `hubInlineState`): text jedným riadkom, tlačidlá
  *Cancel* · *Ask Claude to commit + push* · *Kill session* (danger). `inspectError` (git
  zlyhal) ostáva ako dnes — v oboch režimoch.
- Stav `inspection` v hub režime bez zmeny: *Let Claude commit it* (routuje) a *Discard & kill*
  (RN); pod nimi `HubScopeNote what='session'`? **Nie** — dialóg nepotrebuje banner, iba
  krátky sufix v tlačidle Discard pri `E_FORBIDDEN readonly` („readonly on the hub“).
- Po `E_CONFIRM_REQUIRED` z hubu (UX-84) dialóg neskončí toastom: ostane otvorený so stavom
  „waiting for the hub's operator to approve“ a zatvorí sa po `session:killed`/`session:removed`
  evente (bridge ich už doručuje) — S doplnok, môže ísť do UXPR-27 alebo za ním.

### Kill (`Sidebar.svelte:841-851`, `SessionDetails.svelte:744-756`, bulk `:855-866`)

Jedna kópia na troch miestach → helper `killDialogCopy(sess)` v `session_view.ts` (UXPR-06 ho
zakladá) alebo lokálny snippet: „This kills the tmux session `<name>` on `<host>` and the running
claude state inside it. **The worktree and any uncommitted or unpushed work stay on the host.**“
+ v detaile sekundárne tlačidlo *Safe remove instead…* (nie v bulk dialógu). Ikona Kill podľa A1
(`circle-x`, UXPR-02).

### Discard

Dialóg dnes už vysvetľuje („local-only changes will be lost“, `SessionDetails.svelte:826-831`).
Jediná zmena: pri `force=false` (clean fast path) a hubovom `E_WORKTREE_REMOVE` („surprise dirty
file“) ukázať dôvod inline a ponúknuť *Re-inspect* (znova `inspect_safe_kill`), nie toast.

### Tool detail (`ToolLine.svelte`, `ConversationPanel.svelte`)

- `ToolLine` ostáva per-klik (správne — detail je drahý), ale `ConversationPanel` mu dá prop
  `detailUnsupported: string | null` (z prvého `E_FORBIDDEN`/`E_HUB_PROTOCOL`, cez
  `hubInlineState`); pri `!== null` sa riadok nerozbalí, chevron je muted a `title` nesie vetu;
  jeden riadok pod hlavičkou konverzácie (`data-testid="conv-detail-unsupported"`).
- `loadRetryable` zoznam kódov (`ToolLine.svelte:107`) doplniť `E_FORBIDDEN` (readonly telefón
  ho pri T0 nedostane, starší hub áno).

### Živý indikátor

`probeLive` bez `ownsTheFleet`; po prvom `E_FORBIDDEN`/`E_HUB_PROTOCOL` nastaviť
`probeUnsupported = true` (zastaví interval) a v hlavičke Conversation za status chipom `· not live`
s tooltipom z `hubInlineState`. V bežnom hub režime indikátor **je živý** (UX-81 zaniká).

### Repair (`SessionDetails.svelte:629-638`)

`ConfirmDialog title="Repair workspace?" confirmLabel="Repair" danger`: „Makes `<cwd>` a healthy git
worktree on `<branch>` and its tmux session run there. It may unregister a stale worktree entry,
adopt a checkout that moved, recreate the branch from its base once origin confirms it is gone,
and respawn a pane whose directory vanished. Nothing happens on a healthy session.“ V hub režime
posledná veta navyše: „On a hub this is always the full repair — the automatic check before attach
does not run here.“ Tooltip tlačidla skrátiť na „Repair the worktree directory, git registration
and tmux pane“. Po attachu v hub režime (TerminalView, `ownsTheFleet` vetva) jeden muted riadok pod
hlavičkou terminálu: „workspace not checked on a hub client — Repair workspace checks it“, s
`data-testid="terminal-unchecked-workspace"`, zmizne po prvom výstupe pane.

### Remove from list

Po RN/RE enabled cez `hubActionBlocked('dismiss_agent_session')` (do `ROUTED_ACTIONS`); tooltip
„Hide this inactive agent until it becomes active again“ ostáva.

## ADR návrh: `Visibility::ClientOnly`

> Súbor, ktorý implementačný PR pridá: **`docs/adr/0003-tool-visibility.md`** (po
> `0001-descope-freeze-ship-move.md`, `0002-move-carries-work-as-is.md`). Nižšie je jeho obsah
> v skratke (~1 strana); PR ho prepíše do angličtiny v štýle ADR 0001/0002.

### Kontext

Kto vidí ktoré tools rozhoduje `present::visible_to` (`crates/fleet-core/src/mcp/tools/present.rs:63-68`):
`readonly` token vidí iba `readonly: true` riadky, nemaster vidí iba `Access::Client` riadky. Osi sú
dve — *mód* a *admin*. Neexistuje os „pre koho je tool určený“. Volajúci sú traja
(`auth.rs:63-77`): **master** (operátorov Claude Code, `control-api.md:93-100`, a `fleet-hub` CLI),
**per-host token** (agent bežiaci v session na hoste; hooky; `fleet-agent`), **paired klient**
(telefón, desktop v hub režime, **a operátorov UX agent** — `operator.rs:156-168`: „a client token
in mode full, never the master token“).

Hub-parity fronta pridáva tools, ktoré sú *desktopové operácie preložené na drôt*: 10 git zápisov
(UXPR-10), `get/set_fleet_settings` (09), `catalog_*` (12), usage a nickname (22), a tu
`inspect_safe_kill`, `discard_kill_session`, `session_tool_detail`, `dismiss_agent_session` (26)
— spolu ≈ 18 toolov, ≈ 9 000 B. Všetky sú `Access::Client`, takže ich uvidí aj **agent v session
cez per-host token**, hoci on má git v shelli, transkript na disku a session ukončuje inak.
Dôsledky: (1) `tools/list` agenta rastie z 66 na ≈ 84 toolov — nad hranicou, kde sa presnosť výberu
zhoršuje (spec `2026-09-20-mcp-token-efficiency.md:17-22`: 30–50); Claude Code síce definície
odkladá, ale vyhľadáva ich regexom/BM25 nad menami a popismi (`:24-31`), takže každý nový tool
súťaží o výber; (2) rozpočtový test meria **master** plochu (`tests.rs:2325-2400`,
`definition_bytes(&Caller::master())`) a D4 ho preto dvíha o každú desktopovú paritu, hoci
agentova plocha je tá, ktorú test pôvodne chránil („the widest surface“ bol proxy); (3)
`docs/control-api-reference.md` (`doc_gen.rs:26-40`, `list_all()`) neuvádza, komu je tool určený.

### Možnosti

**(a) Os `Visibility` v `TOOL_POLICIES`.** `ToolPolicy { …, visibility: Visibility }`,
`enum Visibility { All, ClientOnly }`. `visible_to`: `if caller.host_alias.is_some() &&
policy.visibility == ClientOnly { return false; }` **a** rovnaká vetva v gate
(`enforce_admin` alebo nový `enforce_audience`), aby platil test
`the_served_tool_list_matches_the_call_gates` (`tests.rs:2020-2034`: „a tool a caller can see is a
tool it can call, and vice versa“). Tretia hodnota `MasterOnly` zo zadania je **redundantná** —
`Access::Master` už skrýva pred každým nemasterom (`visible_to:67`); dve osi (kto smie · pre koho
je) ostávajú ortogonálne a bez prekryvu. Cena: 1 pole v 77 riadkoch (default `All` cez `const`
konštruktor alebo explicitne), 2 riadky v `present.rs`, 1 v gate, 3 testy, `doc_gen` riadok.
**(b) Samostatný namespace / router** (`desktop_*` prefix alebo druhý `tool_router` mountovaný len
pre klientov). Rieši viditeľnosť, ale rozbíja „meno toolu = meno príkazu“ (03), zdvojuje `audit`,
`resolve_target` a policy tabuľku, a `every_router_tool_has_exactly_one_tool_policy_row`
(`tests.rs:1343`) by musel poznať dva routery. **(c) Nič — spoľahnúť sa na `Access`.** Agent vidí
všetko, rozpočet rastie s každou paritou (D4 už raz zdvihnutý, 06 minulo rezervy, 24 čaká na
výnimku), a `readonly < bytes/2` podmienka sa zhoršuje s každým mutujúcim toolom.

### Rozhodnutie (odporúčanie)

**(a)**, s dvoma spresneniami:

1. **`ClientOnly` = skryté pred per-host tokenom, viditeľné masterovi a paired klientom.** Meno
   ostáva (konsolidácia ho už používa), doc-komentár povie presne toto. Operátorov agent a telefón
   parity tools **vidia** — sú pre nich.
2. **Rozpočtový test zmení, čo stráži:** dve konštanty —
   `AGENT_BUDGET_BYTES` (tvrdý strop, plocha `host_caller("h", Full)`; dnes ≈ 50 300 B podľa
   pomeru zo specu `:163-167` — master 57 603 mínus 11 admin riadkov; nastaviť na meranie + ≤ 2 %,
   ≈ **51 000**) a `MASTER_BUDGET_BYTES` (mäkký strop = operátorov Claude Code s odloženým
   načítaním; zdvihnúť **raz** na **65 500** = 57 603 + 650 + 450 + 4 900 + 835 + 1 970 − 1 010
   trimov z 06). Podmienka `ro_bytes < bytes / 2` sa meria proti agentovej ploche (host readonly
   vs host full). Výpis štyroch riadkov (`master / host full / host readonly / client full`)
   ostáva.

### Dôsledky

- **D4:** UXPR-09 už nedvíha 63 800 „raz za všetkých“ — UXPR-25 nastaví obe konštanty raz a
  UXPR-09/10/12/22/26 označia svoje tools `ClientOnly`, takže agentov strop **nerastie** a master
  strop je nastavený na súčet známych tranží. Ak UXPR-09 pristane pred UXPR-25 (lane D už beží),
  UXPR-25 iba prevezme jeho číslo ako `MASTER_BUDGET_BYTES` a pridá agentov strop.
  `docs/ux/…/README.md` §5 rozhodnutie (d) sa spresní na „dva stropy, nastavené raz v UXPR-25“.
- **Retroaktívne označenie existujúcich toolov** (`list_host_worktrees`, `delete_worktree`,
  `resolve_move`, `related_sessions`, `session_conversations`…) sa v UXPR-25 **nerobí** — PR je
  os bez zmeny správania; kandidátov vyhodnotí neskorší pass (otázka 6).
- **Gate:** per-host token, ktorý zavolá `ClientOnly` tool, dostane `E_FORBIDDEN` s vetou „is
  not served to a per-host token; a paired client or the master calls it“ — nový text v
  `enforce_*`, aby sa dal odlíšiť od „not a client-callable tool“ (`tests.rs:1428-1462` pinuje
  rozdiel medzi admin a unclassified; pribudne tretí).
- **`docs/control-api-reference.md`:** `doc_gen.rs` pridá pod nadpis toolu riadok *Audience:
  master, paired clients (not served to a per-host token)*; `narrative_guide_names_every_tool`
  nezmenený. `control-api.md` odsek *The served tool surface* (`:384-396`, dnes „73 / 63 / 37“ —
  zastarané; policy má 77 riadkov, 11 Master, 40 readonly) prepísať na štyri čísla z testu a
  vetu o `ClientOnly`; *Per-host tokens* (`:35-66`) jedna veta. `hub.md` *Clients* (`:576-583`)
  bullet `full`: „…and the desktop-parity tools (git writes, settings, safe-remove inspection, tool
  detail), which a per-host token is not served“.
- **Telefón (šošovka 20):** nič sa nemení — je paired klient; `readonly` telefón vidí iba
  `readonly: true` podmnožinu ako dnes. `fleet-mobile` spec zdedí tools automaticky.
- **`fleet-agent` / hooky:** per-host token; `ClientOnly` tools nikdy nevolajú — bez dopadu.
- **Kontrakt:** `Visibility` nie je na drôte (`tools/list` je už filtrovaný) → bez
  `REGEN_HUB_CONTRACT`; `REGEN_DOCS` áno (reference dostane riadok Audience).
- **Riziko:** tool, ktorý agent *potrebuje* a niekto označí `ClientOnly` omylom — test
  `client_only_tools_are_not_named_in_the_control_skill` (grep `skills/claude-fleet-control/SKILL.md:63-70`
  tabuľky toolov proti `ClientOnly` riadkom) to zachytí: skill je to, čo agent číta.

## Rozpočet

`the_served_definition_budget_stays_bounded` (`crates/fleet-core/src/mcp/tools/tests.rs:2325-2400`),
`BUDGET_BYTES = 57_700` (`:2357`), meranie 57 603; D4: 63 800 v UXPR-09; 06: po rezervných
trimoch ≈ 63 430.

| Položka | Odhad B | Master/klient priebežne | Agent (host full) |
|---|---|---|---|
| stav po UXPR-22 (06, po trimoch) | | 63 430 | ≈ 50 300 (bez git/settings/assets/usage pod ADR) |
| `inspect_safe_kill` 17 + ~150 + `SafeKillSessionParams` schéma ~330 | +500 | | 0 |
| `discard_kill_session` 20 + ~170 + 3 polia ~330 + `force` ~130 + `confirm_nonce` ~110 | +760 | | 0 |
| `session_tool_detail` 19 + ~150 + 3 polia ~360 | +530 | | 0 |
| `dismiss_agent_session` 21 + ~120 + 1 pole ~140 (RN) | +280 | | 0 |
| `session_activity` → `capture_session` | 0 | | 0 |
| **delta iterácie 07** | **+2 070** | **≈ 65 500** | **0** |
| s RE pre dismiss (otázka 2) | +1 790 | ≈ 65 220 | 0 |

Záver: **bez ADR** iterácia 07 presiahne 63 800 o ≈ 1 700 B a rezervné trimy už nie sú (06 ich
spotrebovalo) → výnimka D4 (zdvih na 65 500 s odsekom) — presne ten „tretí zdvih“, ktorému D4 chcel
predísť. **S ADR** je delta na agentovej ploche 0 B a `MASTER_BUDGET_BYTES = 65 500` pokrýva
všetky tranže 03–07 naraz (UXPR-24 by potom potreboval 66 800 → do jeho odseku). Odporúčam preto
**UXPR-25 pred UXPR-09** v lane D (je malý a nedotýka sa `verdicts.rs`/`remote.rs`); ak 09 už
pristál, UXPR-25 preberie jeho číslo. `ro_bytes < bytes/2` pod ADR: agent readonly (≈ 21 000)
vs agent full (≈ 50 300) — pohodlne.

Ak by sa presný trim vyžadoval aj tak (ADR zamietnuté): jediné dva kandidáty bez straty významu
sú `kill_session` popis (dnes ≈ 620 B, `lifecycle.rs:9-18` — vety o `external` a bg riadkoch
patria do `control-api.md`; −250) a `repair_session` popis (≈ 720 B, `:275-288`; zoznam chýb a
polí RepairReport do reference; −300) — spolu −550, stále nestačí; zvyšok = výnimka D4.

## PR plán (podľa receptu z iterácie 03)

Recept `03-hub-parity-settings.md` → *Recept*, kroky 1–14; D6 pravidlá. Lane D (sekvenčná hub
Rust) · lane E (Svelte) · lane C (zdieľané).

### UXPR-25 — ADR 0003 + os `Visibility` (**S/M**, lane D, **pred UXPR-09**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `docs/adr/0003-tool-visibility.md` (nový) | ADR podľa § vyššie (Context · Options · Decision · Consequences), EN |
| 2 | `crates/fleet-core/src/mcp/guard.rs:79-85` | `pub enum Visibility { All, ClientOnly }`, pole `visibility` v `ToolPolicy`; 77 riadkov `visibility: Visibility::All` (mechanicky; alebo `const fn all(...)` helper — nie, explicitne, ako ostatné polia); `pub fn is_agent_visible(name)` |
| 3 | `mcp/tools/present.rs:63-68` | vetva `host_alias.is_some() && !is_agent_visible` |
| 4 | `mcp/guard.rs` / `mcp/tools/mod.rs` gate | `enforce_audience(&caller, tool)` volaný vedľa `enforce_mode`/`enforce_admin`, vlastná veta |
| 5 | `mcp/tools/tests.rs:2325-2400` | `AGENT_BUDGET_BYTES` (tvrdý, host full) + `MASTER_BUDGET_BYTES` (mäkký, 65 500 alebo číslo z 09); `ro_bytes` proti host full; doc-komentár s tranžami; `every_caller_kind` bez zmeny; nové `a_per_host_token_is_not_served_client_only_tools` (dočasne na syntetickom riadku alebo prázdny set + assert nad `TOOL_POLICIES`), `client_only_tools_are_not_named_in_the_control_skill`, `enforce_audience_names_the_audience` |
| 6 | `mcp/doc_gen.rs:26-40` | riadok *Audience* pri `ClientOnly`; `REGEN_DOCS` ×2 (reference sa nemení, kým nie je prvý `ClientOnly` — test zelený) |
| 7 | `docs/control-api.md:35-66,384-396`, `docs/hub.md:576-583` | vety podľa ADR; čísla plochy z testu |
| 8 | `docs/ux/2026-09-21-audit/README.md` §5 (d) | „dva stropy, nastavené raz v UXPR-25“ (kontrolór) |

~170 riadkov (+~120 testov). Závisí: nič. ∥ so všetkým okrem `guard.rs`/`tests.rs` (lane D).

### UXPR-26 — Rust: session tools + routing + kontrakt (**M**, lane D po UXPR-22; odštep 26a/26b)

**26a — fleet-core (~150 +100):**

| # | Krok receptu | Súbor | Zmena |
|---|---|---|---|
| 1 | 2a | `mcp/tools/params.rs` | `DiscardKillSessionParams`, `SessionToolDetailParams`, `DismissAgentSessionParams` (každé pole `///`) |
| 2 | 9 | `service/safe_kill.rs:51-70`, `transcript.rs:1289-1310` | `Deserialize` na `SafeKillInspection`, `ToolDetail`, `EditDetail`; `#[serde(default)]` na `Option`/`Vec` polia |
| 3 | 2b | `mcp/tools/lifecycle.rs` | `inspect_safe_kill`, `discard_kill_session` (+ `refuse_if_operator`, `confirm_gate`) |
| 4 | 2b | `mcp/tools/orchestration.rs` | `session_tool_detail` (`resolve_target_row` + `fetch_tool_detail`) |
| 5 | 2b | `mcp/tools/session_ops.rs` | `dismiss_agent_session` (ak RN) |
| 6 | 2c | `mcp/guard.rs` | 4 riadky `TOOL_POLICIES` s `visibility: ClientOnly` |
| 7 | 2d | `mcp/tools/tests.rs` | `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` (+2 ro, +2 mut); nové: `inspect_safe_kill_reports_dirty_and_unpushed_from_one_ssh_call` (FakeSsh 1 volanie), `inspect_safe_kill_without_worktree_is_not_safe`, `discard_kill_session_refuses_a_dirty_worktree_unless_force`, `discard_kill_session_refuses_the_operator`, `session_tool_detail_reads_an_edit_and_its_result`, `dismiss_agent_session_refuses_a_working_agent`; rozpočet: agent 0, master ≤ 65 500 |
| 8 | 2e | `docs/control-api.md:250-272` | index *Steering & observing* + `session_tool_detail`; *Lifecycle & recovery* + `inspect_safe_kill`, `discard_kill_session`, `dismiss_agent_session`; `move_session` bez „master token only“ (UX-83); `REGEN_DOCS` ×2 |

**26b — routing (~160 +130):**

| # | Krok | Súbor | Zmena |
|---|---|---|---|
| 9 | 3 | `backend/verdicts.rs:171-184,253-267,317-325,332-338` | 5× `Routed`; `purge_project` text |
| 10 | 4 | `backend/remote.rs` | `inspect_safe_kill(&InspectSafeKillArgs)` (struct dostane `Serialize`), `discard_kill_session(&args, force)` (`json!`, komentár o `force`), `session_tool_detail(&SessionToolDetailArgs)` (`Serialize`; `claude_session_id` len keď je — vzor `:725-743`), `session_activity(id)` = `route_text` + `probe_from_tail`, `dismiss_agent_session(id)` |
| 11 | 5 | `commands/sessions.rs:106-133,203-213,386-452` | `mod routed` ×5; 5× `refuse_local_only` preč; `session_activity` doc-komentár prepísať |
| 12 | 6 | `backend/tests_routing.rs:285,695` | 3 Case do `routed_read_cases()` (`inspect_safe_kill` payload s `dirty_files`; `session_tool_detail` s `claude_session_id`; `session_activity` → tool `capture_session`, `Fake::answering` **textom** — prvý text Case; `route_text` precedens `worktrees.rs:89`), 2 do `routed_mutation_cases()` (`discard_kill_session` s `force: true`; `dismiss_agent_session`) |
| 13 | 9 | `backend/tests_contract.rs` | `sample_safe_kill_inspection()`, `sample_tool_detail()` do `the_whole_contract`; `REGEN_HUB_CONTRACT` ×2 |
| 14 | 7–8 | goldeny | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` ×2, diff prečítať |
| 15 | 10 (min. TS) | `src/lib/hub.ts:192-197,257-274` | `REASONS` −3 kľúče; `ROUTED_ACTIONS` + `discard_kill_session`, `dismiss_agent_session`, `resolve_move` (UX-86) |
| 16 | 10 | `hub_verdicts.test.ts:164-176` | `guardedDirectlyWithOwnsTheFleet` a `handledInlinePerClickNotPreGated` **zmazať** (prázdne skupiny test nedovolí); „the five“ → podľa D6 |
| 17 | 11 (min.) | `SessionDetails.svelte:59-61`, `SessionRowItem.svelte:143`, `ConversationPanel.svelte:720` | `hubBlock(...)` na zmazané kľúče → `hubActionBlocked(...)` / gate preč (inak `svelte-check` padne na `HubAction`) |
| 18 | 13 | `docs/hub.md:996-1002,1181+` | `repair_session` do E_CONFIRM zoznamu; bullet *What is different*: „Safe remove, Discard & kill, tool detail and the live indicator come from the hub“; *Known limitations*: `purge_project` |

~310 spolu → **odštep 26a/26b nutný** (ako 22a/22b). Závisí: UXPR-25 (`visibility` pole),
UXPR-22 (`guard.rs`, `tests.rs`, `verdicts.rs`, `remote.rs`, goldeny). Ak ADR zamietnuté: bez
`visibility`, rozpočet výnimka D4 v 26a.

### UXPR-27 — Svelte: Safe remove / Discard / Tool detail / Repair v hub režime (**M**, lane E po UXPR-07 a 26)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `SessionDetails.svelte:55-76,236-326,695-714,744-865` | gate Safe remove na `safe_kill_session`; stav `unsupported` z `hubInlineState`; Kill dialóg kópia + *Safe remove instead…*; Repair `ConfirmDialog` s hub dodatkom; Remove from list enabled; `resolve_move` gate |
| 2 | `Sidebar.svelte:841-866` | Kill/bulk kópia (zdieľaný snippet alebo `killDialogCopy`) |
| 3 | `ToolLine.svelte:44-50,104-108,164-170` | prop `detailUnsupported`; `E_FORBIDDEN` medzi neretryable |
| 4 | `ConversationPanel.svelte:692-722` | `probeLive` bez `ownsTheFleet`; `probeUnsupported`; `· not live`; `conv-detail-unsupported` riadok |
| 5 | `TerminalView.svelte:455-485` | `terminal-unchecked-workspace` riadok v hub režime |
| 6 | `TransferSheet.svelte:20,150-160` | Finish/Undo `hubActionBlocked('resolve_move')` |
| 7 | testy | `hub_disabled.test.ts:373-411` prepísať (Safe remove **enabled**, dialóg volá `inspect_safe_kill`, `E_FORBIDDEN` → `safe-kill-unsupported` + Ask Claude + Kill; Remove from list enabled); `SessionDetails.test.ts` Repair dialóg + Kill kópia; `ToolLine.test.ts` `detailUnsupported`; `ConversationPanel.test.ts` probe v remote + `not live`; `TransferSheet.test.ts` offline Finish disabled; `hub_verdicts.test.ts` `ROUTED_ACTIONS ⊇ {discard_kill_session, dismiss_agent_session, resolve_move}` |

~230 riadkov (+~200 testov). Závisí: UXPR-07 (`hub_inline_state`), UXPR-26 (routing), mäkko
UXPR-21 (readonly vopred), UXPR-02/03 (ikony Kill/Safe remove), UXPR-06 (`session_view.ts`).
∥ s UXPR-14–18, 23.

### Lanes

```
lane C  zdieľané UI   07 ∥ 21
lane D  hub Rust      25 → { 08 ∥ 09 } → 10 → 11 → 12 → 13 → 22(a→b) → 26(a→b) → [24]
lane E  hub Svelte    14 → 15 · 16 → 17 · 18 · 23 · 27 (po 07, 26; mäkko 21, 02/03, 06)
```

**Regen podľa PR:** 25 — `REGEN_DOCS`; 26a — `REGEN_DOCS`; 26b — `REGEN_LOCAL_ONLY`,
`REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT`; 27 — nič.

## Akceptačné testy

### Rust — `crates/fleet-core/src/mcp/tools/tests.rs`

- UXPR-25: `the_served_tool_list_matches_the_call_gates` (`:2020-2034`) zelený pre všetkých 5
  caller kinds po pridaní vetvy; `a_per_host_token_is_not_served_client_only_tools` (host full aj
  readonly: `visible_to == false`, `enforce_audience` → `E_FORBIDDEN` s vetou „per-host token“;
  master, client full: `true`); `client_only_tools_are_not_named_in_the_control_skill`;
  `the_served_definition_budget_stays_bounded`: agent ≤ `AGENT_BUDGET_BYTES`, master ≤
  `MASTER_BUDGET_BYTES`, `ro < agent/2`, štyri riadky výpisu.
- UXPR-26a: `every_router_tool_has_exactly_one_tool_policy_row`, `every_tool_parameter_is_documented`,
  `annotations_follow_the_policy_table` zelené po 4 tooloch (`inspect_safe_kill`,
  `session_tool_detail` s `readOnlyHint`; `discard_kill_session` s `destructiveHint`);
  `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` rozšírený; šesť nových testov z
  položky 7 (FakeSsh: `inspect_safe_kill` presne 1 `run_shell`, výstup `"?? a.txt\x1emain\x1eorigin/main\x1e2"`
  → `dirty_files.len()==1`, `unpushed_commits==2`, `safe_to_remove==false`).

### Rust — `src-tauri/src/backend/`

- `tests_routing.rs`: `every_routed_row_is_driven_by_a_case` (5 nových Case),
  `every_commands_body_does_what_its_row_says` (`routed::` v 5 telách),
  `every_routed_tool_is_a_tool_the_hub_serves` (`capture_session` pre `session_activity`),
  `every_local_only_message_is_the_one_the_fixture_records` po `REGEN_LOCAL_ONLY`;
  `session_activity_probes_the_hubs_pane_text_locally` (fake vráti 12 riadkov so spinnerom →
  `spinner: Some`, `claude_status: Some("working")`; sentinel „(session pane is empty…)“ → all-None);
  `repair_session_explicit_false_stays_local_only_in_remote_mode` (`:1629`) **nezmenený**.
- `tests_contract.rs`: `the_whole_contract` + 2 samples; golden po `REGEN_HUB_CONTRACT`.
- `tests_verdict_gen.rs`: `generated_json_is_current`, `doc_table_is_current`.

### Frontend — Vitest (`npx vitest run src/lib/hub_verdicts.test.ts src/lib/hub_disabled.test.ts src/lib/SessionDetails.test.ts src/lib/ToolLine.test.ts src/lib/ConversationPanel.test.ts src/lib/TransferSheet.test.ts src/lib/Sidebar.test.ts`)

- `hub_verdicts.test.ts`: „every other REASONS key that is a command name is local_only“ po
  zmazaní 3 kľúčov; „every local_only command is covered“ bez dvoch skupín; `ROUTED_ACTIONS`
  obsahuje `discard_kill_session`, `dismiss_agent_session`, `resolve_move`.
- `hub_disabled.test.ts:373-411` → (1) Safe remove **enabled** v remote, klik volá
  `inspect_safe_kill`, dialóg ukáže `dirty-files`; (2) `inspect_safe_kill` odpovie `E_FORBIDDEN` →
  presne jeden `safe-kill-unsupported`, `confirm-safe-kill-claude` a `kill-from-safe-dialog`
  enabled, žiadny toast; (3) `E_FORBIDDEN` s `readonly` → Discard disabled s „readonly“, Ask Claude
  disabled; (4) Remove from list enabled, volá `dismiss_agent_session`; (5) standalone nezmenené.
- `SessionDetails.test.ts`: Repair otvára `confirm-dialog` s textom „unregister“; v remote text
  obsahuje „full repair“; Kill dialóg obsahuje „stay on“; `details-finish-move` disabled pri
  `offline`.
- `ToolLine.test.ts`: `detailUnsupported` → riadok sa nerozbalí, `title` nesie vetu;
  `E_FORBIDDEN` → bez Retry.
- `ConversationPanel.test.ts`: remote → `session_activity` volaný po `ACTIVITY_POLL_MS`;
  `E_FORBIDDEN` → poll zastavený, `conv-not-live` viditeľný; standalone bez zmeny.
- `TransferSheet.test.ts`: Finish/Undo disabled s offline vetou; enabled pri `connected`.
- `Sidebar.test.ts`: Kill dialóg kópia (riadok aj bulk) obsahuje „stay on“.

### Manuálne (screenshot podľa README §1)

Hub režim, session s worktree: detail → *Safe remove* enabled → dialóg do 2 s ukáže vetvu, upstream
a špinavé súbory; *Discard & kill* odstráni worktree a riadok zmizne cez event; Conversation tab:
rozbalený `› Run …` ukáže príkaz a výsledok; hlavička ukazuje spinner počas generovania; *Repair
workspace* otvorí potvrdenie s hub dodatkom. Nový screenshot `07-safe-remove-hub.jpg`.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| UXPR-25 ADR + `Visibility` + testy + docs | S/M | ~170 riadkov + testy ~120 + generovaný reference |
| UXPR-26a fleet-core (params, derives, 4 tools, policy, testy, reference) | S/M | ~150 + testy ~100 |
| UXPR-26b routing (verdikty, remote ×5, routed ×5, Case ×5, kontrakt, goldeny, min. TS, docs) | M | ~160 + testy ~130 |
| **UXPR-26 spolu** | **M** | ≈310 → odštep a/b |
| UXPR-27 komponenty (SessionDetails, Sidebar, ToolLine, ConversationPanel, TerminalView, TransferSheet) | M | ~230 |
| UXPR-27 testy | M | ~200 |
| **UXPR-27 spolu** | **M** | ≈230 (+200) |

Spolu ≈ **710** riadkov diffu (+≈550 testov) — v súčte fronty 3 660 (po 06) → ≈ 4 370. Verdikty
po bloku: LocalOnly **42**, Routed **67**, RoutedUnless 1, SameInBoth 20 (zo 130).

## Otázky pre vlastníka

Iba to, čo konsolidácia-01 ani 06 nerozhodli; „default“ = prijať odporúčania.

1. **`discard_kill_session` s `confirm: true`** — odchýlka od D3 („`confirm: false` na hube“).
   Tool skladá `kill_session` (confirm) a `delete_worktree` (confirm); s `confirm: false` by bol
   jediný nekonfirmovaný spôsob, ako zahodiť worktree a zabiť session, keď operátor hubu zapol
   `mcp.confirm_destructive`. *Odporúčanie: `confirm: true` — pravidlo „skladaný tool nesmie obísť
   bránu súčastí“ zapísať do D3 ako výnimku; `inspect_safe_kill`, `session_tool_detail`,
   `dismiss_agent_session` ostávajú `confirm: false`.*
2. **`dismiss_agent_session`: RN (nový tool, +280 B master, 0 B agent) alebo RE cez
   `kill_session` (0 B; na working agentovi by hub agenta zastavil, standalone odmietne — jediná
   ochrana je UI gate `isInactiveAgent`)?** *Odporúčanie: RN — rovnaká sémantická medzera, pre ktorú
   `repair_session` ostal `RoutedUnless`; pod ADR je cena pre agenta nulová.*
3. **Dva rozpočtové stropy** (`AGENT_BUDGET_BYTES` tvrdý ≈ 51 000 na host-full plochu,
   `MASTER_BUDGET_BYTES` mäkký 65 500) namiesto jedného 63 800 z D4; UXPR-25 pred UXPR-09.
   *Odporúčanie: áno — inak je 07 tretí zdvih a 24 štvrtý.*
4. **`inspect_safe_kill` ako T0 (`readonly: true`)** — `readonly` telefón uvidí názvy špinavých
   súborov a vetiev. Precedens `capture_session` (T0) ukazuje celý pane. *Odporúčanie: T0.*
5. **`purge_project` Master tool** (06 otázka 6 to posunula sem). Nevratné mazanie Claude stavu
   na všetkých hostoch; hub operátor to dnes nemá kde spraviť okrem standalone appky.
   *Odporúčanie: nie teraz — kôš skryť (UXPR-23); tool do backend backlogu s `confirm: true`,
   T3, až keď ho niekto potrebuje z hubu.*
6. **Retroaktívne `ClientOnly` pre existujúce tools** (`list_host_worktrees`, `delete_worktree`,
   `resolve_move`, `related_sessions`, `session_conversations`, `ensure_operator`/`operator_status`)?
   Každý by agentovi ubral ~300–700 B. *Odporúčanie: nie v UXPR-25 (os bez zmeny správania);
   samostatný S pass po lane D s testom proti `SKILL.md`.*
7. **Starší hub bez `inspect_safe_kill`:** dialóg ponúkne *Ask Claude* + *Kill* (odporúčané), alebo
   iba *Ask Claude* (konzervatívne)? *Odporúčanie: oboje — Kill je aj tak o tlačidlo vedľa; v dialógu
   je aspoň s vetou o worktree (UX-78).*

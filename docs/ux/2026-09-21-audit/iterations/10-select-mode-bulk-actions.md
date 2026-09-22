# Iterácia 10 — Select mode a hromadné akcie

**Šošovka:** multi-select v sidebare, bulk lišta, hromadný prompt / kill / tag / move, výber
klávesnicou, potvrdenia, správanie v hub režime · **Zasahuje:** UX-10 (+ UX-93 z iterácie 08,
UX-35 z 02, UX-78/84 zo 07, FE-4 z plánu 2026-09-10) · **Vstup:** README, screenshot 01,
`iterations/consolidation-01.md` (D1–D6, „default“ na všetky otázky), `08-sidebar-ia.md`
(bulk swap, UXPR-28), `09-session-row.md` (UXPR-32/33, poradie v `SessionRowItem.svelte`),
`02-session-naming.md` (P2, „broadcast nikdy nepomenúva“, B5), `07-hub-parity-sessions.md`
(UXPR-26/27), kód na `3a2eab67` · **Režim:** read-only, žiadne zmeny kódu, žiadny `cargo`;
`npx vitest run src/lib/Sidebar.test.ts src/lib/selection.test.ts src/lib/hub_verdicts.test.ts`
→ 105/105 zelených (východiskový stav). Nálezy od **UX-112**, PR od **UXPR-36**.

## Zhrnutie

1. **UX-10 je potvrdené iba v polovici.** Bulk lišta **existuje** (`SidebarFilters.svelte:163-182`,
   testid `bulk-bar`, `role="toolbar"`, `N selected · → Send prompt · × Kill · clear`), hromadný
   Kill aj hromadný prompt sú implementované a otestované (`Sidebar.test.ts:1148-1188`). Čo
   platí: lišta sa vykreslí **až pri `selectedCount > 0`**, takže zapnutý select mód s nulovým
   výberom nedá žiadny hint, čo výber umožní (UX-112). Checkbox **je** skutočný
   `<input type="checkbox">` s `aria-label` (`SessionRowItem.svelte:179-185`), nie štylizovaný
   `div` — auditový AX klik zlyhal na WKWebView bridgingu (UX-28) a na vnorení inputu do
   `role="button"` riadku (priznaný smell, `:173-177`), nie na chýbajúcom form controle.
2. **Výber je dnes „toggle-only“:** shift, ⌘ aj ctrl-klik robia to isté ako klik v select móde —
   prepnú jeden riadok (`Sidebar.svelte:461-463`). Žiadny rozsah, žiadna kotva, žiadne „vybrať
   všetko v projekte“, žiadny Escape (`:828-830` zatvára iba picker), klávesnica má iba Enter/Space
   na fokusovanom riadku (`:479-484`). Stav žije ako lokálny `Set<number>` v `Sidebar.svelte`
   (`:157`), nie v `selection.ts` — ten drží jediný `selectedSession` pre stredný panel a hromadný
   výber ho **nemení** (test `:1158` to pinuje).
3. **Dve nálezy menia, čo sa dá bezpečne hromadne robiť:** ghost riadky **sú** v select móde
   voliteľné (`SessionRowItem.svelte:168` `|| selectMode`; `toggleSelected` vyraďuje iba
   `external`, `Sidebar.svelte:164`) a hromadný Kill ich pošle do `kill_session` (UX-114); riadky
   skryté filtrom hosta / času / bg / Needs you **ostávajú vybrané** — lišta hlási „3 selected“ pri
   jednom viditeľnom a Kill zabije aj neviditeľné (UX-115).
4. **Hromadný prompt = N × `send_prompt`** (`BulkPromptDialog.svelte:29-33`), nie
   `broadcast_prompt`. Dôsledok: pomenuje N sessions rovnako (UX-35, kým UXPR-04 nezavedie
   `label: false`), nemá progres ani súhrn po zatvorení (dialóg sa zatvorí po 600 ms iba ak všetko
   prešlo, `:37`). Premisa SEC-5 „broadcast bez rate limitu a potvrdenia“ **pre dnešný kód
   neplatí** (`guard.rs:28-32` interval 30 s, `:398-404` `confirm: true`) — ale UI ho aj tak
   nepoužíva a **nemôže**: `broadcast_prompt` cieli filtrom host/projekt/status
   (`prompt.rs:266-271,281-315`), nie zoznamom id, a iba `kind == "work"`. Odporúčanie: ostať pri
   N × `send_prompt` s explicitným `label: false` (UXPR-38), nie broadcast.
5. **Tag a Move hromadne nie sú dostupné a v tejto iterácii ich neodporúčam:** `set_session_tags`
   je iba MCP tool (`guard.rs:501-507`, `orchestration.rs:377`), **nemá Tauri príkaz** ani UI
   (v `hub_verdicts.generated.json` chýba) — hromadný Tag znamená nový príkaz + routing v lane D;
   Transfer sheet je jedno-sessionový (`App.svelte` montuje jeden `<TransferSheet />`, spec
   `2026-09-20-transfer-sheet-design.md:254`; `canMoveSession` `moveEligibility.ts:11-13`).
6. **Návrh** sedí do IA iterácie 08: select mód sa zapne ikonou `IconSelect` **alebo automaticky
   prvým ⌘/shift-klikom**; bulk lišta **nahrádza riadok 2 hlavičky** hned po zapnutí módu (aj pri
   0 vybraných, s hintom), obsah `[3 selected · 1 hidden] [→ Prompt] [× Kill ▾] … (☐) [✕]`;
   `Kill ▾` = Kill / Safe remove… / Discard & kill… s per-akciou „disabled s dôvodom“ podľa mixu
   výberu (druh, stav, hub verdikt); tri-stavový checkbox v hlavičke projektu; shift-klik = rozsah,
   ⌘-klik = toggle, Space / Shift+Space / ↑↓ / Shift+↑↓ / ⌘A / Escape; jeden potvrdzovací dialóg so
   zoznamom (`displayName`, host, stav, riziko z `inspect_safe_kill`, kde je dostupný) a toast s
   počtom výsledkov; neúspešné riadky ostanú vybrané.
7. **Tri PR** v lane F: **UXPR-36** model výberu (`bulk_selection.ts`, rozsah, klávesnica, prune;
   S/M ~140), **UXPR-37** `BulkBar` v slote riadku 2 + `bulk_actions.ts` eligibility +
   `BulkConfirmDialog` + toast (M ~280, pripravený odštep 37a/37b), **UXPR-38** hromadný prompt:
   `label: false` na drôte, progres, čiastočné zlyhanie (S ~110, jediný s Rustom).
   Dvanásť nových nálezov **UX-112…UX-123** (M 6 · L 6).

## Inventár select módu

| # | Prvok | Kde | Správanie dnes | Hub | Test |
|---|---|---|---|---|---|
| 1 | Vstup / výstup | `☑ select` pill `SidebarFilters.svelte:150-159` (`aria-pressed`, `title` „Select several sessions (or shift/cmd-click rows) for bulk actions“); `toggleSelectMode` `Sidebar.svelte:173-176` | zapnutie ukáže checkboxy v každom ne-`readOnly` riadku (`SessionRowItem.svelte:172`); vypnutie **vyčistí výber**; žiadna skratka, žiadny Escape (`Sidebar.svelte:828-830` iba `showProjectPicker`) | lokálne | `Sidebar.test.ts:1171-1188` |
| 2 | Čo vyberá myš | `onSelectSession` `Sidebar.svelte:458-477` | v select móde **každý klik** riadku prepína (neotvára); mimo módu shift / ⌘ / ctrl-klik prepína **jeden** riadok (`:461-463`) — žiadny rozsah, žiadna kotva; checkbox volá `toggleSelected` a `stopPropagation` (`SessionRowItem.svelte:184`) | — | `:1148-1169` (shift + meta = 2 riadky) |
| 3 | Čo vyberá klávesnica | `onKeySession` `Sidebar.svelte:479-484` | Enter / Space na fokusovanom riadku → `onSelectSession(sess)` bez eventu → v select móde toggle, inak otvorí; riadky sú `tabindex="0"` (`SessionRowItem.svelte:166`) → Tab prechádza všetky; žiadne ↑↓, j/k (terminál ich posiela do PTY, `terminal_keys.ts:65-66`), Shift+Space, ⌘A | — | žiadny |
| 4 | Checkbox | `SessionRowItem.svelte:172-186` | `<input type="checkbox" class="select-box" checked={isChecked} aria-label="Select {tmux_name}">` **vnorený v `role="button"` riadku** (komentár `:173-177` to priznáva a odkazuje na „F5 sidebar split“); CSS `.select-box` `:461`, `.sess-row.checked { outline: 1px solid var(--accent) }` `:462` | — | `:1176-1181` |
| 5 | Kto sa **nedá** vybrať | `toggleSelected` `Sidebar.svelte:162-169`; render `{#if selectMode && !readOnly}` `SessionRowItem.svelte:172` | iba `kind === 'external'` (Outside fleet); **ghost riadky sú voliteľné** — `onclick={… (sess.status !== 'ghost' \|\| selectMode) …}` `:168` ich v select móde prepúšťa | — | `:1412-1436` (external) |
| 6 | Stav výberu | `selectMode`, `selectedIds: Set<number>`, `selectedRows` `Sidebar.svelte:156-160` | lokálny `$state`, kľúč `session.id`; `$effect` `:179-184` vyhodí id, ktoré opustili `$sessions`; id je stabilné naprieč reconcile (`ON CONFLICT(host_alias, tmux_name)` `store/sessions.rs:55,131`) → výber **prežije** merge aj `list_sessions` re-fetch, stratí sa iba pri kill + re-discovery (správne). **Neprežije** vypnutie módu; **nezohľadňuje** filtre (bod 13) | — | nepriamo `:1168` |
| 7 | Vzťah k `selectedSession` | `selection.ts:25-48` (jediný `SessionRef`, derived z `$sessions`) | hromadný výber ho **nemení**; klik v select móde neotvorí session; otvorená session ostáva `.selected` a ukazuje hover akcie (`SessionRowItem.svelte:509-510`) aj počas výberu | — | `:1158` `expect(get(selectedSession)).toBeNull()` |
| 8 | Bulk lišta | `SidebarFilters.svelte:163-182` | `{#if selectedCount > 0}` → `div.bulk-bar[role=toolbar][aria-label="bulk actions"]`: `N selected` (`flex: 1`), `→ Send prompt`, `× Kill` (`.pill.danger`), `clear`; **vložená medzi `nav.triage` a `nav.bg-toggle`** → skok ≈ 28 px (UX-93); CSS `:275-285` | `bulkSendBlocked = hubActionBlocked('send_prompt')`, `bulkKillBlocked = hubActionBlocked('kill_session')` `:14-15` → oba Routed, gate iba na stav spojenia; `disabled` + `title` | `:1160-1168, 1182-1187`; **`hub_disabled.test.ts` nemá žiadny bulk prípad** |
| 9 | Hromadný prompt | `BulkPromptDialog.svelte` (132 r.) | `Modal` 520 px; `sendable = kind !== 'shell' && status === 'running'` (`:22`), preskočené riadky sú `skipped` s `title` (`:46-51`); `Promise.allSettled(N × sendPrompt)` (`:29-33`) → `send_prompt` Tauri príkaz; ✓ / ✗ per riadok; ak 0 chýb, `setTimeout(onClose, 600)` (`:37`), inak dialóg ostane; **meno riadku `friendly_name ?? tmux_name`** (`:49`, tretí fallback, UX-40); žiadny toast, žiadny progres, výber sa nevyčistí | `send_prompt` Routed (`hub_verdicts.generated.json:108`); prompt z desktopu nesie marker, ak klient nie je trusted (`control-api.md:601-606`) | `Sidebar.test.ts:1182-1187`; **`BulkPromptDialog.test.ts` neexistuje** |
| 10 | Hromadný Kill | `Sidebar.svelte:854-866` (dialóg), `confirmBulkKill` `:186-203` | `ConfirmDialog danger` s titulkom `Kill N sessions?`, telo vymenuje `<code>{tmux_name}</code> on <code>{host}</code>` a „lose any running claude state inside them“ — **mlčí o worktree** (UX-78); `clearSelected()` **pred** killmi (`:189`); `Promise.allSettled(N × killSession)`; `pushError` per zlyhanie, `forgetSessionUi`, `selectSession(null)` ak bola otvorená; **žiadny súhrnný toast**; iba `kill_session` — Safe remove / Discard hromadne nie sú | `kill_session` Routed, `confirm: true` (`guard.rs`) → s `mcp.confirm_destructive` na hube N × `E_CONFIRM_REQUIRED` → N toastov s `hubNextStep` (`hub.ts:336-365`, UX-84 × N) | `:1162-1168` |
| 11 | Tag | — | `SessionRow.tags` existuje (`sessions.ts:81-82`), tool `set_session_tags` je Client/mut/Quick (`guard.rs:501-507`, `orchestration.rs:377-399`, „Replace a session's tags … up to 16“ `control-api-reference.md:414-416`); **žiadny Tauri príkaz, žiadne UI**, ani jednotlivé | mimo verdiktov | — |
| 12 | Move | `moveEligibility.ts:11-25`, `moves.ts:1-5` („one run per moving session“), `TransferSheet` | iba z detailu / chipu, jedna session; sheet zobrazuje jeden beh | `move_session` Routed, `confirm: true` | — |
| 13 | Interakcia s filtrami | `filteredSessionsByProject` `Sidebar.svelte:310-312` (`buildSessionsByProject(hostFilter, showBgAgents, rowPredicate)`), `recency` na projekt | výber je množina id nad **celým** `$sessions`; zmena filtra skryje riadky, ale nechá ich vybrané; lišta ani dialóg to nehlásia | — | žiadny |
| 14 | Ikony a triedy | `☑` (zdieľané s Tasks, UX-33), `→`, `×`; `.pill` lokálne (UX-95) | iterácia 01: `IconSelect` (`square-check`), `IconSend`, `IconKill` (`circle-x`), `IconClose` (`x`), `IconSafeRemove` (`shield-check`, A3), `IconCaret` | — | — |

**Prečo audit „nevidel lištu“:** na screenshote 01 je select mód vypnutý; audit ho zapol
(checkboxy sa objavili) a klikol na checkbox cez accessibility — klik neprešiel (UX-28: AX strom
okna má 4 elementy, WKWebView nebridguje vnorené `<input>` v `role="button"`), takže
`selectedCount` ostal 0 a lišta sa nevykreslila. Oba javy spolu vytvorili dojem „žiadna bulk
lišta“. Myšou (`fireEvent.click` v teste) to funguje.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-10 | **Opravené (potvrdené s korekciou premisy), M ostáva.** Bulk lišta existuje, ale iba pri `selectedCount > 0` (`SidebarFilters.svelte:163`); zapnutý mód s 0 vybranými nemá žiadny hint (nová formulácia = UX-112). „Nikde sa neobjaví lišta hromadných akcií“ **vyvrátené**; „ani hint, čo sa dá s výberom robiť“ **potvrdené**. Checkbox je form control (`SessionRowItem.svelte:179`), nie div. | `Sidebar.test.ts:1171-1188`; screenshot 01 |
| UX-93 (iter. 08) | **Potvrdené.** `{#if selectedCount > 0}` blok medzi `nav.triage` (`:88-160`) a `nav.bg-toggle` (`:184`); `.bulk-bar` má vlastný `padding`, `border`, `background` → nová výška, nie swap. Rieši UXPR-28 (slot); obsah tu. | `SidebarFilters.svelte:163-182,275-285` |
| UX-35 (iter. 02, časť broadcast) | **Potvrdené a spresnené.** UI bulk prompt **nejde** cez `broadcast_prompt`, ale cez N × `send_prompt` (`BulkPromptDialog.svelte:29-33`) → `record_prompt_outcome` pre každý riadok → N rovnakých mien. Rozhodnutie B5 („broadcast nikdy nepomenúva“) pokrýva iba MCP cestu; UI cesta potrebuje vlastný `label: false` (UXPR-38). | `prompt.rs:342-378`; `Sidebar.svelte:869-871` |
| UX-78 (iter. 07) | **Potvrdené pre bulk.** Kópia bulk Kill dialógu (`Sidebar.svelte:863-865`) = kópia riadku (`:849-850`) = detail (`SessionDetails.svelte:753-754`); o worktree ani slovo; UXPR-27 položka 2 plánuje zdieľaný `killDialogCopy` pre `Sidebar.svelte:841-866` — **kolízia s UXPR-37**, poradie nižšie. | |
| UX-84 (iter. 07) | **Potvrdené × N.** S `mcp.confirm_destructive` na hube prejde desktopov dialóg, potom N × `E_CONFIRM_REQUIRED` → N toastov (`pushError` per riadok, `Sidebar.svelte:194`). | `hub.ts:336-365` |
| SEC-5 (plán 2026-09-10 `:74`, B5 `:196`) | **Vyvrátené pre dnešný kód** — `broadcast_prompt` má rate limit (`guard.rs:28-32`, default 30 s, `E_RATE_LIMITED` + `retry_after_secs` `messaging.rs:84-97`), `confirm: true` (`guard.rs:398-404`, nonce s digestom promptu `control-api.md:612-615`) a marker (`messaging.rs:101`). B5 je landed. Pre túto šošovku dôležité: UI ho nepoužíva, a N × `send_prompt` z UI **nemá** rate limit — správne (je to akcia človeka s potvrdením), ale hromadný prompt musí mať vlastný súhrn a progres. | |
| FE-4 / D2 (plán) | **Landed čiastočne.** „multi-select with bulk kill and send-prompt“ je hotové; „status sort“ nie (UX-92, iter. 08). | `Sidebar.svelte:154-160` komentár „Multi-select for bulk Kill / Send prompt“ |
| README §5 šošovka 10 „bulk bar, klávesnica, potvrdenia“ | lišta existuje; klávesnica **chýba** (bod 3 inventára); potvrdenie iba pre Kill, bez rizika worktree | |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz | Návrh |
|---|---|---|---|---|
| **UX-112** | **M** | **Select mód bez výberu nemá afordanciu.** Zapnutie pill-u ukáže checkboxy, nič viac: lišta až pri N > 0, pill `title` je jediný text; hint `session-actions` (`hints.ts:37-38`) hovorí o hover akciách, nie o výbere. Používateľ nevie, že ho čaká Kill / Send, a že ⌘-klik funguje aj bez módu. | `SidebarFilters.svelte:150-159,163`; screenshot 01 (mód vypnutý → žiadny náznak, čo `select` robí) | Lišta nahradí riadok 2 **hneď po zapnutí módu**: `0 selected · click rows or press Space` + disabled akcie; prvý ⌘/shift-klik mimo módu mód **zapne** (checkboxy + lišta prídu s výberom) — UXPR-37 (+36) |
| **UX-113** | **M** | **Shift-klik nie je rozsah.** `shiftKey \|\| metaKey \|\| ctrlKey` → jeden `toggleSelected` (`Sidebar.svelte:461-463`). Platformová konvencia (Finder, Mail, každý listbox): shift = rozsah od kotvy, ⌘ = toggle. Pri 12 sessions v projekte je to 12 klikov. Test `:1155-1156` dnešné správanie pinuje. | `Sidebar.svelte:458-463`; `Sidebar.test.ts:1148-1169` | Kotva = posledný riadok, ktorý bol toggle-nutý; shift-klik / Shift+Space / Shift+↑↓ vyberie rozsah v **poradí viditeľných riadkov** (poradie z `filteredSessionsByProject` + orphan sekcia, `Sidebar.svelte:691-750`); ⌘/ctrl = toggle (UXPR-36) |
| **UX-114** | **M** | **Ghost a bg riadky prejdú do hromadných akcií bez rozlíšenia.** Ghost je v select móde klikateľný (`SessionRowItem.svelte:168`), `toggleSelected` vyraďuje iba `external` (`Sidebar.svelte:164`) → bulk Kill volá `kill_session` na stratenú tmux session (správna akcia je Dismiss / Recreate); bulk Kill dialóg ho vymenuje ako živý. `bg` riadky (`hasNoPane`, `sessions.ts:556-558`) `kill_session` zvládne (pane-less vetva `lifecycle.rs:864-873`), no dialóg promptu ich prepustí filtrom `kind !== 'shell' && status === 'running'` (`BulkPromptDialog.svelte:22`) bez toho, aby bolo jasné, či `send_prompt` pane-less riadok obslúži; `shell` riadky sú v dialógu „skipped“ (`:46-51`), v Kill nie. Tri druhy + ghost, jedno pravidlo per akcia, roztrúsené v dvoch súboroch. | `SessionRowItem.svelte:168`; `Sidebar.svelte:162-169,186-203`; `BulkPromptDialog.svelte:22` | Jedna funkcia `bulkEligibility(rows)` v `bulk_actions.ts`: pre každú akciu `{ ok: SessionRow[], skipped: { row, reason }[] }` (Prompt: `work`/`review`/`bg` running, nie ghost/shell; Kill: všetko okrem ghost/external; Safe remove: `work`+`worktree_id`+running; Discard: ako Safe); lišta ukáže `Kill (2 of 3)` a `title` s dôvodom; ghost riadok v select móde **nie je voliteľný** (jeho akcie sú Recreate/Dismiss) — UXPR-37 |
| **UX-115** | **M** | **Skryté riadky ostávajú vybrané.** Výber je `Set<id>` nad celým `$sessions`; host `<select>`, čas, `bg off`, Needs you skryjú riadky, lišta hlási `3 selected` pri jednom viditeľnom a Kill dialóg vymenuje aj neviditeľné (používateľ číta mená, nie riadky). Opačný extrém — ticho vyhodiť skryté — by stratil výber pri každom prepnutí hosta. | `Sidebar.svelte:157,179-184,310-312`; `sidebar_index.ts:29-34` | Lišta: `3 selected · 2 hidden` (`hiddenCount = selected − visible`), `title` „hidden by the host/time filter — still included“; potvrdzovací dialóg vymenuje **všetky** s badge `hidden`; ⌘A vyberá iba viditeľné; výber sa nikdy ticho nemení filtrom (UXPR-36 počet, 37 zobrazenie) |
| **UX-116** | **M** | **Hromadný prompt pomenuje N sessions rovnako a nehlási výsledok.** N × `send_prompt` → `record_prompt_outcome` per riadok (UX-35); po úspechu dialóg zmizne po 600 ms bez toastu; pri chybe ostane otvorený s ✗ per riadok, ale výber je netknutý a nič nehovorí „2 of 5 failed“; žiadny progres počas `Promise.allSettled` (tlačidlo `Sending…`). `submit` je vždy `true`. | `BulkPromptDialog.svelte:25-38,55-57`; `prompt.rs:133-195` | `sendPrompt(host, name, prompt, { label: false })` → `SendPromptArgs.label: Option<bool>` `#[serde(default)]` (UXPR-38, alebo v UXPR-04, ak ešte nie je zlúčené); progres `2 / 5` v tlačidle, po dokončení toast `Sent to 4 · 1 failed`, neúspešné riadky ostanú vybrané, dialóg sa zatvorí tlačidlom, nie časovačom |
| **UX-117** | L | **Bulk Kill zahodí výber pred výsledkom a hlási iba zlyhania.** `clearSelected()` na `:189` beží pred `Promise.allSettled`; úspech nemá toast; zlyhané riadky sa nedajú „skúsiť znova“ inak než novým výberom. Jednotlivý Kill (`confirmKill` `:547-566`) sa správa rovnako, ale tam je jeden riadok. | `Sidebar.svelte:186-203` | `runBulk(action, rows)`: čistí výber **po** výsledku, zlyhané id ostanú vybrané (a lišta hlási `1 failed`), toast `Killed 2 sessions` / `Killed 2 · 1 failed` s akciou *Retry* (`toasts.ts:14-17` `ToastAction`) — UXPR-37 |
| **UX-118** | L | **Escape nič nerobí.** `svelte:window onkeydown` zatvára iba picker (`:828-830`); mód sa vypína iba pill-om, `clear` čistí výber, ale mód ostáva; `Modal` má vlastný Escape (`Modal.svelte:5-8`), takže kolízia nehrozí. | `Sidebar.svelte:828-830` | Escape s fokusom v sidebare a mimo editovateľného cieľa: N > 0 → vyčisti výber; N = 0 → vypni mód (dvojkrok ako v Finderi); rešpektovať `e.defaultPrevented` a `closest('dialog')` ako `App.svelte:500-512` — UXPR-36 |
| **UX-119** | L | **Prístupnosť výberu.** `<input>` vnorený v `role="button"` (`SessionRowItem.svelte:172-186`, priznané); zoznam nemá `aria-multiselectable`, riadok nemá `aria-selected`/`aria-checked`; `.bulk-count` nie je `aria-live` (zmena počtu sa nečíta); toolbar bez šípkovej navigácie (`role="toolbar"` ju očakáva). Iterácia 09 (UX-107) rieši fokus riadku, nie výber. | `SessionRowItem.svelte:155-186`; `SidebarFilters.svelte:164-165` | Checkbox ako **sused** tlačidla riadku (`<div class="sess-row-wrap"><input …><div role="button" …>`), `aria-label` z `displayName`; `ul.tree` `aria-multiselectable={selectMode}` s `aria-selected` na riadku (pri `role="button"` použiť `aria-pressed`? — nie: `aria-checked` patrí checkboxu, riadok ostane bez); `.bulk-count` `aria-live="polite"`; ← → v lište — UXPR-37 (po UXPR-33, spoločný `SessionRowItem.svelte`) |
| **UX-120** | L | **Hub: N toastov za jedno potvrdenie a slepota na `confirm_destructive`.** Gate lišty je iba stav spojenia (`hubActionBlocked`, Routed); s `mcp.confirm_destructive` zapnutým na hube dostane každý z N killov `E_CONFIRM_REQUIRED` → N × `pushError` s celou vetou `hubNextStep` (UX-84 × N). Desktop nevie vopred, že hub potvrdenie vyžaduje (03 Q4 navrhuje derived kľúč v `get_fleet_settings`, UXPR-09). | `Sidebar.svelte:191-195`; `hub.ts:336-365`; `guard.rs:24-26` | `runBulk` zoskupí výsledky podľa `error.code`: jeden toast „3 kills are waiting for approval on the hub — this window will follow“; keď UXPR-09 dodá `mcp.confirm_destructive` do `get_fleet_settings`, lišta ukáže `Kill ▾` s `title` „needs approval on the hub“ vopred — UXPR-37 (mäkko 09) |
| **UX-121** | L | **Tri výrazy pre meno v jednom toku:** Kill dialóg `tmux_name` (`Sidebar.svelte:865`), prompt dialóg `friendly_name ?? tmux_name` (`BulkPromptDialog.svelte:49`), checkbox `aria-label="Select {tmux_name}"` (`SessionRowItem.svelte:183`) — rodina UX-40. | | `displayName(sess)` z UXPR-06 (`session_name.ts` podľa UX-111) vo všetkých troch; tmux meno do `title` — UXPR-37/38 |
| **UX-122** | L | **Test medzery.** `hub_disabled.test.ts` (493 r.) nemá bulk prípad (disabled `bulk-send`/`bulk-kill` pri odpojenom hube nie je pinnuté); `BulkPromptDialog` nemá vlastný test (`sendable` filter, čiastočné zlyhanie, auto-close); `Sidebar.test.ts:1148-1188` netestuje Escape, klávesnicu, ghost v select móde, skryté riadky. | `ls src/lib/*.test.ts` | Testy v PR pláne nižšie |
| **UX-123** | **M** | **Žiadne „vybrať všetko“.** Ani v projekte (hlavička `proj-row` `Sidebar.svelte:695-724` má `+` a kôš), ani viditeľné (⌘A), ani „všetky s týmto stavom“ (Needs you filter + výber by to dal zadarmo). Hromadné akcie sú dnes drahšie než N jednotlivých klikov na `×`. | `Sidebar.svelte:695-724`; `SidebarFilters.svelte:88-160` | Tri-stavový checkbox v `proj-row` v select móde (`indeterminate` pri čiastočnom výbere, `aria-label="Select all in {repo}"`), vyberá **viditeľné eligible** riadky projektu; ⌘A s fokusom v `.sidebar` → všetky viditeľné; kombinácia s `Needs you` = „vyber všetko, čo na mňa čaká“ — UXPR-36 (model) + 37 (checkbox) |

Číslovanie: iterácia 09 končí UX-111; táto používa **UX-112…UX-123**. Bez zmeny závažnosti UX-10
(M); UX-114, 115, 123 sú M, lebo menia, **čo** sa hromadne zabije, nie iba ako to vyzerá.

## Návrh

### Princípy

1. **Výber je vždy viditeľný a vždy má lištu.** Mód bez výberu ukazuje lištu s hintom; výber bez
   módu (⌘-klik) mód zapne. Nikdy „checkboxy, ale nič viac“ (UX-112).
2. **Lišta mení interakciu, nie layout** — nahrádza riadok 2 hlavičky v tej istej výške 24 px
   (iterácia 08, UX-93); `IconSelect` toggle ostáva na svojom mieste vpravo, aby mal používateľ
   kotvu, kde mód vypnúť.
3. **Každá akcia hovorí, na koľko riadkov sa vzťahuje a prečo nie na všetky** — `Kill (2 of 3)`,
   `title` s dôvodom; nikdy ticho preskočiť (dnešné `skipped` v dialógu je správny smer, iba
   neskoro).
4. **Platformové konvencie výberu**: shift = rozsah, ⌘ = toggle, Space = toggle fokusovaného,
   Escape = zruš výber → vypni mód, ⌘A = všetko viditeľné. Žiadne písmenové skratky (j/k) — sidebar
   ich nemá a terminál ich vlastní.
5. **Jedno potvrdenie, jeden súhrn.** Dialóg vymenuje riadky s rizikom; po akcii jeden toast s
   počtami; zlyhané ostanú vybrané.
6. **Hromadné = to isté, čo jednotlivé, N-krát** — rovnaké príkazy (`send_prompt`, `kill_session`,
   `safe_kill_session`, `discard_kill_session`), rovnaké verdikty, rovnaká kópia (zdieľaná s
   UXPR-27), žiadny „bulk“ backend okrem `label: false`.

### Bulk lišta — 280 px (`1rem = 14px`, riadok 2 hlavičky podľa iterácie 08)

```
┌──────────────────────────────────────────────────────────────┐
│ [ Search sessions, projects…              / ]  (↻) (⚙) (‹)   │  riadok 1 (bez zmeny)
│ [☑ 3 · 1 hidden] [→ Prompt] [× Kill ▾]         (☑) [✕]      │  riadok 2 = BulkBar (24 px)
└──────────────────────────────────────────────────────────────┘
   ▲ .tag, aria-live   ▲ btn--chip  ▲ btn--chip.btn--crit  ▲ toggle  ▲ btn--icon (clear/exit)
```

- Šírka: 280 − 2 × 7 padding = 266 px. Počet ≈ 78 (`3 · 1 hidden`; bez skrytých ≈ 50), Prompt ≈ 62,
  Kill ▾ ≈ 66, toggle 24, ✕ 24, 4 × 6 gap = 24 → **≈ 278 pri skrytých / 250 bez**. Pri
  `hidden > 0` a 280 px sa count skráti na `3 (+1)` s plným textom v `title`. Žiadny `flex-wrap`
  (princíp 2 iterácie 08).
- **Mód zapnutý, 0 vybraných:** `[☐ 0 selected · click rows or press Space]` + akcie `disabled`
  (`title` „Select at least one session“), `✕` vypne mód.
- **`✕`** = `clear` aj `exit`: pri N > 0 vyčistí výber (lišta ostane), pri 0 vypne mód (rovnako ako
  Escape). Dnešný `clear` pill sa tým ruší (testid `bulk-clear` ostáva na `✕`).
- **`Kill ▾`** (`aria-haspopup="menu"`, panel ako `.picker` `Sidebar.svelte:1075-1086`, vzor
  overený vo WKWebView): `Kill (3)` · `Safe remove… (2)` · `Discard & kill… (2)`; každá položka
  disabled s dôvodom podľa eligibility a hub verdiktu (tabuľka nižšie). Klik na hlavnú časť `Kill`
  otvorí rovno Kill potvrdenie (split ako `+ New session ▾` z UXPR-29).
- **`→ Prompt`** otvorí `BulkPromptDialog` (UXPR-38) s predvyplneným počtom `Send to 3`.
- Ikony: `IconSelect` (`square-check`, aktívny stav `aria-pressed`), `IconSend`, `IconKill`
  (`circle-x`), `IconSafeRemove` (`shield-check`), `IconCaret`, `IconClose` (`x`). Triedy iba z
  `controls.css` (`.btn--chip`, `.btn--toggle`, `.btn--crit`, `.btn--icon`, `.tag`), žiadny `.pill`.

### Bulk lišta — 380 px

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ [ Search sessions, projects…                                / ]  (↻) (⚙) (‹)         │
│ [☑ 3 selected · 1 hidden] [→ Prompt (3)] [× Kill ▾ (2 of 3)]           (☑) [✕]       │
└──────────────────────────────────────────────────────────────────────────────────────┘
                                                        ▲ „2 of 3“ = eligibility, title = dôvod
```

### Zoznam v select móde (280 px, riadok podľa UXPR-33)

```
│ ▾ [◩] papayapos-backend                                     4  (+)   │  ◩ = indeterminate
│   [x] ● check PD-2939 vat sums status…            [⚡ working]       │  checked: outline accent
│       claude-fleet-trn · hazy-pluto             53% · PR✓ · $4.28   │
│   [ ] ● indigo cosmos                               [idle 42m]       │
│   [x] ● unblock PD-2592 service access merge        [idle 15m]       │
│   [ ] ● fix wrapped alcohol report PDF              [idle 1h]        │
│ ▾ [ ] kuk-agent                                             1  (+)   │
│   [ ] ● yes                                        [~97% ▲] [idle]   │
│ ▾ Outside fleet (2)                                                  │  bez checkboxov (readOnly)
│       ▢ claude-fleet · 62e738aa                     [⚡ working]      │
│   ○ dev-foo--main                                     [lost 5m]      │  ghost: bez checkboxu (UX-114)
```

- Checkbox je **sused** riadkového tlačidla (UX-119), zarovnaný na L1 (iterácia 09 to výslovne
  žiada zachovať); hover akcie riadku sa v select móde **nezobrazujú** (checkbox ich nahrádza,
  akcie sú v lište) — jediná výnimka je otvorená session (`.selected`), ktorá ostáva zvýraznená.
- Hlavička projektu: tri-stavový `<input type="checkbox">` pred menom (iba v select móde),
  `indeterminate` ak vybraná časť viditeľných eligible riadkov; klik na hlavičku ďalej
  rozbaľuje/zbaľuje, checkbox `stopPropagation`.

### Klávesnica (fokus v `.sidebar`, mimo editovateľného cieľa)

| Kláves | Bez módu | V select móde |
|---|---|---|
| klik | otvorí | toggle (kotva = riadok) |
| ⌘/ctrl-klik | **zapne mód** + toggle | toggle |
| shift-klik | **zapne mód** + rozsah od kotvy (ak niet kotvy → toggle) | rozsah od kotvy |
| Enter | otvorí | otvorí (jediná cesta k otvoreniu v móde) |
| Space | otvorí (dnes) → **toggle + zapne mód** | toggle fokusovaného |
| Shift+Space | — | rozsah od kotvy po fokusovaný |
| ↑ / ↓ | presun fokusu medzi riadkami (roving, `SessionRowItem` ostáva `tabindex=0`; UXPR-33 rieši `:focus-visible`) | to isté |
| Shift+↑ / ↓ | — | presun + rozšírenie rozsahu |
| ⌘A | zapne mód + vyberie všetky viditeľné eligible | vyberie všetky viditeľné eligible |
| Escape | — | N > 0 → vyčisti; N = 0 → vypni mód |

Poznámka k Space: dnes Space = Enter (`Sidebar.svelte:479-484`). Zmena na toggle je bezpečná — Enter
ostáva na otvorenie a `role="button"` konvencia „Space aktivuje“ sa pre výber v zoznamoch bežne
ohýba (Finder, Gmail). Ak vlastník nechce, Space ostane = Enter a toggle je iba Shift+Space
(otázka 5).

### Model výberu (`src/lib/bulk_selection.ts`, UXPR-36)

```ts
export interface BulkSelection { mode: boolean; ids: ReadonlySet<number>; anchor: number | null }
export const bulkSelection: Readable<BulkSelection>;
export function setMode(on: boolean): void;                 // off → clear
export function toggle(id: number): void;                   // mode = true, anchor = id
export function rangeTo(id: number, order: readonly number[]): void; // od anchor po id v `order`
export function selectAll(ids: readonly number[]): void;    // mode = true
export function clear(): void;                              // ids = ∅, anchor ostáva
export function pruneTo(live: ReadonlySet<number>): void;   // dnešný $effect :179-184
export function hiddenCount(sel: BulkSelection, visible: ReadonlySet<number>): number;
export function visibleOrder(byProject: Map<number, SessionRow[]>, projects: ProjectTreeRow[],
                             collapsed: Set<number>, orphans: SessionRow[]): number[];
```

- Samostatný modul, **nie** `selection.ts`: ten drží jediný `SessionRef` pre stredný panel s
  vlastnou perzistenciou (`session.last`) a 16 testami (`selection.test.ts:46-185`); hromadný
  výber je iný životný cyklus (session-scoped, nikdy nepersistovaný — rovnaký dôvod ako
  `needsYouOnly`, `Sidebar.svelte:131-136`). `Sidebar.svelte:156-184` sa nahradí importom.
- Kľúč ostáva `session.id` (stabilné na `host_alias + tmux_name`, `store/sessions.rs:55`);
  `pruneTo` beží z `$effect` nad `$sessions` ako dnes; ghost (`status === 'ghost'`) sa pri prune
  **tiež** vyhodí (UX-114) — riadok, ktorý sa stane ghostom počas výberu, z výberu vypadne a lišta
  to ukáže poklesom počtu.
- Kotva sa nuluje pri `setMode(false)`; `rangeTo` bez kotvy = `toggle`.

### Eligibility a gating (`src/lib/bulk_actions.ts`, UXPR-37)

| Akcia | Príkaz | Eligible riadok | Dôvod preskočenia (`title`) | Hub verdikt / gate |
|---|---|---|---|---|
| Prompt | `send_prompt` × N (`label: false`) | `kind ∈ {work, review, bg}`, `status === 'running'`, nie ghost | „shell session“, „not running“, „lost session“ | Routed; `hubActionBlocked('send_prompt')`; marker podľa trustu klienta (ako jednotlivý) |
| Kill | `kill_session` × N | `kind ∈ {work, review, bg, shell}`, `status !== 'ghost'` | „lost session — use Recreate or Dismiss“ | Routed, `confirm: true` na hube → jeden zoskupený toast (UX-120) |
| Safe remove… | `safe_kill_session` × N | `kind === 'work'`, `worktree_id !== null`, `claude_session_id !== null`, running, `safe_kill_state !== 'requested'` | „no worktree“, „already requested“, „shell/bg“ | Routed (`ROUTED_ACTIONS` `hub.ts:270`); `hubActionBlocked('safe_kill_session')` |
| Discard & kill… | `discard_kill_session(force)` × N | ako Safe remove | ako Safe remove | **LocalOnly** dnes (`hub.ts:199-200`) → položka `disabled` s `REASONS.discard_kill_session` bez „from the hub“; po UXPR-26 Routed |
| riziko v dialógu | `inspect_safe_kill` × N (paralelne ≤ 5, timeout 3 s) | Safe/Discard eligible | — | **LocalOnly** dnes (`hub.ts:197-198`) → riadok „not checked on this hub“; po UXPR-26 `E_FORBIDDEN`/`E_HUB_PROTOCOL` starší hub → `hub_inline_state` (UXPR-07) |
| Tag | — | — | — | žiadny príkaz — **mimo rozsahu** (otázka 3) |
| Move | — | — | — | Transfer sheet je jednobehový — **mimo rozsahu** (otázka 10) |

`readonly` klient (UXPR-21, `clientIsReadonly`): celá lišta okrem `✕` disabled s jedným `title`
„this client is readonly on the hub“.

### Potvrdzovací dialóg (`BulkConfirmDialog.svelte`, UXPR-37)

```
┌─ Kill 3 sessions? ───────────────────────────────────────── 460 px ─┐
│ The tmux sessions end now. Worktrees and any uncommitted or         │
│ unpushed work stay on their hosts — use Safe remove to check first. │
│                                                                     │
│  ● check PD-2939 vat sums status   claude-fleet-trn   ⚡ working     │
│      hazy-pluto · 2 dirty files · 1 unpushed                   ⚠    │
│  ● unblock PD-2592 service access… claude-fleet-trn   idle 15m      │
│      lkkmkm · clean                                                 │
│  ● yes  (hidden by host filter)    mac                idle 2h       │
│      main · not checked (host offline)                              │
│                                                                     │
│  1 of 3 has uncommitted work.                                       │
│                              [ Cancel ]  [ Safe remove instead… ]  [ Kill 3 ] │
└─────────────────────────────────────────────────────────────────────┘
```

- Riadok: `displayName` (UXPR-06) · host · `StatusChipView` (UXPR-32) · worktree · riziko z
  `inspect_safe_kill` (`dirty_files.length`, `unpushed_commits`, `safe_to_remove` →
  `clean` / `n dirty · m unpushed` / `not checked (reason)`); badge `hidden` pre UX-115.
- Kópia = UXPR-27 `killDialogCopy` (UX-78), rozšírená o množné číslo; *Safe remove instead…*
  prepne na Safe potvrdenie pre eligible podmnožinu (zvyšok ostane vybraný).
- `danger` → fokus na Cancel (`ConfirmDialog.svelte:44-48` vzor); `busy` počas behu s progresom
  `Killing 2 / 3…` v tlačidle; po dokončení dialóg zmizne a príde toast.
- Riziko sa načíta **po otvorení** dialógu (nie pri každom toggle), `Kill` je klikateľný hneď
  („not checked yet“ sa ukáže, ak používateľ predbehne inšpekciu) — nikdy neblokovať Kill na SSH.

### Toast a výsledok (`runBulk`, UXPR-37)

- `Promise.allSettled`; potom **jeden** `push({ kind })`: `Killed 3 sessions` (info) ·
  `Killed 2 · 1 failed` (error, sticky, akcia *Retry failed* → znovu otvorí potvrdenie so zlyhanými);
  chyby zoskupené podľa `code` — `E_CONFIRM_REQUIRED` → jedna veta `hubNextStep` (UX-120).
- Výber: úspešné id vypadnú (riadky odídu zo store → `pruneTo`), zlyhané **ostanú vybrané**, lišta
  hlási `1 selected · 1 failed`. Mód sa vypne až pri 0 vybraných **a** 0 zlyhaných, alebo `✕`/Escape.

### Hromadný prompt (`BulkPromptDialog`, UXPR-38) — odporúčanie: N × `send_prompt`, nie broadcast

| Kritérium | `broadcast_prompt` (MCP tool) | N × `send_prompt` (dnes) | N × `send_prompt` + `label: false` (návrh) |
|---|---|---|---|
| Cieľ | filter host / projekt / `claude_status`, iba `kind == "work"`, mínus controller a operátor (`prompt.rs:281-315`) — **nevie prijať zoznam id** | presne vybrané riadky | presne vybrané riadky |
| Tauri príkaz / hub route | **žiadny** (iba MCP; `hub_verdicts.generated.json` ho nepozná) → nový príkaz + routing + kontrakt (lane D) | `send_prompt` Routed | ako dnes |
| Pomenovanie | nikdy (B5, po UXPR-04) | N rovnakých mien (UX-35) | nikdy |
| Marker | podľa callera; `raw` iba master | podľa trustu desktop klienta — ako jednotlivý prompt | rovnako |
| Rate limit / confirm | 30 s per caller, `confirm: true` | žiadny — je to akcia človeka, dialóg je potvrdením | rovnako |
| Výsledok | `BroadcastSummary { sent, failed, results }` (`prompt.rs:317-331`), **sériový** for-loop (`:365-378`) | per riadok ✓/✗, paralelne | per riadok + progres + toast |

Broadcast by pre UI znamenal nový príkaz s inou sémantikou cieľa (filter, nie výber) a stratu
`review`/`bg` riadkov — bez výhody. Návrh: `SendPromptArgs` dostane
`#[serde(default)] pub label: Option<bool>` (hub klient serializuje celý args struct →
`Serialize` je už tam, treba **non-default riadok v `tests_routing.rs`** a `REGEN_HUB_CONTRACT`;
`REGEN_DOCS`, lebo referencia vypisuje všetky frontendové príkazy); `send_prompt` volá
`send_prompt_inner(…, label.unwrap_or(true))` — presne parameter, ktorý zavádza UXPR-04 P2 bod 7.
Ak UXPR-04 ešte nie je zlúčené, UXPR-38 ten parameter zavedie sám (rovnaký diff, iný PR) a 04 ho
iba použije pre systémové prompty. Frontend: `sendPrompt(host, name, prompt, opts?: { label?: boolean })`
(štvrtý voliteľný parameter, volajúci v `PromptComposer` sa nemenia); dialóg: `displayName`,
`Send to 3`, progres `2 / 3`, výsledok v dialógu **a** toast, tlačidlo `Close` namiesto
`setTimeout(600)`, zlyhané ostanú vybrané, `data-testid` `bulk-target-{id}` ostáva.

## Hub režim

| Ovládač lišty | Príkaz | Verdikt (`hub_verdicts.generated.json`) | Gate | Poznámka |
|---|---|---|---|---|
| `→ Prompt` | `send_prompt` | Routed (`:108`) | `hubActionBlocked('send_prompt')` — **ako dnes** (`SidebarFilters.svelte:14`) | marker: desktop spárovaný bez trustu doručí N označených promptov — rovnaké ako jednotlivý; `hub.md:596-604` |
| `Kill` | `kill_session` | Routed (`:80`) | `hubActionBlocked('kill_session')` — ako dnes (`:15`) | `confirm: true` na hube → zoskupený toast (UX-120); po UXPR-09 `mcp.confirm_destructive` v `get_fleet_settings` → `title` vopred |
| `Safe remove…` | `safe_kill_session` | Routed (`:107`) | `hubActionBlocked('safe_kill_session')` (v `ROUTED_ACTIONS`, `hub.ts:270`) | **nový** v lište; funguje aj na dnešnom hube |
| `Discard & kill…` | `discard_kill_session` | **LocalOnly** (`:39`) | `hubBlock('discard_kill_session')` → položka disabled, `title` = `REASONS` bez „from the hub“ (07 UX-85) | po UXPR-26b Routed → gate sa zmení na `hubActionBlocked`; `hub_disabled.test.ts` prípad sa otočí |
| riziko v dialógu | `inspect_safe_kill` | **LocalOnly** (`:44`) | pred volaním `hubBlock('inspect_safe_kill') !== null` → riadok „not checked on this hub“ (bez toastu) | po UXPR-26: `E_FORBIDDEN`/`E_HUB_PROTOCOL` → `hubInlineState` (UXPR-07) „this hub cannot inspect worktrees yet“ |
| `IconSelect`, `✕`, checkboxy, klávesnica | lokálne | — | bez rozdielu | |
| `readonly` klient | — | — | celá lišta disabled jedným `title` (po UXPR-21; dnes zlyhá až po kliku `E_FORBIDDEN` — UX-45) | |
| Tag / Move | — | — | — | mimo rozsahu; Move ostáva jednotlivý cez Transfer sheet (Routed, `confirm: true`) |

`HubScopeNote` (D5) sa v lište **nepoužije** — lišta nemá „odmietnutú sekciu“, iba per-položku
disabled s dôvodom (rovnaké pravidlo ako chrome v iterácii 08). `hub_inline_state` iba pre riziko
v dialógu.

**Hromadný Move — mimo rozsahu tejto iterácie (odporúčanie: nie).** `moves.ts` drží beh per
session, ale `TransferSheet` je jeden a UI (9 krokov, `move:progress`) je stavané na jeden beh;
`move_session` je `confirm: true` a `Deadline::Lifecycle`; N paralelných presunov na ten istý cieľ
by súperili o SSH a o worktree carry (ADR 0002). Ak by raz bol, patrí do Transfer sheetu ako
front (sekvenčný), nie do lišty.

## PR plán

Všetko v **lane F (sidebar)** z iterácie 08, sekvenčné vnútri; jediný Rust je v UXPR-38 (lane B
koniec). Závislosti sú vyznačené; „mäkko“ = PR funguje aj bez, ale s horším textom/ikonou.

| UXPR | Názov | Súbory | Veľkosť | Závisí od | Paralelnosť / kolízie |
|---|---|---|---|---|---|
| **36** | **Model výberu** — `bulk_selection.ts` (store, `toggle`/`rangeTo`/`selectAll`/`clear`/`pruneTo`/`hiddenCount`/`visibleOrder`), `Sidebar.svelte`: nahradiť `:156-184` importom, `onSelectSession` (`:458-477`) → shift = rozsah / ⌘ = toggle / auto-mód, `onKeySession` (`:479-484`) → Space, Shift+Space, ↑↓, Shift+↑↓, ⌘A (`svelte:window` guard na fokus v `.sidebar` + `!isEditable`), Escape dvojkrok (`:828-830`), ghost nevoliteľný (`SessionRowItem.svelte:168` cez prop `selectable`, nie markup), prune ghostov; `SidebarFilters` dostane `hiddenCount` (iba text `· 1 hidden`, lišta ostáva dnešná) | `bulk_selection.ts` (nový ~90), `Sidebar.svelte` (−30/+60), `SessionRowItem.svelte` (+2: prop), `SidebarFilters.svelte` (+4) | **S/M ~140** (+~150: `bulk_selection.test.ts` nový ~90, `Sidebar.test.ts` +60) | — (nezávisí od 28: lišta ostáva na starom mieste do 37) | `Sidebar.svelte` je spoločný s 23, 28, 29 → **sekvenčne**: 02 → 28 → **36** → 29 → 30 → 31; `SessionRowItem.svelte` +2 riadky v `:168` (mimo regiónov 32/33) → ak 33 beží súbežne, 36 ide **po** 33 |
| **37** | **Bulk lišta + gating + potvrdenie** — `BulkBar.svelte` (nový) v slote riadku 2 (`SidebarFilters` po UXPR-28: `{#if $bulkSelection.mode} <BulkBar/> {:else} host/čas/chip/View {/if}`), `bulk_actions.ts` (`bulkEligibility`, `bulkGate` z verdiktov, `groupErrors`), `Kill ▾` menu (vzor `.picker`), `BulkConfirmDialog.svelte` (zoznam, riziko z `inspectSafeKill` ≤ 5 paralelne, `hidden` badge, *Safe remove instead…*, `busy` progres), `runBulk` v `Sidebar.svelte` (nahradí `:186-203` a `:854-866`; toast, zlyhané ostanú vybrané), checkbox ako sused tlačidla riadku + tri-stavový checkbox v `proj-row` (`:695-724`), `aria-multiselectable`, `aria-live` na počte, `displayName` v `aria-label`; zmazať `.bulk-bar` CSS (`SidebarFilters.svelte:275-285`) | `BulkBar.svelte` (~120), `bulk_actions.ts` (~80), `BulkConfirmDialog.svelte` (~110), `Sidebar.svelte` (−40/+50), `SidebarFilters.svelte` (−25/+8), `SessionRowItem.svelte` (checkbox von z tlačidla, ~15), `hub.ts` (0 — `REASONS.discard_kill_session`/`inspect_safe_kill` už existujú) | **M ~280** (+~220: `BulkBar.test.ts` ~60, `bulk_actions.test.ts` ~60, `BulkConfirmDialog.test.ts` ~50, `Sidebar.test.ts` prepis `:1148-1188,1412-1436` ~30, `hub_disabled.test.ts` +40) | **36**, **28** (slot), **02** (ikony), **06** (`displayName`), **27** (`killDialogCopy` — obaja editujú `Sidebar.svelte:841-866`; 27 skôr, 37 kópiu prevezme), **33** (checkbox mimo `role="button"` mení kostru riadku — až po prepise 33); mäkko **26** (riziko na hube), **21** (`readonly` vopred), **09** (`confirm_destructive` vopred) | **odštep pripravený**, ak > 300: **37a** lišta + eligibility + menu + checkbox v hlavičke (~150), **37b** `BulkConfirmDialog` + riziko + `runBulk` + toast (~130) |
| **38** | **Hromadný prompt** — Rust: `SendPromptArgs.label: Option<bool>` `#[serde(default)]`, `send_prompt` → `send_prompt_inner(…, label.unwrap_or(true))` (ak 04 parameter ešte nemá, zaviesť tu), `tests_routing.rs` non-default riadok; TS: `sendPrompt(…, { label })`, `BulkPromptDialog`: `displayName`, `Send to N`, progres `k / N`, `Close` namiesto `setTimeout`, zlyhané ostanú vybrané (callback `onResult(failedIds)`), toast; `sendable` z `bulkEligibility` (37) | `crates/fleet-core/src/service/sessions/prompt.rs` (+8), `src-tauri/src/commands/sessions.rs` (+1), `src-tauri/src/backend/tests_routing.rs` (+3), `hub_contract.golden.json` (regen), `sessions.ts` (+6), `BulkPromptDialog.svelte` (~40 zmien), `Sidebar.svelte` (+5) | **S ~110** (+~100: `BulkPromptDialog.test.ts` nový ~80, `prompt.rs` test „bulk s `label: false` nepomenuje“ ~20) | **04** (alebo zavedie parameter sám — potom 04 rebase), **37** (`bulkEligibility`), **06** (`displayName`) | Rust časť koliduje s **04** v `prompt.rs` → ide **po 04** (lane B: 04 → 05 → 06 → **38**); TS časť ∥ |

**Poradie v lane F a kolízie so `Sidebar.svelte` / `SessionRowItem.svelte`:**

```
Sidebar.svelte:          23 → 02 → 28 → 36 → 29 → 30 → 31 → 27 → 37 → 38
SessionRowItem.svelte:   02 → 32 → 33 → 06 → 36(+2 r.) → 37(checkbox von)
SidebarFilters.svelte:   02 → 28 → 36(+4 r.) → 30 → 37(slot → BulkBar)
prompt.rs:               04 → 38
```

36 môže ísť **pred** 28 (nedotýka sa hlavičky), ak 28 čaká na 02; potom 28 iba pridá `hiddenCount`
do svojej lišty. 37 je posledný veľký PR na `Sidebar.svelte` v lane F — jeho rebase je najlacnejší
na konci. **Regen:** 36, 37 — žiadny; 38 — `REGEN_HUB_CONTRACT`, `REGEN_DOCS`.

**Testy, ktoré sa musia zmeniť (pinnuté selektory):** `Sidebar.test.ts:1155-1156` (shift + meta
dnes = 2 toggly; po 36 shift = rozsah → test upraviť na ⌘ + ⌘, pridať shift-rozsah),
`:1160,1182,1435` (`bulk-bar` text `N selected` ostáva), `:1168` (`bulk-bar` po kille zmizne —
po 37 zmizne až pri 0 vybraných a 0 zlyhaných; mock kill úspešný → stále platí), `:1176`
(`select-box` 0 pred módom — ostáva), `:1183-1187` (`bulk-send` → `bulk-prompt-dialog` ostáva),
`:1412-1436` (external — ostáva; pridať ghost). Testidy `select-mode`, `bulk-bar`, `bulk-send`,
`bulk-kill`, `bulk-clear`, `select-box`, `confirm-bulk-kill`, `bulk-prompt-dialog`,
`bulk-target-{id}` **nepremenúvať**; nové: `bulk-count`, `bulk-hidden`, `bulk-kill-menu`,
`bulk-safe-remove`, `bulk-discard`, `bulk-confirm`, `bulk-risk-{id}`, `proj-select-all`.

## Akceptačné testy

1. **`bulk_selection.test.ts`** (UXPR-36): `toggle` zapne mód a nastaví kotvu; `rangeTo` vyberie
   uzavretý interval v `order` v oboch smeroch a bez kotvy sa správa ako `toggle`; `selectAll`
   ignoruje id mimo zoznamu; `pruneTo` vyhodí mŕtve a ghost id; `hiddenCount` = |ids ∖ visible|;
   `setMode(false)` vyčistí ids aj kotvu; `visibleOrder` rešpektuje `collapsed` a poradie
   projektov + orphan sekciu.
2. **Shift-rozsah** (`Sidebar.test.ts`): klik riadok 0, shift-klik riadok 3 → `bulk-bar`
   `4 selected`, `select-box` 4 × checked; ⌘-klik riadok 1 → `3 selected`; `selectedSession`
   ostáva `null`.
3. **Auto-mód** (UX-112): bez módu ⌘-klik → `select-mode` `aria-pressed="true"`, checkboxy
   viditeľné, lišta prítomná; `✕` pri N > 0 → `0 selected` + hint; `✕` znova → mód vypnutý, lišta
   nahradená filtrami (po 28: `host`/`active` selecty späť).
4. **Escape dvojkrok** (UX-118): fokus na riadku, Escape → `0 selected`; Escape → mód vypnutý;
   Escape s fokusom v `sidebar-search` → nič (editovateľný cieľ); s otvoreným `dialog` → nič.
5. **Klávesnica**: fokus riadok, Space → checked; ↓ → fokus ďalší (`document.activeElement`);
   Shift+↓ → 2 checked; ⌘A → všetky viditeľné eligible checked, external a ghost nie.
6. **Ghost nevoliteľný** (UX-114): riadok `status: 'ghost'` v móde nemá `select-box`, klik
   nepridá do výberu; ghost počas výberu (merge `status → 'ghost'`) vypadne z výberu, lišta klesne.
7. **Skryté riadky** (UX-115): vyber 3, prepni host filter tak, aby 2 zmizli → `bulk-count`
   `1 selected` + `bulk-hidden` `2 hidden`; `bulk-confirm` vymenuje 3 riadky, 2 s badge `hidden`;
   ⌘A pridá iba viditeľné.
8. **Eligibility** (`bulk_actions.test.ts`): tabuľka druhov × stavov → `ok`/`skipped` s dôvodom
   pre Prompt/Kill/Safe/Discard; `bulk-send` text `Prompt (2 of 3)` pri jednom `shell`
   riadku (shell je Kill-eligible, Prompt-neeligible); `bulk-safe-remove` disabled s `title`
   „no worktree“ pri `worktree_id: null`.
9. **Potvrdenie s rizikom**: `inspect_safe_kill` mock → jeden riadok `2 dirty · 1 unpushed`, druhý
   `clean`, tretí `not checked (…)` pri chybe; súhrn `1 of 3 has uncommitted work`; *Safe remove
   instead…* otvorí Safe potvrdenie s 2 eligible a ponechá tretí vybraný.
10. **runBulk** (UX-117): 3 killy, jeden `Err` → toast `Killed 2 · 1 failed` (kind `error`, akcia
    *Retry failed*), zlyhaný riadok ostáva checked, `bulk-count` `1 selected · 1 failed`; všetky OK
    → toast `Killed 3 sessions`, mód vypnutý.
11. **Hub — zoskupenie** (UX-120): `hubStatus` remote, 3 × `E_CONFIRM_REQUIRED` → **jeden** toast s
    vetou `hubNextStep`; `hub_disabled.test.ts`: `bulk-discard` disabled s `REASONS` textom bez
    „on the hub“, `bulk-safe-remove` enabled, riziko `not checked on this hub`; offline spojenie →
    `bulk-send`/`bulk-kill` disabled s `offlineReason` (dnes netestované, UX-122).
12. **Select all v projekte** (UX-123): `proj-select-all` klik → všetky viditeľné eligible riadky
    projektu checked, checkbox `checked`; odznač jeden → `indeterminate === true`; klik na hlavičku
    mimo checkboxu ďalej zbaľuje.
13. **`BulkPromptDialog.test.ts`** (UXPR-38): `sendable` z eligibility (shell, ghost, non-running
    `skipped` s dôvodom); `send_prompt` volaný N × s `args.label === false`; progres `1 / 2` počas
    pending promise; jeden `Err` → dialóg ostane, `bulk-err-{id}`, `onResult([id])`, toast
    `Sent to 1 · 1 failed`; všetky OK → `Close` enabled, toast `Sent to 2 sessions`, žiadny
    `setTimeout`; mená cez `displayName`.
14. **Rust** (UXPR-38, `prompt.rs` testy): `send_prompt` s `label: Some(false)` nad default
    riadkom → `friendly_name` nezmenené, `last_prompt` nastavené; `label: None` → dnešné správanie;
    `tests_routing.rs` Case so `label: Some(false)` serializuje pole (kontrakt golden).
15. **a11y** (UX-119): `select-box` nie je potomkom `[role="button"]`; `ul.tree`
    `aria-multiselectable="true"` v móde; `bulk-count` má `aria-live="polite"`; každý `svg` v lište
    `aria-hidden="true"`, každé ikonové tlačidlo `aria-label`.

## Odhad

| PR | Veľkosť | Diff (bez testov) | Testy | Rust | Regen |
|---|---|---|---|---|---|
| UXPR-36 model výberu | S/M | ~140 | ~150 | nie | — |
| UXPR-37 lišta + gating + potvrdenie | M (odštep 37a/37b pripravený) | ~280 | ~220 | nie | — |
| UXPR-38 hromadný prompt | S | ~110 (z toho ~12 Rust) | ~100 | áno (`prompt.rs`, `commands`, `tests_routing.rs`) | `REGEN_HUB_CONTRACT`, `REGEN_DOCS` |
| **Spolu** | | **~530** | **~470** | | |

Rozpočet MCP popisov (D4) sa **nemení** — žiadny nový `#[tool]`, `send_prompt` MCP params ostávajú
(`label` je iba na Tauri/hub drôte). Hodnota: UX-10, 93 (obsah), 112–123, časť UX-35 (UI cesta),
UX-78/84 pre bulk; FE-4 „bulk actions“ sa stane kompletným (klávesnica, potvrdenie, výsledok).

## Otázky pre vlastníka

Iba nové; každá s odporúčaním („default“ = odporúčanie).

1. **Auto-zapnutie módu prvým ⌘/shift-klikom** (UX-112): checkboxy a lišta prídu s prvým
   výberom, mód sa vypne Escape/✕. Alternatíva: mód iba cez ikonu, ⌘-klik ostane „tichý“ výber bez
   checkboxov. *Odporúčanie: áno — jediný spôsob, ako výber nikdy nie je neviditeľný.*
2. **Skryté vybrané riadky** (UX-115): ponechať vo výbere s počtom `· n hidden` a badge v dialógu,
   alebo ich pri zmene filtra vyhodiť? *Odporúčanie: ponechať + počet; nikdy ticho meniť výber
   filtrom, nikdy ticho zabiť neviditeľné — dialóg ich vymenuje.*
3. **Hromadný Tag**: `set_session_tags` nemá Tauri príkaz ani jednotlivé UI; hromadný Tag = nový
   príkaz `set_session_tags` (Routed T1, tool existuje) + `verdicts.rs`/`remote.rs`/`tests_routing.rs`
   + `REGEN_*` (lane D, ~120 riadkov) + `TagEditor` (S). *Odporúčanie: mimo UXPR-36–38; zaradiť ako
   samostatný S PR „Tags UI“ (jednotlivý aj hromadný naraz) po lane D, aby sa Tag neobjavil prvý
   raz iba ako bulk akcia.*
4. **Safe remove v `Kill ▾` už v tejto iterácii**: routuje, každý riadok dostane systémový prompt
   pre Claude (N promptov, `label: false` po UXPR-04). *Odporúčanie: áno — je to jediná
   „bezpečná“ hromadná cesta a na hube funguje dnes (UX-77 hovorí, že jednotlivo je nedostupná —
   lišta by bola prvé miesto, kde ju hub klient má).*
5. **Space v select móde = toggle** (dnes Space = Enter = otvoriť). *Odporúčanie: áno; Enter ostáva
   otvoriť. Ak nie, toggle iba Shift+Space.*
6. **Riziková inšpekcia pred Kill** (N × `inspect_safe_kill`, SSH): paralelne ≤ 5, timeout 3 s per
   riadok, `Kill` klikateľný hneď, riadok bez výsledku = „not checked“. Strop N ≤ 20 (nad to iba
   súhrn bez inšpekcie)? *Odporúčanie: áno, so stropom 20.*
7. **`label: false` ako wire pole `SendPromptArgs`** (UXPR-38; `REGEN_HUB_CONTRACT`, starší hub ho
   ignoruje → pomenuje — prijateľná degradácia) vs. čakať iba na UXPR-04 P3 („iba prvý prompt
   pomenúva“, čo bulk nad default riadkami stále pomenuje N-krát). *Odporúčanie: pole; ak 04 nie
   je zlúčené, 38 parameter `send_prompt_inner(label)` zavedie a 04 ho prevezme.*
8. **Discard & kill hromadne**: ponechať v menu disabled s dôvodom do UXPR-26, potom povoliť iba
   pri riadkoch s inšpekciou `clean` (`force=false`) a „Discard“ s `force=true` iba po explicitnom
   druhom potvrdení so zoznamom dirty súborov? *Odporúčanie: áno v tejto forme; hromadný
   `force=true` bez zoznamu nikdy.*
9. **⌘A v sidebare** vyberie všetky viditeľné eligible (nie skryté, nie external/ghost) — iba pri
   fokuse v `.sidebar` mimo `sidebar-search`. *Odporúčanie: áno.*
10. **Hromadný Move**: mimo rozsahu (Transfer sheet jednobehový, `confirm: true`, SSH súbeh).
    *Odporúčanie: nie; ak raz, sekvenčný front v Transfer sheete, nie lišta.*

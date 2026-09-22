# Iterácia 09 — Riadok session: hierarchia metadát, odznaky, stavové glyfy

**Šošovka:** riadok session (`SessionRowItem.svelte` a všetko, čo renderuje) · **Zasahuje:** UX-09,
riadková časť UX-07 a UX-02 · **Záväzné z konsolidácie-01:** D1 (`@lucide/svelte`, sémantický
`icons.ts`), §5 „stavové glyfy v `.ts` → `{glyph, text}` model písať raz spolu s hierarchiou“
(UXPR-19), „`PR↗`, `TransferChip ⇄`, `⌘ ↵ ⇧` ostávajú ako symboly“ · **Vstup:** README, konsolidácia-01,
iterácie 01 (tabuľky A–C), 02 (P4, `displayName`), 08 (View menu, `byTriage`, bulk swap),
screenshot `01-session-terminal.jpg`, kód na `6bf75f9b` · **Režim:** read-only, žiadne zmeny kódu,
žiadny `cargo`. Nálezy od **UX-97**, PR od **UXPR-32**.

## Zhrnutie

1. Riadok nesie **11 rôznych dát na dvoch riadkoch** (audit hovoril o 8): bodka `status`, badge
   druhu, meno, stavový chip, host, tmux meno/worktree, elapsed, kontext %, cena, effort, PR + CI a
   posledný prompt. Tri z nich sú **prázdne alebo mŕtve**: `effort_level` sa nikdy nezapíše
   (`reconcile.rs:606` — `None, // not in claude agents --json`), `8h 23m` je vek od
   `started_at`, ktorý pre tmux-objavené riadky chýba (riadok mlčí, detail ukazuje fallback), a
   `status` bodka je na screenshote na všetkých 9 riadkoch zelená.
2. **UX-09 potvrdené a rozšírené**; UX-07 (riadok) a UX-02 (riadok) potvrdené. Najzávažnejšie nové:
   **UX-100 (H)** — všetky stavové farby sú hard-coded hex (`attention.ts:29-36,69,355-366`) a vo
   **light téme zlyhávajú** (working 1,94 : 1, blocked 1,72 : 1, CI pending 2,21 : 1), hoci tokeny
   `--usage-ok/warn/crit` existujú a `contextColor` ich už používa; **UX-101 (M)** — hover akcie sú
   absolútne polohované cez pravý koniec linky 1 a **prekrývajú stavový chip**, vybraný riadok
   (`yes` na screenshote) tak nemá viditeľný stav; **UX-102 (M)** — linka 2 pri 280 px **vždy
   zalamuje** (fixné položky ≈ 293 px na 243 px), riadok má 50–65 px.
3. Návrh hierarchie: **linka 1** = bodka/ikona druhu · `displayName` · **jeden stavový chip s vekom**
   (`idle 42m`, `stuck 12m`) vpravo; **linka 2** = host · krátky worktree · **klaster signálov**
   (max. 3: kontext % → PR/CI → cena) vpravo, nikdy nezalamuje; **linka 3** = posledný prompt
   (filtrovaný rovnakým stop-listom ako meno), iba v `details on`. Elapsed a effort z riadku miznú
   (elapsed ostáva v detaile, kde má tooltip; effort až keď ho backend zapíše).
4. **Stavový model:** `StatusChip = { icon: IconName | null; text: string; tone: Tone; title }`
   v novom `src/lib/status_chip.ts`, jeden resolver `sessionStatusChip(sess, nowSec)` (stuck ›
   inactive › `claude_status` › `status`), Svelte renderuje `<StatusChipView>` a testy asertujú
   `text`/`tone`, nikdy glyf. Tabuľka C iterácie 01 má v testoch **82 glyfových asserov v 10
   súboroch** (iterácia 01 odhadla ~60) — preto model ide v dvoch PR: session/CI/notifikácie
   (UXPR-32) a usage značky (UXPR-35). **UXPR-19 je týmto nahradené (superseded).**
5. Štyri PR: **UXPR-32** stavový model (S/M ~140 + 120), **UXPR-33** hierarchia riadku + detail
   (M ~230 + 150), **UXPR-34** formátovanie čísel/trvaní + prahy (S ~90 + 80), **UXPR-35** usage
   značky na ten istý model (M ~160 + 130). Poradie v `SessionRowItem.svelte`: 02 → 32 → 33 → 06.
   Sedem otázok pre vlastníka, každá s odporúčaním.

**Korekcia zadania:** `src/lib/tokens.ts` **nie je** formátovanie ceny/kontextu — je to paleta
tém a kontrastný test (`tokens.ts:1-15`). Cenu formátuje `formatCostMicros` (`sessions.ts:144-150`),
tokeny `formatTokens` (`sessions.ts:133-141` **a druhá kópia** `conversation.ts:904-909`), trvanie
`formatElapsed` (`attention.ts:315-325`) a `timeAgo` (`session_status.ts:33-39`). `rowPrompt` a
`rowElapsed` sú v `session_status.ts:42-49`, nie v `session_view.ts` — ten rieši prepínanie
Conversation/Terminal (`session_view.ts:10-30`). UXPR-34 preto zavádza `src/lib/format.ts`.

## Inventár polí

### A. Živý riadok (`kind ∈ {work, bg, shell, review}`, `status !== 'ghost'`, `readOnly = false`)

Rozmery: `1rem = 14px` (`app.css:161`). Linka 1 meno 0,8 rem = **11,2 px** mono; chip 0,65 rem =
**9,1 px**; linka 2 0,65 rem = 9,1 px; signály (kontext, cena, CI, effort) 0,6 rem = **8,4 px**;
host badge 0,7 rem = 9,8 px. Kontrast počítaný z `app.css` (dark `--bg #0f0f0f`, light `#fff`).

| # | Datum | Zdroj (pole → funkcia) | Render | Kedy | Farba | Tooltip | Test |
|---|---|---|---|---|---|---|---|
| 1 | bodka stavu | `sess.status` (tmux lifecycle `running/frozen/orphan/ghost`) | `.status-dot.status-{status}` `:255` | vždy | `:556-559` hex per status, default `--fg-muted` | `title={status}` na `aria-hidden` prvku | — |
| 2 | badge druhu | `relatedCount` prop (`Sidebar.svelte:649`), `sess.kind` | `🔗N` `:256-264`, `🔍` `:265-267`, `▶` `:268-270`, `🤖` `:271-273` | podľa druhu; **`external` nemá badge**, `work` nemá badge | emoji (netematizovateľné, UX-03) | `title` + `aria-label` (shell bez `role`, UX-34) | `Sidebar.test.ts:897-921` (related), `:931,943` (glyfy, iter. 01) |
| 3 | meno | `friendly_name`/`tmux_name` → `primaryName` `:93-94` | `.sess-name` `:274` | vždy | `--fg` | `title={tmux_name}` (pri `friendly off` = text) | `Sidebar.test.ts:1196-1253` |
| 4 | stavový chip | `stuck_kind` → `stuckKindLabel` (`attention.ts:64-66`); `isInactiveAgent` (`sessions.ts:564-566`); `claude_status` → `claudeStatusLabel` (`attention.ts:51-53`, tabuľka `:38-45`) | `.claude-chip` `:275-296`, poradie stuck › inactive › claude_status | keď je aspoň jedno nastavené; `null` → nič | inline `style` z `STATUS_COLOR` hex `:29-36`, `STUCK_COLOR #e64a4a` `:69`, pozadie `{hex}22` | `Claude: {status} — {current_activity}` | `Sidebar.test.ts:1013-1025,1236,1362-1370`; `attention.test.ts:83-90` (iba `not.toBe('')`) |
| 5 | host | `host_alias` | `.host-badge` `:356` | `details on` | `--fg-muted` + `--border` | `aria-label="host …"` | `Sidebar.test.ts:813-816,1238,1245,1293` |
| 6 | tmux meno / worktree | `secondaryName` `:97-101`: `tmux_name` ak je primárne friendly, inak `worktree_key` iba keď nie je sufixom tmux mena | `.sess-secondary` `:357-360` | `details on` a hodnota | `--fg-muted` mono | — | `Sidebar.test.ts:1196-1198,1241,1253` |
| 7 | elapsed | `started_at` → `rowElapsed` (`session_status.ts:42-44`) → `formatElapsed(sessionStart)` (`attention.ts:315-331`) | `.sess-elapsed` `:361-364` | `details on` **a `started_at !== null`** | `--fg-muted` | **žiadny** | `Sidebar.test.ts:1214`; `session_status.test.ts:8-11`; `attention.test.ts:189-195` |
| 8 | kontext % | `context_pct` → `contextLevel` (`attention.ts:78-83`, prahy 70/90 `:75-76`) → `contextColor` (`:88-99`, tokeny `--usage-*`) | `.ctx-badge` s `.ctx-bar` `:365-379`, `role="meter"` | `details on` a `context_pct !== null` | `--usage-ok/warn/crit` (jediné signály s tokenmi) | `Context window N% used` | `Sidebar.test.ts:1028-1041,1243`; `attention.test.ts:94-103` |
| 9 | cena | `usage_*_tokens` → `sessionUsageTokens` (`sessions.ts:123-130`); `usage_cost_micros` → `formatCostMicros` (`:144-150`) | `.cost-badge` `:380-391`; text `unpriced`, keď tokeny > 0 a cena 0 | `details on` a tokeny > 0 | `--fg-muted` × `opacity .75` → **dark 4,28 : 1, light 3,19 : 1** | `Estimated cost $ · N tokens · model` / `Unpriced: …` | `Sidebar.test.ts:1043-1080`; `sessions.test.ts:31-35` |
| 10 | effort | `effort_level` | `.effort-badge` `:392-395`, uppercase | keď je nastavené — **nikdy** (jediný zápis `reconcile.rs:606` je `None`; store `COALESCE` `store/reconcile.rs:378` drží iba historické hodnoty) | `--fg-muted` na `--fg 10 %` | `Effort: {x}` | žiadny (fixtures `Sidebar.test.ts:41,978` majú `null`) |
| 11 | PR | `pr_url` (gh probe `outcome.rs:129-147`, iba `https://`) | `<a class="pr-link">PR↗</a>` `:396-405`, `target=_blank` | `details on` a `pr_url` | `--accent` (light 5,17 : 1, dark 7,54 : 1 ✔) | `Open pull request` | `Sidebar.test.ts:1207` (nepriamo) |
| 12 | CI | `ci_status` (`reduce_ci_status` `outcome.rs:155`) → `ciStatusLabel`/`ciStatusColor` (`attention.ts:342-366`) | `.ci-badge` `:406-414` **iba vnútri `{#if pr_url}`** | `details on`, `pr_url` a `ci_status` | hex `#50c86e/#e64a4a/#d29b4a` → light pending **2,21 : 1** | `CI checks: {status}` | `Sidebar.test.ts:1217`; `attention.test.ts:209-210` (`toContain('CI')`) |
| 13 | posledný prompt | `last_prompt` (200 znakov, `store/sessions.rs:801-812`, zapisuje **každý** `send_prompt` bez filtra `prompt.rs:165`) → `rowPrompt` (`session_status.ts:47-49`) → `promptPreview(…, 48)` (`attention.ts:335-340`) | `.sess-meta` `:416-419`, posledná položka, ellipsis | `details on` a text | `--fg-muted` | celý `last_prompt` | `Sidebar.test.ts:1213-1216,1244`; `session_status.test.ts:12-15`; `attention.test.ts:203-206` |
| 14 | oddeľovače | — | `<span class="sep">·</span>` medzi 5–13 | pri každej položke | `--fg-muted` × `opacity .6` → **dark 3,14 : 1, light 2,43 : 1** | — | — |

Kto zapisuje časové polia: `started_at` iba `set_started_at` (`store/sessions.rs:816-822`,
`COALESCE` — raz) z `lifecycle.rs:634` (fleet `new_session`), `bg_sessions.rs:263`,
`review.rs:92` a `move_session/mod.rs:2693`; **nie** zo `send_prompt`. `context_pct` má dvoch
autorov: pane tail (`pane_intel.rs:283` „N% used“) a hook/transcript (`store/conversations.rs:440-442`
`round(tokens·100/window)`) — riadok obe zobrazí rovnako, `context_source` nevidí.

### B. Hover akcie (`.row-actions`, `:297-352`)

| Akcia | Glyf | Kedy | Blokovanie (hub) | Testid | `aria-label` |
|---|---|---|---|---|---|
| Remove from list | `×` `:299-306` | iba `isInactiveAgent` | `hubBlock('dismiss_agent_session')` `:143` | `remove-from-list` | ✔ |
| Restart | `↻` `:308-314` | vždy | `hubActionBlocked('restart_session')` `:146` | — | ✔ |
| Edit label | `🏷` `:315-322` | vždy | `set_friendly_name` `:148` | `edit-label` | ✔ |
| Rename tmux | `✎` `:323-330` | vždy | `rename_session` `:149` | `rename-tmux` | ✔ |
| Recreate | `♻` `:331-340` | vždy; disabled pri nedostupnom hoste `:135-137` | `recreate_session` `:147` | `recreate-live` | ✔ |
| Kill | `×` `:344-350` | `!isInactiveAgent` | `kill_session` `:145` | — | ✔ |

Zobrazenie: `display: none` → `flex` iba pri `.sess-row:hover` alebo `.sess-row.selected`
(`:505-510`); na linke 1 **absolútne** `right: 0`, plné pozadie `--bg-pane` + 1 rem fade
(`:519-547`). Veľkosť `.icon-btn.small`: písmo 11,9 px, padding 0,1/0,35 rem, `min-width 1,4 rem`
→ ≈ 20 × 16 px (`:448-453`), tri kópie `.icon-btn` (UX-29). Hint `session-actions`
(`hints.ts:37-40`) hovorí „Hover or select…“ — klávesnica nie je cesta.

### C. Ghost riadok (`status === 'ghost'`, `:223-251`)

Iná štruktúra: bodka `status-ghost` · **host badge na linke 1** `:225` · meno (tretí inline výraz
pravidla mena `:226-228`) · `lost 5m ago` (`timeAgo`, `:229-233`, `opacity .6`) · akcie Recreate `↺`
+ Dismiss `×` (**stále viditeľné**, nie hover — `row-actions` je tu flex dieťa `.sess-row`, nie
`.sess-line1`, ale pravidlo `:505-510` ho aj tak skryje mimo hover/selected). Žiadna linka 2.

### D. Read-only riadok „Outside fleet“ (`kind === 'external'`, `readOnly = true`, `:201-222`)

Bodka · meno (`bg:<uuid>` do UXPR-04/06) · stuck/claude chip. **Bez badge druhu, bez hosta, bez linky
2** aj pri `details on` — hoci sekcia je fleet-wide (`buildOutsideFleet` filtruje iba podľa
`hostFilter`, `sidebar_index.ts:53-61`). `bg` riadok je od `work` riadku odlíšený iba `🤖`.

### E. Detail (`SessionDetails.svelte`) — tie isté polia

`h2` = `tmux_name` `:420-424`, friendly pod ním `:427` (opačne než riadok, UX-07 → UXPR-06);
`.sub` `:429-456` = host · `status` text · stuck chip **s vekom** `formatElapsed(stuck_since)` `:438`
· claude chip · `ctx N%` `:447-455`; `<dl>` `:459-520` = Created · Last activity · **Elapsed s
tooltipom** („since fleet started the session“ / „since tmux created…“, `:481-484`) · Last turn ·
Last prompt (celý) · Usage (cena + 4 počítadlá + model, `:496-505`) · Pull request (URL bez
`https://github.com/`, CI chip `:507-520`). Effort v detaile **nie je** — riadok ukazuje pole, ktoré
detail nepozná. `HostDetail.svelte:98-102` skladá stav tretí raz (`stuck: …` › `ghost` ›
`claudeStatusLabel` › `status`).

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-09 | **Potvrdené, rozšírené.** Nie 8, ale **11 dát** (inventár A), z toho 3 bez hodnoty: effort nikdy (`reconcile.rs:606`), elapsed iba pre fleet-vytvorené riadky (`session_status.ts:43`), `status` bodka vždy `running` (screenshot: 9/9 zelených). Hierarchia je iba veľkosť písma (11,2 → 9,1 → 8,4 px) — všetko na linke 2 má rovnakú tlmenú farbu okrem kontextu a PR. Legenda `%`/`$` je iba v `title` (`:372`, `:387-389`). Farba červenej pri 100 % je správna (`--usage-crit`), ale bez ponuky akcie (UX-98). | `SessionRowItem.svelte:354-420`; screenshot 01 riadky `check PD-2939…`, `yes`, `clear` |
| UX-07 (riadok) | **Potvrdené.** Linka 2 nesie celé `dev-martin-janci-kuk-agent--main` (`secondaryName` `:97-101` ho vracia celé, keď je primárne friendly). Pravidlo „worktree iba keď nie je sufixom tmux mena“ (`:99`) rieši opačný prípad (friendly off). Návrh: krátky tvar `<repo>--<worktree>` → `kuk-agent · main`, celé tmux meno do `title` a detailu (UXPR-33; hlavička terminálu a `h2` sú UXPR-06). | `:357-360`; iter. 02 P4 „linka 2 ostáva“ — ostáva, ale skrátená |
| UX-02 (riadok) | **Potvrdené, rozšírené.** Päť glyfov (inventár B) + **prekrytie stavového chipu** (UX-101): `.sess-line1 .row-actions { position:absolute; right:0; background: var(--bg-pane) }` `:519-526` leží presne tam, kde je `.claude-chip` (posledné flex dieťa linky 1 pred akciami). Pri `.selected` sú akcie trvalo zobrazené `:510` → vybraný riadok nikdy neukáže stav. Na screenshote riadok `yes` (vybraný): `↻ 🏷 ✎ ♻ ×`, žiadny `· idle`. | `:505-547`; screenshot 01 |
| UX-03 (riadok) | **Potvrdené.** `🔗 🔍 ▶ 🤖` `:256-273`, `⚠` `:214,283`, glyfy v `STATUS_LABEL` `:38-45` a `ciStatusLabel` `:342-353`. Ikony z UXPR-02; reťazce z UXPR-32. | tabuľky A–C iter. 01 |
| UX-33 (`⚡`) | **Potvrdené.** `⚡ working` v riadku (`attention.ts:39`) a `⚡` = nová bg session (`Sidebar.svelte:790`) na jednej obrazovke (screenshot: pravý dolný roh sidebaru vs. prvý riadok). | — |
| UX-34 (shell badge) | **Potvrdené.** `▶` `:268-270` bez `role="img"`. | — |
| UX-40 (tri fallbacky mena) | **Potvrdené, +1.** Štvrtý výraz: `attention.ts:303-305` `displayName(s, friendly)` (používa `stuckMessage`). Iterácia 02 ho nenašla; UXPR-06 ho má zjednotiť. Zároveň **kolízia názvu**: iter. 02 plánuje *nový* `src/lib/session_view.ts`, ktorý **už existuje** s `resolveSessionView` (`session_view.ts:10-30`, importuje ho `prefs.ts:10`). | — |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz | Návrh |
|---|---|---|---|---|
| **UX-97** | **M** | **`8h 23m` je vek od `started_at`, nie aktivita — a riadok s detailom nesúhlasí.** `rowElapsed` vracia `''` pri `started_at === null` (`session_status.ts:43`), detail ukazuje `formatElapsed(sessionStart)` s fallbackom `created_at` (`SessionDetails.svelte:483`, `attention.ts:329-331`) → tmux-objavená session má v detaile „Elapsed 3d 2h“ a v riadku nič. `started_at` stampuje iba fleet create/bg/review/move, nikdy prompt. Bez tooltipu; v triage zozname je užitočný **vek stavu** (`idle_since`, `stuck_since`, `last_activity_at`), nie vek session. Iterácia 08 Q5 sem posunula „vek idle v chipe riadku“. | `store/sessions.rs:816-822`; volajúci `lifecycle.rs:634`, `bg_sessions.rs:263`, `review.rs:92`, `move_session/mod.rs:2693`; screenshot `8h 23m`, `7h 25m` | Elapsed z riadku **von** (ostáva v detaile s tooltipom); vek stavu do chipu: `idle 42m`, `blocked 3m`, `stuck 12m` z `bucketSince` (`attention.ts:208-221`); `working` bez veku (UXPR-32/33) |
| **UX-98** | **M** | **100 % kontext bez akcie.** Chip pri `crit` má tooltip „Context window 100% used“ (`:372`) a nič viac; `/compact` preset existuje (`composer_presets.ts:16`). Prahy 70/90 (`attention.ts:75-76`) rozsvietia 3 z 8 riadkov na screenshote (83 %, 79 %, 100 %); pri 80/95 by svietili 2. Vek hodnoty (`context_at`, `context_stale`, `sessions.ts:108-110`) sa nezobrazuje — po kompakcii je chip stále červený, kým nepríde usage riadok. | `SessionRowItem.svelte:365-379`; `conversation.ts:923-931` zdieľa `contextLevel` | Prahy **80/95** (jedna konštanta, Conversation hlavička sa zmení s ňou); `crit` tooltip „Context 97 % — send /compact or start a new session“; chip je `<button>` → vyberie riadok a predvyplní `/compact` v composeri; `context_stale` → `~97 %` (UXPR-34 + 33) |
| **UX-99** | L | **Formát čísel: tri triedy šírky, `unpriced`, dva `formatTokens`.** `$4.28 / $53.19 / $194 / $3,128` (`formatCostMicros` `sessions.ts:144-150`, hranica 100 $) — `tabular-nums` nepomôže, keď sa mení počet znakov; „unpriced“ (`:390`) je slovo bez významu pre používateľa; `formatTokens` existuje dvakrát s **inými výstupmi** (`sessions.ts:133-141` 1 234 → `1.2k`; `conversation.ts:904-909` → `1k`). | screenshot: 9 cien, 4 šírky; `sessions.test.ts:31-35`, `conversation.test.ts:469-475` | `src/lib/format.ts`: `formatMoney` (< 10 $ dve desatinné, inak celé s oddeľovačom), jeden `formatTokens`, `formatDuration`, `formatAge`; „unpriced“ → `1.2M tok` tlmene (UXPR-34) |
| **UX-100** | **H** | **Stavové farby sú hex a vo light téme zlyhávajú.** Chip má text v `STATUS_COLOR` na pozadí `{hex}22`: light **working 1,94 : 1, blocked 1,72 : 1, completed 2,93, failed/stuck 3,26, stopped/idle 3,08**; CI `pending` 2,21 : 1 (`ciStatusColor` `attention.ts:355-366`); dark všetko ≥ 4,38 (failed/stuck 4,38 tesne pod 4,5 pre text). `contextColor` (`:88-99`) tokeny používa, zvyšok nie; `STUCK_COLOR`, `.icon-btn.danger`, `.err` majú `#e64a4a` natvrdo (UX-30). Šošovka 18 (light téma) to zdedí, ale farby vlastní riadok. | `attention.ts:29-36,69,355-366`; `SessionRowItem.svelte:212,219,281,293,411,455,488`; výpočet z `app.css:4-19` | Tón namiesto farby: `tone: ok/warn/crit/info/muted` → CSS triedy `.chip--ok { color: var(--usage-ok) }` …, `info` = `--accent`, `muted` = `--fg-muted`; žiadny inline `style` (UXPR-32) |
| **UX-101** | **M** | **Hover akcie prekrývajú stavový chip.** `.sess-line1 .row-actions` je `position:absolute; right:0` s plným pozadím a fade `:519-547`; chip je posledná položka linky 1 → pri hoveri aj pri `.selected` (trvalo, `:510`) zmizne. Používateľ rozhoduje o Restart/Kill bez toho, aby videl stav. | screenshot 01 riadok `yes`: akcie viditeľné, `· idle` nie | Akcie nad **pravým koncom linky 2** (signály sú menej urgentné než stav), 3 viditeľné (Restart, Label, Kill) + `…` menu (Rename tmux, Recreate); zobraziť aj pri `:focus-within` (UXPR-33) |
| **UX-102** | **M** | **Linka 2 pri 280 px vždy zalamuje.** Dostupná šírka ≈ 280 − 19,6 − 5,6 − 11,9 = **243 px**; fixné položky (`flex-shrink: 0` `:653`) s PR: host `claude-fleet-trn` ≈ 93 + elapsed 35 + ctx 36 + cena 30 + PR 20 + CI 25 + 6 × 9 (sep + gap) ≈ **293 px** → prompt aj tmux meno idú na tretí riadok; výška riadku 35 → 50–65 px. Na screenshote (sidebar ≈ 600 px) sú 3-riadkové `check PD-2939…` a `unblock PD-2592…`. Spec dvoch liniek to zámerne dovolila („The line wraps rather than clipping“), ale počítala s tým, že prompt je posledný — nie s tým, že sa zalomí každý riadok s PR. | `SessionRowItem.svelte:638-656`; spec `2026-09-12…two-line-rows-design.md:148-153`; `App.svelte:67` default 280 | Linka 2 `flex-wrap: nowrap`: host + worktree `min-width:0` s ellipsis, klaster signálov `flex-shrink:0` vpravo; prompt na vlastnú linku 3 (UXPR-33) |
| **UX-103** | L | **Posledný prompt bez filtra:** `yes`, `go`, `push`, `/clear` (screenshot 5 z 8 riadkov). `set_last_prompt` sa volá pre každý prompt (`prompt.rs:165`), P2 stop-list iterácie 02 platí iba pre meno. Linka nesie šum, pre ktorý sa zalamuje. | `prompt.rs:133-195`; `attention.ts:335-340` | `promptPreview` preskočí slash príkazy a stop-list slová (TS kópia P2 zoznamu alebo `isNoisePrompt`), fallback prázdny → linka 3 sa nevykreslí; backend `last_prompt` ostáva pravdivý (UXPR-34) |
| **UX-104** | L | **`· idle` a `⚡ working` sú dáta, nie štýl.** `STATUS_LABEL` nesie glyf v reťazci (`attention.ts:38-45`); `· idle` vyzerá ako zabudnutý oddeľovač (screenshot). Testy pinnujú iba `not.toBe('')` / `toHaveTextContent('working')` (`attention.test.ts:83`, `Sidebar.test.ts:1236`) — zmena je lacná. `HostDetail.svelte:100` a `stuckMessage` (`attention.ts:308-310`) tie reťazce tiež čítajú. | tabuľka C iter. 01 | `StatusChip.text` = slovo z vocabulary (`working`, `blocked`, `completed`, `failed`, `stopped`, `idle`), ikona zvlášť (UXPR-32) |
| **UX-105** | L | **Effort badge je mŕtvy kód.** `effort_level` zapisuje iba reconcile a to `None` (`reconcile.rs:606` „not in claude agents --json; reserved for future“); `COALESCE(excluded.effort_level, effort_level)` (`store/reconcile.rs:378`) drží iba staré hodnoty. Riadok má 4 riadky markupu + 10 CSS (`:392-395,616-625`) pre pole, ktoré detail nepozná. | `grep effort_level` v `crates/` | Z riadku odstrániť; keď backend začne písať, patrí do detailu a do tooltipu ceny (UXPR-33) |
| **UX-106** | **M** | **Tri druhy riadkov, tri štruktúry.** Live: dve linky; ghost: jedna linka s hostom vľavo a `lost 5m ago` vpravo (`:223-251`); external: jedna linka bez hosta a badge (`:201-222`). `bg` riadok = `work` riadok + `🤖`; external v sekcii fleet-wide bez hosta. Klávesové správanie sa tiež líši: ghost nereaguje na klik mimo select mode (`:167-169`). | inventár C, D; spec agent rows §5 `:133-137` „status chip and name only“ | Jedna štruktúra: linka 1 (ikona druhu · meno · chip), linka 2 (host · zdroj · signály). Ghost: chip `{IconGhost, 'lost 5m', muted}` + akcie Recreate/Dismiss; external: `IconExternal` (iter. 02 P5), linka 2 = host · basename `cwd` · kontext; bg: `IconBot` (UXPR-33) |
| **UX-107** | **M** | **Klávesnica sa k akciám nedostane.** Riadok je `role="button" tabindex="0"` (`:165-166`) bez `:focus-visible` štýlu (grep `focus` v `SessionRowItem.svelte`/`Sidebar.svelte` → 0); akcie sa ukážu iba pri `:hover`/`.selected` (`:509-510`) — fokusovaný nevybraný riadok ich nemá; Enter/Space vyberie (`Sidebar.svelte:479-484`), až potom sú akcie v tab poradí. `.icon-btn.small` ≈ 20 × 16 px pod 24 px podlahou (`app.css:25-29` „24px is both the floor and the answer“); iterácia 01 navrhla `btn--sm` 20 px — v rozpore s tou poznámkou. | `SessionRowItem.svelte:155-171,297-352,431-455` | `.sess-row:focus-visible { outline: var(--ring-w) solid var(--ring) }`, `.sess-row:focus-within .row-actions { display:flex }`, akcie `btn btn--icon btn--quiet` **24 px**, 3 + `…` (UXPR-33; upraviť UXPR-02 návrh `btn--sm`) |
| **UX-108** | L | **`title` a `aria` duplicity.** `status-dot` je `aria-hidden` a má `title={status}` (tooltip bez AX mena, `:255`); `claude-chip title="Claude: working"` = viditeľný text; `sess-name title={tmux_name}` pri `friendly off` = text; `host-badge aria-label="host X"` = text; `PR↗` bez `aria-label` (čítačka: „PR north east arrow“); `ci-badge` bez role; `.sep` správne `aria-hidden`. `role="meter"` na `<span>` je v poriadku, ale `aria-label="context usage"` nehovorí úroveň. | `:206-207,220,255,274,294,356,398-405,408-413` | Jedno prístupné meno na chip (`aria-label={chip.title}`), `title` iba kde nesie *ďalšiu* informáciu (hub `*Blocked` dôvod, model, tokeny); `PR↗` `aria-label="Open pull request"` (UXPR-32/33) |
| **UX-109** | L | **8,4 px písmo pre signály.** Kontext, cena, CI, effort majú `font-size: 0.6rem` (`:589,605,612,617`), chip 9,1 px; `--control-font-sm` 11 px existuje (`app.css:41`) a nikde v riadku sa nepoužíva. Pod 10 px prestáva fungovať aj `tabular-nums`. | výpočet vyššie | Linka 1 12 px (`--control-font`), linka 2 a chipy **10 px** (`--row-font-2`), signály nikdy pod 10 px; výška riadku ≈ 36 px (UXPR-33) |
| **UX-110** | L | **`status` bodka je druhý farebný kanál bez textu.** `running/frozen/orphan/ghost` → 4 hex farby (`:556-559`); `frozen`/`orphan` nemajú nikde text (detail ukazuje `status` slovom `:431`, riadok nie). Na screenshote 9/9 zelených → 0 bitov informácie; stav a chip si môžu odporovať (zelená bodka + červený stuck). | `:255,549-559` | Bodka **ostáva** ako kotva skenovania, ale farbu berie z `StatusChip.tone`; `status !== 'running'` sa stane chipom (`frozen` info, `orphan` warn) — jeden model (UXPR-32) |
| **UX-111** | L | **Kolízia mena súboru pre UXPR-06** a štvrtý `displayName` (viď UX-40 vyššie): `session_view.ts` existuje (`resolveSessionView`); iter. 02 PR-B položka 2 ho chce vytvoriť. | `session_view.ts:1-30`; `attention.ts:303-305` | UXPR-06 → `src/lib/session_name.ts` (alebo do `session_status.ts`); `attention.ts:303` importuje odtiaľ |

## Návrh hierarchie

### Princípy

1. **Jedna otázka na linku.** Linka 1: *čo to je a v akom je stave* (identita + stav + vek stavu).
   Linka 2: *kde a za koľko* (host, worktree, signály). Linka 3 (voliteľná): *čo naposledy
   dostala* (prompt).
2. **Jeden stavový chip, jeden tón.** Bodka, chip a farba signálov vychádzajú z jedného
   `StatusChip`/`tone`; žiadny inline `style` s hex.
3. **Signály majú rozpočet.** Max. **3** položky v klastri, poradie podľa akčnosti: kontext % (dá
   sa niečo urobiť) → PR/CI (dá sa kliknúť) → cena (iba informácia). Effort a elapsed z riadku
   miznú (UX-105, UX-97). Klaster nezalamuje, nikdy pod 10 px.
4. **Vek do chipu, nie do linky.** `idle 42m`, `blocked 3m`, `stuck 12m`, `lost 5m`; `working`
   bez veku (mení sa každých 30 s a nič nehovorí). Vek = `nowSec − bucketSince(s, bucket)`
   (`attention.ts:208-221`), formát `formatAge` (UXPR-34): `42s`, `5m`, `3h`, `2d` — jedna
   jednotka, chip je úzky.
5. **Rovnaká kostra pre work/bg/shell/review/external/ghost.** Líšia sa ikonou druhu, obsahom
   chipu a dostupnými akciami, nie rozložením.
6. **Detail nesmie odporovať riadku:** rovnaký `StatusChip`, rovnaké prahy, rovnaký `formatMoney`;
   detail navyše ukazuje to, čo riadok vynechá (Elapsed s tooltipom, štyri počítadlá tokenov, celý
   prompt, effort keď bude).

### Wireframe — 280 px, `details on` (linka 3 zobrazená)

```
280 px                                                             ← sidebar default (App.svelte:67)
┌──────────────────────────────────────────────────────────────────┐
│ ● check PD-2939 vat sums status…                  [⚡ working]   │  L1  12 px  · bodka = tone
│   claude-fleet-trn · hazy-pluto              53% · PR✓ · $4.28   │  L2  10 px  · host · worktree · signály
│   vies to otestovat?                                             │  L3  10 px  · prompt (iba details on)
├──────────────────────────────────────────────────────────────────┤
│ ● indigo cosmos                                    [idle 42m]    │  L1
│   claude-fleet-trn · indigo-cosmos                 83% · $8.36   │  L2  amber 83 %
│   push                                       ← L3 sa nevykreslí: „push“ je v stop-liste (UX-103)
├──────────────────────────────────────────────────────────────────┤
│ ● yes                                       [~97% ▲] [idle 2h]   │  L1  kontext crit sa PROMOTUJE na L1 (Q3)
│   claude-fleet-trn · main                              $3,128    │  L2
├──────────────────────────────────────────────────────────────────┤
│ ● clear                                            [idle 15m]    │  L1
│   claude-fleet-trn · rustic-jupiter          79% · PR✓ · $322    │  L2
└──────────────────────────────────────────────────────────────────┘
   ▲ 6 px         ▲ displayName (UXPR-06 tlmí auto meno)  ▲ StatusChip: ikona + text + vek, tone

hover / focus-within (akcie prekryjú PRAVÝ koniec L2, stav ostáva viditeľný — UX-101):
│ ● yes                                                [idle 2h]   │
│   claude-fleet-trn · main            [↻] [🏷] [×] […]            │  3 akcie 24 px + overflow
```

### Wireframe — 380 px, `details off`

```
380 px
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ ● check PD-2939 vat sums status                                     [⚡ working]       │
│   claude-fleet-trn · papayapos-backend · hazy-pluto             53% · PR✓ · $4.28     │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ ◇ agent-31 (bg)                                                       [completed 1h]  │  ◇ = IconBot
│   mefistos · claude-fleet                                                   $0.42     │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ ▢ claude-fleet · 62e738aa                                             [⚡ working]     │  ▢ = IconExternal, Outside fleet
│   claude-fleet-trn · ~/projects/claude-fleet                                  24%     │  bez akcií (readOnly)
├────────────────────────────────────────────────────────────────────────────────────────┤
│ ○ dev-foo--main                                                        [lost 5m]      │  ○ = IconGhost, tone muted
│   mac · main                                          [↺ Recreate] [× Dismiss]        │  akcie viditeľné trvalo (nie hover)
└────────────────────────────────────────────────────────────────────────────────────────┘
```

`details off` = linky 1 + 2 (≈ 36 px); `details on` pridáva linku 3 (≈ 48 px). Dnes je `details
off` iba linka 1 (≈ 22 px) — zmena sémantiky prepínača je otázka Q1 (odporúčanie nižšie). Pri
380 px sa do linky 2 zmestí aj `<repo>` pred worktree (`project.repo` z `ProjectTreeRow`); pri
280 px iba worktree. Host badge sa skryje, keď je aktívny filter jedného hosta (nový prop
`showHost` z `Sidebar.svelte`, hodnota `$hostFilter === 'all'`) — pri 280 px to vráti ~90 px.

### Klaster signálov — pravidlá

| Signál | Kedy | Text | Tón | Tooltip | Akcia |
|---|---|---|---|---|---|
| kontext | `context_pct !== null` | `53%`; `~53%` pri `context_stale` | `ok` < 80, `warn` ≥ 80, `crit` ≥ 95 (`CONTEXT_WARN_PCT/CRIT_PCT`) | `Context 53 % of 200k (transcript, 2m ago)`; pri crit `… — send /compact or start a new session` | `<button>` → `onSelectSession` + composer preset `/compact` (iba crit) |
| PR + CI | `pr_url` | `PR↗` (symbol ostáva) + `<StatusChipView chip={ciChip}>` ikonou bez textu | CI tón: passing `ok`, failing `crit`, pending `warn`; bez `ci_status` neutrálny `--accent` | `Open pull request · CI failing` | `<a target=_blank>` |
| cena | tokeny > 0 | `formatMoney` → `$4.28`, `$53`, `$194`, `$3,128`; bez ceny `1.2M tok` | `muted` | nezmenený (`Estimated cost … tokens … model`) | — |
| ~~effort~~ | — | — | — | — | odstránené (UX-105) |
| ~~elapsed~~ | — | — | — | — | do chipu ako vek stavu (UX-97) |

Max. 3 → keď sú všetky tri, klaster má ≈ 36 + 32 + 40 + 2 × 9 ≈ 126 px pri 10 px písme; linka 2
má pri 280 px ≈ 243 px → host + worktree dostanú ≥ 117 px s ellipsis. Nikdy `flex-wrap`.

### Čo sa presúva do detailu (`SessionDetails.svelte`)

- `h2` = `displayName`, `tmux_name` pod ním s CopyButton (UXPR-06, ostáva).
- `.sub` `:429-456` → `<StatusChipView chip={sessionStatusChip(session, nowSec)}>` (jeden resolver
  namiesto dvoch `{#if}` vetiev) + `ctx` chip z toho istého `contextChip()`; text `status`
  (`:431`) zmizne, keď je `running` (inak je to chip).
- `<dl>`: **Elapsed ostáva** (s tooltipom, `:481-484`) — je to jediné miesto pre vek session;
  pridať `Idle since` / `Stuck since` riadok, keď existuje (`idle_since`, `stuck_since`), aby vek
  v chipe mal kde byť presný.
- Usage `:496-505`: `formatMoney` z `format.ts`; „unpriced (model)“ ostáva tu (detail má priestor).
- Pull request `:507-520`: CI chip cez `StatusChipView`.
- Effort: pridať `<dt>Effort` **až keď** backend pole zapíše (dnes by bol trvalo prázdny).
- `HostDetail.svelte:98-102` `sessionState` → `sessionStatusChip(...).text` (tretia kópia zmizne).

## Stavový model `{glyph, text}`

### Typ (nový `src/lib/status_chip.ts`, UXPR-32)

```ts
import type { IconName } from './icons';          // kľúče sémantického modulu z UXPR-01

export type Tone = 'ok' | 'warn' | 'crit' | 'info' | 'muted';

export interface StatusChip {
  /** Sémantická ikona z icons.ts; null = iba text (idle). */
  icon: IconName | null;
  /** Slovo z vocabulary (pane_intel ClaudeStatus / StuckKind / CiStatus), nie próza. */
  text: string;
  /** Vek stavu, už naformátovaný (`42m`); undefined = bez veku. */
  age?: string;
  tone: Tone;
  /** Prístupné meno + tooltip; jediné miesto pre prózu. */
  title: string;
}

export function claudeStatusChip(status: ClaudeStatus, since: number | null, now: number): StatusChip;
export function stuckChip(kind: StuckKind, since: number | null, now: number): StatusChip;
export function lifecycleChip(status: string, lostAt: number | null, now: number): StatusChip | null; // frozen/orphan/ghost
export function inactiveChip(): StatusChip;
export function ciChip(status: CiStatus): StatusChip;
export function contextChip(pct: number, stale: boolean): StatusChip;          // text `53%`, tone z contextLevel
/** Jediný resolver pre riadok, detail a HostDetail: stuck › inactive › lifecycle ≠ running › claude_status › null. */
export function sessionStatusChip(s: SessionRow, now: number): StatusChip | null;
export function notificationChip(status: string | null): StatusChip;          // conversation.ts:305-310
```

Odchýlka od zadania: tón má **päť** hodnôt — `info` je potrebný pre `completed` (dnes modrá
`#6c8ebf`, `attention.ts:32`) a `frozen`; bez neho by dokončený bg agent svietil zelenou ako
`working`. Mapa tón → token: `ok → --usage-ok`, `warn → --usage-warn`, `crit → --usage-crit`,
`info → --accent`, `muted → --fg-muted`; pozadie `color-mix(in srgb, currentColor 12%, transparent)`.
Všetky tokeny už majú kontrast ≥ 4,5 : 1 v oboch témach (`tokens.ts:78-95`, `app.css:14-19,73-77`).

`StatusChipView.svelte` (nový, ~40 riadkov): `<span class="chip chip--{tone}" role="img"
aria-label={title} title={title}>{#if icon}<svelte:component this={ICONS[icon]} size={12}
aria-hidden="true"/>{/if}{text}{#if age} <span class="age">{age}</span>{/if}</span>`. Testy
asertujú `data-tone`, `textContent` a `aria-label`, nikdy glyf.

### Tabuľka C iterácie 01 → model

| Dnes (reťazec) | Miesto | `icon` (icons.ts meno → Lucide) | `text` | `tone` | Kto ho renderuje po zmene |
|---|---|---|---|---|---|
| `⚡ working` | `attention.ts:39` | `IconWorking` → `zap` | `working` | `ok` | riadok, detail, HostDetail, HostsList (počty) |
| `⏸ blocked` | `:40` | `IconBlocked` → `pause` | `blocked` (+ vek) | `warn` | tie isté |
| `✓ done` | `:41` | `IconDone` → `check` | `completed` | `info` | tie isté |
| `✗ failed` | `:42` | `IconFailed` → `x` | `failed` | `crit` | tie isté |
| `■ stopped` | `:43` | `IconStopped` → `square` | `stopped` | `muted` | tie isté |
| `· idle` | `:44` | `null` | `idle` (+ vek) | `muted` | tie isté |
| `⚠ stuck: press Enter` | `SessionRowItem.svelte:214,283`; `SessionDetails.svelte:438` | `IconStuck` → `triangle-alert` | `stuck: press Enter` (text pinnutý `Sidebar.test.ts:1020`, `SessionDetails.test.ts:247`) | `crit` | riadok, detail, HostDetail, `stuckMessage` |
| `inactive` | `SessionRowItem.svelte:288` | `IconStopped` | `inactive` | `muted` | riadok |
| `lost 5m ago` | `:229-233` | `IconGhost` → `ghost` | `lost` + vek | `muted` | riadok (ghost) |
| `frozen` / `orphan` (iba bodka) | `:557-558` | `IconFrozen` → `snowflake`, `IconOrphan` → `unlink` | `frozen` / `orphan` | `info` / `warn` | riadok, detail |
| `✓ CI` / `✗ CI` / `… CI` | `attention.ts:345-349` | `IconDone` / `IconFailed` / `IconPending` → `loader-circle` | `CI` | `ok` / `crit` / `warn` | riadok (ikona bez textu za `PR↗`), detail |
| `6 ⚡2 ⏸1` | `hosts_view.ts:125-126` | `IconWorking`, `IconBlocked` | `sessionCounts` vráti iba čísla; `text` pole **zmazať** | — | `HostsList.svelte` renderuje `<Icon> N` |
| `• ✕ ⏸ ✓` | `conversation.ts:305-310` | `IconDot`/`IconFailed`/`IconStopped`/`IconDone` | — (ikona pred vetou) | `info`/`crit`/`warn`/`ok` | `ConversationPanel` notifikácie |
| `△ low soon`, `▲ LOW`, `■ limit` | `account_usage.ts:98-110,287-291` | `IconUsageCaution` → `triangle-alert` (outline), `IconUsageLow` → `triangle-alert`, `IconUsageLimit` → `octagon-alert` | `low soon` / `LOW` / `limitWording()` | `warn` / `crit` / `crit` | `UsageBlock`, `NewSessionDialog` chipy (UXPR-35) |
| `◷ 14m`, `?`, `…` | `hosts_view.ts:264-284` | `IconStale` → `clock`, `null`, `IconPending` | `{age, state}` — `mark` pole zmazať | `muted` | `HostsView` group header (UXPR-35; **UX-68 z iter. 06 rieši `?`** — koordinovať s UXPR-23) |
| `⏸ ⚠ 🔑 ○ ◷` v `UsageLine.glyph` | `account_usage.ts:318-320,454-595` | `IconBlocked`, `IconAlert`, `IconLogin` → `key-round`, `IconOffline` → `circle`, `IconStale` | `text` už existuje | `tone` už existuje | `UsageBlock` (UXPR-35) |
| `usage ▲ label 5h 8% left` | `usage_glance.ts:169-171,298,325-328` | ako riadok vyššie | text bez glyfu | podľa `level` | `App.svelte` stavový riadok (UXPR-35) |
| `🔑 ⚠ ⬆` `HostAttention.glyph` | `hosts_view.ts:189-209` | `IconLogin`, `IconAlert`, `IconUpgrade` → `arrow-up` | `kind` | `warn` | `HostsList` (UXPR-35) |
| `PR↗` | `SessionRowItem.svelte:405` | **ostáva symbol** (konsolidácia §5) + `aria-label="Open pull request"` | — | `--accent` | riadok, detail |
| `⇄ moving to …` | `TransferChip.svelte:37-57` | **ostáva symbol** (konsolidácia §5) | — | — | bez zmeny; `TransferChip.test.ts:81-139` bez zmeny |
| `⌘ ↵ ⇧ ↑↓` | rôzne | **ostávajú** (klávesy) | — | — | — |

Injektívnosť (iter. 01 test): `check` má jeden význam „done/passed“ (`IconDone` zdieľajú
`completed` a CI passing), `x` jeden „failed“ (`failed`, CI failing, notifikácia failed), `pause`
jeden „blocked/paused“ (`blocked`, počty, notifikácia stopped → **nie**: notifikácia `stopped`
používa `IconStopped` = `square`, aby `pause` ostal iba pre `blocked`). `triangle-alert` má dnes dva
významy (stuck, usage low) — rozlíšiť `IconStuck` (`triangle-alert`) vs `IconUsageLow`
(`gauge`/`battery-low`) je otázka Q6.

### Testy, ktoré model zlomí (82 asserov, spočítané `grep` cez `src/lib/*.test.ts`)

| Súbor | Asserov | PR | Náhrada |
|---|---|---|---|
| `account_usage.test.ts` | 25 | 35 | `expect(badge).toMatchObject({ icon: 'IconUsageLow', text: 'LOW', tone: 'crit' })`; texty bez glyfu |
| `usage_glance.test.ts` | 16 | 35 | `glance.text` bez glyfu + `glance.icon` |
| `UsageBlock.test.ts` | 16 | 35 | `within(line).getByRole('img', { name })` alebo `data-tone` |
| `conversation.test.ts:644-651` | 6 | 32 | `notificationChip(null).icon === 'IconDot'` … |
| `hosts_view.test.ts:86-101,169` | 5 | 32 (`sessionCounts`) / 35 (`freshnessMark`) | `toEqual({ total: 6, working: 2, blocked: 1 })`; `{ age: '14m', state: 'stale' }` |
| `HostsView.test.ts:149,158,161,348` | 4 | 35 | `data-state="stale"`, `getByRole('img', { name: /offline/ })` |
| `NewSessionDialog.test.ts:1121-1162` | 4 | 35 | text bez `▲`/`◷`, tón cez `data-tone` |
| `TransferChip.test.ts:81-139` | 4 | — | **bez zmeny** (`⇄` ostáva) |
| `HostsList.test.ts` | 1 | 35 | role/name |
| `ToolLine.test.ts:69` | 1 | UXPR-03 | (iter. 01) |
| `Sidebar.test.ts:1020,1236`; `SessionDetails.test.ts:247,311` | 4 (substring) | 32 | nezlomia sa; doplniť `data-tone` asserty |
| `attention.test.ts:83-90,209-210` | 3 | 32 | `claudeStatusChip(s).text === s`; `ciChip('passing').tone === 'ok'` |

## Prístupnosť a hustota

- **Výška riadku:** cieľ **36 px** (L1 14 + gap 2 + L2 13 + padding 2 × 3,5) pri `details off`,
  **48 px** s linkou 3. Dnes 35 px bez zalomenia, 50–65 px zalomený (UX-102), 22 px `details off`.
  Pri 60 sessions a 900 px vysokom zozname: dnes ≈ 15–18 riadkov na obrazovku, po zmene 25
  (`details off`) / 18 (`details on`) — a žiadny riadok nemá inú výšku než susedný.
- **Písmo:** L1 12 px (`--control-font`), L2/chipy/signály 10 px (nový token `--row-font-2: 10px`
  v `app.css` vedľa `--control-font-sm`), nikdy 8,4 px (UX-109). Meno ostáva mono (`--mono`,
  `app.css:23`); linka 2 host/worktree mono, signály `tabular-nums`.
- **Kontrast:** žiadny hex a žiadna `opacity` na texte (UX-100, inventár 9 a 14): cena a oddeľovače
  `--fg-muted` plné (dark 6,73 : 1, light 5,33 : 1); oddeľovač môže byť `--control-border-strong`
  (3,14 : 1, nie text). Chipy cez tón → tokeny s ≥ 4,5 : 1. `tokens.test.ts` dostane páry
  `usage-ok/warn/crit` vs `bg` (dnes iba vs `bg-pane`, `tokens.ts:81-83`), lebo riadok sedí na `--bg`.
- **Fokus a hit area (UX-107):** `.sess-row:focus-visible` = jediný 2 px ring z `controls.css:41-44`
  (riadok nie je `.btn`, pravidlo sa pridá cielene); `.sess-row:focus-within .row-actions {
  display: flex }`; akcie `btn btn--icon btn--quiet` **24 × 24 px** (`--control-h`), 3 viditeľné +
  `…` (`aria-haspopup="menu"`), medzera `--control-gap`; celý pás 3 × 24 + 24 + 3 × 6 = **114 px**
  nad pravým koncom L2, stav na L1 ostáva viditeľný (UX-101). Hint `session-actions` text →
  „Hover, focus or select a session to restart, label or kill it; more under …“.
- **`aria` na chipe:** `role="img"` + `aria-label={chip.title}` (napr. „Claude: blocked for 3 min —
  waiting for permission“), ikona `aria-hidden`; `title` iba keď nesie viac než label. Kontextový
  chip ostáva `role="meter"` s `aria-valuenow` (`:373-377`) a dostane `aria-valuetext="53 % of 200k"`.
  `PR↗` link `aria-label="Open pull request"`. Bodka ostáva `aria-hidden` **bez `title`**.
- **Pohyb:** riadok dnes nič neanimuje (`conversation_theme.test.ts:98-112` ho preto nekontroluje).
  Odporúčanie: **žiadna animácia `working`** — 20 pulzujúcich bodiek v sidebare je šum; statický
  `IconWorking` + `ok` tón stačí. Ak by sa raz pridal spinner pre `CI pending`, musí mať
  `@media (prefers-reduced-motion: reduce) { animation: none }` — a ten test ho **nezachytí**: jeho
  glob (`conversation_theme.test.ts:5`) číta iba `ConversationPanel`, `ConversationHeader`,
  `ToolLine` a `SubagentBlock`; PR, ktorý do riadku pridá animáciu, musí `SessionRowItem.svelte`
  a `StatusChipView.svelte` do toho globu doplniť.
- **Checkbox v `role="button"`** (`:172-186`, priznaný a11y smell) — mimo šošovky (10), ale
  UXPR-33 ho nesmie zhoršiť: `select-box` zarovnaný na L1 ostáva.

## PR plán

**UXPR-19 je nahradené (superseded) dvojicou UXPR-32 + UXPR-35** — konsolidácia ho odložila
„po šošovke 9, aby sa model písal raz“; tu je napísaný raz (`status_chip.ts`) a delí sa iba
podľa *renderujúcich* súborov (session vs. usage), lebo 82 asserov v 10 súboroch by v jednom PR
prekročilo ~300 riadkov.

| UXPR | Názov | Súbory | Veľkosť | Závisí od | Paralelnosť / kolízie |
|---|---|---|---|---|---|
| **32** | **Stavový model** — `status_chip.ts` (typ, `claudeStatusChip`, `stuckChip`, `lifecycleChip`, `inactiveChip`, `ciChip`, `contextChip`, `sessionStatusChip`, `notificationChip`), `StatusChipView.svelte`, +9 sémantických exportov v `icons.ts` (`IconWorking`, `IconBlocked`, `IconDone`, `IconFailed`, `IconStopped`, `IconStuck`, `IconPending`, `IconGhost`, `IconFrozen`, `IconOrphan`, `IconDot`), tón → CSS triedy; `attention.ts` zmaže `STATUS_LABEL`/`STATUS_COLOR`/`STUCK_COLOR`/`ciStatusLabel`/`ciStatusColor` (`:29-53,69,342-366`), `displayName` `:303` zostáva do UXPR-06; `hosts_view.ts:113-128` `sessionCounts` bez `text`; `conversation.ts:305-310` → `notificationChip`; render: `SessionRowItem.svelte:208-222,275-296` (iba náhrada chipov, **nie** markup), `SessionDetails.svelte:432-446,511-518`, `HostDetail.svelte:98-102`, `HostsList.svelte` (počty), `ConversationPanel.svelte` (mark) | `status_chip.ts` ~110, `StatusChipView` ~40, úpravy ~60 → **S/M ~140** (+120: `status_chip.test.ts` nový ~70, `attention.test.ts` −20/+15, `conversation.test.ts:644-651`, `hosts_view.test.ts:86-101`, `HostsView.test.ts:161`, `Sidebar.test.ts`/`SessionDetails.test.ts` +`data-tone`) | **01** (ikony) | lane A; ∥ s B, D. `SessionRowItem.svelte`: **po 02** (02 mení akcie `:297-352`, 32 mení chipy `:275-296` — iné regióny, ale jeden zapisovateľ na súbor → sekvenčne). `SessionDetails.svelte`/`ConversationPanel.svelte`: UXPR-27 (iter. 07) edituje iné regióny (Safe remove `:695-704`, repair) — poradie 27 → 32 alebo 32 → 27, nie súbežne. `HostsList.svelte`/`HostDetail.svelte`: **po UXPR-23** (iter. 06) |
| **34** | **Formátovanie a prahy** — `src/lib/format.ts` (nový: `formatMoney`, `formatTokens` jediný, `formatDuration` ← `formatElapsed`, `formatAge` ← `timeAgo` jedna jednotka, `promptPreview` + `isNoisePrompt` so stop-listom P2 a slash príkazmi), `sessions.ts:133-150` a `conversation.ts:904-909` → re-export/import, `attention.ts:75-76` prahy **80/95** + `:315-340` presun, `session_status.ts:33-49` → `format.ts`; texty tooltipov kontextu (`crit` → „send /compact…“) | **S ~90** (+80: `format.test.ts` nový; `sessions.test.ts:31-35`, `conversation.test.ts:469-475`, `attention.test.ts:94-103,189-206`, `session_status.test.ts`, `Sidebar.test.ts:1028-1041,1060`) | — | ∥ so všetkým (nové súbory + presuny); **pred 33** (33 ho konzumuje). `conversation.ts` koliduje s ničím z fronty |
| **33** | **Hierarchia riadku** — `SessionRowItem.svelte` prepis markupu `:201-425` a CSS `:430-688` (jedna kostra pre live/ghost/external/bg, L1 chip vpravo, L2 host · worktree · `RowSignals`, L3 prompt pri `details on`, akcie 3 + `…` nad L2, `:focus-visible`/`:focus-within`, `showHost` prop, effort a elapsed von, kontext chip ako `<button>` s `/compact` pri crit, `~` pri `context_stale`), `RowSignals.svelte` (nový ~80), `Sidebar.svelte:639-660` (+`showHost`, `onCompact` callback → composer preset), `SessionDetails.svelte:429-456,459-520` (StatusChipView, `Idle since`/`Stuck since`, `formatMoney`), `app.css` `--row-font-2`, `hints.ts:38` text | **M ~230** (+150: `Sidebar.test.ts` — línia/testid asserty `:813-816,1196-1306,1362-1370` ostávajú (testidy `sess-row`, `sess-details`, `host-badge`, `claude-chip`, `context-badge`, `cost-badge`, `ci-badge`, `sess-meta`, `sess-tmux-name` **zachované**), nové: bez zalomenia (`getComputedStyle` nie je v jsdom → assert na triedu `nowrap` + snapshot-free počet detí), ghost/external štruktúra, fokus, `/compact`; `SessionDetails.test.ts` ±10) | **02, 32, 34**; mäkko 28 (View menu „Details line“ text — ak 28 landne skôr, 33 iba upraví label) | lane A koniec; **pred 06** (06 potom mení iba `displayName()` volanie + `data-name-source` na L1 — ~5 riadkov v riadku namiesto ~15). `SessionDetails.svelte`: po 27 a 32. Ak 33 landne pred 28, prepínač `rows.details` mení sémantiku (Q1) — 28 to má prevziať v labeli |
| **35** | **Usage značky na model** — `account_usage.ts:98-110,287-291,318-320,454-595` (`glyph` → `icon`, tón už existuje), `usage_glance.ts:169-171,298,325-328`, `hosts_view.ts:189-209,264-284` (`HostAttention.glyph` → `icon`; `freshnessMark` → `{ age, state, title }`), render `UsageBlock.svelte`, `HostsList.svelte`, `HostsView.svelte` (group freshness), `NewSessionDialog.svelte` (usage chipy), `App.svelte` (glance) | **M ~160** (+130: `account_usage.test.ts` 25, `usage_glance.test.ts` 16, `UsageBlock.test.ts` 16, `hosts_view.test.ts` 4, `HostsView.test.ts` 4, `NewSessionDialog.test.ts` 4, `HostsList.test.ts` 1) | **32** (typ + `StatusChipView`), 01 | **po UXPR-23** (iter. 06 edituje `hosts_view.ts`, `usage_glance.ts`, `HostsView`, `HostsList`, `UsageBlock`) a koordinovať s **UX-68** (iter. 06: `?` bez dôvodu — rieši sa v tom istom `freshnessMark`; nech 23 zmení sémantiku a 35 iba tvar). `NewSessionDialog.svelte`: UXPR-29 (iter. 08) mení `NewBgSessionDialog`, nie tento — bez kolízie |

```
lane A  ikony        01 → 02 → 32 → 33 → 06(B)          34 ∥ (pred 33)
                                 └→ 35 (po 23 z lane E-hosts)
```

Poradie v `SessionRowItem.svelte`: **UXPR-02 → 32 → 33 → 06.** Zdôvodnenie: 02 je mechanická
náhrada glyfov za ikony a landne prvý (iterácia 08 ho už chce pred UXPR-28); 32 vymení chipy bez
dotyku rozloženia; 33 prepíše kostru — keby šlo 06 skôr, jeho `data-name-source` a tooltip by 33
prepísal. UXPR-02 navrhnutý `btn--sm` 20 px (iter. 01 položka 3) treba **zmeniť na 24 px** už v
UXPR-01/02 (UX-107), inak 33 kópie zase ruší.

Regen: žiadny (`REGEN_*` sa netýka — žiadne Rust zmeny). Backend follow-up mimo tejto šošovky:
`effort_level` buď začať plniť, alebo zmazať stĺpec (Q5).

## Akceptačné testy

1. **`status_chip.test.ts`** (UXPR-32): pre každý `ClaudeStatus` z `CLAUDE_STATUSES` je
   `claudeStatusChip(s, null, 0).text === s`; `tone` matica (`working → ok`, `blocked → warn`,
   `completed → info`, `failed → crit`, `stopped/idle → muted`); `icon` je kľúč `icons.ts`
   (`Object.keys(ICONS)` obsahuje ho) alebo `null` iba pre `idle`; `stuckChip('press_enter').text
   === 'stuck: press Enter'` a `tone === 'crit'`; `sessionStatusChip` poradie: stuck › inactive ›
   `frozen` › claude_status; `age`: `blocked` s `idle_since = now − 180` → `'3m'`, `working` →
   `undefined`; **žiadny test neobsahuje glyf** — lint assert: `expect(chip.text).toMatch(/^[\w\s:%~.-]+$/u)`
   pre všetky vetvy.
2. **`StatusChipView`** (`Sidebar.test.ts`): `getByTestId('claude-chip')` má `data-tone="ok"` pri
   `working`, `aria-label` začína „Claude: working“; `querySelector('svg')` má `aria-hidden="true"`;
   `textContent` neobsahuje `⚡`. Stuck riadok: jeden chip s `data-tone="crit"`, `claude-chip`
   chýba (existujúci `:1024` ostáva).
3. **Kontrast** (`tokens.test.ts`): pridať páry `{ fg: 'usage-ok'|'usage-warn'|'usage-crit', bg:
   'bg', min: 4.5 }` a `{ fg: 'fg-muted', bg: 'bg', min: 4.5 }`; nový test číta
   `SessionRowItem.svelte` + `status_chip.ts` + `StatusChipView.svelte` a **zlyhá pri
   `#[0-9a-f]{3,6}`** v `<style>` alebo `style=` (rovnaký vzor ako `controls.test.ts` číta CSS regexom).
4. **`format.test.ts`** (UXPR-34): `formatMoney(4_280_000) === '$4.28'`, `(53_190_000) === '$53'`,
   `(194_000_000) === '$194'`, `(3_128_000_000) === '$3,128'`, `(5_000) === '<$0.01'`;
   `formatTokens(1_234) === '1.2k'` a `conversation.ts` už `formatTokens` neexportuje (assert
   `import * as c from './conversation'; expect('formatTokens' in c).toBe(false)`);
   `formatAge(42) === '42s'`, `(3_600·3 + 60·12) === '3h'`, `(86_400·2) === '2d'`;
   `promptPreview('/clear') === ''`, `('yes') === ''`, `('go') === ''`, `('Implement the triage
   filter\nsecond') === 'Implement the triage filter'`; `contextLevel(79.9) === 'ok'`, `(80) ===
   'warn'`, `(94.9) === 'warn'`, `(95) === 'crit'` (nahrádza `attention.test.ts:97-102`);
   `Sidebar.test.ts:1028-1041` fixtures 72/95/10 → 85/97/10.
5. **Hierarchia** (`Sidebar.test.ts`, UXPR-33): L1 obsahuje `sess-name` a `claude-chip`, **nie**
   `host-badge` (`:1245` ostáva); L2 (`sess-details`) obsahuje `host-badge`, `sess-tmux-name`,
   `context-badge`, `cost-badge`, `ci-badge`, **nie** `sess-meta` ani `sess-elapsed`; `sess-meta`
   je v `[data-testid="sess-prompt"]` (L3) iba pri `rows.details = true`; pri `false` L2 **ostáva**
   (zmena oproti `:1265,1292,1306` — Q1); `effort-badge` neexistuje ani pri `effort_level:
   'high'`; `sess-details` má triedu `nowrap` a najviac **3** deti s `data-signal`; s `pr_url` +
   `context_pct` + tokenmi presne 3, bez tokenov 2.
6. **Kontext crit** (UXPR-33): `context_pct: 97` → `context-badge` je `<button>` s `title`
   obsahujúcim `/compact`; klik volá `onSelectSession` a `onCompact(sess)`; `context_stale: true`
   → text `~97%`; `context_pct: 53` → `<span>`, nie button.
7. **Ghost a external** (UXPR-33): ghost riadok má `sess-details` s `host-badge` a chip
   `data-tone="muted"` s textom `lost` a `age`; `ghost-recreate`/`ghost-dismiss` sú viditeľné bez
   hoveru (nie pod `.row-actions`); external riadok (`kind: 'external'`, cez `outside-fleet`) má
   `host-badge` na L2, `role="img"` badge `IconExternal` (po UXPR-06 — dovtedy textový chip
   `outside`), žiadne `row-actions`.
8. **Klávesnica** (UXPR-33): po `row.focus()` má `row.querySelector('.row-actions')` triedu/atribút
   viditeľnosti (`data-visible="true"` nastavované z `:focus-within` cez `onfocusin` — jsdom
   nepočíta CSS); `getAllByRole('button')` v akciách má 4 (3 + `…`); klik `…` otvorí menu s
   `Rename tmux session` a `Recreate`; každá akcia má `aria-label` a `svg[aria-hidden]`.
9. **Detail súhlasí s riadkom** (`SessionDetails.test.ts`): pre tú istú fixture je
   `details-claude-status` `data-tone` rovnaký ako v riadku; `details-context` text `97%` s
   rovnakým `data-level`; `details-cost` používa `formatMoney` (`'$53 estimated'`); `Idle since` sa
   zobrazí pri `idle_since !== null`.
10. **Usage značky** (UXPR-35): `severityBadge('low')` → `{ icon: 'IconUsageLow', word: 'LOW' }`
    a `word` bez `▲`; `freshnessMark` → `{ age: '14m', state: 'stale' }` (bez `◷`);
    `UsageBlock` riadok `rate_limited` má `role="img"` s menom obsahujúcim „rate-limiting“ a
    `data-tone="warn"`; `grep -c '[⚡⏸✓✗■◷▲△🔑○]' src/lib/*.ts` (mimo `TransferChip`,
    `BranchList`, klávesových symbolov) = **0** — ako lint test v `icons.test.ts`.
11. **`svelte-check`** čistý; `npx vitest run` celý suite (memory: nepipe-ovaný).

## Odhad

| PR | Veľkosť | Diff bez testov | Testy |
|---|---|---|---|
| UXPR-32 stavový model | S/M | ~140 | ~120 |
| UXPR-34 formátovanie + prahy | S | ~90 | ~80 |
| UXPR-33 hierarchia riadku + detail | M | ~230 | ~150 |
| UXPR-35 usage značky | M | ~160 | ~130 |
| **Spolu (nahrádza UXPR-19 M ~150 + 60)** | | **~620** | **~480** |

UXPR-33 je najbližšie k limitu; pripravený odštep **33a** (kostra + L2 klaster + akcie, ~170) /
**33b** (detail `SessionDetails.svelte` + `Idle since` + `hints.ts`, ~60), ak recenzent chce.

## Otázky pre vlastníka

1. **Sémantika `rows.details`** — dnes `off` = iba linka 1 (22 px); návrh `off` = linky 1 + 2
   (36 px), `on` = + prompt (48 px). Zmení sa `Sidebar.test.ts:1265,1292,1306` a label vo View menu
   (UXPR-28: „Details line“ → „Last prompt line“). *Odporúčanie: áno — host a worktree sú identita
   vo viac-hostovom fleete, prompt je jediné, čo je voliteľné; jednolinkový „compact“ režim
   nechať na neskôr ako tretiu hodnotu, ak si ho niekto vypýta.*
2. **Prahy kontextu 80/95 namiesto 70/90** (`attention.ts:75-76`; mení aj Conversation hlavičku cez
   `conversation.ts:925`). *Odporúčanie: 80/95 — 70 % rozsvieti tretinu fleetu a amber stráca
   význam; 95 % je blízko auto-kompakcie Claude Code.*
3. **Kontext ≥ 95 % ako triage?** Dnes nie je v `TRIAGE_BUCKETS` (`attention.ts:123-132`); riadok
   s 100 % je `idle` a pri status sorte (UXPR-30) skončí dole. Voľby: (a) nový bucket
   `context_full` medzi `failed` a `done_unread`, (b) iba promotovať chip na L1 (wireframe riadok
   `yes`), (c) nič. *Odporúčanie: (b) teraz v UXPR-33 (čisto vizuálne, žiadna zmena `classify`),
   (a) ako otázka pre šošovku 10/UXPR-30 — bucket mení aj počítadlo „Needs you“.*
4. **`/compact` na klik z riadku** — tlačidlo v chipe vyberie session a predvyplní composer
   (neodošle). Je to prvá akcia z riadku, ktorá píše do composera. *Odporúčanie: áno, ale iba
   predvyplniť; odoslanie ostáva na ⌘↵ — konzistentné s quick-action chipmi v Conversation.*
5. **`effort_level`** — odstrániť z riadku hneď (UX-105) a (a) backend začne pole plniť z
   `claude agents --json`/hooku, alebo (b) migrácia stĺpec zruší. *Odporúčanie: (a) ako backend
   nález mimo tejto fronty; do riadku sa nevracia — patrí do detailu a tooltipu ceny.*
6. **Ikona `triangle-alert` pre dva významy** (stuck vs. usage low) porušuje injektívnosť z
   iterácie 01. *Odporúčanie: `IconStuck = triangle-alert`, `IconUsageLow = battery-low`,
   `IconUsageCaution = battery-medium`, `IconUsageLimit = octagon-alert` — batéria číta „koľko
   ostáva“ presne tak, ako usage okno.*
7. **Tón `info` (piaty)** — zadanie počítalo so štyrmi (`ok/warn/crit/muted`). *Odporúčanie:
   prijať `info` (`--accent`) pre `completed`/`frozen`; bez neho dokončený agent svieti ako
   pracujúci.*

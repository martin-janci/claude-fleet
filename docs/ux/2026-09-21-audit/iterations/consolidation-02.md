# Konsolidácia 02 — po iteráciách 6–10

**Krok protokolu:** README §5 bod 3 (po 4–5 iteráciách jeden konsolidačný krok: dedup, priorita,
rozpad na PR; jeden klaster = jeden PR, max ~300 riadkov diffu) · **Vstup:**
`docs/ux/2026-09-21-audit/README.md`, `iterations/consolidation-01.md` (register UX-01…67, D1–D6,
UXPR-01…20, §4 „default“), `iterations/06-hub-parity-hosts-accounts.md` …
`10-select-mode-bulk-actions.md`, kód na `9c8ceabc` (iba na overenie čísel:
`hub_verdicts.generated.json` local_only **70** / routed 39 / routed_unless 1 / same_in_both 20;
`BUDGET_BYTES = 57_700` v `crates/fleet-core/src/mcp/tools/tests.rs:2357`; `src/lib/session_view.ts`
**existuje**, `src/lib/session_name.ts` nie; `commands/sessions.rs:577` posiela celý `SendPromptArgs`
do hubového toolu `send_prompt`, ktorého schéma je `SendPromptParams` v `mcp/tools/params.rs:236-259`;
`discover_hosts` je `Access::Client, readonly: true` v `guard.rs:127-132`) · **Režim:** read-only,
žiadne zmeny kódu, žiadny `cargo`, žiadny `git` zápis okrem tohto súboru.

## Zhrnutie

1. Register rastie na **123 nálezov**: 56 nových z iterácií 6–10 (UX-68…UX-123; H 4 · M 26 · L 26).
   Z 14 auditových nálezov, ktoré čakali na šošovky 6–17, ich tento blok spracoval **8**
   (UX-08, 09, 10, 12, 19, 20, 21 a časť UX-27): 3 potvrdené bez zmeny (09, 12, 20), 4 potvrdené
   **s korekciou premisy** (08, 10, 19, 21 — 21 stúpa M → H), 1 spresnený (27). Zvyšných 7
   (UX-13, 14, 22, 24, 25, 27, 28) čaká na šošovky 11–17. UX-13 sa iterácia 07 **nedotkla** (zadanie
   sa pýtalo „by 07?“ — nie).
2. Sedem premís z konsolidácie-01, README alebo zo zadaní iterácií padlo: SEC-5 „broadcast bez
   potvrdenia“ (10), „hubov `safe_kill_session` počíta tie isté fakty“ (07), „`bg off` je default“ a
   „skrýva Outside fleet“ (08), Hosts chord je **⌘I**, nie ⌘1 (08), D4 „rezervné trimy sú rezerva“
   (06 ich spotrebuje, 07 potrebuje ďalších +2 070 B), `session_view.ts` „nový“ v UXPR-06 (súbor
   existuje — UX-111), a „UXPR-38 stojí 0 B rozpočtu“ (10) — hub route serializuje `SendPromptArgs`
   do `SendPromptParams`, takže pole `label` je aj MCP parameter (≈ +120 B na každej ploche).
3. Osem rozhodnutí „raz“ (§2, D7–D14). Najväčšie: **D7 — dva stropy rozpočtu** (`AGENT_BUDGET_BYTES`
   tvrdý 51 000 na host-full plochu, `MASTER_BUDGET_BYTES` mäkký 65 500 pokrývajúci tranže 03–07),
   nastavené **raz v UXPR-25**, ktorý ide **pred UXPR-08/09** — nahrádza D4 (63 800). **D8 — jedno
   poradie zapisovateľov** pre `Sidebar.svelte` / `SidebarFilters.svelte` / `SessionRowItem.svelte`,
   ktoré **odpútava lane F (sidebar) od lane D (hub Rust)**: položky UXPR-23 a UXPR-27 v `Sidebar.svelte`
   sa presúvajú do UXPR-02/29 a UXPR-37. **D9** potvrdzuje `StatusChip {icon,text,age?,tone,title}`
   (82 glyfových asserov v 10 súboroch) a ruší UXPR-19. **D10** — hromadný prompt = N × `send_prompt`
   s `label: Option<bool>` `#[serde(default)]`, pole zavádza **UXPR-04**, UXPR-38 ho konzumuje.
4. Fronta má **38 riadkov** `UXPR-01…38`: **35 v hlavnej fronte**, 1 odložený (24), 1 nahradený
   (19 → 32 + 35), 1 zrušený (20 — otázka A2 „default“ = emoji v strome súborov ostávajú). Diff
   hlavnej fronty ≈ **6 160** riadkov bez testov (+≈ 4 730 testov, + generované). Kritická cesta je
   lane D: `25 → 09 → 10 → 11 → 12 → 13 → 22a → 22b → 26a → 26b` (10 sekvenčných PR, bolo 5) a za ňou
   `27`. Lane A (ikony + riadok), B (pomenovanie), C (zdieľané), F (sidebar) bežia paralelne s D.
5. Verdikty: LocalOnly **70 → 42** (−2 Settings, −10 git, −7 Assets, −4 Hosts/usage, −5 sessions;
   s odloženým UXPR-24 **40**), Routed 39 → 67. Rozpočet: master 57 603 → ≈ 65 500 (+13,7 %), agent
   ≈ 50 300 → ≈ 49 000 (klesá — parity tools sú `ClientOnly`, trimy a `discover_hosts → Master`).
6. Pre vlastníka ostáva **23 otázok** v siedmich témach (§4) z 39 pôvodných; 16 rozhoduje §2. Každá má
   odporúčanie — odpoveď „default“ na celý blok je platná.

## 1. Register nálezov UX-68…UX-123

Stav: **nové** (vzniklo v iterácii 6–10) · pri auditových nálezoch nižšie **potvrdené** / **opravené** /
**vyvrátené** ako v konsolidácii-01. `↔` = prekryv; `≡` = ten istý vzor. PR podľa §3. Závažnosť je
verdikt iterácie.

| ID | Sev | Stav | Iter. | Zhrnutie | PR |
|---|---|---|---|---|---|
| UX-68 | H | nové | 06 | `?` v stĺpci účtov bez dôvodu a bez východiska: `freshnessMark` zahodí `status`/`detail`/`source_host`; v hub režime je refresh disabled | UXPR-22, 23 (sémantika), 35 (tvar) |
| UX-69 | H | nové | 06 | usage v hub režime je iba event-driven: bootstrap čítanie preskočené, `FleetResync` usage nerelistuje, `…` až 5 min; premisa „nothing to route to“ nepravdivá — hub má `UsageCache` | UXPR-22 |
| UX-70 | M | nové | 06 | `instead` mieri na `fleet-hub` subpríkazy, ktoré neexistujú (`add/remove/hide_host`, `provision_hosts`); token trio nemenuje existujúce CLI — rodina UX-42 | UXPR-22b |
| UX-71 | M | nové | 06 | hubov skrytý `local` host desktop lístuje, počíta v „Hosts 6“ a pripína účet so `?`; žiadny „show hidden“ | UXPR-23 (D12) |
| UX-72 | M | nové | 06 | jeden ovládač, dve vety: token-mode/Rotate nesú dôvod `remove_host`; `REASONS.add/remove/hide_host` identické 3× | UXPR-22b (`FLEET_ADMIN`), 23 |
| UX-73 | M | nové | 06 | falošné „usage every 5 min“ a „via mefistos“ v hub režime — polluje hub, nie tento proces | UXPR-23 |
| UX-74 | M | nové | 06 | `readonly` klient zlyhá až po kliku aj pri routovaných mutáciách — Hosts kontext UX-45/57 | UXPR-21 |
| UX-75 | L | nové | 06 | fetch triggery zo specu („Hosts open“, „New-session open“, Retry) v hub režime mlčia — gate `ownsTheFleet` namiesto `hubActionBlocked` | UXPR-22b (min. TS), 23 |
| UX-76 | L (bezp.) | nové | 06 | hubov `discover_hosts` je Client/readonly a vracia operátorove SSH aliasy každému paired klientovi; jediný konzument `add_host` je Master | UXPR-22a (D11) |
| UX-77 | H | nové | 07 | Safe remove v hub režime je slepá ulička **aj pre routovanú cestu**: tlačidlo disabled cez `inspect_safe_kill`, `safe_kill_session` (Routed) je iba za dialógom — ostáva blind Kill | UXPR-26, 27 |
| UX-78 | M | nové | 07 | tri spôsoby ukončenia bez vysvetlenia; Kill dialóg mlčí o worktree (uncommitted/unpushed ostáva na hoste) — rovnaká kópia riadok/detail/bulk ↔ UX-117 | UXPR-06 (`killDialogCopy`, D13), 27, 37 |
| UX-79 | M | nové | 07 | Repair workspace beží bez potvrdenia, hoci hub ho vedie `confirm: true`; pre-attach kontrola na hube ticho preskočená ↔ UX-62 | UXPR-27 |
| UX-80 | M | nové | 07 | detail tool-callu v hub režime zlyhá per riadok tou istou `E_LOCAL_ONLY` vetou ako `role="alert"` bez Retry | UXPR-26, 27 |
| UX-81 | L | nové | 07 | živý indikátor Conversation na hube ticho vypnutý; refusal zbytočný — `probe_from_tail` je čistá funkcia nad `capture_session` | UXPR-26 (RE), 27 |
| UX-82 | L | nové | 07 | `instead` radí „use Kill instead“ — Kill je pre neaktívneho agenta skrytý v riadku aj detaile | UXPR-26 (RN `dismiss_agent_session`) |
| UX-83 | L (docs) | nové | 07 | doc drift: `control-api.md` „move_session master only“ (policy Client), `hub.md` E_CONFIRM zoznam bez `repair_session`, test „the five“ pri štyroch | UXPR-26b |
| UX-84 | L | nové | 07 | dvojité potvrdenie, druhé až po kliku: desktopov dialóg → hub `E_CONFIRM_REQUIRED` → toast; nič vopred nehovorí, že hub potvrdenia vyžaduje ↔ UX-120 (×N) | UXPR-27 (po 09), 37 |
| UX-85 | L | nové | 07 | `instead`/`REASONS` vety do prázdna: `inspect_safe_kill` „retire … from the hub“, `discard_kill_session` „from the hub“, `purge_project` — ≡ UX-42/49/70 | UXPR-26b |
| UX-86 | L | nové | 07 | `resolve_move` routuje, ale nie je v `ROUTED_ACTIONS` → Finish/Undo bez gate na spojenie | UXPR-26b (min. TS), 27 |
| UX-87 | M | nové | 08 | časový filter je **projektový** (`project.last_session_at`), tvári sa session-ovo: `8h` ukáže projekt so všetkými 30-dňovými sessions | UXPR-31 |
| UX-88 | M | nové | 08 | vyhľadávanie filtruje **projekty**, nie riadky (zhoda jednej session = všetky); tri vyhľadávania, tri správania (sidebar / ⌘K / Hosts `/`) | UXPR-31 |
| UX-89 | L | nové | 08 | host pills sa neškálujú (2–3 riadky pri 280 px) a duplikujú fakty Hosts view | UXPR-28 |
| UX-90 | L | nové | 08 | Tasks a Settings sú deti `nav aria-label="host filter"`; Tasks bez chordu; `☑` = Tasks aj Select ↔ UX-33 | UXPR-28 |
| UX-91 | M | nové | 08 | prepínače zobrazenia ako text „on/off“ (stav kódovaný 2×); `bg off` bez „(n hidden)“ a Outside fleet ho ignoruje | UXPR-28 |
| UX-92 | M | nové | 08 | žiadny status sort v zozname — `byTriage` exportovaný a nevolaný; `blocked` pod `idle`; FE-4 „status sort“ nie je landed | UXPR-30 |
| UX-93 | L | nové | 08 (potvrdené 10) | bulk lišta sa vkladá medzi riadky filtrov → skok ≈ 28 px | UXPR-28 (slot), 37 (obsah) |
| UX-94 | L | nové | 08 | tri zo siedmich hintov visia na chrome, ktorý sa mení; id ponechať, kotvy presunúť | UXPR-28, 29 |
| UX-95 | L | nové | 08 | lokálne CSS pod 24 px podlahou (`.pill` 18, `.search` 24,7, `.icon-btn` 21,6, `.new-btn`, `.theme-toggle`) ↔ UX-29, 107 | UXPR-28, 29 |
| UX-96 | L | nové | 08 | `NewBgSessionDialog` ponúka skryté hosty, kým pills a `NewSessionDialog` ich vyraďujú ↔ UX-71 | UXPR-29 |
| UX-97 | M | nové | 09 | `8h 23m` je vek od `started_at` (iba fleet-vytvorené riadky), nie aktivita; riadok a detail nesúhlasia; užitočný je vek **stavu** | UXPR-32 (age v chipe), 33 (elapsed von) |
| UX-98 | M | nové | 09 | 100 % kontext bez akcie; prahy 70/90 rozsvietia 3 z 8 riadkov; `context_stale` neviditeľný | UXPR-34 (prahy 80/95, D14), 33 (`<button>` → `/compact`) |
| UX-99 | L | nové | 09 | formát čísel: štyri šírky ceny, „unpriced“, **dva** `formatTokens` s inými výstupmi | UXPR-34 (`format.ts`) |
| UX-100 | H | nové | 09 | stavové farby sú hex (`STATUS_COLOR`, `STUCK_COLOR`, `ciStatusColor`) a vo **light téme zlyhávajú** (working 1,94 : 1, blocked 1,72 : 1) ↔ UX-30 | UXPR-32 (tón → tokeny) |
| UX-101 | M | nové | 09 | hover akcie sú absolútne cez pravý koniec linky 1 a **prekrývajú stavový chip**; vybraný riadok nikdy neukáže stav | UXPR-33 |
| UX-102 | M | nové | 09 | linka 2 pri 280 px vždy zalamuje (fixné položky ≈ 293 px na 243 px) → riadok 50–65 px | UXPR-33 |
| UX-103 | L | nové | 09 | posledný prompt bez filtra (`yes`, `go`, `push`, `/clear` na 5 z 8 riadkov) — P2 stop-list platí iba pre meno ↔ UX-05 | UXPR-34 (`promptPreview` + `isNoisePrompt`) |
| UX-104 | L | nové | 09 | `· idle` / `⚡ working` — glyf v dátach (`STATUS_LABEL`); `· idle` vyzerá ako zabudnutý oddeľovač | UXPR-32 |
| UX-105 | L | nové | 09 | effort badge je mŕtvy kód: `effort_level` zapisuje iba reconcile a to `None` | UXPR-33 (von); backend Q-II.16 |
| UX-106 | M | nové | 09 | tri druhy riadkov, tri štruktúry (live 2 linky · ghost 1 linka s hostom vľavo · external bez hosta a badge); klávesové správanie sa tiež líši | UXPR-33 (jedna kostra) |
| UX-107 | M | nové | 09 | klávesnica sa k akciám nedostane (`:focus-visible` 0×, akcie iba `:hover`/`.selected`); akcie ≈ 20 × 16 px pod 24 px podlahou — iter. 01 `btn--sm` 20 px v rozpore ↔ UX-34, 95 | UXPR-33; korekcia UXPR-01/02 (24 px) |
| UX-108 | L | nové | 09 | `title`/`aria` duplicity; `PR↗` bez `aria-label` („PR north east arrow“); `ci-badge` bez role | UXPR-32, 33 |
| UX-109 | L | nové | 09 | 8,4 px písmo pre signály (`0.6rem`); `--control-font-sm` 11 px sa v riadku nepoužíva | UXPR-33 (`--row-font-2` 10 px) |
| UX-110 | L | nové | 09 | `status` bodka je druhý farebný kanál bez textu (9/9 zelených = 0 bitov); `frozen`/`orphan` nikde ako slovo | UXPR-32 (`lifecycleChip`) |
| UX-111 | L | nové | 09 | kolízia mena súboru: `session_view.ts` **existuje** (`resolveSessionView`), iterácia 02 ho chce vytvoriť; štvrtý výraz mena v `attention.ts:303` ↔ UX-40 | UXPR-06 (`session_name.ts`, D13) |
| UX-112 | M | nové | 10 | select mód bez výberu nemá afordanciu: checkboxy, žiadna lišta, žiadny hint — presnejšia forma UX-10 | UXPR-37 (+36) |
| UX-113 | M | nové | 10 | shift-klik nie je rozsah — shift/⌘/ctrl robia toggle jedného riadku; test to pinuje | UXPR-36 |
| UX-114 | M | nové | 10 | ghost a bg riadky prejdú do hromadných akcií bez rozlíšenia: bulk Kill volá `kill_session` na stratenú tmux session; pravidlá roztrúsené v dvoch súboroch | UXPR-36 (model), 37 (`bulkEligibility`) |
| UX-115 | M | nové | 10 | riadky skryté filtrom ostávajú vybrané; lišta hlási „3 selected“ pri jednom viditeľnom, Kill zabije aj neviditeľné ↔ UX-87/88 | UXPR-36 (`hiddenCount`), 37 (badge `hidden`) |
| UX-116 | M | nové | 10 | hromadný prompt = N × `send_prompt` → pomenuje N sessions rovnako (UX-35) a nehlási výsledok (auto-close 600 ms, bez progresu) | UXPR-38 (D10) |
| UX-117 | L | nové | 10 | bulk Kill zahodí výber **pred** výsledkom, hlási iba zlyhania, žiadny Retry | UXPR-37 (`runBulk`) |
| UX-118 | L | nové | 10 | Escape nič nerobí (zatvára iba picker); mód sa vypína iba pillom | UXPR-36 (dvojkrok) |
| UX-119 | L | nové | 10 | prístupnosť výberu: `<input>` vnorený v `role="button"`, bez `aria-multiselectable`, `.bulk-count` bez `aria-live`, toolbar bez šípok ↔ UX-28 | UXPR-37 (po 33) |
| UX-120 | L | nové | 10 | hub: N toastov za jedno potvrdenie (`E_CONFIRM_REQUIRED` × N); slepota na `mcp.confirm_destructive` ↔ UX-84 | UXPR-37 (mäkko 09) |
| UX-121 | L | nové | 10 | tri výrazy mena v bulk toku (Kill dialóg `tmux_name`, prompt dialóg `friendly ?? tmux`, checkbox `aria-label`) — rodina UX-40 | UXPR-37, 38 (`displayName` z 06) |
| UX-122 | L | nové | 10 | test medzery: `hub_disabled.test.ts` bez bulk prípadu, `BulkPromptDialog` bez testu, Escape/klávesnica/ghost/skryté netestované | UXPR-36–38 |
| UX-123 | M | nové | 10 | žiadne „vybrať všetko“ (projekt, ⌘A, stav) — hromadné akcie drahšie než N klikov na `×` | UXPR-36 (model), 37 (tri-stavový checkbox) |

**Číslovanie:** žiadne dve iterácie nepoužili to isté číslo pre rôzne nálezy (06: 68–76, 07: 77–86,
08: 87–96, 09: 97–111, 10: 112–123). Jedna anomália v **zadaní tejto konsolidácie**: „tri/štyri
`displayName` výrazy UX-40/**106**/121“ — UX-106 je „tri druhy riadkov, tri štruktúry“; nález o
štvrtom `displayName` a kolízii súboru je **UX-111**. Správny trojlístok je UX-40 / 111 / 121.

### Auditové nálezy, ktorým sa v tomto bloku zmenil stav

| ID | Sev | Stav po bloku 2 | Iter. | Čo sa zmenilo | PR |
|---|---|---|---|---|---|
| UX-02 (riadok) | H | potvrdené, rozšírené | 09 | päť glyfov + **prekrytie stavového chipu** hover akciami (UX-101); `.selected` riadok stav nikdy neukáže | UXPR-02, 33 |
| UX-03 (riadok) | M | potvrdené | 09 | `🔗 🔍 ▶ 🤖`, `⚠`, glyfy v `STATUS_LABEL` a `ciStatusLabel` → ikony z 02, reťazce z 32 | UXPR-02, 32 |
| UX-04 / UX-31 | L / M | potvrdené | 08 | kôš je v riadku projektu, nie v chrome; skryť v hub režime — vlastníkom je **UXPR-02** (D8), nie 23 | UXPR-02 |
| UX-07 (riadok) | M | potvrdené | 09 | linka 2 nesie celé tmux meno; skrátený tvar `<repo> · <worktree>`, celé do `title`/detailu | UXPR-33 (riadok), 06 (hlavička, `h2`) |
| UX-08 | H | **opravené** | 08 | „~90 px“ = iba štyri riadky chipov; hlavička **134 px**, chrome so pätičkou **≈ 200 px = 20 %** panelu (27 % na 13"); mieša **päť** druhov ovládačov (F/V/M/A/N); `Needs you (0)` **nesvieti načerveno** (`class:hot` iba pri N > 0) — svieti glyfom `⚠` a textom | UXPR-28, 30 |
| UX-09 | M | potvrdené, rozšírené | 09 | nie 8, ale **11 dát**, z toho 3 prázdne/mŕtve (effort nikdy, elapsed iba fleet-vytvorené, bodka 9/9 zelená); hierarchia je iba veľkosť písma | UXPR-32, 33, 34 |
| UX-10 | M | **opravené** | 10 | „nikde sa neobjaví lišta“ **vyvrátené** — `bulk-bar` existuje (`SidebarFilters.svelte:163-182`), ale iba pri `selectedCount > 0`; „ani hint“ **potvrdené** (UX-112). Checkbox **je** `<input type="checkbox">`; auditový AX klik zlyhal na WKWebView bridgingu + vnorení v `role="button"` (UX-28, 119), nie na chýbajúcom form controle | UXPR-36, 37 |
| UX-11 | L | potvrdené, + | 08 | tlačidlo je 100 % šírky, 21,6 px — celý riadok pätičky pre jedno nastavenie; `cf:theme` mimo `prefs.ts` | UXPR-28 (View menu › Theme) |
| UX-12 | L | potvrdené, rozšírené | 08 | picker zahadzuje poradie „naposledy aktívne“, ktoré `list_projects` vracia; bez hosta, bez vyhľadávania; `⚡` → dialóg so skrytými hostami (UX-96); tri tvary plusu | UXPR-29 |
| UX-13 | H | **neoverené** (bez zmeny) | — | iterácia 07 sa FAB/operátora nedotkla; ostáva pre šošovku 11 s poznámkou z 02 | šošovka 11 |
| UX-19 | M | **opravené** | 06 | *Danger* tlačidlá **nie sú** „viditeľné, hoci LocalOnly“ v zmysle funkčné — sú `disabled` s dôvodom v `title` (dôvod patrí `remove_host` aj pre token-mode a Rotate, UX-72); *Token* riadok vypíše vetu odmietnutia **ako hodnotu** (`HostDetail.svelte:256-258`); chýbajúci usage refresh potvrdený, koreň je UX-69 (H) | UXPR-22, 23 |
| UX-20 | M | potvrdené | 06 | `＋ Add project…` disabled iba s `title`; hub tool neexistuje; routing odložený (UXPR-24) kvôli rozpočtu a `create_remote` | UXPR-24 (odložené); 29 (`title` bez „on the hub“) |
| UX-21 | **M → H** | **opravené, spresnené** | 07 | Safe remove je nedostupný **celý**, vrátane routovanej cesty `safe_kill_session` (UX-77); tool detail padá per klik; `session_activity` ticho nie je živý; `dismiss` má ekvivalent, ale rada mieri na skryté tlačidlo; `purge_project` patrí šošovke 6 — „nefungujú“ platí pre 5 zo 6 | UXPR-26, 27 |
| UX-27 (časť `?`) | L | potvrdené, spresnené | 06 | `?` v hlavičke Hosts je *keyboard legend*, `?` v stĺpci účtov je „usage neznáme“ — dva významy toho istého glyfu na jednej obrazovke ↔ UX-33; šošovka 16 ostáva vlastníkom | UXPR-23 (tooltip `?` v stĺpci) |
| UX-33 (`⚡`) | L | potvrdené | 09 | `⚡ working` v riadku a `⚡` = nová bg session na jednej obrazovke | UXPR-32, 29 (`⚡` pill zaniká) |
| UX-34 (`▶`) | M | potvrdené | 09 | shell badge bez `role="img"` | UXPR-02 |
| UX-35 (broadcast) | H | potvrdené, spresnené | 10 | UI bulk prompt **nejde** cez `broadcast_prompt`, ale N × `send_prompt` → N rovnakých mien; B5 pokrýva iba MCP cestu | UXPR-38 (D10) |
| UX-40 | L | potvrdené, +1 | 09 | **štvrtý** výraz `attention.ts:303-305 displayName(s, friendly)`; kolízia mena súboru → UX-111 | UXPR-06 (D13) |
| UX-45 (+57, 74) | M | potvrdené, špecifikované | 06 | `client_mode` persistovaný pri párovaní + `routed_readonly` v generovanom JSON → **UXPR-21** (otázka C5 „default“ = áno) | UXPR-21 |
| UX-62 ↔ UX-79 | M | potvrdené (vzor) | 07 | potvrdenia nekonzistentné aj pri Repair (bez dialógu, hub `confirm: true`) | UXPR-16, 27 |
| UX-78, UX-84 | M, L | potvrdené pre bulk | 10 | bulk Kill kópia mlčí o worktree; `E_CONFIRM_REQUIRED` × N toastov (UX-120) | UXPR-37 |
| UX-93 | L | potvrdené | 10 | `{#if selectedCount > 0}` blok s vlastným paddingom/borderom → nová výška, nie swap | UXPR-28, 37 |

### Premisy konsolidácie-01, README a zadaní, ktoré iterácie 6–10 vyvrátili

| Premisa | Kde stála | Verdikt | Dôsledok |
|---|---|---|---|
| SEC-5 „`broadcast_prompt` bez rate limitu a potvrdenia“ | plán 2026-09-10 `:74`, 02 B5 | **vyvrátené** (10): `guard.rs:28-32` interval 30 s, `:398-404` `confirm: true`, marker `messaging.rs:101` — B5 je landed | UI broadcast nepoužíva a **nemôže** (cieli filtrom, nie zoznamom id, iba `kind == work`) → D10 |
| „hubov `safe_kill_session` už počíta tie isté fakty“ | zadanie iterácie 07 | **vyvrátené**: iba pošle prompt s nonce; clean check až vo `finalize_safe_kill` po READY a je to *iný* check (iba porcelain) | `inspect_safe_kill` potrebuje vlastný T0 tool (UXPR-26) |
| „`bg off` je default“ | zadanie iterácie 08, screenshot 03 | **vyvrátené**: `showBgAgents` default `true` (`sessions.ts:175`); operátor ho vypol | View menu label „Background agents (n hidden)“ |
| „`bg off` skryje `bg:<uuid>` v Outside fleet“ | zadanie 08 | **vyvrátené**: `buildOutsideFleet` prepínač ignoruje; skrýva iba `kind='bg'` | Outside fleet ostáva nezávislé (Q-II.13) |
| „Hosts view (⌘1)“ | README §1 tabuľka screenshotov, hlavička iterácie 06 | **nepresné**: chord je **⌘I** / Ctrl+Shift+H (`app_views.ts:76,82,92`) | README oprava (§7) |
| verdikt `list_account_usage` „cache is empty; read usage on the hub“; doc „nothing to route to“ | `verdicts.rs:520-526`, `commands/account_usage.rs:5-10` | **vyvrátené** (06): store plní hubov event stream; hub má presne `AccountUsageSnapshot` v `UsageCache` — chýba iba `#[tool]` | T0 tool ≈ 185 B (UXPR-22) |
| „Danger → Hide/Remove sú viditeľné, hoci LocalOnly“ | README UX-19 | **opravené**: disabled s `title` | README §7 |
| „~90 px filter chipov“, „8 metadát“ | README UX-08, UX-09 | **opravené**: 134/200 px; 11 dát | README §7 |
| D4 „63 800 raz; trimy `fleet_health`/`plan_sync` sú rezerva, nie podmienka“ | konsolidácia-01 §2 D4 | **neudržateľné**: 06 rezervy spotrebuje (+835 → 63 430), 07 potrebuje +2 070 (→ 65 500), 24 ďalších +1 300 — tretí a štvrtý zdvih | **D7** (dva stropy) |
| UXPR-06 vytvorí `src/lib/session_view.ts` | konsolidácia-01 §3 riadok 06, iterácia 02 PR-B | **vyvrátené** (UX-111): súbor existuje (`resolveSessionView`, importuje ho `prefs.ts`) | **D13** `session_name.ts` |
| UXPR-19 `{glyph, text}` model, ~60 asserov | konsolidácia-01 §3 riadok 19, iterácia 01 §C | **nahradené** (09): 82 asserov v 10 súboroch; model má `icon/text/age/tone/title` | **D9**; UXPR-19 → 32 + 35 |
| „`src/lib/tokens.ts` = formátovanie ceny/kontextu“ | zadanie iterácie 09 | **vyvrátené**: je to paleta a kontrastný test; formátovanie je v `sessions.ts`, `attention.ts`, `session_status.ts`, `conversation.ts` | `format.ts` (UXPR-34) |
| „UX-28: checkbox nie je form control“ | implicitne audit UX-10/28 | **vyvrátené** (10): je `<input>`; zlyhal AX bridging WKWebView + vnorenie v `role="button"` | UX-119 → UXPR-37; zvyšok šošovka 17 |
| „UXPR-38 nemení rozpočet (D4): `label` je iba na Tauri/hub drôte“ | iterácia 10 §Odhad | **korekcia konsolidácie**: `routed::send_prompt` volá `hub.route("send_prompt", &args)` (`commands/sessions.rs:577`) — celý `SendPromptArgs` ide do hubového MCP toolu, ktorého schéma je `SendPromptParams`; aby nový hub `label: false` **honoroval**, pole musí byť v `SendPromptParams` s `///` (test `every_tool_parameter_is_documented`) → **≈ +120 B na každej ploche** vrátane agentovej (`send_prompt` je `Visibility::All`); starší hub pole ignoruje (serde default) — degradácia prijateľná | D7 tabuľka, D10 |
| „local_only 70 → 65“ a zároveň „`fix/attachments-hub-parity` −4 landed“ | iterácia 07 | **konzistentné** — overené: generovaný JSON má 70 pri `9c8ceabc`, kde `pick_attachments` už je `SameInBoth` (`verdicts.rs:375`); 70 je baseline **po** attachments | trajektória §6 začína na 70 |

### Prekryvy, ktoré treba riešiť ako jeden vzor (doplnok k tabuľke konsolidácie-01)

| Vzor | Nálezy | Kde sa rieši raz |
|---|---|---|
| `instead`/`REASONS` ukazuje na cieľ, ktorý neexistuje alebo odpovedá inak | blok 1: UX-42, 49, 50, 54, 60 · blok 2: **UX-70, 82, 85** | D6 bod 1 (každý routing PR maže svoje vety); 22b `const FLEET_ADMIN_IS_THE_OPERATORS` + CLI mená pre token trio; 26b inspect/discard/purge; `purge_project` text z 06 |
| Jedna funkcia pre zobrazené meno | UX-07, 15, 40 · **UX-111, 121** (+ `BulkPromptDialog:49`, Kill dialóg `:865`, checkbox `aria-label`) | **D13** `session_name.ts` v UXPR-06; konzumenti 27, 33, 37, 38 |
| Kill dialóg mlčí o worktree | **UX-78** (riadok, detail) · **UX-117/UX-78 bulk** (10) | `killDialogCopy(sess \| rows)` v UXPR-06 (D13); 27 (detail), 37 (riadok + bulk v `Sidebar.svelte`) |
| Desktop nevie mód klienta vopred | UX-45, 57 · **UX-74**; stavy `files-forbidden` (16), `applyBlocked` (18), nickname (23), Discard/Ask v dialógu (27), celá bulk lišta (37) | **UXPR-21** (S, ∥ so všetkým) |
| `E_CONFIRM_REQUIRED` až po kliku | **UX-84** · **UX-120** (×N) | 27 „the hub will also ask its operator“ po UXPR-09 (C4 derived kľúč); 37 `groupErrors` → jeden toast; slice 2 FAB spec presunie potvrdenie za hub (šošovka 11) |
| `?` bez dôvodu / dva významy `?` | **UX-68** (stĺpec) · UX-27 (legenda) · `freshnessMark` tvar v UXPR-35 (09) | 23 mení **sémantiku** (`title` zo `status`/`detail`/`source_host`), 35 iba **tvar** (`{age, state, title}`), šošovka 16 glyf legendy |
| Skryté hosty: dve pravidlá | **UX-71** (Hosts view, hubov `local`) · **UX-96** (`NewBgSessionDialog`) | 29 (jeden `usableHost` filter) + **D12** (23) |
| Bulk lišta: kde a čo | **UX-93** (skok) · **UX-112** (bez afordancie) | 28 dáva **slot** riadku 2 (swap), 37 dáva **obsah** (`BulkBar.svelte`) |
| Výber vs. filtre | **UX-115** (skryté vybrané) · UX-87/88 (filtre menia množinu riadkov v 31) | 36 `hiddenCount` nad `visible` množinou — **36 pred 31**, aby 31 iba zmenil, čo je `visible` |
| Jeden glyf dva významy | UX-33 (`⚡`, `☑`, `🏷`) · **UX-90** (`☑` Tasks/Select) · 09 `triangle-alert` stuck vs. usage low | 28 `IconSelect`/`IconTasks`; 29 ruší `⚡` pill; 32 `IconWorking`; **D9** batériové ikony pre usage |
| 24 px podlaha | UX-29, **95** (chrome) · **UX-107** (akcie 20 × 16 px) · iter. 01 `btn--sm` 20 px | **UXPR-01/02 nastavia 24 px** (`--control-h`), 28/29/33 iba používajú `controls.css` triedy |
| Stavové farby hex | UX-30 (01) · **UX-100** (09) | 32: `tone` → `--usage-ok/warn/crit`, `--accent`, `--fg-muted`; test proti `#[0-9a-f]{3,6}` v riadku |
| Bez potvrdenia tam, kde hub má `confirm: true` | UX-62 (git checkout/pull) · **UX-79** (Repair) | 16, 27 |
| Tretia kópia skladania stavu | `SessionRowItem` · `SessionDetails.sub` · `HostDetail.svelte:98-102` | 32 `sessionStatusChip` — jeden resolver |
| Trimy popisov ako rezerva | 06 (spotrebuje `fleet_health` −480, `plan_sync` −530) · 07 („už nie sú“) | **D7**: trimy sa robia raz v UXPR-25 a vstupujú do základu oboch meraní |

## 2. Rozhodnutia naprieč iteráciami (D7+)

### D7 — Rozpočet popisov: dva stropy, nastavené raz v UXPR-25

**Stav problému.** D4 (63 800, jedna zmena v UXPR-09) stál na troch tranžiach (03 +650, 04 +450, 05
+4 900 → 63 603, „197 B pod stropom“) a na dvoch rezervných trimoch. Iterácia 06 ukázala, že +835 B
za usage/nickname sa zmestí **iba** so spotrebou oboch trimov (−1 010 → 63 430); iterácia 07 pridáva
+2 070 B (štyri session tools) a UXPR-24 ďalších +1 300 — to sú tretí a štvrtý zdvih, ktorým D4
chcel predísť. Zároveň 07 ukázala, že test dnes meria **master** plochu (`definition_bytes(&Caller::master())`,
`tests.rs:2325-2400`), hoci plocha, ktorú spec token-efficiency chráni, je **agentova** (per-host
token v session, dnes ≈ 50 300 B), a že všetky parity tools sú desktopové operácie, ktoré agent v
session nepotrebuje.

**Rozhodnutie (jedno, konečné):**

1. **ADR 0003 `Visibility::ClientOnly`** (07, možnosť (a)): os `visibility ∈ {All, ClientOnly}` v
   `TOOL_POLICIES`; `ClientOnly` = **skryté pred per-host tokenom**, viditeľné masterovi a paired
   klientom (telefón, desktop v hub režime, operátorov agent). Vetva v `present::visible_to` **a** v
   gate (`enforce_audience`), aby platil `the_served_tool_list_matches_the_call_gates`. Tretia
   hodnota `MasterOnly` sa **nezavádza** (`Access::Master` už skrýva). Bez `REGEN_HUB_CONTRACT`
   (`Visibility` nie je na drôte); `REGEN_DOCS` áno (riadok *Audience* v referencii).
2. **Dve konštanty namiesto `BUDGET_BYTES`** v `the_served_definition_budget_stays_bounded`:
   - `AGENT_BUDGET_BYTES` — **tvrdý strop** na plochu `host_caller("h", Full)`: **51 000**
     (meranie ≈ 50 300 + ≤ 2 %). Podmienka `ro_bytes < bytes / 2` sa meria proti agentovej ploche
     (host readonly vs. host full).
   - `MASTER_BUDGET_BYTES` — **mäkký strop** na master plochu (operátorov Claude Code s odloženým
     načítaním definícií): **65 500** = 57 603 + 650 (03) + 450 (04, netto) + 4 900 (05 A) + 835
     (06) + 2 070 (07, RN dismiss) − 1 010 (trimy). Zvyšuje sa iba s odsekom v doc-komentári, ktorý
     vymenuje tranže; výnimka je vopred známa iba pre UXPR-24 (+1 300 → 66 800).
   - Výpis štyroch riadkov `master / host full / host readonly / client full` do popisu PR ostáva.
3. **Trimy `fleet_health` (−480) a `plan_sync` (−530)** sa **presúvajú z UXPR-22 do UXPR-25**
   (text ide do `docs/control-api.md`). Dôvod: obidva tools vidí aj agent, takže trim zväčšuje
   headroom pod tvrdým stropom; a konštanty sa majú nastaviť **raz nad základom, ktorý už trimy
   obsahuje** (bez toho by UXPR-22 menil popisy cudzích toolov, čo D4 zakazoval iným PR).
4. **UXPR-25 ide na začiatok lane D, pred UXPR-08/09.** Lane D ešte nebeží (žiadny UXPR nie je
   zlúčený), takže „ak 09 pristál skôr, 25 preberie jeho číslo“ z iterácie 07 nenastane. UXPR-09 už
   **nezdvíha nič** — jeho úloha „`BUDGET_BYTES` → 63 800 raz“ z konsolidácie-01 sa ruší; ostáva mu
   UX-48 (počet príkazov bez čísla v `CLAUDE.md`).
5. **Každý nový parity tool z tejto fronty je `ClientOnly`** (09, 10, 12, 22, 26; aj odložený 24).
   Retroaktívne označenie existujúcich toolov sa **nerobí** (07 Q6 → samostatný S pass po lane D s
   testom `client_only_tools_are_not_named_in_the_control_skill`).
6. **`SendPromptParams.label`** (UXPR-38/04) je výnimka z bodu 5 — nie je to nový tool, ale pole
   existujúceho `Visibility::All` toolu; ≈ +120 B ide na **všetky** plochy (korekcia iterácie 10).

**Čo robí každý hub-Rust PR s konštantami a plochami:**

| PR | `AGENT_BUDGET_BYTES` (tvrdý) | `MASTER_BUDGET_BYTES` (mäkký) | Δ master | Δ agent | Poznámka |
|---|---|---|---|---|---|
| **25** | **zavádza 51 000** | **zavádza 65 500** | −1 010 (trimy) | −1 010 | nahrádza `BUDGET_BYTES`; doc-komentár s tabuľkou tranží 03–07; nové testy audience |
| 08 | nedotýka | nedotýka | −210 (škrt „in the app“ ×4) | −210 | asset tools vidí aj agent |
| 09 | nedotýka | nedotýka (**už nedvíha**) | +650 (2 × `ClientOnly`) | 0 | `Access::Trusted` variant (D3 (a)) |
| 10 | nedotýka | nedotýka | +4 900 (10 × `ClientOnly`) | 0 | variant A (D-1 „default“) |
| 12 | nedotýka | nedotýka | +660 (2 × `ClientOnly`) | 0 | (04 uvádzalo +450 netto vrátane −210 z 08) |
| 22a | nedotýka | nedotýka | +835 (3 × `ClientOnly`) −180 (`discover_hosts` → Master) | −180 | `discover_hosts` mizne z klientskej **aj** agentovej plochy |
| 26a | nedotýka | nedotýka | +2 070 (4 × `ClientOnly`; +1 790 pri RE dismiss) | 0 | D11 rozhoduje RN |
| 38 (lane B) | nedotýka | nedotýka | ≈ +120 (`label` pole) | ≈ +120 | `Visibility::All` tool; korekcia iterácie 10 |
| [24] odložený | nedotýka | **jediná výnimka: 66 800** s odsekom | +1 300 (2 × `ClientOnly`) | 0 | po šošovke 20 |
| **Súčet po 26** | 51 000 | 65 500 | ≈ 65 500 (57 603 → +13,7 %) | ≈ 49 000 (50 300 → −2,6 %) | agent klesá, master rastie iba o desktopové parity |

**README §5 (d)** sa mení na: „(d) rozpočet MCP popisov má **dva stropy** — `AGENT_BUDGET_BYTES`
tvrdý 51 000 na plochu per-host tokenu, `MASTER_BUDGET_BYTES` mäkký 65 500 — nastavené **raz v
UXPR-25** (ADR 0003 `Visibility::ClientOnly`); parity tools sú `ClientOnly`, ďalšie PR konštanty
nemenia (jediná známa výnimka UXPR-24 → 66 800)“.

### D8 — Jedno poradie zapisovateľov pre sidebar (lane F) a odpútanie od lane D

Tri iterácie dali tri poradia pre tri súbory. Kolízie, ktoré každá ohlásila: 08 — UXPR-02 musí ísť
pred 28 (oba prepisujú `SidebarFilters.svelte`), 23 a 28/29 zdieľajú `Sidebar.svelte`; 09 —
`SessionRowItem.svelte` 02 → 32 → 33 → 06, `HostDetail`/`HostsList` po 23, 35 po 23; 10 —
`Sidebar.svelte` 23 → 02 → 28 → 36 → 29 → 30 → 31 → 27 → 37 → 38, 36 po 33 na `SessionRowItem`,
37 po 27 (`killDialogCopy`). Problém poradia z iterácie 10: **23 a 27 sú lane E a čakajú na lane D**
(23 po 22b, 27 po 26b — koniec kritickej cesty), ale stoja na začiatku a v strede lane F. Sidebar by
tak čakal na hub Rust, s ktorým nesúvisí. Zmeny vlastníctva, ktoré to riešia (každá je ≤ 15 riadkov):

1. **Kôš `purge_project` skrytý v hub režime** (UX-04/31, 06 tabuľka `:94`, 07 `:113`) → **UXPR-02**
   (už mení `.purge-btn` a `Sidebar.svelte`); UXPR-23 sa `Sidebar.svelte` **nedotýka**.
2. **`＋ Add project…` `title` bez „Do it on the hub“** (06 položka 7) → **UXPR-29** (položka ide do
   šípky split tlačidla, `title = REASONS.add_project` — 08 to už tak píše).
3. **`killDialogCopy(rows)`** (UX-78) sa definuje v **UXPR-06** (`session_name.ts`, D13 — 06 je lane
   B, nezávislé od D); **UXPR-37** ho použije pre riadkový aj bulk Kill dialóg v `Sidebar.svelte:841-866`
   (oba bloky aj tak nahrádza `runBulk` + `BulkConfirmDialog`); **UXPR-27** ho použije iba v
   `SessionDetails.svelte` a `Sidebar.svelte` sa **nedotýka**.
4. **Ghost nevoliteľný** (UX-114): UXPR-36 to rieši **v modeli** (`toggle`/`selectAll` ignorujú
   `status === 'ghost'`, `pruneTo` ich vyhodí) — prop `selectable` v `SessionRowItem.svelte:168` nie
   je nutný; skrytie checkboxu ide v UXPR-37, ktorý checkbox aj tak vyťahuje z `role="button"`.
   UXPR-36 sa `SessionRowItem.svelte` **nedotýka**.
5. `HostDetail.svelte`/`HostsList.svelte`: UXPR-32 mení `:98-102` a počty, UXPR-23 mení `:238-300` a
   `:107-170` — iné regióny; keďže 23 je neskoré, poradie je **32 → 23** (opak návrhu 09); 23 pri
   rebase prevezme `sessionStatusChip`. UXPR-35 ostáva **po 23** (23 mení sémantiku `freshnessMark`,
   35 tvar).

**Výsledné poradie (záväzné; „→“ = sekvenčne, jeden zapisovateľ na súbor):**

```
Sidebar.svelte:          02 → 28 → 36 → 29 → 30 → 31 → 37 → 38
SidebarFilters.svelte:   02 → 28 → 36(+4 r.) → 30 → 37(slot → BulkBar)
SessionRowItem.svelte:   02 → 32 → 33 → 06 → 37(checkbox von z role=button)
SessionDetails.svelte:   03 → 32 → 33 → 06 → 27
HostDetail / HostsList:  32 → 23 → 35
hosts_view.ts, usage_glance.ts, UsageBlock, HostsView:  23 → 35
ConversationPanel.svelte / TerminalView.svelte:  (03) → 32/06 → 27
prompt.rs:               04 → 38
```

Lane F (`28 → 36 → 29 → 30 → 31 → 37 → 38`) tak závisí iba od lane A (01, 02; 37 aj od 33) a lane B
(06 pre 37/38, 04 pre 38 Rust); od lane D má iba **mäkké** väzby (37: 26 riziko na hube, 21 readonly
vopred, 09 `confirm_destructive` vopred — bez nich má horší text, nie horšie správanie).

**Čo môže bežať ako paralelné worktrees súbežne:** lane A (01 → 02 → 32 → 33 → 06), lane B (04 →
05), lane C (07, 21, 25 — tri nezávislé malé PR hned), lane D (od 25), lane F (od 28 po 02),
UXPR-34 (nové `format.ts` + presuny, koliduje s ničím — kedykoľvek pred 33), UXPR-14/15 (po 09, 07)
a 16/17 (po 11, 07), 18 (po 13, 07). **Nie súbežne:** dva PR z toho istého riadku vyššie; 28 a 33 sú
mäkko viazané (label „Details line“ → „Last prompt line“, D14) — kto landne druhý, upraví label.

### D9 — Stavový model `StatusChip` nahrádza `{glyph, text}` a UXPR-19

Potvrdené z iterácie 09, záväzné pre celý blok:

- `src/lib/status_chip.ts`: `StatusChip { icon: IconName | null; text: string; age?: string;
  tone: Tone; title: string }`, `Tone = ok | warn | crit | info | muted` (**päť** hodnôt — `info` pre
  `completed`/`frozen`, aby dokončený agent nesvietil ako pracujúci; 09 Q7 rozhodnuté áno). Jeden
  resolver `sessionStatusChip(s, now)` (stuck › inactive › lifecycle ≠ running › `claude_status` ›
  null) pre riadok, detail aj `HostDetail`. `StatusChipView.svelte` renderuje `role="img"
  aria-label={title}`, ikona `aria-hidden`, tón cez CSS triedu → tokeny (`ok → --usage-ok`,
  `warn → --usage-warn`, `crit → --usage-crit`, `info → --accent`, `muted → --fg-muted`), pozadie
  `color-mix(… currentColor 12%)`, **žiadny inline `style` s hex** (UX-100).
- **Ikony** (D1 injektívnosť): `IconWorking = zap`, `IconBlocked = pause`, `IconDone = check`
  (zdieľajú `completed` a CI passing — jeden význam), `IconFailed = x`, `IconStopped = square`,
  `IconStuck = triangle-alert`, `IconPending = loader-circle`, `IconGhost = ghost`, `IconFrozen =
  snowflake`, `IconOrphan = unlink`, `IconDot`; usage: **`IconUsageLow = battery-low`,
  `IconUsageCaution = battery-medium`, `IconUsageLimit = octagon-alert`** (09 Q6 rozhodnuté áno —
  `triangle-alert` ostáva iba pre stuck). `PR↗`, `⇄`, klávesové symboly ostávajú (konsolidácia-01 §5).
- **UXPR-19 je nahradené** dvojicou **UXPR-32** (session/CI/notifikácie/lifecycle, ≈ 140) + **UXPR-35**
  (usage značky, ≈ 160); delí sa podľa *renderujúcich* súborov, lebo 82 asserov by v jednom PR
  prekročilo 300 riadkov. Testy asertujú `text`/`tone`/`data-tone`/`aria-label`, **nikdy glyf**; lint
  assert `expect(chip.text).toMatch(/^[\w\s:%~.-]+$/u)` a `grep -c '[⚡⏸✓✗■◷▲△🔑○]' src/lib/*.ts = 0`
  (mimo `TransferChip`, `BranchList`, kláves) v `icons.test.ts`.
- **82 glyfových asserov, ktoré sa prepisujú** (podľa 09 §Testy): `account_usage.test.ts` 25 (35),
  `usage_glance.test.ts` 16 (35), `UsageBlock.test.ts` 16 (35), `conversation.test.ts:644-651` 6 (32),
  `hosts_view.test.ts:86-101,169` 5 (32 `sessionCounts` / 35 `freshnessMark`),
  `HostsView.test.ts:149,158,161,348` 4 (35), `NewSessionDialog.test.ts:1121-1162` 4 (35),
  `TransferChip.test.ts:81-139` 4 (**bez zmeny**), `HostsList.test.ts` 1 (35), `ToolLine.test.ts:69`
  1 (UXPR-03), `Sidebar.test.ts:1020,1236` + `SessionDetails.test.ts:247,311` 4 substring (32 —
  nezlomia sa, doplniť `data-tone`), `attention.test.ts:83-90,209-210` 3 (32). Spolu 82, z toho 78
  sa mení (4 v `TransferChip` ostávajú).
- Kontrast: `tokens.test.ts` dostane páry `usage-ok/warn/crit` a `fg-muted` vs `bg` (riadok sedí na
  `--bg`, nie `--bg-pane`); nový test číta `SessionRowItem.svelte` + `status_chip.ts` +
  `StatusChipView.svelte` a zlyhá pri `#[0-9a-f]{3,6}`.

### D10 — Hromadný prompt: N × `send_prompt` s `label: Option<bool>`, pole zavádza UXPR-04

Rozhodnutie: **nie `broadcast_prompt`** (cieli filtrom host/projekt/status a iba `kind == work`,
nemá Tauri príkaz ani routing — UI by stratilo `review`/`bg` riadky a presný výber bez výhody);
ostať pri N × `send_prompt` z `BulkPromptDialog` a pridať **wire pole**:

- `SendPromptArgs.label: Option<bool>` s **`#[serde(default)]`** (`service/sessions/prompt.rs:43-49`),
  `send_prompt` → `send_prompt_inner(…, label.unwrap_or(true))` — presne parameter, ktorý UXPR-04
  P2 bod 7 zavádza pre systémové prompty. **Pole zavádza UXPR-04** (lane B, už edituje `prompt.rs`
  a `hub_contract.golden.json`); UXPR-38 ho iba konzumuje (`sendPrompt(host, name, prompt,
  { label: false })`) a ide **po 04** (10 to už tak radí). Ak by 04 z akéhokoľvek dôvodu meškal za
  38, 38 pole zavedie sám a 04 rebasne — druhá voľba, nie default.
- **Hubová strana:** keďže `routed::send_prompt` posiela celý `SendPromptArgs` do hubového toolu
  (`commands/sessions.rs:577`), `SendPromptParams` (`params.rs:236-259`) dostane to isté pole
  `/// Set false to leave the session's friendly name untouched (bulk sends). Default true.`
  `#[serde(default)]` — inak nový hub pole ticho zahodí a bulk cez hub pomenuje ďalej. Toto je
  ≈ +120 B na **všetkých** plochách (D7 tabuľka) a **jediná zmena MCP schémy v celej lane B/F**.
- **Kontrakt:** `#[serde(default)]` je povinné (bez neho výpadok proti staršiemu hubu — memory
  „hub contract golden“); starší hub pole ignoruje → bulk pomenuje ako dnes (prijateľná degradácia,
  zapísať do `hub.md` *What is different*). `REGEN_HUB_CONTRACT` (golden sa mení), nedefaultný riadok
  `label: Some(false)` v `tests_routing.rs` (memory „hub-routed command args“), `REGEN_DOCS`
  (referencia vypisuje všetky frontendové príkazy aj MCP params). MCP `broadcast_prompt` sa nemení;
  B5 („broadcast nikdy nepomenúva“) ostáva pre MCP cestu.
- UX-116 zvyšok (progres `k / N`, toast, `Close` namiesto `setTimeout(600)`, zlyhané ostanú vybrané,
  `displayName`) je čisto TS v UXPR-38.

### D11 — Hub tools pre sessions a hosty: tiery, jedna výnimka z `confirm: false`

Rozhodnuté raz pre UXPR-22 a 26 (otázky 06 Q1/Q4/Q6 a 07 Q1/Q2/Q4/Q5 sa tým uzatvárajú):

| Tool / zmena | Tier | Policy | Rozhodnutie |
|---|---|---|---|
| `list_account_usage` | T0 | Client, ro, Quick, `ClientOnly` | RN (UXPR-22a) — priorita bloku 06 |
| `refresh_account_usage` | **T0** | Client, **`readonly: true`**, Lifecycle, `ClientOnly` | precedens `probe_host` („re-reads external state“); floor 5 min/účet + `MAX_CONCURRENT_FETCHES = 2` sú mantinel; `readonly` telefón má vidieť čerstvé číslo (06 Q1 → T0) |
| `set_account_nickname` | T1 | Client, mut, Quick, `ClientOnly` | RN — kozmetika v registri hubu, vratná |
| `tunnel_status` → `fleet_health` | — | RE, 0 B | `routed::` = `fleet_health` + `list_hosts` → `map_tunnel_states` |
| `discover_hosts` | **T3** | **`Access::Master`**, ro | UX-76: tool vracia operátorove SSH aliasy; jediný konzument `add_host` je Master (06 Q4 → áno, 1 riadok v 22a; `hub.md` *Clients* doplniť) |
| `inspect_safe_kill` | **T0** | Client, ro, Quick, `ClientOnly` | RN; obsah (cesty, vetva, ahead) je menej než `capture_session` (T0) ukazuje z pane (07 Q4 → T0) |
| `discard_kill_session` | T1 | Client, mut, Lifecycle, **`confirm: true`**, `ClientOnly` | RN; **jediná výnimka z D3 „`confirm: false` na hube“** — pravidlo do D3: *skladaný tool nesmie obísť `confirm` bránu svojich súčastí* (`kill_session` a `delete_worktree` sú `confirm: true`); `refuse_if_operator` doplniť (07 Q1 → áno) |
| `session_tool_detail` | T0 | Client, ro, Lifecycle, `ClientOnly` | RN (ID-adresované čítanie; rozšírenie `session_conversation` odmietnuté) |
| `session_activity` → `capture_session` | — | RE, 0 B | `route_text` + lokálny `probe_from_tail`; sentinel prázdneho pane → all-`None` |
| `dismiss_agent_session` | T1 | Client, mut, Quick, `ClientOnly` | **RN** (+280 B master, 0 agent), nie RE cez `kill_session` — RE by na working agentovi hub zastavil, kde standalone odmietne; rovnaká sémantická medzera, pre ktorú `repair_session` ostal `RoutedUnless` (07 Q2 → RN) |
| `repair_session` | — | `RoutedUnless` **bez zmeny** | UI dostane `ConfirmDialog` + hub dodatok (UXPR-27) |
| `purge_project` | T3 | **žiadny tool** | kôš v hub režime **skrytý** (UXPR-02, D8); Master tool `confirm: true` ide do backend backlogu, až keď ho niekto z hubu potrebuje (06 Q6, 07 Q5 → áno/nie) |
| `add_host` / `remove_host` / `hide_host` / `provision_hosts` | T3 | existujú, Master | UI **disabled** s krátkym `title` „Hub operator only (master token)“; veta raz v `HubScopeNote what='hosts'`; `instead` = jedna `const FLEET_ADMIN_IS_THE_OPERATORS` |
| `list_host_tokens` / `set_host_token_mode` / `rotate_host_token` | — | refused | UI **skryť** (Token riadok) v hub režime; `instead` menuje `fleet-hub host-token-mode`, `agent-token --rotate`, `provision_hosts rotate=true` |
| `add_project` / `list_github_repos` | T1 / T0 | odložené **UXPR-24** | po šošovke 20; `create_remote` **vypustiť** zo hubovej schémy (analógia `force`, C3) — otázka o načasovaní ostáva (Q-II.1) |

### D12 — Skryté hosty a hubov `local` riadok

(06 Q3 → (a) + zloženie v oboch režimoch.) `groupHostsByAccount(hosts, accounts, { hideHidden })`:
skryté hosty sa zložia do riadku `N hidden ▸` na konci zoznamu **v oboch režimoch**; summary počíta
iba nezakryté (`5 · 5 online`); v **remote** režime sa `local` s `hidden && !reachable` **nelístuje
vôbec** — je to hubov stroj (`hub.local_host=false`, `serve.rs:123-136`), nie klientov host, a
nesmie pripínať účet so `?`. `NewBgSessionDialog` použije ten istý `usableHost` filter ako pills a
`NewSessionDialog` (UX-96, UXPR-29). UXPR-23 (Hosts view) + UXPR-29 (dialóg).

### D13 — Jeden modul mena: `src/lib/session_name.ts` (nie `session_view.ts`)

UX-111 ruší premisu UXPR-06 „`session_view.ts` (nový)“ — súbor existuje a rieši prepínanie
Conversation/Terminal (`resolveSessionView`, import v `prefs.ts:10`). UXPR-06 preto zakladá
**`src/lib/session_name.ts`** s: `displayName(sess, friendly?)` (nahrádza **štyri** výrazy: tri z
02 + `attention.ts:303-305`), `secondaryName` (krátky tvar `<repo> · <worktree>` pre linku 2,
UXPR-33 konzumuje), `killDialogCopy(rows: SessionRow[])` (UX-78; jednotné aj množné číslo, veta
„The worktree and any uncommitted or unpushed work stay on `<host>`; use Safe remove to check
first.“). Konzumenti: 27 (detail Kill + *Safe remove instead…*), 33 (riadok), 37 (riadkový + bulk
Kill dialóg, checkbox `aria-label`, `BulkConfirmDialog`), 38 (`BulkPromptDialog`), `agent_context.ts`
(UX-15). Testy: `session_name.test.ts` (nový) namiesto rozšírenia `session_view.test.ts`.

### D14 — Riadok a detail zdieľajú sémantiku prepínača a prahy

- **`rows.details`** (09 Q1 → áno): `off` = linky 1 + 2 (host · worktree · signály, ≈ 36 px), `on` =
  + linka 3 (posledný prompt, ≈ 48 px). Dnešné `off` = iba linka 1 (22 px) zaniká; jednolinkový
  „compact“ režim iba ak si ho niekto vypýta. View menu (UXPR-28) label **„Last prompt line“**
  (nie „Details line“); testid `toggle-row-details` ostáva; `Sidebar.test.ts:1265,1292,1306` sa
  prepíšu v UXPR-33. Kľúč `rows.details` sa **nepremenúva** (žiadna migrácia prefs).
- **Prahy kontextu 80/95** namiesto 70/90 (09 Q2 → áno) — jedna konštanta
  `CONTEXT_WARN_PCT/CRIT_PCT` v `format.ts`/`attention.ts`, Conversation hlavička
  (`conversation.ts:925`) sa mení s ňou; tooltip `crit` „Context 97 % — send /compact or start a new
  session“; `context_stale` → `~97 %`. UXPR-34.
- Účinok na počítadlá: `Needs you` a `TRIAGE_BUCKETS` sa **nemenia** (kontext ≥ 95 % ako bucket je
  Q-II.14).

## 3. Fronta PR — zlúčená `UXPR-01…UXPR-38`

Poradie = závislosti. Veľkosť je odhad diffu **bez testov a generovaných súborov** (testy v
zátvorke). „∥“ = paralelný worktree; „→“ = sekvenčne po. Riadky 01–18 sú z konsolidácie-01;
**tučne** sú zmeny oproti nej. Súbory sú hlavné, nie úplné.

| UXPR | Pôvod | Názov | Súbory | Veľkosť | Regen | Závisí od | Paralelnosť |
|---|---|---|---|---|---|---|---|
| **01** | 01 PR-1 commit 1 | Ikony — základ: `@lucide/svelte`, tokeny, `icons.ts`, test injektívnosti; **ikonové tlačidlá 24 px (`--control-h`), nie `btn--sm` 20 px (UX-107)**; **+ exporty pre 32 pripravené menami (D9)** | `package.json`, `pnpm-lock.yaml`, `src/app.css`, `src/lib/controls.css`, `src/lib/icons.ts` (nový), `icons.test.ts` | S ~120 (+100) | — | — | ∥ (lane A) |
| **02** | 01 PR-1 commit 2, položky 6–8 | Ikony — riadok session, filtre, sidebar; zrušiť `.icon-btn` kópie; **kôš `purge_project` v hub režime skrytý `{#if ownsTheFleet}` (UX-04/31, D8 bod 1)**; akcie 24 px | `SessionRowItem.svelte`, `SidebarFilters.svelte`, `Sidebar.svelte`, `Sidebar.test.ts`, `hub_disabled.test.ts` (kôš) | M ~150 | — | 01 | ∥ s 03; **prvý zapisovateľ všetkých troch sidebar súborov (D8)** |
| **03** | 01 PR-1b, položky 9–13 | Ikony — detail, Files, terminál, App, 15 jednoriadkových náhrad; **Kill `circle-x` (A1), Safe remove `shield-check` (A3)** | `SessionDetails`, `FilesPanel`, `TerminalView`, `App.svelte`, `CommitGraph`, `TransferSheet`, … | S ~80 | — | 01 | ∥ s 02; **pred** 06, 17, 27, 32 (spoločné súbory) |
| **04** | 02 PR-A, položky 1–8, 10–13 | Pomenovanie — Rust: migrácia 040, proveniencia, filter P2, systémové prompty, `external` default, backfill, MCP popisy; **`SendPromptArgs.label: Option<bool>` `#[serde(default)]` + to isté pole v `SendPromptParams` (D10)**; nedefaultný Case `label: Some(false)` | `migrations/040_*.sql`, `store/{schema,rows,sessions,mod}.rs`, `service/sessions/{prompt,lifecycle,reconcile}.rs`, `service/{bg_sessions,safe_kill,messages}.rs`, `sessions/review.rs`, `mcp/tools/{messaging,session_ops,lifecycle,params}.rs`, `src-tauri/src/lib.rs`, `tests_routing.rs`, `hub_contract.golden.json` | M ~290 (+330) | `REGEN_DOCS`, `REGEN_HUB_CONTRACT` | — | ∥ (lane B); **najbližšie k limitu — ak > 300, `label` pole odštepiť do 38** |
| **05** | 02 PR-A, položka 9 | Pomenovanie — hook `UserPromptSubmit` pomenúva, `/clear` resetuje `prompt` label | `service/hooks.rs` | S ~40 (+60) | — | 04 | → 04 |
| **06** | 02 PR-B | Pomenovanie — TS: **`session_name.ts` (nie `session_view.ts` — UX-111, D13)**: `displayName` (4 výrazy), `secondaryName`, **`killDialogCopy` (UX-78)**; tlmené auto meno, otočená hierarchia detailu (`h2`), prefix agenta, badge `outside`, validácia názvu worktree (B7) | `sessions.ts`, `session_name.ts` (nový), `SessionRowItem`, `SessionDetails`, `TerminalView`, `HostDetail`, `quick_switcher.ts`, `agent_context.ts`, `attention.ts:303`, `NewSessionDialog` (B7) | S/M ~140 (+90) | — | 04; **33** (`SessionRowItem`, `SessionDetails` — D8); mäkko 01 | → 03, 33; **pred 27, 37, 38** (konzumenti helperov) |
| **07** | nový (dedupe 03/04/05) | `HubScopeNote.svelte` + `hub_inline_state.ts` + testy; **`what` union rozšíriť o `'hosts'` (06)** | `src/lib/HubScopeNote.svelte`, `src/lib/hub_inline_state.ts`, testy | S ~80 (+40) | — | — | ∥ (lane C) |
| **08** | 04 PR-4c | Hub má katalóg: `fleet-hub catalog set/show`, load pri boote, škrt „in the app“ ×4, docs, Dockerfile | `crates/fleet-hub/src/{main,serve}.rs`, `mcp/tools/assets.rs` (popisy), `Dockerfile`, `docs/hub.md` | S/M ~150 (+60) | `REGEN_DOCS` | **25** (spoločný `tests.rs` meraním) | ∥ s 09 (kolízia iba v generovanom `control-api-reference.md` → re-regen) |
| **09** | 03 PR-3a | Settings parita — `get_fleet_settings` (T0), `set_fleet_setting` (T2, `Access::Trusted`), verdikty, remote, routing Case, min. TS; **oba tools `ClientOnly`; rozpočet sa NEDVÍHA (D7)**; UX-48 raz; **derived `mcp.confirm_destructive` (C4)** | `mcp/tools/{params,fleet}.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `backend/{verdicts,remote,tests_routing}.rs`, `commands/sessions.rs`, `local_only.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `SettingsDialog.svelte` (min.), `SettingsDialog.hub.test.ts`, `docs/hub.md`, `docs/control-api.md`, `CLAUDE.md` | M ~230 (+140) | `REGEN_DOCS`, `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` | **25**; 07 nie je nutné | lane D po 25 |
| **10** | 05 PR-5a1 | Git zápisy — fleet-core: `Serialize + JsonSchema + ///` na 7 args structoch, `run_shell_bounded(10 s, 90 s)` pre fetch/pull/push (UX-63, D-8), 10 toolov (T1 + `repo_push` T2), **všetky `ClientOnly`**, `force` zo schémy vypustené (C3), policy, testy mantinelov | `service/{repo_mutate,repo}.rs`, `mcp/tools/repo.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `docs/control-api.md` | M ~260 (+150) | `REGEN_DOCS` | 09 (`guard.rs`, `tests.rs`), 08 | → 09 |
| **11** | 05 PR-5a2 | Git zápisy — routing: 10× `Routed`, zmazať `NO_GIT_WRITE_TOOL`, remote ×10, `mod routed` ×10, 10 Case, `REASONS.repo_write` von, `FilesPanel` min. (`writeRefused`) | `backend/{verdicts,remote,tests_routing}.rs`, `commands/mutate.rs`, `local_only.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `FilesPanel.svelte` (min.), `hub_disabled.test.ts`, `docs/hub.md` | M ~240 (+180) | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` | 10 | → 10 |
| **12** | 04 PR-4a1 | Assets parita — fleet-core: `catalog_config`, `catalog_get_asset` (T0, **`ClientOnly`**), `AssetListing.last_sync`, policy, readonly test | `mcp/tools/{params,assets}.rs`, `service/catalog/mod.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `docs/control-api.md` | S ~110 (+40) | `REGEN_DOCS` | 08, 10 | → 10 |
| **13** | 04 PR-4a2 | Assets parita — routing 7×, texty `instead` (UX-50, 54), `CATALOG_IS_A_CHECKOUT` prepis, `PlanArgs: Serialize`, kontrakt, `REASONS.catalog_config` von, `AssetsPanel` min. | `backend/{verdicts,remote,tests_routing,tests_contract}.rs`, `commands/assets.rs`, `service/catalog/sync/mod.rs`, `local_only.golden.json`, `hub_contract.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `AssetsPanel.svelte` (min.), `hub_disabled.test.ts`, `docs/hub.md` | M ~230 (+90, +120 samples) | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT` | 12, 11 | → 11, 12 |
| **14** | 03 PR-3b1 | Settings IA — `SettingsNav` (7 skupín, ľavá nav, badge `fleet`/`this app`), grid, panely, `settings.tab` v prefs, deep-link z pätičky | `SettingsNav.svelte` (nový), `SettingsDialog.svelte`, `settings_dialog.css`, `app_views.ts`, `App.svelte`, `SettingsDialog.test.ts`, `App.hub.test.ts` | M ~250 (+150) | — | 09, 07 | ∥ s 16, 18 (lane E) |
| **15** | 03 PR-3b2 | Settings — `HubStatusCard`, Control API read-only karta, „Copy on select“ do Terminálu, Replay disabled v hub režime, `.section-header` všade, skrátená próza | `HubStatusCard.svelte` (nový), `McpSettings.svelte`, `SettingsDialog.svelte`, `SettingsDialog.hub.test.ts` | S/M ~180 (+60) | — | 14 | → 14 |
| **16** | 05 PR-5b1 | Files — potvrdenia (checkout vetvy, Pull s `behind`, Push `danger`), stavy `files-hub-unsupported` / `files-forbidden` / `files-push-untrusted`, chyby `RemoteToolbar` ako riadok; `FilesPanel.test.ts` + `.hub.test.ts` (UX-61) | `FilesPanel.svelte`, `FileList.svelte`, `RemoteToolbar.svelte`, testy | M ~180 (+150) | — | 11, 07; **mäkko 21** | ∥ s 14, 18 |
| **17** | 05 PR-5b2 | Files — `Skeleton.svelte` (150 ms, reduced-motion), empty CTA podľa módu, „Stage files to commit“, agent-busy hint (UX-64), „No remotes“, „No commits yet“ | `Skeleton.svelte` (nový), `FileList`, `FileViewer`, `BranchList`, `CommitGraph`, `FilesPanel`, testy | S/M ~150 (+150) | — | 16; mäkko 01 | → 16, → 03 |
| **18** | 04 PR-4b | Assets panel v hub režime — 4 stavy, toolbar podľa matice, `AssetDetail.readonly`, `SyncPlanDialog.applyBlocked`, `last_sync`, zmazať `loadInventory` (UX-53, E2) | `AssetsPanel.svelte`, `AssetDetail.svelte`, `AssetList.svelte`, `SyncPlanDialog.svelte`, `SecretsPanel.svelte`, `App.svelte`, `assets.ts`, testy | M ~220 (+180) | — | 13, 07; **mäkko 21** | ∥ s 14–17 |
| ~~**19**~~ | 01 PR-2 | ~~Ikony — stavové glyfy v `.ts` → `{glyph, text}`~~ **NAHRADENÉ → UXPR-32 + UXPR-35 (D9)** | — | — | — | — | — |
| ~~**20**~~ | 01 PR-3 | ~~Ikony — `fileicons.ts` (45 emoji → Lucide)~~ **ZRUŠENÉ** — otázka A2 „default“ = emoji v strome súborov ostávajú ako vedomá výnimka s allowlistom v `icons.test.ts` (allowlist ide do UXPR-01) | — | — | — | — | — |
| **21** | 06 § `client_mode` | `client_mode` persistovaný pri párovaní (`hub.client_mode`), `status()` ho číta, `disconnect` maže; `verdict_gen` pridá `routed_readonly` do generovaného JSON; `clientIsReadonly()` + vetva v `hubActionBlocked` **pred** klikom (UX-45/57/74) | `backend/mod.rs`, `commands/hub.rs`, `backend/tests_pairing.rs`, `backend/verdict_gen.rs`, `hub_verdicts.generated.json` (gen), `hub.ts`, `hub_verdicts.test.ts`, `hub_disabled.test.ts`, `App.hub.test.ts` | S ~90 (+80) | `REGEN_HUB_VERDICTS` | — | ∥ so všetkým (lane C); jediný spoločný súbor s lane D je generovaný JSON (idempotentný regen) |
| **22** | 06 PR plán | Hosts/usage parita — **22a** fleet-core: `RefreshAccountUsageParams`, `SetAccountNicknameArgs` derives, `Deserialize` na `AccountUsageSnapshot`/`Window`/`UsageOutcomeKind`, `usage_cache` do `FleetTools`, 3 tools (T0/T0/T1, `ClientOnly`), `discover_hosts → Master` (UX-76), reference · **22b** routing: 4× `Routed` (`tunnel_status → fleet_health`), `const FLEET_ADMIN_IS_THE_OPERATORS`, 8 opravených `instead` (UX-70), remote ×4, `mod routed` ×4, 4 Case, `FleetResync` + `list_account_usage` (UX-69), kontrakt `sample_usage_snapshot`, goldeny, min. TS (`REASONS` −4, `FLEET_ADMIN` hodnota, gate `ownsTheFleet` → bez/`hubActionBlocked`), docs | `mcp/tools/{params,fleet,mod}.rs`, `service/{hosts,account_usage}.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `crates/fleet-hub/src/serve.rs`, `src-tauri/src/lib.rs`, `backend/{verdicts,remote,tests_routing,tests_contract,events}.rs`, `commands/{account_usage,hosts,onboarding}.rs`, goldeny, `hub.ts`, `hub_verdicts.test.ts`, `App.svelte:236-244`, `HostsView.svelte` (gate), `HostDetail.svelte:84-85`, `NewSessionDialog.svelte:419-423`, `App.hub.test.ts`, `HostsView.test.ts`, `docs/{hub,control-api}.md` | M ~280 (+200) → **odštep 22a ~130 / 22b ~150** | 22a `REGEN_DOCS`; 22b `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT` | 13 (`guard.rs`, `tests.rs`, `verdicts.rs`, `remote.rs`, goldeny); **trimy `fleet_health`/`plan_sync` už v 25 (D7)** | → 13 (lane D) |
| **23** | 06 PR plán | Hosts view v hub režime — `HubScopeNote what='hosts'` (jediný odsek), kadencia `usage from <hub> · every 5 min` (UX-73), summary bez skrytých + `N hidden ▸`, `local` hidden nelístovaný v remote (**D12**), `freshnessMark` s dôvodom + `data-status` (**UX-68 sémantika**), `usageReason` helper, Token riadok/Rotate `{#if ownsTheFleet}`, fakt `provisioned`, Danger `title` krátky, `UsageBlock` refresh podľa `hubActionBlocked` + footer `· hub`, `FooterState 'unsupported'` (Q-II.4), `HostsView.hub.test.ts` (nový). **Bez `Sidebar.svelte` (D8: kôš → 02, Add project → 29)** | `HostsView.svelte`, `hosts_view.ts`, `HostsList.svelte`, `HostDetail.svelte`, `UsageBlock.svelte`, `usage_glance.ts`, `App.svelte:836-844`, testy | M ~210 (+180) | — | 07, 22b; **32** (`HostDetail`/`HostsList` — D8 bod 5); mäkko 21 | ∥ s 14–18, 27 (lane E); **pred 35** |
| **24** | 06 PR plán | `add_project` (T1, `create_remote` vypustené) + `list_github_repos` (T0) — `AddProjectArgs`/`AddProjectSource` derives (tagged enum), tools `ClientOnly`, verdikty, remote, routed, 2 Case, kontrakt, `REASONS.add_project` von, `AddProjectDialog` `create_remote` `{#if ownsTheFleet}`, Cancel disabled v remote; **`MASTER_BUDGET_BYTES` → 66 800 s odsekom (jediná výnimka D7)** | `service/add_project.rs`, `mcp/tools/{params,fleet}.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `backend/{verdicts,remote,tests_routing,tests_contract}.rs`, `commands/projects.rs`, goldeny, `hub.ts`, `hub_verdicts.test.ts`, `AddProjectDialog.svelte`, `ProjectPicker.svelte` (po 29), `docs/hub.md` | M ~230 (+150) | `REGEN_DOCS`, `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT` | 26b (koniec lane D), 29; **šošovka 20** | **odložené** (Q-II.1) |
| **25** | 07 § ADR | **ADR 0003 `Visibility::ClientOnly` + os `visibility` v `TOOL_POLICIES` (77 riadkov `All`), `present::visible_to` + `enforce_audience` vetva; `AGENT_BUDGET_BYTES` 51 000 (tvrdý, host full) + `MASTER_BUDGET_BYTES` 65 500 (mäkký) nahrádzajú `BUDGET_BYTES`; `ro_bytes` proti host full; trimy `fleet_health` (−480) a `plan_sync` (−530) popisov (D7 bod 3); `doc_gen` riadok *Audience*; testy `a_per_host_token_is_not_served_client_only_tools`, `client_only_tools_are_not_named_in_the_control_skill`, `enforce_audience_names_the_audience`; `control-api.md` *The served tool surface* (štyri čísla z testu), *Per-host tokens*; `hub.md` *Clients*** | `docs/adr/0003-tool-visibility.md` (nový), `mcp/guard.rs:79-85`, `mcp/tools/present.rs:63-68`, `mcp/tools/mod.rs` (gate), `mcp/tools/tests.rs:2325-2400`, `mcp/tools/fleet.rs:8-10` (trim), `mcp/tools/assets.rs` (`plan_sync` trim), `mcp/doc_gen.rs:26-40`, `docs/control-api.md`, `docs/hub.md` | S/M ~190 (+120) | `REGEN_DOCS` | — | **začiatok lane D, pred 08/09** (D7 bod 4); ∥ so všetkým okrem `guard.rs`/`tests.rs` |
| **26** | 07 PR plán | Session parita — **26a** fleet-core: `DiscardKillSessionParams`, `SessionToolDetailParams`, `DismissAgentSessionParams`; `Deserialize` na `SafeKillInspection`, `ToolDetail`, `EditDetail`; tools `inspect_safe_kill` (T0), `discard_kill_session` (T1, **`confirm: true`**, `refuse_if_operator`), `session_tool_detail` (T0), `dismiss_agent_session` (T1 RN) — všetky `ClientOnly` (D11); 6 nových testov; reference (+ `move_session` bez „master only“, UX-83) · **26b** routing: 5× `Routed` (`session_activity → capture_session` RE, `route_text` + `probe_from_tail`), `purge_project` text, remote ×5, `mod routed` ×5, 5 Case (prvý textový Case), kontrakt (2 samples), goldeny, min. TS (`REASONS` −3, `ROUTED_ACTIONS` + `discard_kill_session`/`dismiss_agent_session`/**`resolve_move`** (UX-86), allowlisty `guardedDirectlyWithOwnsTheFleet`/`handledInlinePerClickNotPreGated` zmazať, `hubBlock` → `hubActionBlocked` v `SessionDetails:59-61`, `SessionRowItem:143`, `ConversationPanel:720`), `hub.md` E_CONFIRM + `repair_session`, *What is different*, *Known limitations* `purge_project` | `mcp/tools/{params,lifecycle,orchestration,session_ops}.rs`, `service/safe_kill.rs:51-70`, `service/transcript.rs:1289-1310`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `backend/{verdicts,remote,tests_routing,tests_contract}.rs`, `commands/sessions.rs:106-133,203-213,386-452`, goldeny, `hub.ts:192-197,257-274`, `hub_verdicts.test.ts:164-176`, `docs/{hub,control-api}.md` | M ~310 (+230) → **odštep 26a ~150 / 26b ~160 nutný** | 26a `REGEN_DOCS`; 26b `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT` | 25 (`visibility`), 22 (lane D) | → 22b (lane D) |
| **27** | 07 PR plán | Safe remove / Discard / Tool detail / Repair / živý indikátor v hub režime — gate Safe remove na `safe_kill_session` (UX-77), stav dialógu `unsupported` z `hubInlineState` (Ask Claude + Kill, Q-II.5), Kill dialóg kópia z **`killDialogCopy` (06)** + *Safe remove instead…* (UX-78), Discard `E_WORKTREE_REMOVE` inline + *Re-inspect*, Repair `ConfirmDialog` s hub dodatkom (UX-79), `ToolLine.detailUnsupported` + `E_FORBIDDEN` neretryable, `ConversationPanel` `probeLive` bez `ownsTheFleet` + `· not live` (UX-81), `TerminalView` `terminal-unchecked-workspace` riadok, `TransferSheet` Finish/Undo `hubActionBlocked('resolve_move')` (UX-86), Remove from list enabled, UX-84 riadok „the hub will also ask its operator“ (po 09). **Bez `Sidebar.svelte` (D8 bod 3 — Kill kópia v riadku/bulk je 37)** | `SessionDetails.svelte`, `ToolLine.svelte`, `ConversationPanel.svelte`, `TerminalView.svelte`, `TransferSheet.svelte`, `hub_disabled.test.ts:373-411`, `SessionDetails.test.ts`, `ToolLine.test.ts`, `ConversationPanel.test.ts`, `TransferSheet.test.ts`, `hub_verdicts.test.ts` | M ~220 (+190) | — | 07, 26b, **06** (`killDialogCopy`), **33, 32** (`SessionDetails` — D8); mäkko 21, 03 (ikony), 09 | ∥ s 14–18, 23 (lane E); posledný na `SessionDetails.svelte` |
| **28** | 08 PR plán | Sidebar chrome — dva riadky: host/čas `<select>` (UX-89), chip pozornosti (`.btn--crit`, zatiaľ vždy), `IconSelect`, **bulk swap slot** (UX-93), `ViewMenu.svelte` (Show: `Background agents (n hidden)` / `Friendly names` / **`Last prompt line` (D14)**; Theme `SegmentedControl` s `Auto (dark)`), Settings do riadku 1, Tasks → pás záložiek (`tasksOpen` v `app_views.ts`, Q-II.9), theme riadok von z pätičky (UX-11), `controls.css` triedy namiesto `.pill`/`.icon-btn`/`.theme-toggle` (UX-95), hint kotvy (UX-94), `title` hosta bez verzií | `SidebarFilters.svelte` (prepis ~200 → ~170), `ViewMenu.svelte` (nový ~110), `Sidebar.svelte` (−20), `App.svelte` (+25), `app_views.ts` (+5), `hints.ts`, `SidebarFilters.test.ts` (nový), `ViewMenu.test.ts` (nový), `Sidebar.test.ts` (~14 miest), `App.hosts.test.ts` | M ~250 (+220) → odštep 28a riadky + bulk slot ~130 / 28b `ViewMenu` + Theme + Tasks ~120 pripravený | — | 01, **02 pred** (D8) | **začiatok lane F**; ∥ s B, C, D |
| **29** | 08 PR plán | Pätička — split `+ New session ▾` (`.btn--primary` 28 px), `ProjectPicker.svelte` (search `fuzzyMatchFields`, host `<select>` z `pickerHost → last-host → local`, RECENT (≤ 5 podľa `last_session_at`) / ALL, `role="option"` + `aria-activedescendant`), šípka: `Background session…` / `Add project…` (**`title` bez „Do it on the hub“ — D8 bod 2**, UX-20), `⚡` preč, hint `bg-session` na šípku, `NewBgSessionDialog` `usableHost` filter (UX-96, D12) + `initialHost` | `Sidebar.svelte` (footer + picker von, −80/+40), `ProjectPicker.svelte` (nový ~140), `NewBgSessionDialog.svelte` (+10), `hints.ts`, `ProjectPicker.test.ts` (nový), `Sidebar.test.ts:539-620`, `hub_disabled.test.ts:431-442` | M ~225 (+200) | — | 28, **36** (`Sidebar.svelte` — D8) | → 36 (lane F) |
| **30** | 08 PR plán | Pozornosť a poradie — chip skrytý pri `N === 0 && !needsYouOnly` (UX-08 zvyšok, Q-II.10), `byTriage` v `buildSessionsByProject` per projekt (UX-92), pref `rows.sort ∈ {status, recent}` default `status` (Q-II.12), SORT radio vo View menu; tripwire ≤ 2 volaní ostáva | `SidebarFilters.svelte` (+6), `sidebar_index.ts` (+20), `sessions.ts` (+6), `ViewMenu.svelte` (+25), `Sidebar.svelte` (+4), `sidebar_index.test.ts`, `Sidebar.test.ts:1086-1145`, `prefs.test.ts` (nový) | S ~70 (+120) | — | 28, 36 (`SidebarFilters`), 29 (4 riadky `Sidebar.svelte`) | → 29 (lane F) |
| **31** | 08 PR plán | Sémantika filtrov — čas na **session** (`matchesRecency(s, r, now)` nad `last_activity_at`, UX-87), search ako riadkový predikát cez `fuzzyMatchFields` (UX-88), projekt viditeľný podľa riadkov, label `Active`, chord `/` (Q-II.11; iba mimo editovateľného cieľa a Hosts view) | `sidebar_index.ts` (+30), `session_status.ts` (±15), `Sidebar.svelte` (−15/+15), `App.svelte` (+12), `session_status.test.ts`, `sidebar_index.test.ts`, `Sidebar.test.ts:700-724,558` | S ~100 (+130) | — | 28, 30 (`sidebar_index.ts`), **36 pred** (UX-115 `hiddenCount` nad `visible`) | → 30 (lane F) |
| **32** | 09 PR plán (**nahrádza 19**) | **Stavový model** — `status_chip.ts` (typ, `claudeStatusChip`, `stuckChip`, `lifecycleChip`, `inactiveChip`, `ciChip`, `contextChip`, `sessionStatusChip`, `notificationChip`), `StatusChipView.svelte`, +11 exportov v `icons.ts`, tón → CSS triedy (UX-100, 104, 110); `attention.ts` zmaže `STATUS_LABEL`/`STATUS_COLOR`/`STUCK_COLOR`/`ciStatusLabel`/`ciStatusColor`; `hosts_view.ts` `sessionCounts` bez `text`; `conversation.ts:305-310` → `notificationChip`; render iba náhrada chipov (nie markup) v `SessionRowItem:208-222,275-296`, `SessionDetails:432-446,511-518`, `HostDetail:98-102`, `HostsList` (počty), `ConversationPanel` (mark); `tokens.test.ts` páry vs `bg` + hex lint | `status_chip.ts` (nový ~110), `StatusChipView.svelte` (nový ~40), `icons.ts`, `attention.ts`, `hosts_view.ts:113-128`, `conversation.ts`, 5 Svelte súborov, `status_chip.test.ts` (nový), `attention.test.ts`, `conversation.test.ts:644-651`, `hosts_view.test.ts:86-101`, `HostsView.test.ts:161`, `Sidebar.test.ts`, `SessionDetails.test.ts`, `tokens.test.ts` | S/M ~140 (+120) | — | 01; **02 pred** (`SessionRowItem` — D8); **03 pred** (`SessionDetails`) | lane A; ∥ s B, D, F; **pred 23, 27, 33** |
| **33** | 09 PR plán | **Hierarchia riadku** — `SessionRowItem.svelte` prepis markupu `:201-425` a CSS `:430-688`: jedna kostra live/ghost/external/bg (UX-106), L1 `displayName` + `StatusChip` s vekom vpravo (UX-97), L2 host · `secondaryName` · `RowSignals` (max. 3, `nowrap`, UX-102), L3 prompt iba `rows.details` (D14), akcie 3 + `…` 24 px nad pravým koncom L2 + `:focus-visible`/`:focus-within` (UX-101, 107), `showHost` prop, effort a elapsed von (UX-105), kontext chip `<button>` → `onCompact` `/compact` pri crit + `~` pri `context_stale` (UX-98, Q-II.15), `--row-font-2` 10 px (UX-109), `aria` bez duplicít (UX-108); `SessionDetails` `.sub` → `StatusChipView`, `Idle since`/`Stuck since`, `formatMoney`; `hints.ts:38` text | `SessionRowItem.svelte`, `RowSignals.svelte` (nový ~80), `Sidebar.svelte:639-660` (+`showHost`, `onCompact` — **mimo regiónov lane F**), `SessionDetails.svelte:429-456,459-520`, `app.css`, `hints.ts`, `Sidebar.test.ts:813-816,1196-1306,1362-1370` (testidy zachované), `SessionDetails.test.ts` | M ~230 (+150) → odštep 33a kostra ~170 / 33b detail ~60 pripravený | — | **02, 32, 34**; mäkko 28 (label) | lane A; **pred 06, 37** (D8); `Sidebar.svelte:639-660` je región mimo lane F — koordinovať rebase s 28/36 |
| **34** | 09 PR plán | **Formátovanie a prahy** — `src/lib/format.ts` (`formatMoney`, jediný `formatTokens`, `formatDuration` ← `formatElapsed`, `formatAge` jedna jednotka, `promptPreview` + `isNoisePrompt` so stop-listom P2 a slash príkazmi — UX-99, 103), `sessions.ts:133-150` a `conversation.ts:904-909` → import, prahy **80/95** (`attention.ts:75-76`, D14), `session_status.ts:33-49` → `format.ts`, tooltipy kontextu | `format.ts` (nový), `sessions.ts`, `conversation.ts`, `attention.ts`, `session_status.ts`, `format.test.ts` (nový), `sessions.test.ts:31-35`, `conversation.test.ts:469-475`, `attention.test.ts:94-103,189-206`, `session_status.test.ts`, `Sidebar.test.ts:1028-1041,1060` | S ~90 (+80) | — | — | **∥ so všetkým** (nové súbory + presuny); pred 33 |
| **35** | 09 PR plán (**nahrádza 19**) | **Usage značky na model** — `account_usage.ts` (`glyph` → `icon`, batériové ikony D9), `usage_glance.ts`, `hosts_view.ts:189-209,264-284` (`HostAttention.glyph` → `icon`; `freshnessMark` → `{ age, state, title }` — **tvar; sémantika z 23**), render `UsageBlock`, `HostsList`, `HostsView` (group freshness), `NewSessionDialog` (usage chipy), `App.svelte` (glance); 66 asserov v 7 súboroch | `account_usage.ts`, `usage_glance.ts`, `hosts_view.ts`, `UsageBlock.svelte`, `HostsList.svelte`, `HostsView.svelte`, `NewSessionDialog.svelte`, `App.svelte`, `account_usage.test.ts`, `usage_glance.test.ts`, `UsageBlock.test.ts`, `hosts_view.test.ts`, `HostsView.test.ts`, `NewSessionDialog.test.ts`, `HostsList.test.ts` | M ~160 (+130) | — | 32, 01; **23 pred** (D8 bod 5) | lane A koniec; po lane E-hosts; `NewSessionDialog` bez kolízie s 29 |
| **36** | 10 PR plán | **Model výberu** — `bulk_selection.ts` (`toggle`/`rangeTo`/`selectAll`/`clear`/`pruneTo`/`hiddenCount`/`visibleOrder`; kotva; ghost ignorovaný **v modeli** — D8 bod 4), `Sidebar.svelte:156-184` → import, `onSelectSession` shift = rozsah / ⌘ = toggle / auto-mód (UX-112, 113, Q-II.17), `onKeySession` Space, Shift+Space, ↑↓, Shift+↑↓, ⌘A (Q-II.19), Escape dvojkrok (UX-118), prune ghostov (UX-114), `SidebarFilters` `hiddenCount` text `· 1 hidden` (UX-115). **Bez `SessionRowItem.svelte`** | `bulk_selection.ts` (nový ~90), `Sidebar.svelte` (−30/+60), `SidebarFilters.svelte` (+4), `bulk_selection.test.ts` (nový ~90), `Sidebar.test.ts:1148-1169` (+60) | S/M ~140 (+150) | — | **28** (`Sidebar.svelte` poradie D8; funkčne nezávislé) | → 28 (lane F); **pred 29, 31** |
| **37** | 10 PR plán | **Bulk lišta + gating + potvrdenie** — `BulkBar.svelte` v slote riadku 2 (aj pri 0 vybraných s hintom), `bulk_actions.ts` (`bulkEligibility` per akcia — UX-114, `bulkGate` z verdiktov, `groupErrors` — UX-120), `Kill ▾` menu (Kill / Safe remove… (Q-II.6) / Discard & kill… disabled do 26b, Q-II.8), `BulkConfirmDialog.svelte` (zoznam `displayName` · host · `StatusChipView` · riziko z `inspect_safe_kill` ≤ 5 ∥, strop 20 (Q-II.7) · badge `hidden`, *Safe remove instead…*, `busy` progres), `runBulk` (toast `Killed 2 · 1 failed` + *Retry failed*, zlyhané ostanú vybrané — UX-117), **Kill kópia riadok + bulk z `killDialogCopy` (D8 bod 3)**, checkbox ako **sused** riadkového tlačidla + tri-stavový v `proj-row` (UX-119, 123), `aria-multiselectable`, `aria-live` na počte, `readonly` klient celá lišta disabled (21); zmazať `.bulk-bar` CSS | `BulkBar.svelte` (nový ~120), `bulk_actions.ts` (nový ~80), `BulkConfirmDialog.svelte` (nový ~110), `Sidebar.svelte:186-203,841-866,695-724` (−40/+60), `SidebarFilters.svelte` (−25/+8), `SessionRowItem.svelte` (checkbox von, ~15), `BulkBar.test.ts`, `bulk_actions.test.ts`, `BulkConfirmDialog.test.ts`, `Sidebar.test.ts:1148-1188,1412-1436`, `hub_disabled.test.ts` (+40, UX-122) | M ~290 (+220) → **odštep 37a lišta + eligibility + menu + checkbox ~150 / 37b dialóg + riziko + `runBulk` + toast ~140 pripravený** | — | 36, 28 (slot), 02 (ikony), **06** (`displayName`, `killDialogCopy`), **33** (kostra riadku); mäkko 26b (riziko na hube), 21, 09 | → 31 (lane F); posledný veľký PR na `Sidebar.svelte` |
| **38** | 10 PR plán | **Hromadný prompt** — Rust: `send_prompt_inner(…, label.unwrap_or(true))` konzumuje pole z **04** (D10); ak 04 pole nemá, zavedie ho tu (druhá voľba). TS: `sendPrompt(…, { label })`, `BulkPromptDialog`: `displayName`, `Send to N`, progres `k / N`, `Close` namiesto `setTimeout`, zlyhané ostanú vybrané (`onResult(failedIds)`), toast; `sendable` z `bulkEligibility` (37); `BulkPromptDialog.test.ts` (nový, UX-122); test `prompt.rs` „`label: Some(false)` nepomenuje“ | `service/sessions/prompt.rs` (+8), `commands/sessions.rs` (+1), `sessions.ts` (+6), `BulkPromptDialog.svelte` (~40), `Sidebar.svelte` (+5), `BulkPromptDialog.test.ts` (nový ~80), `prompt.rs` test | S ~110 (+100) | (`REGEN_HUB_CONTRACT`, `REGEN_DOCS` — iba ak pole zavádza sám) | **04** (pole), 37 (`bulkEligibility`), 06 (`displayName`) | koniec lane B **a** lane F; Rust ∥ (po 04), TS → 37 |

**Súčty:** hlavná fronta (35 PR: 01–18 bez 19/20, 21–23, 25–38) ≈ **6 160** riadkov bez testov a
generovaných (+≈ **4 730** testov) — blok 1 ≈ 3 070 (+2 100), blok 2 ≈ 3 090 (+2 630): 06 → 590
(+460), 07 → 710 (+550), 08 → 645 (+670), 09 → 620 (+480), 10 → 540 (+470), korekcie ±15.
Odložený 24 ≈ 230 (+150). UXPR-19 (≈ 150 + 60) a UXPR-20 (≈ 60 + 20) z fronty vypadli. Nad ~300
riadkami sú bez odštepu iba 26 (310 → 26a/26b **nutné**); s pripraveným odštepom 22 (280), 37
(290), 04 (290 — `label` pole je odštepiteľné do 38), 28 (250), 33 (230).

**Lanes a kritická cesta:**

```
lane A  ikony + riadok   01 → 02 → 32 → 33 → 06(B) ………→ 35 (po 23)          34 ∥ (kedykoľvek pred 33)
lane B  pomenovanie      04 → 05 → 06 (po 03, 33) → 38 (po 37)
lane C  zdieľané         07 ∥ 21 ∥ 25(začiatok D)                              — tri malé PR hneď
lane D  hub Rust         25 → { 08 ∥ 09 } → 10 → 11 → 12 → 13 → 22a → 22b → 26a → 26b → [24]   ← kritická cesta
lane E  hub Svelte       14 → 15 (po 09, 07) · 16 → 17 (po 11, 07; 17 po 03) · 18 (po 13, 07)
                         23 (po 22b, 07, 32) · 27 (po 26b, 07, 06, 33)
lane F  sidebar          02 → 28 → 36 → 29 → 30 → 31 → 37 (po 06, 33) → 38(B)
```

Kritická cesta je lane D: **10 sekvenčných PR** (`25 → 09 → 10 → 11 → 12 → 13 → 22a → 22b → 26a →
26b`, bolo 5), za ňou `27` a odložený `24`. Dôvod sekvenčnosti je nezmenený (každý edituje
`guard.rs`, `tests.rs`, `verdicts.rs`, `remote.rs`, `tests_routing.rs`, `hub.ts`,
`hub_verdicts.test.ts` a goldeny). Poradie **Settings → git → Assets → Hosts/usage → sessions**
drží hodnotu (C nálezy 16/42, H 59/60, potom H 68/69, H 77). Lane F je po D8 **nezávislá od D**;
najdlhšia ne-D cesta je `01 → 02 → 28 → 36 → 29 → 30 → 31 → 37 → 38` (9 PR) so vstupmi `32 → 33` a
`04 → 06`. Lane E ostáva za D.

**Regen podľa PR (kontrolný zoznam, D6 bod 3 — každý 2×, goldeny prečítať):**

| Príkaz | PR |
|---|---|
| `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` | 04, 08, 09, 10, 12, **25**, **22a**, **26a**, (38 iba ak zavádza pole), [24] |
| `REGEN_LOCAL_ONLY=1 cargo test -p claude-fleet --lib local_only` | 09, 11, 13, **22b**, **26b**, [24] |
| `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` | 09, 11, 13, **21** (`routed_readonly`), **22b**, **26b**, [24] |
| `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` | 04 (`friendly_name_source` + **`label`**), 13, **22b** (`sample_usage_snapshot`), **26b** (`sample_safe_kill_inspection`, `sample_tool_detail`), [24] |
| žiadny regen (čisto Svelte/TS) | 01–03, 05–07, 14–18, 23, 27, 28–37 |

## 4. Otvorené otázky pre vlastníka (Q-II)

Zlúčené z piatich reportov (**39** pôvodných: 06 → 7, 07 → 7, 08 → 8, 09 → 7, 10 → 10).
Vypustené, lebo ich rozhoduje §2: 06 Q1 (T0 refresh), Q3 (skryté hosty), Q4 (`discover_hosts`
Master), Q6 (kôš skryť) → D11/D12; 07 Q1 (`confirm: true`), Q2 (RN dismiss), Q3 (dva stropy), Q4
(`inspect` T0), Q5 (`purge` Master tool — nie) → D7/D11; 09 Q1 (`rows.details`), Q2 (prahy 80/95)
→ D14, Q6 (batériové ikony), Q7 (tón `info`) → D9; 10 Q7 (`label` wire pole) → D10; 10 Q10
(hromadný Move — nie) → §5 bez šošovky. Dedup proti konsolidácii-01 §4 („default“): C3 `force`
vypustiť → analógia `create_remote` prijatá v D11; C4 derived `mcp.confirm_destructive` → UX-84/120
vopred; C5 `client_mode` → UXPR-21. Ostáva **23**, každá s odporúčaním; „default“ = prijať všetky.

### A — Hub parita a prístup

1. **Načasovanie UXPR-24** (`add_project` + `list_github_repos`; 06 Q2): routovať hneď po lane D
   (rozhodnutie (b) README), alebo až keď telefón (šošovka 20) potrebuje pridávať projekty?
   `create_remote` je už vypustené (D11). *Odporúčanie: po šošovke 20; dovtedy disabled s krátkym
   `title` (UXPR-29).*
2. **`trusted` v `/pair` odpovedi a `hub.client_trusted`** (06 Q5): desktop by poznal T2 stav
   vopred (Push, `set_fleet_setting`); vyžaduje zmenu hubu (`pairing.rs` redeem + `/pair` handler) a
   novší hub. *Odporúčanie: nie teraz; T2 sa učí z prvého `E_FORBIDDEN` cez `hub_inline_state`
   (UXPR-07); backend backlog ako follow-up UXPR-21.*
3. **Retroaktívne `ClientOnly` pre existujúce tools** (07 Q6: `list_host_worktrees`, `delete_worktree`,
   `resolve_move`, `related_sessions`, `session_conversations`, `ensure_operator`/`operator_status`;
   ~300–700 B každý z agentovej plochy). *Odporúčanie: nie v UXPR-25; samostatný S pass po lane D s
   testom proti `SKILL.md`.*

### B — Hosts view a usage

4. **`FooterState 'unsupported'`** pre starší hub bez `list_account_usage` (06 Q7): nový stav v
   pätičke (tón `muted`, „usage — hub too old“), alebo zložiť do `unavailable`? *Odporúčanie: nový
   stav — `unavailable` znamená „Anthropic endpoint“, čo by zavádzalo.*

### C — Safe remove, Discard a hromadná bezpečnosť

5. **Starší hub bez `inspect_safe_kill`** (07 Q7): dialóg ponúkne *Ask Claude* + *Kill*, alebo iba
   *Ask Claude*? *Odporúčanie: oboje — Kill je aj tak o tlačidlo vedľa; v dialógu je aspoň s vetou o
   worktree (UX-78).*
6. **Safe remove v `Kill ▾` už v UXPR-37** (10 Q4): routuje dnes, každý riadok dostane systémový
   prompt (`label: false` po 04). *Odporúčanie: áno — jediná bezpečná hromadná cesta a prvé miesto,
   kde ju hub klient má (UX-77).*
7. **Riziková inšpekcia pred hromadným Kill** (10 Q6): N × `inspect_safe_kill` paralelne ≤ 5,
   timeout 3 s/riadok, `Kill` klikateľný hneď, strop N ≤ 20 (nad to iba súhrn). *Odporúčanie: áno,
   strop 20.*
8. **Discard & kill hromadne** (10 Q8): v menu disabled s dôvodom do UXPR-26b; potom `force=false`
   iba pri riadkoch s inšpekciou `clean`, `force=true` iba po druhom potvrdení so zoznamom dirty
   súborov. *Odporúčanie: áno v tejto forme; hromadný `force=true` bez zoznamu nikdy.*

### D — Sidebar IA

9. **Tasks do pásu záložiek vedľa `Hosts ⌘I`** (08 Q4; zasahuje `App.svelte`, šošovka 13), alebo
   do View menu ako sekcia *Open*? *Odporúčanie: pás záložiek — Tasks je fleet-wide pohľad ako
   Hosts; chord neprideľovať (šošovka 16).*
10. **Chip pozornosti skrytý pri nule** (08 Q5): odoberá jediný prepínač, ktorý pri N = 0 ukazoval
    `idle_long` riadky; status sort (UXPR-30) ich zdvihne nad `idle`. *Odporúčanie: skryť.*
11. **Chord `/` na fokus vyhľadávania** (08 Q3), mimo editovateľného cieľa a terminálu; ⌘F patrí
    Conversation find. *Odporúčanie: áno, `/` (šošovka 16 to iba zapíše do cheat-sheetu).*
12. **Default `rows.sort`** (08 Q8): `status` alebo `recent` (= dnes)? *Odporúčanie: `status` — FE-4 a
    P13 tak boli zamýšľané; `recent` ostáva voľbou v menu.*
13. **`Background agents` off a Outside fleet** (08 Q7): má skrývať aj `bg:<uuid>` riadky?
    *Odporúčanie: nie — Outside fleet je vlastná zbaliteľná sekcia; iba label + „(n hidden)“.*
    Súvisí: **host filter vždy `<select>`** (08 Q1) vs. `SegmentedControl` pri ≤ 3 hostoch —
    *odporúčanie vždy `<select>`* (jedno pravidlo, natívna AX vo WKWebView); **časový filter na
    session** (08 Q2, UX-87) — *odporúčanie áno, label `Active: 8h`*; **theme do View menu** (08 Q6)
    — *odporúčanie áno, `Auto (dark)` ukáže rozhodnutie*. (Tri podotázky s jedným „default“.)

### E — Riadok session

14. **Kontext ≥ 95 % ako triage** (09 Q3): (a) nový bucket `context_full` (mení aj počítadlo „Needs
    you“), (b) iba promotovať chip na L1, (c) nič. *Odporúčanie: (b) teraz v UXPR-33; (a) ako otázka
    pre UXPR-30, ak sa ukáže potreba.*
15. **`/compact` na klik z riadku** (09 Q4): chip vyberie session a **predvyplní** composer, neodošle.
    *Odporúčanie: áno, iba predvyplniť — konzistentné s quick-action chipmi v Conversation.*
16. **`effort_level`** (09 Q5): z riadku odstrániť hneď (UX-105) a (a) backend začne pole plniť, alebo
    (b) migrácia stĺpec zruší? *Odporúčanie: (a) ako backend nález mimo fronty; do riadku sa nevracia
    — patrí do detailu a tooltipu ceny.*

### F — Výber a klávesnica

17. **Auto-zapnutie módu prvým ⌘/shift-klikom** (10 Q1): checkboxy a lišta prídu s prvým výberom;
    alternatíva „tichý“ výber bez checkboxov. *Odporúčanie: áno — výber nikdy nie je neviditeľný.*
18. **Skryté vybrané riadky** (10 Q2, UX-115): ponechať s počtom `· n hidden` a badge v dialógu, alebo
    pri zmene filtra vyhodiť? *Odporúčanie: ponechať + počet; nikdy ticho meniť výber filtrom, nikdy
    ticho zabiť neviditeľné.*
19. **Space v select móde = toggle** (10 Q5; dnes Space = Enter = otvoriť) a **⌘A vyberie všetky
    viditeľné eligible** iba pri fokuse v `.sidebar` mimo vyhľadávania (10 Q9). *Odporúčanie: oboje
    áno; Enter ostáva otvoriť. Ak nie Space, toggle iba Shift+Space.*

### G — Mimo rozsahu tejto fronty

20. **Hromadný Tag** (10 Q3): `set_session_tags` nemá Tauri príkaz ani jednotlivé UI; hromadný Tag =
    nový príkaz (Routed T1, tool existuje) + routing v lane D (~120 riadkov) + `TagEditor`.
    *Odporúčanie: mimo UXPR-36–38; samostatný S PR „Tags UI“ (jednotlivý aj hromadný naraz) po lane
    D — §5.*
21. **`purge_project` Master tool** (06 Q6 / 07 Q5 — rozhodnuté „nie teraz“ v D11): potvrdiť, že kôš v
    hub režime je **skrytý** (nie disabled) a tool ide do backlogu. *Odporúčanie: áno.*
22. **`REASONS.add_host/remove_host/hide_host` zlúčiť do jednej hodnoty `FLEET_ADMIN`** (06, UX-72) —
    test `REASONS_KEYS_THAT_ARE_NOT_COMMANDS` to dovolí len ako kľúč; hodnoty sa smú zdieľať.
    *Odporúčanie: áno v UXPR-22b (min. TS).*
23. **Odštepy 22a/22b, 26a/26b, 37a/37b, 28a/28b, 33a/33b** ako samostatné PR, alebo jeden PR s dvoma
    commitmi (limit ~300 platí na PR)? *Odporúčanie: 26 vždy dva PR (310); ostatné jeden PR s dvoma
    commitmi, druhý PR iba ak recenzent chce.*

## 5. Čo ide do iterácií 11–20

Položky, ktoré päť reportov vytlačilo mimo šošovky, priradené k README §5 šošovkám 11–20. „—“ = nemá
šošovku.

| Šošovka | Položka | Pôvod | Poznámka |
|---|---|---|---|
| **11** Agent panel a FAB | operátor v sidebare (vlastná sekcia vs. riadok projektu, `IconAgent`, „Fleet operator“ ako `chosen`) | kons.-01 §5 (02 P6/Q7) | UXPR-04 už dá operátorovi `chosen`; UX-13 závažnosť prehodnotiť (panel hlásil pravdu) |
| 11 | **slice 2 confirmation channel**: `E_CONFIRM_REQUIRED` klientovi cez event, `confirm_nonce`, dialóg „waiting for the hub's operator to approve“ zatvorený `session:killed` eventom | 07 §Safe remove, UX-84; 10 UX-120 | UXPR-27/37 dávajú toast/riadok; 11 navrhne kanál; `repo_push`, `repo_delete_branch`, `discard_kill_session` sú kandidáti na `confirm: true` |
| 11 | kontextový chip agenta preberá `displayName` (UX-15) a `HostsView` kontext | 06, D13 | `agent_context.ts` v UXPR-06 |
| **12** Conversation hlavička | `· not live` indikátor a `conv-detail-unsupported` riadok — umiestnenie v hlavičke s 10 prvkami (UX-22) | 07 UXPR-27 | 27 ich dá pod hlavičku; 12 rozhodne finálne miesto |
| 12 | `notificationChip` (D9) a Timeline ikony pre git `mcp_call` riadky | 09 UXPR-32; 05 | po UXPR-01/32 |
| 12 | prahy 80/95 v hlavičke (D14), `~97 %` pri `context_stale` | 09 | zdieľaná konštanta z UXPR-34 |
| **13** Terminál chrome | `terminal-unchecked-workspace` riadok pod hlavičkou terminálu v hub režime (UX-79) | 07 UXPR-27 | zmizne po prvom výstupe pane |
| 13 | Tasks do pásu záložiek vedľa `Hosts ⌘I` (Q-II.9), `App.svelte:934-939` okraj (UX-25), tmux meno 3× (UX-07 — hlavička rieši UXPR-06) | 08 UXPR-28 | chord je **⌘I** |
| **14** Prázdne/loading/chybové stavy | prevziať `HubScopeNote what='hosts'` (jediný odsek na pohľad) a `hub_inline_state` stavy `unsupported` (Safe remove dialóg, tool detail, usage pätička) | 06, 07, D5 | 14 nesmie navrhnúť tretí komponent |
| 14 | `FooterState 'unsupported'` (Q-II.4); `?` s dôvodom (UX-68); `Skeleton.svelte` (17) pre Hosts a sidebar; prázdny stav pickera (`ProjectPicker`) | 06, 08 | |
| **15** Toasty | **bulk súhrnný toast** (`Killed 2 · 1 failed` + *Retry failed*, `ToastAction`), **zoskupenie N hub toastov** podľa `error.code` (UX-120), `hubNextStep` veta, sticky error toasty, UX-24 update toast bez tlačidla | 10 UXPR-37; 07 UX-84 | 37 zavádza `groupErrors`; 15 zjednotí stack a trvanie |
| **16** Klávesnica | `/` fokus vyhľadávania (Q-II.11), ⌘A, Space / Shift+Space, ↑↓ / Shift+↑↓, dvojkrokový Escape (UX-118), Tasks bez chordu, `?` legenda vs. `?` stĺpec (UX-27/06), hint texty na meniacom sa chrome (UX-94), `session-actions` hint „Hover, focus or select…“ | 08, 10, 06, 09 | 36 implementuje, 16 zapíše cheat-sheet a overí kolízie s terminálom (`terminal_keys.ts`) |
| **17** Prístupnosť | vnorený `<input>` v `role="button"` (UX-119 — 37 vyťahuje), `aria-multiselectable`/`aria-live` (37), kontrast UX-100 (32 rieši, 17 overí `tokens.test.ts` páry), hover-only akcie UX-107 (33 rieši `:focus-within`), Tasks/Settings v `nav host filter` (UX-90), `title`/`aria` duplicity (UX-108), `role="option"` v pickeri (29), WKWebView AX bridging (UX-28 — auditový klik) | 09, 10, 08 | 17 je **audit po** lane A/F, nie návrh |
| **18** Light téma | UX-100 hard-coded hex (po 32 odpadá — 18 overí screenshotmi), `Auto (dark)` label vo View menu, `--usage-*` vs `--bg` páry, `.icon-btn.danger`/`.err` `#e64a4a` (UX-30) | 09, 08 | |
| **19** Onboarding | `check_local_prereqs` LocalOnly v hub režime — `OnboardingCard` má ukázať hubové fakty, nie checklist; `tunnel_status` RE (22b) ho odblokuje; „Replay setup guide“ slepý (UX-46); `hub.client_mode` `null` pre staršie párovanie (21) | 06, 03 | |
| **20** Telefón | usage `?` sémantika a `list_account_usage` T0 zdedí telefón automaticky; **asset obrazovka chýba** (UX-55); `ClientOnly` nič nemení (paired klient); `readonly` telefón vidí `inspect_safe_kill` (T0, D11); `add_project` až po 20 (Q-II.1) | 06, 07, D7 | `fleet-mobile` spec zdedí tools automaticky |

**Bez šošovky (navrhnuté kam):**

| Položka | Pôvod | Kam |
|---|---|---|
| „Assets slice 2“: Layers UI (UX-52), `catalog_load` Master tool, `list_secrets`, `resolve_preview.full`, `catalog.pull_interval_secs`, `catalog_spawn_author_session` (UX-58) | 04 (kons.-01 §5) | vlastná šošovka 4b po 20 |
| **Tags UI**: Tauri príkaz `set_session_tags` (Routed T1, tool existuje) + routing + `TagEditor`, jednotlivý aj hromadný | 10 Q3 (Q-II.20) | samostatný S PR po lane D |
| **Hromadný Move** — nie; ak raz, sekvenčný front v Transfer sheete, nie lišta | 10 Q10 | `docs/superpowers/specs/2026-09-20-transfer-sheet-design.md` *Non-goals* |
| `purge_project` Master tool (`confirm: true`, T3) | 06 Q6, 07 Q5, D11 | backend backlog; `hub.md` *Known limitations* |
| `trusted` v `/pair` odpovedi | 06 Q5 (Q-II.2) | backend backlog, follow-up UXPR-21 |
| retroaktívny `ClientOnly` pass s testom proti `SKILL.md` | 07 Q6 (Q-II.3) | S PR po lane D |
| `effort_level` plniť z `claude agents --json`/hooku alebo stĺpec zrušiť | 09 Q5 (Q-II.16) | backend backlog |
| `context_full` triage bucket | 09 Q3 (Q-II.14 (a)) | otázka pre UXPR-30, ak sa ukáže potreba |
| reconcile/repair tick čítajú nastavenie živo; `repo_stash`/`repo_discard` zámerne nie; `E_BUSY` agent-busy nie | kons.-01 §5 | bez zmeny |
| `broadcast_prompt` zo UI — nie (filter, nie id; iba `work`) | 10, D10 | zapísať do `docs/control-api.md` pri `broadcast_prompt` |
| `session_activity` tick vs. `capture_session` poll 2 s cez hub — záťaž N otvorených klientov | 07 (implicitne) | backend follow-up: overiť po UXPR-26b na `fleet.rlt.sk` |

Auditové nálezy bez šošovky v tomto bloku: UX-13, 14 (šošovka 11), 22 (12), 24 (15), 25 (13), 27
(16), 28 (17) — 7, všetky majú šošovku v README §5.

## 6. Metriky bloku 2 a kumulatívne

| Metrika | Blok 2 (iterácie 6–10) | Kumulatívne (1–10) |
|---|---|---|
| Nálezy v registri | **+56** (UX-68…123) | **123** (28 audit + 39 blok 1 + 56 blok 2) |
| Nové nálezy podľa závažnosti | H 4 (68, 69, 77, 100) · M 26 · L 26 (z toho 1 bezp. UX-76, 1 docs UX-83) | nové: C 2 · H 9 · M 41 · L 42 · bez sev 1 (UX-58) |
| Auditové nálezy pod šošovkou | 8 (UX-08, 09, 10, 12, 19, 20, 21, 27-časť) | 22 z 28 |
| — potvrdené bez zmeny | 3 (09, 12, 20) | 12 |
| — potvrdené s korekciou premisy („opravené“) | 4 (08, 10, 19, 21) + 1 spresnené (27) | 9 + 1 |
| — vyvrátené úplne | 0 (vyvrátené boli premisy zadaní a plánu, nie UX-ID) | 0 |
| — neoverené | 7 (13, 14, 22, 24, 25, 27, 28) | 7 |
| Zmeny závažnosti | UX-21 M → **H** | 1 |
| Vyvrátené premisy (zadania, README, kons.-01) | 14 (tabuľka §1) | — |
| Duplikáty/vzory zlúčené | 16 riadkov v tabuľke prekryvov | 25 |
| Rozhodnutia „raz“ | 8 (D7–D14) | 14 (D1–D14) |
| PR v pôvodných reportoch | 18 (21, 22, 23, 24 · 25, 26, 27 · 28–31 · 32–35 · 36–38) | 35 |
| PR vo fronte | +18 riadkov; 19 nahradené, 20 zrušené | **38 riadkov** = 35 hlavná + 1 odložený (24) + 1 nahradený (19) + 1 zrušený (20) |
| Súčet diffu | ≈ 3 090 (+≈ 2 630 testov) | ≈ **6 160** (+≈ 4 730) hlavná fronta; + 230 (+150) odložené |
| Najväčšie PR | 26 ≈ 310 (odštep nutný), 37 ≈ 290, 22 ≈ 280 | 04 ≈ 290 (`label` odštepiteľné) |
| Kritická cesta | +5 PR v lane D (25, 22a, 22b, 26a, 26b) | **10** sekvenčných (`25 → 09 → 10 → 11 → 12 → 13 → 22a → 22b → 26a → 26b`), potom 27 |
| Nové hub tools | 7 (3 Hosts/usage + 4 sessions) + 2 RE (`tunnel_status`, `session_activity`) + 1 policy zmena (`discover_hosts` → Master); odložené 2 (24) | **21** (2 Settings + 10 git + 2 Assets + 3 + 4) + 1 rozšírenie (`last_sync`) + 2 RE; odložené 2 |
| Nové Svelte komponenty | `ViewMenu`, `ProjectPicker`, `StatusChipView`, `RowSignals`, `BulkBar`, `BulkConfirmDialog` (6) + moduly `status_chip.ts`, `format.ts`, `bulk_selection.ts`, `bulk_actions.ts`, `session_name.ts` | 11 komponentov + 6 modulov |
| Nové migrácie | 0 | 1 (040) |
| Nový ADR | 0003 `Visibility::ClientOnly` | 1 |
| Otázky pre vlastníka | 39 → **23** (A 3 · B 1 · C 4 · D 5 · E 3 · F 3 · G 4) | 28 (blok 1, zodpovedané „default“) + 23 |

**Trajektória verdiktov (zo 130 príkazov; baseline `9c8ceabc` LocalOnly 70 / Routed 39 / RoutedUnless 1 / SameInBoth 20):**

| Po PR | LocalOnly | Routed | Čo sa routuje |
|---|---|---|---|
| dnes | 70 | 39 | — |
| 09 | 68 | 41 | `get_fleet_settings`, `set_fleet_setting` |
| 11 | 58 | 51 | 10 git zápisov |
| 13 | **51** (cieľ kons.-01) | 58 | 7 Assets |
| 22b | 47 | 62 | `list_account_usage`, `refresh_account_usage`, `set_account_nickname`, `tunnel_status` |
| 26b | **42** | **67** | `inspect_safe_kill`, `discard_kill_session`, `session_tool_detail`, `session_activity`, `dismiss_agent_session` |
| [24] | 40 | 69 | `add_project`, `list_github_repos` |

Zvyšných 42 LocalOnly po fronte: PTY/terminál (SameInBoth mimo), fleet admin T3 (4: `add/remove/hide_host`,
`provision_hosts` — disabled s dôvodom), token trio (3 — skryté), `discover_hosts`/`probe_ssh_alias`
(2 — skryté za `+ Add host`), `purge_project` (skrytý), `check_local_prereqs`, `install_fleet_hook`,
`repair_session` `explicit: false` (RoutedUnless), 25 Assets (katalógový checkout), Settings zápisy
mimo `SPECS`, `hub_*`/`mcp_*` lokálne príkazy.

**Trajektória rozpočtu (B; master / agent):**

| Po PR | Master | Agent (host full) | Konštanty |
|---|---|---|---|
| dnes | 57 603 | ≈ 50 300 | `BUDGET_BYTES` 57 700 |
| 25 | 56 593 (−1 010 trimy) | ≈ 49 290 | **`AGENT` 51 000 / `MASTER` 65 500** |
| 08 | 56 383 | ≈ 49 080 | — |
| 09 | 57 033 | 49 080 | — |
| 10 | 61 933 | 49 080 | — |
| 12 | 62 593 | 49 080 | — |
| 22a | 63 248 (+835 −180) | ≈ 48 900 (−180 `discover_hosts`) | — |
| 26a | **≈ 65 320** | 48 900 | — |
| 04/38 (`label`) | ≈ 65 440 | ≈ 49 020 | — (pod 65 500 o ≈ 60 B — **tesné**; ak meranie presiahne, výnimka D7 najbližšia stovka + odsek) |
| [24] | ≈ 66 740 | 49 020 | `MASTER` → 66 800 (jediná výnimka) |

Pozn.: 07 počítalo 65 500 bez `label` (+120) — rezerva je ≈ 60 B. Ak by UXPR-25 nameral inak (odhady sú
z čítania, nie z `cargo test`), platí pravidlo D7: `MASTER_BUDGET_BYTES` = meranie po 26a + najbližšia
stovka, s odsekom; agentov strop sa **nikdy** nezdvíha kvôli parity toolu.

## Nezrovnalosti medzi reportmi (na vedomie kontrolórovi)

1. **Zadanie tejto konsolidácie — „UX-40/106/121“:** UX-106 je „tri druhy riadkov“; nález o
   `displayName` a kolízii súboru je **UX-111**. Register a D13 používajú 40/111/121.
2. **Zadanie — „UX-13 by 07?“:** iterácia 07 sa UX-13 (FAB/operátor) nedotkla; stav ostáva neoverený,
   šošovka 11.
3. **10 vs. hub route — „UXPR-38 = 0 B“:** `commands/sessions.rs:577` posiela `SendPromptArgs` do
   hubového `send_prompt`; aby hub `label` honoroval, pole ide do `SendPromptParams` (MCP schéma)
   → ≈ +120 B na všetkých plochách. Presunuté do UXPR-04 (D10); rozpočtová tabuľka D7 to počíta.
4. **09 vs. kons.-01 — poradie 06 a 33:** kons.-01 dala 06 „→ 03 (spoločné súbory)“ a nič viac; 09
   žiada 02 → 32 → 33 → 06 na `SessionRowItem`. Platí 09 (D8); 06 sa posúva za 33.
5. **10 vs. 08/06/07 — 23 a 27 v `Sidebar.svelte`:** 10 ich radí do lane F poradia (23 prvé, 27 pred
   37), čo by lane F podriadilo lane D. Vyriešené presunom položiek (D8 body 1–3); 23 a 27 sa
   `Sidebar.svelte` nedotýkajú.
6. **06 vs. 07 — trimy `fleet_health`/`plan_sync`:** 06 ich spotrebuje v UXPR-22, 07 ich považuje za
   spotrebované a stavia nový strop 65 500 **vrátane** −1 010. Aby číslo platilo, trimy sa musia
   stať — D7 ich dáva do UXPR-25 (základ merania), nie do 22.
7. **09 vs. 06 — `HostDetail`/`HostsList` poradie:** 09 „32 po 23“; 23 je za lane D. D8 bod 5 otáča na
   32 → 23 (iné regióny, 23 rebasne).
8. **08 vs. 09 — label prepínača:** 08 píše „Details line“, 09 mení sémantiku `rows.details` (D14) →
   label „Last prompt line“; kto landne druhý, upraví.
9. **07 — „local_only 70 → 65“ a „attachments −4 landed“:** obe pravdivé — 70 je už po attachments
   (overené v generovanom JSON pri `9c8ceabc`).
10. **06 — UXPR-22 veľkosť „~280“ vs. súčet položiek (~130 + ~150):** sedí; odštep 22a/22b pripravený,
    lane D ho aj tak vyžaduje (dva regen kroky).
11. **01/02 vs. 09 — 24 px:** iterácia 01 navrhla `btn--sm` 20 px pre akcie riadku; 09 (UX-107) a
    `app.css:25-29` („24px is both the floor and the answer“) to vyvracajú. Korekcia ide do
    UXPR-01/02, aby 33 kópie zase nerušil.
12. **05 vs. 09 — `session_view.ts`:** 02/kons.-01 „nový“; existuje. D13 → `session_name.ts`.
13. **Kons.-01 riadok UXPR-20 vs. otázka A2 „default“:** A2 odporúčanie „emoji ostávajú, UXPR-20 potom
    odpadá“ bolo prijaté → 20 je zrušené; allowlist v teste ide do UXPR-01.

## 7. Navrhované úpravy README (`docs/ux/2026-09-21-audit/README.md`)

Kontrolór aplikuje; tento dokument README nemení.

| Miesto | Dnes | Zmeniť na |
|---|---|---|
| §1 tabuľka screenshotov, riadok `03-hosts-view.jpg` | „Hosts view (⌘1)“ | „Hosts view (**⌘I** / Ctrl+Shift+H)“ (iter. 08; `app_views.ts:76,82,92`) |
| §2 úvod nad hub-parity tabuľkou | „…po konsolidácii-01 je cieľ 51“ | „…po konsolidácii-01 cieľ 51, po konsolidácii-02 **42** (40 s odloženým UXPR-24)“ |
| §2 UX-08 | „zaberajú ~90 px“ | „štyri riadky chipov = 91 px, celá hlavička 134 px, chrome so pätičkou ≈ 200 px (20 % panelu); `Needs you (0)` svieti glyfom `⚠`, nie farbou (iter. 08)“; Sev ostáva H |
| §2 UX-09 | „8 rôznych metadát“ | „11 dát, z toho 3 prázdne alebo mŕtve (effort, elapsed pre tmux-objavené, `status` bodka) — iter. 09; stavový model D9“ |
| §2 UX-10 | „nikde sa neobjaví lišta hromadných akcií ani hint“ | „bulk lišta existuje (`bulk-bar`), ale iba pri `selectedCount > 0`; zapnutý mód bez výberu nemá hint (UX-112); checkbox je `<input>`, auditový AX klik zlyhal na WKWebView bridgingu (iter. 10)“; Sev ostáva M |
| §2 UX-13 | poznámka z kons.-01 | doplniť „iter. 07 sa panelu nedotkla; ostáva pre šošovku 11“ |
| §2 UX-19 | „*Danger → Hide host / Remove host…* sú viditeľné, hoci sú LocalOnly“ | „*Danger* tlačidlá sú disabled s dôvodom iba v `title` (dôvod patrí `remove_host` aj pre token-mode a Rotate — UX-72); *Token* riadok vypíše vetu odmietnutia ako hodnotu; koreň chýbajúceho usage je UX-69 (H) — hub má `UsageCache`, chýba iba tool (iter. 06)“ |
| §2 UX-21 | Sev **M** | Sev **H**; text: „Safe remove je v hub režime nedostupný celý, aj routovaná cesta `safe_kill_session` (UX-77); tool detail padá per riadok; živý indikátor ticho vypnutý (iter. 07)“ |
| §2 UX-27 | — | doplniť „`?` v hlavičke Hosts (keyboard legend) a `?` v stĺpci účtov (usage neznáme) sú dva významy jedného glyfu (iter. 06, UX-68)“ |
| §3 klaster 4 | „‚details on‘ ako jediný prepínač hustoty“ | „…; stavový model `StatusChip` (consolidation-02 D9), `rows.details` off = 2 linky / on = + prompt (D14)“ |
| §3 klaster 3 | „…zápisy idú podľa prístupovej politiky T0–T3…“ | doplniť „; hub tools pre sessions a hosty v `consolidation-02.md` §2 D11 (`discard_kill_session` `confirm: true` je jediná výnimka)“ |
| §5 tabuľka šošoviek, riadky 6–10 | bez stavu | označiť **hotové** s odkazmi: 6 → `iterations/06-hub-parity-hosts-accounts.md` (UXPR-21–24), 7 → `07-hub-parity-sessions.md` (UXPR-25–27, ADR 0003), 8 → `08-sidebar-ia.md` (UXPR-28–31), 9 → `09-session-row.md` (UXPR-32–35), 10 → `10-select-mode-bulk-actions.md` (UXPR-36–38) |
| §5 tabuľka šošoviek, riadok 9 „Očakávaný výstup“ | „primárny/sekundárny riadok, legenda % a $“ | doplniť „→ `StatusChip` model (D9) nahrádza UXPR-19“ |
| §5 tabuľka šošoviek, riadok 11 | „drawer layout, stavový model panelu“ | doplniť „+ slice 2 confirmation channel (UX-84/120), operátor v sidebare“ |
| §5 tabuľka šošoviek, riadok 15 | „stack, akcie, trvanie“ | doplniť „+ bulk súhrnný toast a zoskupenie N hub toastov (UXPR-37)“ |
| §5 tabuľka šošoviek, riadok 16 | „cheat-sheet, tooltipy so skratkou“ | doplniť „+ `/`, ⌘A, Space, dvojkrokový Escape z iter. 08/10; `?` legenda vs. `?` stĺpec“ |
| §5 Rozhodnutia (a) | „**Lucide** (`@lucide/svelte` …)“ | **bez zmeny** |
| §5 Rozhodnutia (d) | „rozpočet MCP popisov `BUDGET_BYTES` sa zdvíha **raz** na 63 800 v `UXPR-09`, ďalšie PR ho nemenia“ | „rozpočet MCP popisov má **dva stropy** — `AGENT_BUDGET_BYTES` tvrdý 51 000 (plocha per-host tokenu) a `MASTER_BUDGET_BYTES` mäkký 65 500 — nastavené **raz v `UXPR-25`** (ADR 0003 `Visibility::ClientOnly`, pred UXPR-08/09); parity tools sú `ClientOnly`; ďalšie PR konštanty nemenia (jediná známa výnimka `UXPR-24` → 66 800)“ (consolidation-02 D7) |
| §5 Rozhodnutia (e) | „…`confirm: false` na hube“ | „…`confirm: false` na hube, s jednou výnimkou: skladaný tool nesmie obísť `confirm` bránu svojich súčastí (`discard_kill_session`, consolidation-02 D11)“ |
| §5 za odkazom na konsolidáciu-01 | — | „**Konsolidácia po iteráciách 6–10:** `iterations/consolidation-02.md` (register UX-68…123, D7–D14, fronta UXPR-01…38 — 35 hlavná + 1 odložený, 23 otázok Q-II).“ |
| §4 prvá odrážka (FE-4) | „FE-4 (triage, bulk actions)“ | „FE-4 (triage, bulk actions — bulk kill/prompt landed, status sort **nie** (UX-92, UXPR-30); klávesnica a potvrdenie s rizikom v UXPR-36/37)“ |

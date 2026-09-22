# Iterácia 01 — Ikonový systém a vizuálny jazyk

**Šošovka:** ikony a vizuálny jazyk · **Zasahuje:** UX-01, UX-02, UX-03, UX-04, UX-11 ·
**Vstup:** `docs/ux/2026-09-21-audit/README.md`, screenshoty 01 a 05, kód v `src/` na
`82f304a4` · **Režim:** read-only review, žiadne zmeny kódu.

## Zhrnutie

1. Appka nemá ikonový systém: **62 miest** v `src/**/*.svelte` používa Unicode glyf
   alebo emoji ako ikonu (inventár nižšie), a ďalších ~40 glyfov je zapečených do
   textových reťazcov v `src/lib/*.ts` (stavové značky `⚡ working`, `◷`, `▲`, …).
   Žiadna ikonová knižnica nie je v `package.json` ani v `pnpm-lock.yaml`.
2. Všetkých päť nálezov šošovky je **potvrdených**; UX-04 s korekciou: kôš nie je
   „na jeden klik“ (otvára `ConfirmDialog`) a na lokálnom desktope je skrytý až do
   hoveru — trvalo viditeľný je iba v **hub režime**, kde ho `disabled` stav prebije
   (`.icon-btn:disabled { opacity: 0.6 }` má vyššiu špecificitu než `.purge-btn { opacity: 0 }`).
3. **Korekcia schváleného rozhodnutia:** balík `lucide-svelte` je na npm od 2026-05
   označený `deprecated` („Please use @lucide/svelte instead“), verzia 1.0.1, peer
   `svelte ^3 || ^4 || ^5.0.0-next.42`. Nástupca **`@lucide/svelte`** (1.47.0,
   2026-09-17, peer `svelte ^5`, ISC, nulové runtime závislosti, 7 MB unpacked vs
   31 MB) je jediný správny výber pre Svelte 5. Návrh nižšie počíta s `@lucide/svelte`.
4. Odporúčaný tvar fixu: **žiadny generický `<Icon name="…">`** (string registry zabíja
   tree-shaking), ale sémantický modul `src/lib/icons.ts`, ktorý re-exportuje priame
   importy `@lucide/svelte/icons/<name>` pod **významovými** menami
   (`IconRefresh`, `IconRestart`, `IconRecreate`, `IconReconnect`, …). Význam → ikona sa
   tak definuje na jednom mieste a test vie vynútiť, že štyri „točiace sa šípky“ sú
   štyri rôzne ikony.
5. Šesť nových nálezov UX-29…UX-34 (duplikovaný `.icon-btn` v troch súboroch, emoji
   ignorujúce `color`, nedefinovaný `--color-error`, `cursor: progress` na disabled,
   dve rôzne ikony pre jeden „Recreate“, chýbajúce `aria-label` na 5 ikonových
   tlačidlách).
6. Odhad: jeden PR veľkosti **M** (≈250–300 riadkov diffu bez lockfile), rozdelený na
   dva commity; druhý, menší PR (S) na stavové glyfy v `.ts` reťazcoch a `fileicons.ts`
   sa odkladá na konsolidačný krok.

## Inventár glyfov

Zdroj: `grep -P "[^\x00-\x7F]"` nad `src/App.svelte` a `src/lib/*.svelte` (bez komentárov
a CSS), doplnený o `×`, `＋`, `…` a o `.ts` súbory, ktoré generujú texty. Význam je z
`title=`/`aria-label`/kontextu. Lucide meno je v tvare pre priamy import
`@lucide/svelte/icons/<kebab>` (komponent PascalCase).

### A. Ikonové tlačidlá a akcie (predmet PR-1)

| Glyf | Miesto | Význam | Lucide | Poznámka |
|---|---|---|---|---|
| `↻` | `src/lib/SidebarFilters.svelte:74-76` | Refresh zoznamu (pri `loading` sa mení na `…`) | `refresh-cw` | **1 glyf = 3 významy** (refresh / restart / reconnect), viď UX-01 |
| `↻` | `src/lib/FilesPanel.svelte:301` | Refresh Files | `refresh-cw` | text `hit ↻ to retry` na `:310` treba prepísať na „Refresh“ |
| `↻` | `src/lib/Sidebar.svelte:741` | v prázdnom stave „click ↻ to scan“ | — (text) | odkaz na glyf v próze |
| `↻` | `src/lib/SessionRowItem.svelte:314` | **Restart claude** v session | `rotate-ccw` | iný význam než Refresh |
| `↻ Restart` | `src/lib/SessionDetails.svelte:627` | Restart claude (detail) | `rotate-ccw` | |
| `↻ reconnect` | `src/lib/TerminalView.svelte:1112` | Detach + re-attach PTY | `plug-zap` | tretí význam toho istého glyfu; „Reconnect“ tlačidlo na `:1096` je bez ikony |
| `↺` | `src/lib/SessionRowItem.svelte:242` | Recreate (ghost riadok) | `recycle` | **iný glyf než živý Recreate `♻`** pre ten istý význam — UX-33 |
| `♻` | `src/lib/SessionRowItem.svelte:340` | Recreate (živý riadok) | `recycle` | emoji, render závisí od fontu (screenshot 01: sivá Apple verzia) |
| `♻ Recreate` | `src/lib/SessionDetails.svelte:655` | Recreate (detail) | `recycle` | |
| `🏷` | `src/lib/SessionRowItem.svelte:322` | Edit label | `tag` | plnofarebné žlté emoji medzi sivými glyfmi (crop screenshotu 01) |
| `🏷 Edit label` | `src/lib/SessionDetails.svelte:606` | Edit label (detail) | `tag` | |
| `🏷 friendly on/off` | `src/lib/SidebarFilters.svelte:205` | prepínač friendly mien | `tag` | ten istý glyf pre akciu aj prepínač zobrazenia |
| `✎` | `src/lib/SessionRowItem.svelte:330` | Rename tmux session | `pencil` | |
| `✎ Rename tmux session` | `src/lib/SessionDetails.svelte:618` | Rename (detail) | `pencil` | |
| `×` | `src/lib/SessionRowItem.svelte:350` | **Kill session** (destruktívne) | `circle-x` + `btn--crit` | `×` znamená v appke aj „zavrieť“ (nižšie) |
| `×` | `src/lib/SessionRowItem.svelte:306` | Remove from list (inactive agent) | `list-x` | |
| `×` | `src/lib/SessionRowItem.svelte:250` | Dismiss ghost session | `list-x` | |
| `× Kill` | `src/lib/SidebarFilters.svelte:179` | bulk Kill | `circle-x` | |
| `×` | `src/lib/SettingsDialog.svelte:351`, `:720`; `src/lib/ConversationHeader.svelte:292`; `src/lib/ConversationPanel.svelte:1380`, `:1713`; `src/lib/Toasts.svelte:20` | Close / Dismiss / Remove UI prvku | `x` | neškodné zavretie — musí vyzerať inak než Kill |
| `✕` | `src/lib/AgentPanel.svelte:135`, `:154`; `src/lib/HintLayer.svelte:72`; `src/lib/OnboardingCard.svelte:120` | Close / Dismiss | `x` | **dva rôzne „x“ glyfy** (`×` U+00D7 vs `✕` U+2715) pre to isté |
| `🗑️` | `src/lib/Sidebar.svelte:726` | Purge project state | `trash-2` | emoji ignoruje `color: var(--color-error)` — UX-30 |
| `⏏ Safe remove` | `src/lib/SessionDetails.svelte:703` | Safe remove (inšpekcia + kill) | `log-out` | Lucide nemá „eject“; alternatíva `shield-check` |
| `🩹 Repair workspace` | `src/lib/SessionDetails.svelte:637` | Repair worktree | `wrench` | |
| `→ Send prompt` | `src/lib/SessionDetails.svelte:642`; `src/lib/SidebarFilters.svelte:172` | Otvoriť composer | `send` | |
| `🔍 Review` | `src/lib/SessionDetails.svelte:646` | Spustiť review session | `search` | rovnaké ako badge review (B) |
| `⇄ Move to host…` / `⇄ Move back` | `src/lib/SessionDetails.svelte:665`, `:670` | Presun session | `arrow-left-right` | `⇄` aj v `TransferChip.svelte:37-57` (stavový text, PR-2) |
| `+` | `src/lib/Sidebar.svelte:717` | New session v projekte | `plus` | ASCII |
| `+ New session` | `src/lib/Sidebar.svelte:781` | New session (footer) | `plus` | ASCII |
| `＋ Add project…` | `src/lib/Sidebar.svelte:809` | Add project | `plus` | **fullwidth** U+FF0B — tretí tvar plusu |
| `⚡` | `src/lib/Sidebar.svelte:790` | Nová **bg session** | `bot` | `⚡` inak znamená „working“ (`attention.ts:39`, `hosts_view.ts:125`) — kolízia významov; bez `aria-label` |
| `✦` | `src/lib/AgentFab.svelte:45` | Agent FAB | `sparkles` | 20 px, jediné správne `aria-hidden` na glyfe |
| `☑` | `src/lib/SidebarFilters.svelte:112` | Tasks (fleet-wide) | `list-checks` | |
| `☑ select` | `src/lib/SidebarFilters.svelte:158` | Select mode | `square-check` | ten istý glyf ako Tasks, iný význam |
| `⚙` | `src/lib/SidebarFilters.svelte:120` | Settings | `settings` | |
| `‹` / `›` | `src/lib/SidebarFilters.svelte:84`; `src/App.svelte:616`, `:639`, `:651` | Hide/Show sidebar, Hide/Show details pane | `panel-left-close`/`panel-left-open`, `chevron-left`/`chevron-right` | `.strip-expand` používa `writing-mode: vertical-rl` — SVG sa nerotuje, treba `transform` alebo ikonu bez textu |
| `⧉` | `src/lib/CopyButton.svelte:38` | Copy | `copy` → po skopírovaní `check` | už na `.btn--icon` |
| `⌾` | `src/lib/ConversationPanel.svelte:1745` | Attach files | `paperclip` | už na `.btn--icon` |
| `↑` / `…` | `src/lib/ConversationPanel.svelte:1754` | Send (pri odosielaní `…`) | `arrow-up` / `loader-circle` | |
| `↑` / `↓` | `src/lib/ConversationHeader.svelte:290-291` | Find prev/next | `chevron-up`/`chevron-down` | |
| `↓ N new` | `src/lib/ConversationPanel.svelte:1598` | Skok na koniec | `arrow-down` | |
| `⏎ Press Enter` | `src/lib/ConversationPanel.svelte:1648` | Poslať Enter do REPL | `corner-down-left` | |
| `🎲` | `src/lib/NewSessionDialog.svelte:711` | Roll a new name | `dices` | |
| `⎇` | `src/lib/CommitGraph.svelte:120` | Create branch from commit | `git-branch-plus` | bez `aria-label` (iba `title`) |
| `⤓` | `src/lib/CommitGraph.svelte:125` | Checkout commit (detached) | `arrow-down-to-line` | bez `aria-label` |
| `← Back …` | `src/lib/BackgroundDetail.svelte:51`; `src/lib/FilesPanel.svelte:361` | Späť | `arrow-left` | |
| `theme: dark` | `src/lib/Sidebar.svelte:791-799` | Cyklovanie témy | `sun-moon` / `sun` / `moon` podľa stavu | UX-11 |

### B. Odznaky druhu / stavu v riadku (predmet PR-1, iba SVG náhrada)

| Glyf | Miesto | Význam | Lucide | Poznámka |
|---|---|---|---|---|
| `🤖` | `src/lib/SessionRowItem.svelte:272`; `src/lib/SidebarFilters.svelte:193` | background agent / prepínač bg | `bot` | test `Sidebar.test.ts:943` hľadá text `🤖` |
| `🔍` | `src/lib/SessionRowItem.svelte:266` | review session | `search` | test `Sidebar.test.ts:931` hľadá text `🔍` |
| `▶` | `src/lib/SessionRowItem.svelte:269` | shell session | `terminal` | bez `role="img"`/`aria-label` (susedné badge ich majú) |
| `🔗N` | `src/lib/SessionRowItem.svelte:263` | N related sessions | `link` | testy idú cez `data-testid`, prežijú |
| `⚠ stuck: …` | `src/lib/SessionRowItem.svelte:214`, `:283` | stuck chip | `triangle-alert` | |
| `⚠ Needs you (N)` | `src/lib/SidebarFilters.svelte:148` | triage filter | `triangle-alert` | svieti aj pri 0 (UX-08, iná šošovka) |
| `≡ details on/off` | `src/lib/SidebarFilters.svelte:215` | prepínač detailov | `rows-3` alebo `list` | |
| `▾` / `▸` | `src/lib/Sidebar.svelte:704`, `:763`; `src/lib/FileList.svelte:215`; `src/lib/ConversationHeader.svelte:330`, `:354`; `src/lib/ConversationPanel.svelte:1676`; `src/lib/UsageBlock.svelte:160` | caret rozbaľovania | `chevron-down` / `chevron-right` | `Sidebar` rotuje `.caret.collapsed` cez CSS — SVG to zvládne rovnako |
| `●` / `○` | `src/lib/HostsList.svelte:167`; `src/lib/HostDetail.svelte:152`; `src/lib/BranchList.svelte:40` | online / offline / current branch | `circle` (fill) / `circle` | alebo ponechať ako CSS bodku (`.status-dot` už existuje) |
| `⚠` | `src/App.svelte:832`; `src/lib/HostsView.svelte:400`; `src/lib/SettingsDialog.svelte:377`, `:423`, `:464`; `src/lib/PromptComposer.svelte:140` | výstraha | `triangle-alert` | |
| `✓` / `✗` / `✕` | `src/lib/BulkPromptDialog.svelte:53-55`; `src/lib/PromptComposer.svelte:143-146`; `src/lib/OnboardingCard.svelte:140`; `src/lib/ToolLine.svelte:138`; `src/lib/SubagentBlock.svelte:53`; `src/lib/SessionDetails.svelte:592` | ok / fail | `check` / `x` | `ToolLine.test.ts:69` hľadá `✕` v `textContent` |
| `◐` | `src/lib/OnboardingCard.svelte:140` | busy | `loader-circle` + `animate-spin` | |
| `○ ◌ ✓ ✕` | `src/lib/TransferSheet.svelte:414-418` | kroky presunu (CSS `content:`) | `circle`/`loader-circle`/`check`/`x` | v CSS `::before`, treba presunúť do markupu |

### C. Stavové glyfy v textových reťazcoch (`.ts`) — PR-2, mimo tejto iterácie

| Glyf | Miesto | Význam | Lucide (ak sa raz nahradí) |
|---|---|---|---|
| `⚡ working`, `⏸ blocked`, `✓ done`, `✗ failed`, `■ stopped` | `src/lib/attention.ts:39-43` | `claude_status` label | `zap`, `pause`, `check`, `x`, `square` |
| `✓ CI` / `✗ CI` | `src/lib/attention.ts:345-347` | CI stav | `check`/`x` |
| `⚡N ⏸N` | `src/lib/hosts_view.ts:125-126` | počty sessions v hostoch | `zap`/`pause` |
| `▲ △ ■ ◷ ⏸ ⚠ 🔑 ○` | `src/lib/account_usage.ts:104-108`, `:287-291`, `:454-595`; `src/lib/usage_glance.ts:169-325`; `src/lib/hosts_view.ts:189-209` | usage severity / stale / login | `triangle-alert`, `square`, `clock`, `key-round`, `circle` |
| `• ✕ ⏸ ✓` | `src/lib/conversation.ts:306-309` | notification mark | `dot`, `x`, `pause`, `check` |
| `PR↗` | `src/lib/SessionRowItem.svelte:405` | externý odkaz | `external-link` |
| `↑N ↓N` | `src/lib/BranchList.svelte:42` | ahead/behind | text, ponechať |
| `⌘ ↵ ⇧ ↑↓` | `QuickSwitcher.svelte:44,190-192`, `ConversationPanel.svelte:1746`, `HostsView.svelte:360-362`, `app_views.ts:92-102` | klávesové skratky | **ponechať** — sú to symboly kláves, nie ikony |

Tieto reťazce sú pinnuté v ~60 asseroch (`account_usage.test.ts`, `usage_glance.test.ts`,
`UsageBlock.test.ts`, `HostsView.test.ts:161`, `hosts_view.test.ts:86-101`, `App.hosts.test.ts:457-478`,
`conversation.test.ts:648-651`, `TransferChip.test.ts:81-139`). Nahradiť ich SVG znamená zmeniť
dátový model (text → `{glyph, text}` už čiastočne existuje v `account_usage.ts`) — samostatný PR.

### D. Ikony súborov — `src/lib/fileicons.ts`

45 emoji per typ súboru (`🐳 📦 🦀 🐍 …`), komentár v hlavičke výslovne: „Emoji are chosen over an
icon library to keep the frontend dependency-free“. Toto rozhodnutie padá s prijatím Lucide;
náhrada (`file-code`, `file-json`, `file-text`, `folder`/`folder-open`, …) je PR-3, mechanicky
jednoduchý (jedna mapa), ale vizuálne stratí rozlíšenie jazykov farbou — treba rozhodnutie
owner-a, či to chce.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-01 | **Potvrdené, rozšírené.** `↻` nesie tri významy: Refresh (`SidebarFilters.svelte:76`, `FilesPanel.svelte:301`), Restart claude (`SessionRowItem.svelte:314`, `SessionDetails.svelte:627`), Reconnect PTY (`TerminalView.svelte:1112`). Navyše ghost Recreate používa `↺` (`SessionRowItem.svelte:242`) — štvrtá „točiaca šípka“. Loading stav je `…` (`SidebarFilters.svelte:75`), rovnako ako `…` v Send (`ConversationPanel.svelte:1754`) a Fetch/Pull/Push (`RemoteToolbar.svelte:30-44`) — appka nemá spinner. | crop screenshotu 01: v riadku je `↻` sivý tenký glyf, na hover panely nerozoznateľný od `↺`. |
| UX-02 | **Potvrdené.** Hover akcie `↻ 🏷 ✎ ♻ ×` (`SessionRowItem.svelte:308-351`): `🏷` je plnofarebné Apple emoji (žltý štítok), `✎` sa na macOS renderuje ako farebná ceruzka, `♻` a `↻` sú monochromatické textové glyfy, `×` je typografický znak. Päť tlačidiel, tri renderovacie režimy, žiadna spoločná veľkosť (`.icon-btn.small { font-size: 0.85rem }` — emoji s tým nespolupracuje). Bez `title` nečitateľné. | `SessionDetails.svelte:606-655` opakuje tú istú sadu s textom. |
| UX-03 | **Potvrdené.** 62 výskytov v markupu (tabuľky A+B), ďalších ~40 v `.ts` (C) a 45 v `fileicons.ts` (D). Farebnosť emoji sa nedá tematizovať: `.purge-btn { color: var(--color-error…) }` na `🗑️` nemá efekt, `.bg-badge`/`.review-badge` (`SessionRowItem.svelte:484-486`) nastavujú iba `font-size`. Light téma dostane tie isté farebné emoji na bielom. | crop filtrov: `🤖 bg off` farebný robot, `🏷 friendly on` farebný štítok, `☑`/`≡`/`⚠` sivý text. |
| UX-04 | **Potvrdené s korekciou.** (a) *Nie je* to jeden klik: `onclick` nastaví `pendingPurge` a otvorí `<ConfirmDialog danger>` (`Sidebar.svelte:898-909`). (b) *Nie je* plnofarebný: Apple renderuje `🗑️` sivo; CSS tint `color: var(--color-error, #f44336)` (`Sidebar.svelte:1076`) emoji ignoruje a `--color-error` nie je v `src/app.css` definovaný (fallback `#f44336` sa nikdy neuplatní na glyf). (c) Trvalá viditeľnosť je **artefakt hub režimu**: `.purge-btn { opacity: 0 }` (`:1073`) by kôš skryl do hoveru, ale `purge_project` je `LocalOnly` (`src/lib/hub_verdicts.generated.json:53`) → tlačidlo je `disabled` → `.icon-btn:disabled { opacity: 0.6; cursor: progress }` (`:951`, špecificita (0,2,0) > (0,1,0)) ho natrvalo zobrazí. Na lokálnom desktope je nález L; v hub režime je to mŕtve tlačidlo s kurzorom „čakaj“ vedľa každého projektu — viď UX-31. | crop hlavičky projektu: kôš viditeľný pri všetkých 4 projektoch bez hoveru. |
| UX-11 | **Potvrdené.** `theme: {$theme}` je `<button class="theme-toggle">` (`Sidebar.svelte:791-799`) so 100 % šírkou, `font-size: 0.75rem`, `color: var(--fg-muted)`, border `var(--border)` (`:1082-1092`) — vizuálne identický s informačným `.tag`. Jediná indikácia klikateľnosti je `cursor: pointer` a `title`. Cyklus auto→light→dark bez náhľadu ďalšieho stavu. | screenshot 01 vľavo dole. |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-29 | M | **`.icon-btn` je definovaný trikrát** s takmer identickým telom (`padding: 0.25rem 0.5rem; font-size: 0.9rem; min-width: 1.6rem; border-radius: 5px`) a ďalšie 5 lokálnych variantov (`.refresh`, `.reconnect`, `.strip-expand`, `.center-collapse`, `.ghost`) — zatiaľ čo `src/lib/controls.css:137-146` už má kanonický `.btn--icon` (24×24, `font-size: 14px`) a jeho hlavička výslovne varuje pred „per-component scoped copies“. Iba Conversation/CopyButton ho používajú. Ikonový PR má tieto kópie zrušiť, nie do nich vkladať SVG. | `src/lib/SessionRowItem.svelte:431-455`, `src/lib/Sidebar.svelte:935-958`, `src/lib/SidebarFilters.svelte:250-266`, `src/lib/FilesPanel.svelte:478-485`, `src/lib/TerminalView.svelte:1332-1340`, `src/App.svelte:910-921`, `src/lib/SessionDetails.svelte:998-1006` |
| UX-30 | L | **`--color-error` nie je definovaný** v `src/app.css` (existujú `--usage-warn`, `--usage-crit`, `--accent`). `.purge-btn` a `.icon-btn.danger:hover { color: #e64a4a }` (`SessionRowItem.svelte:455`), `.err { color: #e64a4a }` (`:488`) používajú hard-coded červené namiesto `var(--usage-crit)`, ktoré `controls.css` už viaže na `.btn--crit`. | `grep -n "color-error" src/app.css` → 0 výsledkov |
| UX-31 | M | **`cursor: progress` na disabled ikonových tlačidlách.** Všetky tri `.icon-btn:disabled` kópie nastavujú `cursor: progress` — v hub režime je tak `LocalOnly` kôš (a iné odmietnuté akcie) prezentovaný ako „prebieha operácia“, hoci je trvalo nedostupný. `controls.css:52-58` používa `cursor: default` + `opacity: 0.55`. | `src/lib/Sidebar.svelte:951`, `SessionRowItem.svelte:447`, `SidebarFilters.svelte:266`; screenshot 01 (hub režim) |
| UX-32 | L | **Dva glyfy „x“ pre zavretie** (`×` U+00D7 v 7 miestach, `✕` U+2715 v 4 miestach) a ten istý `×` pre destruktívny Kill (`SessionRowItem.svelte:350`, `SidebarFilters.svelte:179`). Zavrieť panel a zabiť session vyzerá rovnako; Kill nemá `btn--crit` tón v pokoji, iba `.danger:hover`. | tabuľka A |
| UX-33 | L | **Jeden význam, dve ikony:** Recreate je `↺` na ghost riadku (`SessionRowItem.svelte:242`) a `♻` na živom riadku (`:340`) aj v detaile (`SessionDetails.svelte:655`); `☑` je Tasks (`SidebarFilters.svelte:112`) aj Select (`:158`); `⚡` je „working“ (`attention.ts:39`) aj „nová bg session“ (`Sidebar.svelte:790`); `🏷` je akcia Edit label aj prepínač friendly. Mapa význam→ikona musí byť injektívna oboma smermi. | tabuľky A–C |
| UX-34 | M | **Ikonové tlačidlá bez `aria-label`** (iba `title`, prístupné meno je samotný glyf): Refresh `SidebarFilters.svelte:74`, nová bg session `Sidebar.svelte:784-790`, create branch / checkout `CommitGraph.svelte:117-125`, shell badge `SessionRowItem.svelte:269` (bez `role="img"`). Glyfy v tlačidlách s `aria-label` nie sú `aria-hidden` (okrem `AgentFab.svelte:45`), takže VoiceOver číta „Restart clockwise open circle arrow“. Súvisí s UX-28 (šošovka 17). | `grep -B6 ">↻<\|>⚡<\|>⎇<\|>⤓<"` |

## Návrh riešenia (PR plán)

### Rozhodnutia, ktoré PR presadzuje

1. **Knižnica: `@lucide/svelte`** (nie deprecated `lucide-svelte`). Peer `svelte ^5` ✔ (repo má
   `^5.0.0`), `moduleResolution: bundler` ✔ (subpath `exports` `./icons/*`), ISC licencia,
   nulové závislosti → `pnpm-lock.yaml` +1 balík. Bundle: priamy import
   `@lucide/svelte/icons/refresh-cw` je jeden malý Svelte komponent s `iconNode` poľom
   (~0,5–1 KB min pred gzipom); ~40 ikon ≈ 30 KB / ~9 KB gzip. Barrel import
   `import { RefreshCw } from '@lucide/svelte'` sa vo Vite build tiež tree-shakuje
   (`sideEffects: false`), ale v dev serveri načíta ~1 600 modulov — preto **iba subpath importy**.
2. **Žiadny `<Icon name="…">` so string registry.** Namiesto toho sémantický modul
   `src/lib/icons.ts`:
   ```ts
   // Význam → ikona, na jednom mieste. Štyri „točiace šípky“ sú štyri rôzne ikony.
   export { default as IconRefresh }   from '@lucide/svelte/icons/refresh-cw';
   export { default as IconRestart }   from '@lucide/svelte/icons/rotate-ccw';
   export { default as IconRecreate }  from '@lucide/svelte/icons/recycle';
   export { default as IconReconnect } from '@lucide/svelte/icons/plug-zap';
   export { default as IconLabel }     from '@lucide/svelte/icons/tag';
   export { default as IconRename }    from '@lucide/svelte/icons/pencil';
   export { default as IconKill }      from '@lucide/svelte/icons/circle-x';
   export { default as IconClose }     from '@lucide/svelte/icons/x';
   export { default as IconRemoveRow } from '@lucide/svelte/icons/list-x';
   export { default as IconPurge }     from '@lucide/svelte/icons/trash-2';
   // … (tabuľky A, B)
   ```
   Pomenované re-exporty ESM sú tree-shakované rovnako ako priame importy. Komponenty
   importujú `import { IconRestart } from './icons'` a renderujú
   `<IconRestart size={14} aria-hidden="true" />`. (Lucide Svelte props: `size`, `color`,
   `strokeWidth`, `absoluteStrokeWidth`, `class`, rest props → `<svg>`; overiť v
   `node_modules/@lucide/svelte/dist/Icon.svelte.d.ts` pri implementácii.)
3. **Sizing tokeny** do `src/app.css` vedľa `--control-*`:
   | Token | Hodnota | Použitie |
   |---|---|---|
   | `--icon-sm` | `12px` | inline v texte a chipoch (`.tag`, `.btn--chip`), = `--control-font` |
   | `--icon-md` | `14px` | default v `.btn--icon` (24 px box), nahrádza `font-size: 14px` v `controls.css:140` |
   | `--icon-lg` | `20px` | FAB (48 px), prázdne stavy |
   Stroke: pri 14 px sa default `stroke-width: 2` škáluje na ~1,2 px a na non-retina sa
   stráca. Nastaviť **`absoluteStrokeWidth` + `strokeWidth={1.5}`** globálne cez CSS
   (`.btn svg.lucide { stroke-width: 1.5 }`; Lucide renderuje `vector-effect` len pri
   `absoluteStrokeWidth`), alebo obaliť raz vo `icons.ts` — rozhodnúť pri implementácii,
   preferujem CSS (jedno pravidlo, žiadny wrapper komponent).
4. **Theming** je zadarmo: Lucide SVG má `stroke="currentColor"`, takže `.btn { color:
   var(--control-fg-quiet) }` a `.btn--quiet:hover { color: var(--control-fg) }` v
   `controls.css:80-90` už ikony tematizujú; Kill/Purge dostanú `.btn--crit` (existuje,
   `controls.css:148-165`, viazané na `--usage-crit`). Light téma padá z toho istého.
5. **Tooltipy zostávajú:** každé ikonové tlačidlo si ponechá `title=` (často nesie aj
   `*Blocked` dôvod z hub režimu — to je funkčný text, nie dekorácia) a dostane/ponechá
   `aria-label`; SVG dostane `aria-hidden="true"`. Tlačidlá s textom (`SessionDetails`
   `.ghost`) dostanú ikonu pred text a prístupné meno zostáva text.

### Súbory a poradie (jeden PR, dva commity)

**Commit 1 — základ (bez zmeny vizuálu ostatných komponentov):**

| # | Súbor | Zmena |
|---|---|---|
| 1 | `package.json`, `pnpm-lock.yaml` | `pnpm add @lucide/svelte` |
| 2 | `src/app.css:26-44` | `--icon-sm/md/lg` tokeny |
| 3 | `src/lib/controls.css:137-146` | `.btn--icon` → `> svg { width/height: var(--icon-md) }`, zrušiť `font-size: 14px`; `.btn svg.lucide { stroke-width: 1.5; flex: 0 0 auto }`; `.btn--icon.btn--sm` (20 px box pre hover akcie v riadku, `--icon-sm`) |
| 4 | `src/lib/icons.ts` (nový) | sémantické re-exporty podľa tabuliek A+B |
| 5 | `src/lib/icons.test.ts` (nový) | testy nižšie |

**Commit 2 — migrácia (nahradiť glyf, zrušiť lokálny `.icon-btn`):**

| # | Súbor | Riadky | Zmena |
|---|---|---|---|
| 6 | `src/lib/SessionRowItem.svelte` | `:242`, `:250`, `:263-272`, `:306-350`, CSS `:431-455`, `:484-486` | 5 hover akcií + ghost akcie na `btn btn--icon btn--quiet btn--sm`, Kill/Dismiss `btn--crit`; badge `🤖🔍▶🔗` na `<IconBot size=12>` s `role="img"` + `aria-label` (shell badge ich dostane); zmazať `.icon-btn*`, `.review-badge`/`.bg-badge` font-size |
| 7 | `src/lib/SidebarFilters.svelte` | `:74-84`, `:112`, `:120`, `:148`, `:158`, `:172-179`, `:193-215`, CSS `:250-266` | Refresh (loading → `IconLoader class="spin"`), Hide sidebar, Tasks, Settings, Needs you, select, bg/friendly/details chipy; `aria-label="Refresh"`; zmazať `.icon-btn` |
| 8 | `src/lib/Sidebar.svelte` | `:704`, `:717`, `:726`, `:741`, `:763`, `:781-799`, `:809`, CSS `:935-958`, `:1073-1092` | caret → `IconChevronDown` (CSS rotácia zostáva), `+` → `IconPlus`, kôš → `IconPurge` na `btn--icon btn--crit`, bg session → `IconBot` + `aria-label`, `＋` → `IconPlus`, theme → `IconSunMoon/Sun/Moon` + text; text `click ↻ to scan` → `click Refresh to scan`; `.purge-btn` opacity pravidlo prepísať tak, aby `disabled` **neprebil** skrytie (`.proj-row:not(:hover) .purge-btn:not(:focus-visible) { opacity: 0 }` s vyššou špecificitou), zmazať `.icon-btn`, `.theme-toggle` → `btn btn--quiet` |
| 9 | `src/lib/SessionDetails.svelte` | `:592`, `:606-703` | ikona pred text v `.ghost` tlačidlách (`IconLabel`, `IconRename`, `IconRestart`, `IconRepair`, `IconSend`, `IconReview`, `IconRecreate`, `IconMove`, `IconSafeRemove`); `.ghost` → `btn btn--quiet is-bounded` |
| 10 | `src/lib/FilesPanel.svelte` | `:301`, `:310`, `:361`, CSS `:478-485` | Refresh → `IconRefresh`, text `hit ↻ to retry` → „hit Refresh to retry“, Back → `IconArrowLeft`; zmazať `.refresh` |
| 11 | `src/lib/TerminalView.svelte` | `:1096`, `:1112`, CSS `:1332-1340` | `IconReconnect` v oboch reconnect tlačidlách; `.reconnect` → `btn btn--quiet is-bounded` |
| 12 | `src/App.svelte` | `:616`, `:639`, `:651`, `:832`, CSS `:910-921` | `IconPanelLeftOpen/Close`, `IconChevronLeft/Right`, `IconAlert`; `.strip-expand` zrušiť `writing-mode` (SVG sa neotočí), nechať vertikálny pás |
| 13 | `src/lib/AgentFab.svelte:45`, `src/lib/AgentPanel.svelte:135,154`, `src/lib/CopyButton.svelte:38`, `src/lib/ConversationPanel.svelte:1380,1598,1648,1713,1745,1754`, `src/lib/ConversationHeader.svelte:290-292,330,354`, `src/lib/NewSessionDialog.svelte:711`, `src/lib/CommitGraph.svelte:120,125` (+ `aria-label`), `src/lib/HintLayer.svelte:72`, `src/lib/OnboardingCard.svelte:120,140`, `src/lib/Toasts.svelte:20`, `src/lib/SettingsDialog.svelte:351,720`, `src/lib/HostsView.svelte:400`, `src/lib/TransferSheet.svelte:414-418` (CSS `content:` → markup), `src/lib/FileList.svelte:215`, `src/lib/UsageBlock.svelte:160` | jednoriadkové náhrady |

Poradie je zvolené tak, aby commit 1 bol zelený sám (nič ho nepoužíva) a commit 2 išiel
od najväčších hriešnikov (riadok session, filtre) k jednoriadkovým náhradám — ak sa PR
musí zmenšiť, položky 9–13 sa dajú odložiť bez nekonzistencie v sidebare.

**Mimo tohto PR** (konsolidačný krok): tabuľka C (stavové glyfy v `.ts` reťazcoch —
vyžaduje `{glyph, text}` model a prepis ~60 asserov), tabuľka D (`fileicons.ts`),
`PR↗`, `TransferChip.svelte` `⇄`. Tie glyfy sú *text so značkou*, nie ikonové tlačidlá,
a screen-readery ich čítajú prijateľne.

### Testy, ktoré sa zlomia (a čo s nimi)

| Test | Prečo | Úprava |
|---|---|---|
| `src/lib/Sidebar.test.ts:931` `getByText('🔍')` | badge sa stane SVG | `getByRole('img', { name: 'review session' })` (aria-label už existuje) |
| `src/lib/Sidebar.test.ts:943` `queryByText('🤖')` | badge sa stane SVG | `queryByRole('img', { name: 'background agent' })` |
| `src/lib/ToolLine.test.ts:69` `toContain('✕')` | iba ak sa nahradí `ToolLine.svelte:138` (položka 13) | `within(row).getByTitle('Failed')` |
| `src/lib/controls.test.ts` | číta `controls.css` regexom; pridanie pravidiel neláme, ale zmena `.btn--icon` bloku môže | skontrolovať po úprave `:137-146` |
| `src/lib/HostsView.test.ts:174` `not.toMatch(/[×🚫]/u)` | **nezlomí sa** (SVG nemá textContent), ale stratí zmysel | doplniť `expect(row.querySelector('svg.lucide-x')).toBeNull()` |
| `src/lib/hub_disabled.test.ts:363` | filtruje podľa `title === REASON`, glyf nerieši | bez zmeny |
| `src/lib/TransferChip.test.ts:81-139`, `account_usage.test.ts`, `usage_glance.test.ts`, `UsageBlock.test.ts`, `hosts_view.test.ts`, `App.hosts.test.ts:457-478`, `conversation.test.ts:648-651` | pinnujú glyfy z tabuľky C | **nedotýkať sa** v tomto PR |

Ostatné testy vyberajú tlačidlá cez `data-testid` (`sidebar-refresh`, `recreate-live`,
`edit-label`, `rename-tmux`, `purge-project`, `ghost-recreate`, …) alebo cez `aria-label`
(`AgentFab.test.ts:21` `name: /agent/i`) — prežijú bez zmeny. `svelte-check` prejde,
`@lucide/svelte` má `.d.ts` pre každý subpath.

## Akceptačné testy

Nové `src/lib/icons.test.ts` (Vitest + `@testing-library/svelte`, jsdom renderuje SVG bez
problémov):

1. **Injektívnosť mapy význam → ikona.** Import všetkých exportov z `icons.ts`, render
   každého, načítať `svg.getAttribute('class')` (Lucide dáva `lucide lucide-<name>`) a
   overiť, že `IconRefresh`, `IconRestart`, `IconRecreate`, `IconReconnect` majú štyri
   rôzne triedy; všeobecne `new Set(classes).size === exports.length` s explicitným
   allowlistom zdieľaní (napr. `IconLabel` a `IconFriendlyToggle` smú byť `tag`, ak sa
   to rozhodne).
2. **Žiadny surový glyf v markupe.** Ako `controls.test.ts`: `readFileSync` nad
   `src/App.svelte` + `src/lib/*.svelte`, odstrániť `<style>…</style>` a `<!-- -->`,
   regex `/[↻↺♻🏷✎🤖🔍▶⚡✦☑⚙‹›⧉⌾🎲⎇⤓🗑✕⏏🩹⇄＋]/u` → prázdny výsledok. Allowlist pre
   klávesové symboly (`⌘ ↵ ⇧ ⏎`) a pre súbory z tabuľky C, kým sa nespraví PR-2. Toto je
   regresná brzda proti návratu emoji.
3. **Prístupnosť ikonových tlačidiel.** Render `SessionRowItem` s live session: každé
   `button` v `.row-actions` má neprázdny `aria-label`, obsahuje presne jeden `svg` s
   `aria-hidden="true"` a `title` nie je prázdny. Render `SidebarFilters`: `sidebar-refresh`
   má `aria-label="Refresh"`; `new-bg-session-btn` má `aria-label`.
4. **Loading stav je spinner, nie `…`.** `SidebarFilters` s `loading=true`: tlačidlo
   `sidebar-refresh` obsahuje `svg.lucide-loader-circle` a jeho `textContent` neobsahuje `…`.
5. **Kôš v hub režime.** `Sidebar` s `purgeProjectBlocked !== null`: `purge-project` je
   `disabled`, má `cursor` iný než `progress` (čítať `getComputedStyle`; jsdom vracia
   inline/štýlové hodnoty z `<style>` blokov len čiastočne — ak nespoľahlivé, test na
   `controls.css` regex `.btn:disabled[^}]*cursor: default`), a nemá triedu `icon-btn`.
6. **Tokeny.** `readFileSync('src/app.css')` obsahuje `--icon-sm: 12px`, `--icon-md: 14px`,
   `--icon-lg: 20px`; `controls.css` `.btn--icon` neobsahuje `font-size: 14px`.
7. **`svelte-check` = 0 chýb** (`npx svelte-check --tsconfig ./tsconfig.json`) a
   `npx vitest run` celé, nie filtrované (memory: filtrované per-task behy schovali
   červený test).
8. **Vizuálne (manuálne, jeden screenshot podľa README §1):** riadok session na hover —
   päť ikon rovnakej hrúbky a farby `--control-fg-quiet`, Kill červený; light téma bez
   farebných emoji; Retina aj 1× (stroke 1,5 px absolútny).

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| Commit 1 (dep, tokeny, `icons.ts`, test) | S | ~120 riadkov + lockfile |
| Commit 2 položky 6–8 (riadok, filtre, sidebar) | M | ~150 riadkov (väčšina je mazanie CSS kópií) |
| Commit 2 položky 9–13 (detail, files, terminal, app, drobné) | S | ~80 riadkov |
| **PR-1 spolu** | **M** | ≈300–350 riadkov bez lockfile; nad limitom „~300“ z README §5 — ak treba, položky 9–13 ako PR-1b |
| PR-2 stavové glyfy v `.ts` (tabuľka C) | M | ~60 asserov + `{glyph,text}` model |
| PR-3 `fileicons.ts` (tabuľka D) | S | jedna mapa, rozhodnutie o farbe |

**Otázky pre owner-a:**

1. Súhlas s **`@lucide/svelte`** namiesto deprecated `lucide-svelte` (mení text
   rozhodnutia (a) v README §5)?
2. Kill vs Close: má Kill dostať odlišnú ikonu (`circle-x`, červený tón v pokoji), alebo
   stačí ten istý `x` s `btn--crit`? Návrh: odlišná ikona.
3. `fileicons.ts` — chce owner stratiť farebné rozlíšenie jazykov (Lucide je monochróm),
   alebo emoji v strome súborov ponechať ako vedomú výnimku?
4. Má Safe remove ikonu `log-out` alebo `shield-check` (Lucide nemá eject)?

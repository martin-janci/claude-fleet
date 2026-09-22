# Konsolidácia 01 — po iteráciách 1–5

**Krok protokolu:** README §5 bod 3 (po 4–5 iteráciách jeden konsolidačný krok: dedup, priorita,
rozpad na PR; jeden klaster = jeden PR, max ~300 riadkov diffu) · **Vstup:**
`docs/ux/2026-09-21-audit/README.md`, `iterations/01-icon-system.md` … `05-hub-parity-files-git.md`,
kód na `e8591e45` (iba na overenie čísel: `BUDGET_BYTES = 57_700` v
`crates/fleet-core/src/mcp/tools/tests.rs:2357`, `hub_verdicts.generated.json` 70 LocalOnly / 39 Routed,
`src/lib/HubScopeNote.svelte` neexistuje) · **Režim:** read-only, žiadne zmeny kódu, žiadny `cargo`.

## Zhrnutie

1. Register má **67 nálezov**: 28 z auditu, 39 nových z iterácií (UX-29…UX-67). Z 28 auditových
   sa 14 dostalo pod šošovku: 9 potvrdených bez zmeny, 5 potvrdených **s korekciou premisy**
   (UX-04, 06, 17, 18, 23), 0 úplne vyvrátených. Zvyšných 14 čaká na šošovky 6–17.
2. Tri korekcie menia, čo treba stavať: hub **nemá** kam „do it on the hub“ (UX-42 ≡ UX-49 — hub
   nemá settings tool a katalóg nikdy nenačíta), `bg:<uuid>` riadky **nie sú operátor** (UX-06) a
   git zápisy v hub režime **nezlyhávajú**, iba ticho nefungujú (UX-18 → UX-59).
3. Päť vecí sa rozhoduje **raz**, nie per PR (§2): `@lucide/svelte`; stĺpec
   `friendly_name_source`; jedna prístupová politika pre hub tools (štvorstupňová: readonly · full
   · full+trusted · master); `BUDGET_BYTES` **63 800** nastavený raz; zdieľaný `HubScopeNote` +
   helper pre stavy staršieho hubu.
4. Fronta má **20 PR** (`UXPR-01…20`), ≈3 300 riadkov diffu bez testov a generovaných súborov
   (+≈2 200 testov); 18 v hlavnej fronte, 2 odložené (stavové glyfy v `.ts`, `fileicons.ts`).
   Kritická cesta je reťaz hub-parity v Ruste (`UXPR-09 → 10 → 11 → 12 → 13`), lebo všetky
   editujú `guard.rs`, `tests.rs`, `verdicts.rs`, `remote.rs` a generované goldeny. Ikony,
   pomenovanie a Svelte panely bežia paralelne.
5. Pre vlastníka ostáva **28 otázok** v piatich témach (§4), každá s odporúčaním — odpoveď
   „default“ na celý blok je platná.

## 1. Register nálezov UX-01…UX-67

Stav: **potvrdené** (šošovka potvrdila bez zmeny) · **opravené** (potvrdené, ale premisa alebo
detail auditu boli nesprávne — text nižšie je už opravený) · **vyvrátené** · **nové** (vzniklo v
iterácii) · **neoverené** (žiadna šošovka ho ešte nespracovala; závažnosť je auditová). Závažnosť
preberá verdikt iterácie, kde ho zmenila. `↔` = prekryv/duplikát; `≡` = ten istý vzor. PR podľa §3.

| ID | Sev | Stav | Iter. | Zhrnutie | PR |
|---|---|---|---|---|---|
| UX-01 | H | potvrdené | 01 | `↻` = Refresh / Restart / Reconnect, plus `↺` ghost Recreate — štyri „točiace šípky“; loading je `…`, appka nemá spinner ↔ UX-33 | UXPR-01, 02, 03 |
| UX-02 | H | potvrdené | 01 | hover akcie `↻ 🏷 ✎ ♻ ×` — tri renderovacie režimy, žiadna spoločná veľkosť, nečitateľné bez tooltipu | UXPR-02 |
| UX-03 | M | potvrdené | 01 | 62 glyfov v markupe + ~40 v `.ts` reťazcoch + 45 v `fileicons.ts`; farba emoji sa nedá tematizovať | UXPR-01–03, 19, 20 |
| UX-04 | L | opravené | 01 | kôš **nie je** one-click (otvára `ConfirmDialog`) ani plnofarebný; trvalo viditeľný je iba v **hub režime**, lebo `.icon-btn:disabled { opacity: .6 }` prebije `.purge-btn { opacity: 0 }` ↔ UX-31 | UXPR-02 |
| UX-05 | C | potvrdené | 02 | rozšírené: pomenúvajú aj systémové prompty (↔ UX-35); prvý junk prompt vyhrá navždy, lebo ochrana je len rovnosť s vetvovým defaultom (↔ UX-37) | UXPR-04, 05 |
| UX-06 | M | opravené | 02 | `bg:<uuid>` sú `kind='external'` riadky (agenti bez tmux nároku), **nie operátor**; operátor je `work` session „fleet operator“ a na screenshote 03 nebeží; riziko duplikátu fleet session v Outside fleet (`find_by_unique_cwd`) | UXPR-04 (P5), 06 |
| UX-07 | M | potvrdené | 02 | tmux meno 3× na obrazovke; detail má hierarchiu **naopak** (h2 = tmux), Hosts detail ignoruje prepínač ↔ UX-40 | UXPR-06 |
| UX-08 | H | neoverené | — | štyri riadky filter-chipov, `Needs you (0)` s výstrahou | šošovka 8 |
| UX-09 | M | neoverené | — | 8 metadát v riadku bez hierarchie | šošovka 9 |
| UX-10 | M | neoverené | — | select mode bez bulk lišty | šošovka 10 |
| UX-11 | L | potvrdené | 01 | `theme: dark` je tlačidlo vizuálne identické s `.tag` | UXPR-02 |
| UX-12 | L | neoverené | — | dve tlačidlá „vytvor“ bez spoločnej logiky — **v README §5 nie je priradené žiadnej šošovke**; patrí do 8 | šošovka 8 |
| UX-13 | H | neoverené | — | panel „agent is not running“ vs. FAB composer — pozn.: iterácia 02 zistila, že operátor na screenshote **nebežal** (panel hlásil pravdu); šošovka 11 má závažnosť prehodnotiť | šošovka 11 |
| UX-14 | H | neoverené | — | panel `position: fixed` prekrýva terminál | šošovka 11 |
| UX-15 | M | potvrdené | 02 | chip aj prefix agenta nesú auto meno „yes“; `tmux_name` sa agentovi nepovie | UXPR-06 |
| UX-16 | C | potvrdené | 03 | rozšírené: 3× ten istý odsek + 2 pod Control API + 4 odseky prózy v Hub sekcii; test `SettingsDialog.hub.test.ts:267-278` dnešný stav pinuje ↔ UX-42, 44, 51 | UXPR-09, 14, 15 |
| UX-17 | H | opravené | 04 | premisa README („hub pritom `list_assets` servíruje“) **neplatí**: hub servíruje iba definíciu, katalóg nikdy nenačíta (↔ UX-49); 5 príkazov je routovateľných bez nového toolu, 2 potrebujú nový, 25 ostáva odmietnutých | UXPR-08, 12, 13, 18 |
| UX-18 | H | opravené | 05 | `instead` text **existuje** (`NO_GIT_WRITE_TOOL`) a nič **nezlyhá** — gate je pre-emptívny (`writeBlocked`), dôvod je iba v `title`; podstata (mŕtvy commit box) platí ↔ UX-59 | UXPR-10, 11, 16 |
| UX-19 | M | neoverené | — | Hosts detail: Token odmietnutie inline, Danger akcie viditeľné | šošovka 6 |
| UX-20 | M | neoverené | — | `＋ Add project…` disabled iba s tooltipom | šošovka 6 |
| UX-21 | M | neoverené | — | safe-kill náhľad, tool detail v hub režime | šošovka 7 |
| UX-22 | M | neoverené | — | Conversation hlavička s 10 prvkami | šošovka 12 |
| UX-23 | M | opravené | 03 | „nie centrovaný modal“ **vyvrátené** (natívny `<dialog>` + `showModal()`, focus trap; FE-8/C3 landed); podstata potvrdená: 11 sekcií na jednej strane, bez navigácie, próza pri každom poli ↔ UX-47 | UXPR-14, 15 |
| UX-24 | M | neoverené | — | update toast bez tlačidla | šošovka 15 |
| UX-25 | L | neoverené | — | prvky nalepené na okraj | šošovka 13 |
| UX-26 | L | potvrdené | 05 (hub časť) | 4 nezávislé „Loading…“ bez skeletu, prázdne stavy bez CTA; v hub režime dva skoky, holý text viditeľný dlhšie | UXPR-17 |
| UX-27 | L | neoverené | — | `/` hint bez vysvetlenia | šošovka 16 |
| UX-28 | M | neoverené | — | AX strom 4 elementy ↔ UX-34 | šošovka 17 |
| UX-29 | M | nové | 01 | `.icon-btn` definovaný 3× + 5 lokálnych variantov, kým `controls.css` má kanonický `.btn--icon` | UXPR-02, 03 |
| UX-30 | L | nové | 01 | `--color-error` nie je definovaný; hard-coded `#e64a4a` namiesto `--usage-crit` | UXPR-01 |
| UX-31 | M | nové | 01 | `cursor: progress` na disabled → LocalOnly akcie vyzerajú ako „prebieha“ ↔ UX-04 | UXPR-02 |
| UX-32 | L | nové | 01 | dva „x“ glyfy (`×`/`✕`); Kill vyzerá ako Close | UXPR-02 (+ otázka A1) |
| UX-33 | L | nové | 01 | jeden význam dve ikony (Recreate `↺`/`♻`), jeden glyf dva významy (`☑`, `⚡`, `🏷`) — zrkadlo UX-01; mapa musí byť injektívna | UXPR-01 (test) |
| UX-34 | M | nové | 01 | ikonové tlačidlá bez `aria-label`, SVG/glyf bez `aria-hidden` ↔ UX-28 | UXPR-02, 03 |
| UX-35 | H | nové | 02 | systémové prompty pomenúvajú: safe-kill („i want to safely remove“), `[msg #N …]`, review seed, `broadcast_prompt` (N riadkov jedno meno) | UXPR-04 |
| UX-36 | M | nové | 02 | pomenovanie závisí od vstupnej cesty — prompt v tmux paneli nikdy nepomenuje | UXPR-05 |
| UX-37 | M | nové | 02 | žiadna proveniencia mena; label rovný defaultu sa prepíše, „indigo cosmos“ prežíva iba vďaka veľkému písmenu | UXPR-04 |
| UX-38 | M | nové | 02 | vetvový default dedí junk názvy worktree („Qqq“, „Lkkmkm“) — mimo šošovky | otázka B7; §5 |
| UX-39 | L | nové | 02 | `fleet-hub` nikdy nevolá `backfill_friendly_names` | UXPR-04 |
| UX-40 | L | nové | 02 | tri fallback výrazy pre zobrazené meno ↔ UX-07 → jedna `displayName()` | UXPR-06 |
| UX-41 | L | nové | 02 | MCP popisy sľubujú dnešné správanie → `REGEN_DOCS`, pozor na rozpočet | UXPR-04 |
| UX-42 | C | nové | 03 | „read and change them on the hub“ nemá kde: hub nemá settings tool, CLI nemá `settings`, jediná cesta je `sqlite3` v kontajneri — **≡ UX-49**, rodina UX-54, UX-60 („`instead` ukazuje na cieľ, ktorý nevie odpovedať“) | UXPR-09 |
| UX-43 | H | nové | 03 | klient je slepý k politike, ktorá zabíja jeho sessions (`gc.*`, `playbooks.*`, `move.*`) — čítanie nemá dôvod odmietať | UXPR-09 |
| UX-44 | M | nové | 03 | Hub sekcia = 4 odseky prózy pred jedným tlačidlom; fakty roztrúsené vo vetách ↔ UX-16, 51 | UXPR-15 |
| UX-45 | M | nové | 03 | desktop po reštarte nevie `client_mode` (`readonly`) — zápisy padnú až po kliku; **opakované** v 04 (UX-57) a 05 (stavy Files) → jedno rozhodnutie | otázka C5; §5 |
| UX-46 | L | nové | 03 | „Copy on select“ pod *Setup guide*; „Replay setup guide“ v hub režime slepé | UXPR-14, 15 |
| UX-47 | L | nové | 03 | nekonzistentné nadpisy sekcií (`h4` mimo `.section-header`) | UXPR-14 |
| UX-48 | L | nové | 03 | počet príkazov „123“ (CLAUDE.md) / „74 zo 123“ (README) vs. generovaných **130** — **opakované** v PR plánoch 04 a 05 → opraviť raz, bez čísla | UXPR-09 |
| UX-49 | C | nové | 04 | hub katalóg nikdy nenačíta (`CATALOG` napĺňa iba Tauri `catalog_load`), takže všetkých 10 asset toolov odpovedá `E_CATALOG_NOT_CONFIGURED`; 9 `instead` viet a `hub.md:1020` ukazujú do prázdna **≡ UX-42** | UXPR-08 |
| UX-50 | H | nové | 04 | `CATALOG_IS_A_CHECKOUT` pripnutý na 7 príkazov, ktoré checkout nepotrebujú (Store-only / čisté funkcie) | UXPR-13 |
| UX-51 | M | nové | 04 | hub režim panelu = 2 odseky bez jediného faktu ↔ UX-16, 44 | UXPR-18 |
| UX-52 | M | nové | 04 | 7 layer príkazov bez UI — parita lacná (2 RE riadky), ale Layers UI neexistuje | UXPR-13 (routing); UI → §5 |
| UX-53 | L | nové | 04 | `assets_inventory` je mŕtve volanie (store `inventory` nik nečíta) | UXPR-18 (+ otázka E2) |
| UX-54 | M | nové | 04 | `catalog_import_host` `instead` sľubuje `import_assets`, ktorý na hube číta `$HOME` **hubového procesu** — rodina UX-42 | UXPR-13 (text), 18 (skryť) |
| UX-55 | L | nové | 04 | telefónny klient z Assets parity nezdedí nič — README §4 pre UX-17 neplatí | §5 (šošovka 20); README |
| UX-56 | L | nové | 04 | rozpočet popisov platí ~210 B za vetu „Requires … in the app“ ×4, na hube nepravdivú | UXPR-08 |
| UX-57 | M | nové | 04 | plán zo `plan_sync` žije 10 min v registri hubu a klient ho nikdy neaplikuje; `readonly` klient padne už pri plánovaní ↔ UX-45 | UXPR-18 |
| UX-58 | — | nové | 04 | **iba odkaz** (riadky 99 a 450 v 04), nie je v tabuľke *Nové nálezy* a nemá závažnosť: `catalog_spawn_author_session` cez hubov `new_session` by potreboval katalógový repo ako projekt na hubovom hoste; navrhujem **L**, mimo rozsahu | §5 |
| UX-59 | H | nové | 05 | odmietnutie git zápisov je neviditeľné (iba `title`; textarea s normálnym placeholderom); „Commit 0 files“ disabled z dvoch dôvodov naraz — presnejšia forma UX-18 | UXPR-11 (min.), 16, 17 |
| UX-60 | H | nové | 05 | verdikt LocalOnly stojí na argumente („race pod agentom“), ktorý platí rovnako pre standalone; hub SSH má, chýba iba 10 `#[tool]` riadkov — rodina UX-42 | UXPR-10, 11 |
| UX-61 | M | nové | 05 | `FilesPanel` nemá vlastný test; hub správanie pinnuté iba na sub-komponentoch | UXPR-16 (testy) |
| UX-62 | M | nové | 05 | potvrdenia nekonzistentné: checkout vetvy, Pull a Push bežia okamžite, checkout commitu a delete majú dialóg | UXPR-16 |
| UX-63 | M | nové | 05 | `fetch/pull/push` pod 10 s `REPO_TIMEOUT_SECS` — platí aj standalone; `run_shell_bounded` existuje | UXPR-10 |
| UX-64 | L | nové | 05 | commit pod pracujúcim agentom bez UI signálu (oba režimy) | UXPR-17 |
| UX-65 | L | nové | 05 | `repo_delete_branch.force` je na drôte, UI ho nikdy nepošle — hub tool by `-D` dal každému `full` klientovi | UXPR-10 (+ otázka C3) |
| UX-66 | L | nové | 05 | chyby Fetch/Pull/Push = červený `!` s textom v `title` | UXPR-16 |
| UX-67 | L | nové | 05 | `REASONS.repo_write` je syntetický kľúč; s routingom musí zmiznúť kľúč aj allowlist — ten istý krok receptu ako `REASONS.get_fleet_settings` (03) a `REASONS.catalog_config` (04). Pozn.: 05 ho v nadpise *Loading skeleton (UX-26, UX-67)* cituje omylom — patrí tam UX-59 | UXPR-11 |

**Číslovanie:** žiadne dve iterácie nepoužili to isté číslo pre rôzne nálezy (01: 29–34, 02: 35–41,
03: 42–48, 04: 49–57 + odkaz 58, 05: 59–67). Jediná anomália je UX-58 (odkaz bez záznamu, vyššie).

**Prekryvy, ktoré treba riešiť ako jeden vzor, nie trikrát:**

| Vzor | Nálezy | Kde sa rieši raz |
|---|---|---|
| `instead`/`REASONS` veta ukazuje na hub tool alebo CLI, ktoré neexistujú alebo odpovedajú iné | UX-42, 49, 50, 54, 60 (+ UX-16, 17, 18 ako symptómy) | §2 D3 (politika) + recept 03; každý routing PR zmaže svoj kľúč (UX-67) |
| Hub režim odpovedá prózou namiesto faktu | UX-16, 44, 51, 59 | §2 D5 `HubScopeNote` + karty faktov (UXPR-15, 16, 18) |
| Desktop nevie mód klienta vopred | UX-45, 57, stavy `files-forbidden` | otázka C5, §5 |
| Starší hub → `E_FORBIDDEN`/`E_HUB_PROTOCOL` → jeden riadok, bez toastu, porovnávať kód | 03 §Starší hub, 04 §6, 05 §Starší hub | §2 D5 helper `hub_inline_state.ts` (UXPR-07) |
| Počet príkazov v próze | UX-48 (03), PR položky v 04 a 05 | UXPR-09, bez čísla v CLAUDE.md |
| Rozpočet `BUDGET_BYTES` | 03: 58 400 · 04: 58 800/58 900 · 05: 63 800/62 400 | §2 D4 — jedno číslo, jedna zmena |
| Prístupnosť ikonových tlačidiel | UX-28, 34 | UXPR-02/03 riešia `aria-*` na ikonách; zvyšok šošovka 17 |
| Jedna funkcia pre zobrazené meno | UX-07, 15, 40 | UXPR-06 `session_view.ts` |
| `icons.ts` vs `<Icon name>` | 05 §Ikony píše `<Icon name="refresh-cw">`; 01 to výslovne odmieta (string registry zabíja tree-shaking) | platí 01: `import { IconRefresh } from './icons'` |

## 2. Rozhodnutia naprieč iteráciami (raz, nie per PR)

### D1 — Ikonová knižnica: `@lucide/svelte`

`lucide-svelte` je od 2026-05 na npm `deprecated` (peer `svelte ^3 || ^4 || ^5.0.0-next.42`);
nástupca `@lucide/svelte` (1.47.0, peer `svelte ^5`, ISC, 0 runtime závislostí) je jediná správna
voľba pre Svelte 5. Pravidlá z iterácie 01, záväzné pre každý PR, ktorý pridá ikonu:

- **iba subpath importy** `@lucide/svelte/icons/<kebab>`; barrel import v dev serveri načíta ~1 600 modulov;
- **žiadny generický `<Icon name="…">`** — sémantický modul `src/lib/icons.ts` re-exportuje pod
  významovými menami (`IconRefresh` ≠ `IconRestart` ≠ `IconRecreate` ≠ `IconReconnect`); test
  injektívnosti mapy je súčasť `UXPR-01`;
- tokeny `--icon-sm/md/lg` (12/14/20 px), stroke 1,5 absolútny cez CSS, `aria-hidden` na SVG,
  `aria-label` + `title` na tlačidle;
- README §5 rozhodnutie (a) sa mení na `@lucide/svelte` (viď *Navrhované úpravy README*).

### D2 — Proveniencia mena: stĺpec `friendly_name_source`

Migrácia **040** pridá `sessions.friendly_name_source TEXT ∈ {default, prompt, chosen}`; `replaceable`
prestane porovnávať s vetvovým defaultom a číta zdroj. Rozhodnutia, ktoré platia pre celý blok:

- **jeden stĺpec, tri hodnoty**, bez rozlišovania agent vs. človek (pre ochranu ani UI to netreba);
- pravidlá žijú v `fleet-core` (`service/` + `store/`), lebo hub beží ten istý kód a telefón číta
  `friendly_name` doslovne; frontend iba zobrazuje;
- backfill = jednorazový Rust prechod v `Store::open*` po `migrate()` (dostane ho aj hub — UX-39),
  nie SQL, nie reconcile;
- wire field s `#[serde(default)]` (inak výpadok proti staršiemu hubu) → `REGEN_HUB_CONTRACT`;
  TS pole voliteľné;
- MCP popisy `send_prompt`, `new_bg_session`, `set_friendly_name` — jedna klauzula každý → `REGEN_DOCS`.

### D3 — Prístupová politika pre hub tools, ktoré menia fleet

Tri parity iterácie odpovedali odlišne: 03 dáva `set_fleet_setting` Client `full` (alternatíva
Master alebo trusted), 04 necháva `apply_sync`/`set_secret`/`set_host_layers` Master, 05 dáva 10 git
zápisov Client `full`, `repo_push` **trusted**, `force` Master a navrhuje nový tier viditeľnosti
`ClientOnly`. **Jedna politika — štyri stupne podľa dosahu (blast radius):**

| Stupeň | Kto | Kritérium | Tools (nové **tučne**, existujúce obyčajne) |
|---|---|---|---|
| **T0 Client readonly** | každý spárovaný klient, aj `readonly` | čisté čítanie faktov, ktoré rozhodujú o klientových sessions; bez tajomstiev | **`get_fleet_settings`**, **`catalog_config`**, **`catalog_get_asset`**, `list_assets` (+`last_sync`), `list_layers`, `propose_layers`, `scan_assets`, `repo_changes/tree/file/diff/log/branches/commit/commit_diff` |
| **T1 Client full** | `full` token | mutácia **vo vnútri session** (jej worktree) alebo v registri hubu; vratné alebo chránené service vrstvou (`E_DIRTY`, `--ff-only`, validácia, `shell::quote`) | **`repo_stage`, `repo_unstage`, `repo_commit_create`, `repo_create_branch`, `repo_delete_branch` (iba `-d`), `repo_checkout`, `repo_checkout_commit`, `repo_fetch`, `repo_pull`**, `plan_sync`, `kill_session`, `recreate_session`, … |
| **T2 Client full + trusted** | `full` + `fleet-hub client trust <name>` | koná **v mene hubu voči systému mimo fleetu** alebo mení **politiku pre celý fleet**; stále ohraničené (`SPECS` validácia, nikdy `--force`) | **`repo_push`**, **`set_fleet_setting`** |
| **T3 Master** | master token (operátor, `fleet-hub` CLI) | zapisuje na disky hostov, credentials, členstvo, alebo je nevratné | `apply_sync`, `set_host_layers`, `set_secret`, `provision_hosts`, budúce `delete_secret`/`list_secrets`/`catalog_load` (ak vzniknú), `-D` (**alebo `force` zo schémy vypustiť** — odporúčam, viď otázka C3) |

Čo sa tým mení oproti iteráciám: `set_fleet_setting` ide z Client `full` (03) na **T2 trusted** —
rovnaká dilema, akú 03 aj 05 pomenovali („trust“ sa rozširuje z „nemarkuj prompty“ na „smie konať
v mene operátora v overených hraniciach“), rozhodnutá raz a rovnako pre push aj nastavenia.
Cena pre vlastný desktop je nulová (`hub.md` už radí trustovať desktop, ktorý si spároval sám);
netrusted telefón vidí Settings read-only a Push disabled **s jedným riadkom dôvodu**, čo je aj tak
lepšie UX než `E_FORBIDDEN` po kliku. Ostatné stupne sa s iteráciami zhodujú.

Implementácia — dve možnosti, vybrať v `UXPR-09` a držať v `UXPR-10`:

- **(a) odporúčané:** nový variant `Access::Trusted` v `TOOL_POLICIES` + jedna vetva v gate
  (`guard.rs`), takže `annotations_follow_the_policy_table`, readonly test a rozpočtový test tier
  vidia deklaratívne;
- **(b) bez zmeny guardu:** kontrola v tele toolu `caller.is_master() || caller.is_trusted_client()`
  (návrh 05 pre push) — ak sa zvolí, použiť ju rovnako pre `set_fleet_setting`.

Spoločné pravidlá pre všetky hub tools z tohto bloku: `confirm: false` (hub nemá approvera;
potvrdenie robí desktopov `ConfirmDialog`, slice 2 FAB specu ho neskôr presunie za hub cez voliteľný
`confirm_nonce`); meno toolu = meno príkazu, výnimka iba tam, kde tool už existuje
(`catalog_list_assets → list_assets` atď.); `Deadline::Quick` pre Store/CATALOG/lokálny git,
`Lifecycle` pre `fetch/pull/push` a `scan_assets`. Tier viditeľnosti `Visibility::ClientOnly`
(05, otázka C2) je **ortogonálna os** a nerozhoduje sa v tomto bloku — ide do §5.

Pravidlo UI pre odmietnuté stupne (04): **disabled s dôvodom** pre to, čo na hube existuje, ale
tento token nesmie (Apply, Set secret, Push bez trustu); **skryť** to, čo nemá hubový ekvivalent a
je o inom stroji (Pull katalógu, Import, New/Edit/Delete/Lint, Commit/Push katalógu, Open in
session). Nie je to porušenie rozhodnutia (b) z README §5 — skryté prvky sú inou rolou, nie
odmietnutím.

### D4 — Rozpočet popisov `BUDGET_BYTES`: jedno číslo, jedna zmena

Miesto: `crates/fleet-core/src/mcp/tools/tests.rs:2357` (`const BUDGET_BYTES: usize = 57_700`),
meranie 57 603. Tranže z iterácií: +650 (03) → +450 netto (04, vrátane −210 za škrt „in the app“ ×4)
→ +4 900 (05, variant A) = **≈63 600**. Rozhodnutie:

- konštanta sa zdvihne **raz, v `UXPR-09`** (prvý PR, ktorý pridáva tool), na **63 800** (variant A)
  resp. **62 400** (variant B), s odsekom v doc-komentári, ktorý vymenuje všetky tri tranže a prečo
  sa nedali trimovať (každá schéma nesie `session_id` s povinným `///`);
- `UXPR-10` a `UXPR-12` konštantu **nedotýkajú**; iba ak meranie presiahne, zdvihne sa na najbližšiu
  stovku nad meranie a komentár sa doplní;
- škrt „Requires catalog_configure + catalog_load in the app.“ ×4 (UX-56) ide v `UXPR-08`, ktorý
  beží pred alebo súbežne s `UXPR-09`, takže úspora je v základe;
- voliteľné trimy (`plan_sync` ~530 B, `fleet_health` ~480 B) sú rezerva, nie podmienka;
- podmienka `ro_bytes < bytes / 2` platí v každom kroku (03 a 04 pridávajú readonly plochu, 05 iba
  mutujúcu) — čísla `master / host full / host readonly / client full` vypísať do popisu PR.

Dôvod „raz“: tri PR editujúce jednu konštantu = tri konflikty v tej istej línii a tri odseky, ktoré
si navzájom nesedia; navyše všetky tri Rust-tool PR sú aj tak sekvenčné (`guard.rs`, `tests.rs`).

### D5 — Zdieľané hub UI: `HubScopeNote.svelte` + `hub_inline_state.ts`

Iterácia 03 komponent navrhla (do PR-3b), 04 ho „ak ho PR-3b ešte nezaviedol, vytvoriť tu“, 05 ho
používa pre tri odmietnuté stavy. Aby tri Svelte PR nebežali sekvenčne kvôli jednému súboru,
vzniká **samostatný malý PR `UXPR-07`** s:

- `src/lib/HubScopeNote.svelte` — jednoriadkový banner, props `{ what: 'settings' | 'catalog' | 'git' }`,
  text z `$hubStatus` (URL, meno klienta), `data-testid="hub-scope-note"`;
- `src/lib/hub_inline_state.ts` — `hubInlineState(err): { kind, text } | null` mapujúci **kód**
  (`E_FORBIDDEN` s `readonly`/`trust`, `E_HUB_PROTOCOL`, `E_HUB_CONTRACT`, `E_HUB_UNREACHABLE`,
  `E_HUB_UNAVAILABLE`, `E_CATALOG_NOT_CONFIGURED`) na jednu vetu („this hub cannot serve … yet —
  update the hub“, „this client is readonly on the hub“, „needs the hub operator's trust
  (`fleet-hub client trust <name>`)“) — nikdy text správy, nikdy toast (vzor
  `NewSessionDialog.svelte:180-205`);
- testy oboch.

Settings (`UXPR-14/15`), Assets (`UXPR-18`) a Files (`UXPR-16`) ho importujú; žiadny z nich ho
nedefinuje.

### D6 — Pravidlá, ktoré sa opakujú v každom routing PR (napísané raz)

1. Zmazať kľúč z `REASONS` v `src/lib/hub.ts` a jeho allowlist v `hub_verdicts.test.ts`
   (`gatedBySettingsDialog` / `gatedByAssetsPanel` / `gatedByFilesPanel`; `repo_write` aj z
   `REASONS_KEYS_THAT_ARE_NOT_COMMANDS`, komentár „the five“ → „the three“) — test to vynúti, PR-a
   preto nesie minimálny TS diff (recept 03, kroky 10–11).
2. Routované mutácie do `ROUTED_ACTIONS` + `hubActionBlocked` (offline veta).
3. Regen v poradí `REGEN_DOCS` → `REGEN_LOCAL_ONLY` → `REGEN_HUB_VERDICTS` → `REGEN_HUB_CONTRACT`,
   každý 2× (beh s regen hlási FAILED); goldeny **prečítať**.
4. `CLAUDE.md:110` „all 123 commands“ → bez čísla („every command in `generate_handler!`; the count
   is in the generated table in `docs/hub.md`“) — iba v `UXPR-09`, potom sa nedotýkať (UX-48).
5. Testy `<Komponent>.hub.test.ts`: „does not call the local-only commands on mount“ zúžiť, pridať
   „calls `<command>` in remote mode and renders the hub's values“, „standalone is untouched“ ostáva.
6. Celý suite, nepipe-ovaný, s vlastným `CARGO_TARGET_DIR`.

## 3. Fronta PR (`UXPR-NN`)

Poradie = závislosti. Veľkosť je odhad diffu **bez testov a generovaných súborov** (testy v
zátvorke). „∥“ = môže bežať v paralelnom worktree; „→“ = sekvenčne po. Súbory sú hlavné, nie úplné.

| UXPR | Pôvod | Názov | Súbory | Veľkosť | Regen | Závisí od | Paralelnosť |
|---|---|---|---|---|---|---|---|
| **01** | 01 PR-1 commit 1 | Ikony — základ: `@lucide/svelte`, tokeny, `icons.ts`, test injektívnosti | `package.json`, `pnpm-lock.yaml`, `src/app.css`, `src/lib/controls.css`, `src/lib/icons.ts` (nový), `icons.test.ts` | S ~120 (+100) | — | — | ∥ (lane A) |
| **02** | 01 PR-1 commit 2, položky 6–8 | Ikony — riadok session, filtre, sidebar; zrušiť `.icon-btn` kópie; kôš v hub režime | `SessionRowItem.svelte`, `SidebarFilters.svelte`, `Sidebar.svelte`, `Sidebar.test.ts` | M ~150 | — | 01 | ∥ s UXPR-03 |
| **03** | 01 PR-1b, položky 9–13 | Ikony — detail, Files, terminál, App, 15 jednoriadkových náhrad | `SessionDetails`, `FilesPanel`, `TerminalView`, `App.svelte`, `CommitGraph`, `TransferSheet`, … | S ~80 | — | 01 | ∥ s 02; **pred** 06 a 17 (spoločné súbory) |
| **04** | 02 PR-A, položky 1–8, 10–13 | Pomenovanie — Rust: migrácia 040, proveniencia, filter P2, systémové prompty, `external` default, backfill, MCP popisy | `migrations/040_*.sql`, `store/{schema,rows,sessions,mod}.rs`, `service/sessions/{prompt,lifecycle,reconcile}.rs`, `service/{bg_sessions,safe_kill,messages}.rs`, `sessions/review.rs`, `mcp/tools/{messaging,session_ops,lifecycle}.rs`, `src-tauri/src/lib.rs`, `hub_contract.golden.json` | M ~280 (+320) | `REGEN_DOCS`, `REGEN_HUB_CONTRACT` | — | ∥ (lane B) |
| **05** | 02 PR-A, položka 9 | Pomenovanie — hook `UserPromptSubmit` pomenúva, `/clear` resetuje `prompt` label | `service/hooks.rs` | S ~40 (+60) | — | 04 | → 04 |
| **06** | 02 PR-B | Pomenovanie — TS: `displayName`, tlmené auto meno, otočená hierarchia detailu, prefix agenta, badge `outside` | `sessions.ts`, `session_view.ts` (nový), `SessionRowItem`, `SessionDetails`, `TerminalView`, `HostDetail`, `quick_switcher.ts`, `agent_context.ts` | S ~120 (+80) | — | 04; mäkko 01 (`IconExternal`, inak textový chip) | → 03 (spoločné súbory); ∥ s lane D |
| **07** | nový (dedupe 03/04/05) | `HubScopeNote.svelte` + `hub_inline_state.ts` + testy | `src/lib/HubScopeNote.svelte`, `src/lib/hub_inline_state.ts`, testy | S ~80 (+40) | — | — | ∥ |
| **08** | 04 PR-4c | Hub má katalóg: `fleet-hub catalog set/show`, load pri boote, škrt „in the app“ ×4, docs, Dockerfile | `crates/fleet-hub/src/{main,serve}.rs`, `mcp/tools/assets.rs` (popisy), `Dockerfile`, `docs/hub.md` | S/M ~150 (+60) | `REGEN_DOCS` | — | ∥ s 09 (kolízia iba v generovanom `control-api-reference.md` → re-regen) |
| **09** | 03 PR-3a | Settings parita — `get_fleet_settings` (T0), `set_fleet_setting` (T2), verdikty, remote, routing Case, min. TS; **`BUDGET_BYTES` → 63 800 raz**; **UX-48 raz** | `mcp/tools/{params,fleet}.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `backend/{verdicts,remote,tests_routing}.rs`, `commands/sessions.rs`, `local_only.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `SettingsDialog.svelte` (min.), `SettingsDialog.hub.test.ts`, `docs/hub.md`, `docs/control-api.md`, `CLAUDE.md` | M ~230 (+140) | `REGEN_DOCS`, `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` | 07 nie je nutné (min. TS iba ruší odmietnutia) | začiatok lane D |
| **10** | 05 PR-5a1 | Git zápisy — fleet-core: `Serialize + JsonSchema + ///` na 7 args structoch, `run_shell_bounded` pre fetch/pull/push (UX-63), 10 toolov (T1 + `repo_push` T2), policy, testy mantinelov | `service/{repo_mutate,repo}.rs`, `mcp/tools/repo.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `docs/control-api.md` | M ~260 (+150) | `REGEN_DOCS` | 09 (`guard.rs`, `tests.rs`), 08 (škrt v základe rozpočtu) | → 09 |
| **11** | 05 PR-5a2 | Git zápisy — routing: 10× `Routed`, zmazať `NO_GIT_WRITE_TOOL`, remote ×10, `mod routed` ×10, 10 Case, `REASONS.repo_write` von, `FilesPanel` min. (`writeRefused`) | `backend/{verdicts,remote,tests_routing}.rs`, `commands/mutate.rs`, `local_only.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `FilesPanel.svelte` (min.), `hub_disabled.test.ts`, `docs/hub.md` | M ~240 (+180) | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` | 10 | → 10 |
| **12** | 04 PR-4a1 | Assets parita — fleet-core: `catalog_config`, `catalog_get_asset` (T0), `AssetListing.last_sync`, policy, readonly test | `mcp/tools/{params,assets}.rs`, `service/catalog/mod.rs`, `mcp/guard.rs`, `mcp/tools/tests.rs`, `docs/control-api.md` | S ~110 (+40) | `REGEN_DOCS` | 08, 10 | → 10 |
| **13** | 04 PR-4a2 | Assets parita — routing 7× (`list_assets`, `list_layers`, `propose_layers`, `scan_assets`, `plan_sync`, 2 nové), texty `instead` (UX-50, 54), `CATALOG_IS_A_CHECKOUT` prepis, `PlanArgs: Serialize`, kontrakt (~22 samples), `REASONS.catalog_config` von, `AssetsPanel` min. | `backend/{verdicts,remote,tests_routing,tests_contract}.rs`, `commands/assets.rs`, `service/catalog/sync/mod.rs`, `local_only.golden.json`, `hub_contract.golden.json`, `hub.ts`, `hub_verdicts.test.ts`, `AssetsPanel.svelte` (min.), `hub_disabled.test.ts`, `docs/hub.md` | M ~230 (+90, +120 samples) | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS`, `REGEN_HUB_CONTRACT` | 12, 11 (spoločné `verdicts.rs`, `remote.rs`, goldeny) | → 11, 12 |
| **14** | 03 PR-3b1 | Settings IA — `SettingsNav` (7 skupín, tablist, badge `fleet`/`this app`), grid, panely, `settings.tab` v prefs, deep-link z pätičky, helper `openGroup` v testoch | `SettingsNav.svelte` (nový), `SettingsDialog.svelte`, `settings_dialog.css`, `app_views.ts`, `App.svelte`, `SettingsDialog.test.ts`, `App.hub.test.ts` | M ~250 (+150) | — | 09, 07 | ∥ s 16, 18 (lane E) |
| **15** | 03 PR-3b2 | Settings — `HubStatusCard` (fakty + Disconnect + `<details>`), Control API read-only karta, „Copy on select“ do Terminálu, Replay disabled v hub režime, `.section-header` všade, skrátená próza | `HubStatusCard.svelte` (nový), `McpSettings.svelte`, `SettingsDialog.svelte`, `SettingsDialog.hub.test.ts` | S/M ~180 (+60) | — | 14 (ten istý `SettingsDialog.svelte`) | → 14 |
| **16** | 05 PR-5b1 | Files — potvrdenia (checkout vetvy, Pull s `behind`, Push `danger` s `set_upstream`), stavy `files-hub-unsupported` / `files-forbidden` / `files-push-untrusted`, chyby `RemoteToolbar` ako viditeľný riadok podľa kódu; `FilesPanel.test.ts` + `.hub.test.ts` (UX-61) | `FilesPanel.svelte`, `FileList.svelte` (časť), `RemoteToolbar.svelte`, `FilesPanel.test.ts`, `FilesPanel.hub.test.ts`, `RemoteToolbar.test.ts` | M ~180 (+150) | — | 11, 07 | ∥ s 14, 18 |
| **17** | 05 PR-5b2 | Files — `Skeleton.svelte` (150 ms, reduced-motion), empty CTA podľa módu, „Stage files to commit“, agent-busy hint (UX-64), „No remotes“, „No commits yet“ | `Skeleton.svelte` (nový), `FileList`, `FileViewer`, `BranchList`, `CommitGraph`, `FilesPanel`, `FileList.test.ts`, `Skeleton.test.ts`, `FileViewer.test.ts` | S/M ~150 (+150) | — | 16 (spoločné súbory); mäkko 01 | → 16, → 03 |
| **18** | 04 PR-4b | Assets panel v hub režime — 4 stavy (unsupported / empty / configured-not-loaded / catalog), toolbar podľa matice (skryť Pull/Import/New/Lint/Commit/Push/strip; Secrets disabled), `AssetDetail.readonly`, `AssetList.onimport?`, `SyncPlanDialog.applyBlocked`, `last_sync` z listingu, zmazať `loadInventory` (UX-53), `onCatalogLoaded` bez `repoStatus` | `AssetsPanel.svelte`, `AssetDetail.svelte`, `AssetList.svelte`, `SyncPlanDialog.svelte`, `SecretsPanel.svelte`, `App.svelte`, `assets.ts`, `AssetsPanel.hub.test.ts` (nový), `SyncPlanDialog.test.ts`, `AssetDetail.test.ts` | M ~220 (+180) | — | 13, 07 | ∥ s 14–17 |
| **19** | 01 PR-2 | Ikony — stavové glyfy v `.ts` reťazcoch (`⚡ working`, `✓ CI`, usage `▲△■◷`, `PR↗`, `TransferChip ⇄`) → `{glyph, text}` model, ~60 asserov | `attention.ts`, `hosts_view.ts`, `account_usage.ts`, `usage_glance.ts`, `conversation.ts`, `TransferChip.svelte`, `SessionRowItem.svelte:405`, testy | M ~150 (+60) | — | 01 | **odložené** — po šošovke 9 (riadok session), aby sa model písal raz |
| **20** | 01 PR-3 | Ikony — `fileicons.ts` (45 emoji → Lucide `file-*`) | `src/lib/fileicons.ts` | S ~60 (+20) | — | 01, otázka A2 | **odložené** — čaká na rozhodnutie o farbe |

**Súčty:** hlavná fronta 01–18 ≈ **3 070** riadkov (+≈2 100 testov, + generované); odložené 19–20
≈ 210 (+80). Všetky PR sú pod ~300 riadkami bez testov; `UXPR-04` (~280) a `UXPR-10` (~260) sú
najbližšie k limitu — odštep `UXPR-05` a rozdelenie 5a na 10/11 sú presne tie, ktoré iterácie samy
navrhli.

**Lanes a kritická cesta:**

```
lane A  ikony        01 → { 02 ∥ 03 } ………………………………………………………… 19, 20 (odložené)
lane B  pomenovanie  04 → 05 → 06            (06 až po 03)
lane C  zdieľané UI  07
lane D  hub Rust     { 08 ∥ 09 } → 10 → 11 → 12 → 13        ← kritická cesta
lane E  hub Svelte   14 → 15   (po 09, 07)
                     16 → 17   (po 11, 07; 17 až po 03)
                     18        (po 13, 07)
```

Lane D je sekvenčná, lebo každý PR edituje `guard.rs`, `tests.rs` (rozpočet), `verdicts.rs`,
`remote.rs`, `tests_routing.rs`, `hub.ts`, `hub_verdicts.test.ts` a generované goldeny — paralelné
worktrees by sa zbiehali v tých istých riadkoch. Poradie **Settings → git → Assets** je zmena
oproti iteráciám (3 → 4 → 5): Settings má dva C nálezy (UX-16, 42), git dva H (UX-59, 60) a používa
sa denne, Assets má C (UX-49) riešené už v `UXPR-08`, ktorý beží hneď; zvyšok Assets parity je
menšia hodnota a väčší kontrakt. Lane A, B, C a E bežia paralelne s D v samostatných worktrees
(memory: jeden zapisovateľ na súbor — kolízie sú vyznačené v stĺpci *Paralelnosť*).

**Regen podľa PR (kontrolný zoznam):**

| Príkaz | PR |
|---|---|
| `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` | 04, 08, 09, 10, 12 |
| `REGEN_LOCAL_ONLY=1 cargo test -p claude-fleet --lib local_only` | 09, 11, 13 |
| `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` | 09, 11, 13 |
| `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` | 04, 13 |

## 4. Otvorené otázky pre vlastníka

> **Odpoveď vlastníka (2026-09-22): „default“ — všetky odporúčania nižšie sú prijaté**
> a platia ako rozhodnutia pre UXPR-01…20 a iterácie 6–20.

Zlúčené z piatich reportov (35 pôvodných). Vypustené: otázky, ktoré §2 rozhoduje (`@lucide/svelte`
— 01 Q1; rozdelenie PR — 03 Q7 a odštepy v 04/05; jeden rozpočet — zlúčené), a tie, ktoré patria
šošovke 11 (operátor v sidebare — 02 Q7, iba jej časť o mene ostáva v B). Každá má odporúčanie;
„default“ = prijať všetky odporúčania.

### A — Ikony

1. **Kill vs Close** (01 Q2): odlišná ikona pre Kill (`circle-x`, `btn--crit` v pokoji) alebo ten istý
   `x` s tónom? *Odporúčanie: odlišná ikona.*
2. **`fileicons.ts`** (01 Q3): stratiť farebné rozlíšenie jazykov (Lucide je monochróm), alebo emoji v
   strome súborov ponechať ako vedomú výnimku? *Odporúčanie: ponechať emoji ako výnimku s allowlistom
   v teste — `UXPR-20` potom odpadá.*
3. **Safe remove** (01 Q4): `log-out` alebo `shield-check` (Lucide nemá eject)? *Odporúčanie: `shield-check`.*

### B — Pomenovanie sessions

1. **Reset po `/clear`** (02 Q1): auto meno z promptu sa po `/clear` vráti na default; `resume` nemení
   nič. *Odporúčanie: áno.*
2. **Backfill ≤ 2-slovných mien** (02 Q2): jednorazovo padnú na default aj ručné labely s ≤ 2 slovami
   spred migrácie (bez proveniencie sa nedajú odlíšiť od „yes“); alternatíva je konzervatívny reset
   iba presných zhôd so stop-listom a slash príkazmi. *Odporúčanie: plný reset, jedna veta v CHANGELOG-u.*
3. **Default pre `external` riadok** (02 Q3): `<repo> · <id8>` alebo Claude-ovo `agent.name`; má ísť
   `cwd` do vlastného stĺpca? *Odporúčanie: `<repo> · <id8>`; `cwd` iba ako parameter `upsert_bg_session`.*
4. **Overenie duplikátu UX-06e** (02 Q4): na hube porovnať `claude_session_id` tmux riadkov na
   `claude-fleet-trn` s uuid v `bg:62e738aa…`/`bg:8df9227c…`; pri zhode treba fix `find_by_unique_cwd`
   pre vzdialené hosty. *Odporúčanie: overiť pred `UXPR-04` (jeden `sqlite3` dotaz).*
5. **Broadcast nikdy nepomenúva** (02 Q5). *Odporúčanie: áno.*
6. **Stop-list jazyky** (02 Q6): EN + SK pevne. *Odporúčanie: pevne, ďalšie až na požiadanie.*
7. **Validácia názvu worktree** (02 Q8, UX-38): odmietnuť názvy bez samohlásky / < 3 znaky v
   `NewSessionDialog`? *Odporúčanie: áno ako S doplnok `UXPR-06`; inak ostáva default meno obeťou.*

### C — Prístup, bezpečnosť, potvrdenia

1. **Prístupová politika T0–T3** (03 Q1 + 04 Q2 + 05 Q1): prijať rebrík z §2 D3 — `set_fleet_setting`
   a `repo_push` na **trusted**, `apply_sync`/`set_secret`/`set_host_layers` Master? A implementácia
   (a) `Access::Trusted` vs (b) kontrola v tele? *Odporúčanie: rebrík áno, (a).*
2. **`Visibility::ClientOnly`** (05 Q3): tier toolov, ktoré per-host token (agent) nevidí, aby agentova
   plocha nerástla o git tools? *Odporúčanie: nie v tomto bloku; ADR po šošovke 7 (§5).*
3. **`repo_delete_branch.force`** (05 Q4, UX-65): odmietnuť ne-master v tele (~150 B), alebo pole zo
   schémy a Tauri args vypustiť? *Odporúčanie: vypustiť.*
4. **`mcp.confirm_destructive` ako derived read-only kľúč** v `get_fleet_settings` (03 Q2), aby Hub
   karta ukázala skutočný stav; ísť ďalej a dať ho do `SPECS`? *Odporúčanie: derived kľúč áno, do
   `SPECS` nie (dva zapisovatelia jedného kľúča).*
5. **Persistovať `client_mode`** pri párovaní (03 Q3, UX-45; opakované 04/05), aby UI vopred vypínalo
   zápisy `readonly` klienta? *Odporúčanie: áno, samostatný S PR mimo tejto fronty (§5).*
6. **Agent-busy** (05 Q5, UX-64): iba UI hint, alebo backendové `E_BUSY` pri `claude_status == working`?
   *Odporúčanie: iba hint (backend by menil standalone a stál na heuristike).*
7. **Potvrdenie pri checkout vetvy a Pull** (05 Q6, UX-62): zaviesť, alebo spoľahnúť sa na
   `E_DIRTY`/`--ff-only`? *Odporúčanie: zaviesť — zjednotí s checkout commitu.*

### D — Hub tools a rozpočet

1. **Variant A (10 toolov 1:1, `BUDGET_BYTES` 63 800) alebo B (6 toolov, 62 400)** (05 Q2 + 03 Q6 +
   04 Q7). *Odporúčanie: A — drží „meno toolu = meno príkazu“; rozpočet nie je tvrdý strop.*
2. **(Re)načítanie hubového katalógu** (04 Q1): (a) iba pri boote + reštart; (b) Master tool
   `catalog_load` (+280 B); (c) `catalog.pull_interval_secs` tick s `catalog:loaded` eventom.
   *Odporúčanie: (a) v `UXPR-08`, (c) ako follow-up; (b) až keď bude potrebný ručný zásah.*
3. **Secrets v hub režime** (04 Q3): (a) tlačidlo disabled s dôvodom (0 B); (b) `list_secrets`
   Client/readonly — mená a hosty, nikdy hodnoty (+180 B). *Odporúčanie: (a) teraz.*
4. **Docker obraz hubu** (04 Q4): má `git`? Deploy kľúč pre `remote_url` — mount `~/.ssh`, HTTPS token,
   alebo bind-mount hotového checkoutu a `set --path` bez remote? *Odporúčanie: bind-mount checkoutu
   bez remote na NAS; `git` do obrazu tak či tak.*
5. **`catalog_resolve_preview`** (04 Q5): nechať refused kým nie je Layers UI, alebo `full: bool` param
   (+130 B) už teraz? *Odporúčanie: nechať.*
6. **`catalog_get_asset` a veľkosť odpovede** (04 Q6): poznámka v popise vs `with_resources` prepínač.
   *Odporúčanie: poznámka; jeden asset je desiatky kB.*
7. **`reconcile.interval_secs` / `repair.tick_interval_secs` na hube** (03 Q5): stačí „restart the hub
   to apply“, alebo má tick čítať nastavenie pri každom prechode? *Odporúčanie: kópia „restart to
   apply“ teraz; živé čítanie ako backend follow-up (§5).*
8. **UX-63 wall clock** pre fetch/pull/push (05 Q7): 60 s, 90 s, konfigurovateľné? *Odporúčanie: pevných
   90 s (`run_shell_bounded(connect 10 s, wall 90 s)`), `Deadline::Lifecycle`.*

### E — UI detaily

1. **Ľavá navigácia so 7 skupinami vs horné taby** (03 Q4). *Odporúčanie: ľavá nav — badge
   `fleet`/`this app` je hlavný prínos.*
2. **Zmazať mŕtvy `inventory` store** (04 Q8, UX-53) v `UXPR-18`, alebo nechať pre budúcu Layers UI?
   *Odporúčanie: zmazať.*
3. **Skeleton oneskorenie 150 ms + `prefers-reduced-motion`** (05 Q8). *Odporúčanie: áno; skeleton
   vždy by v standalone preblikával.*

## 5. Čo ide do iterácií 6–20

Položky, ktoré päť reportov vytlačilo mimo šošovky, priradené k tabuľke šošoviek v README §5.
„—“ = nemá šošovku; navrhnuté kam.

| Položka | Pôvod | Šošovka | Poznámka |
|---|---|---|---|
| `list_account_usage` / `refresh_account_usage` LocalOnly (usage v pätičke) | 03 §Mapa | 6 | kandidát na T0 tool |
| `tunnel_status` → nahradiť `fleet_health` per-host tunnel health; `check_local_prereqs` | 03 tabuľka VERDICTS | 6 / 19 | bez nového toolu |
| Persistovať `client_mode` (UX-45) | 03 Q3, 04 UX-57, 05 stavy | 6 alebo 20 | odporúčam **S backend PR kedykoľvek** — odblokuje inline stavy vo všetkých troch paneloch |
| `Visibility::ClientOnly` tier | 05 Q3 | 7 (sessions/MCP) → ADR | ortogonálna os k D3; rozhoduje, či agentova plocha rastie o git tools |
| Slice 2 confirmation channel (`E_CONFIRM_REQUIRED` klientovi, `confirm: true` pre `repo_push`, `repo_delete_branch`) | 05 §Mantinely 10, FAB spec | 11 | dialógy z `UXPR-16` sa stanú odpoveďou na event, layout ostáva |
| Operátor v sidebare (vlastná sekcia vs. riadok projektu, `IconAgent` badge, „Fleet operator“ ako `chosen`) | 02 P6, Q7 | 11 | `UXPR-04` už dá operátorovi `chosen`; UX-13 závažnosť prehodnotiť (panel hlásil pravdu) |
| Validácia názvu worktree (UX-38) | 02 Q8 | — → otázka B7 | ak „default“, ide ako S doplnok `UXPR-06` |
| Stavové glyfy v `.ts` (`{glyph, text}` model, tabuľka C) | 01 §C | 9 (riadok session) → `UXPR-19` | písať model raz spolu s hierarchiou metadát |
| `fileicons.ts` (tabuľka D) | 01 §D | — → otázka A2 / `UXPR-20` | |
| `Needs you (0)` svieti aj pri 0 | 01 tabuľka B | 8 | |
| Layers UI (UX-52), `resolve_preview.full`, `list_secrets`, `catalog.pull_interval_secs`, `catalog_load` Master tool | 04 §Mimo rozsahu | — → „Assets slice 2“ | routing vrstiev je v `UXPR-13` zadarmo; UI nie je v tabuľke šošoviek — navrhujem doplniť ako 4b po šošovke 20 |
| `catalog_spawn_author_session` cez hubov `new_session` (UX-58) | 04 Tier 3 | — → „Assets slice 2“ | katalógový repo ako projekt na hubovom hoste |
| Telefón nemá asset obrazovku (UX-55) | 04 | 20 | README §4 opraviť: mobile zdedí iba hub s katalógom, nie UI |
| Reconcile/repair tick čítajú nastavenie živo | 03 Q5 | — → backend backlog | dnes „restart the hub to apply“ |
| `repo_stash` / `repo_discard` | 05 §Mimo rozsahu | — | **zámerne nie** — discard je agentova vec; zapísať do `docs/hub.md` *Known limitations* |
| Timeline ikony pre git `mcp_call` riadky | 05 PR-5b položka 8 | 12 (Conversation) | voliteľné po `UXPR-01` |
| `PR↗`, `TransferChip ⇄`, `⌘ ↵ ⇧` (ponechať ako symboly kláves) | 01 §C | 9 / 13 → `UXPR-19` | klávesové symboly nikdy nenahrádzať |
| Light téma s farebnými emoji | 01 UX-03 | 18 | po lane A z veľkej časti odpadá |
| „Replay setup guide“ v hub režime, Onboarding card volá 3 LocalOnly | 03 UX-46 | 19 | `UXPR-15` iba disabluje tlačidlo |
| Prístupnosť ikonových tlačidiel nad rámec `aria-label` (fokus, AX bridging WKWebView) | 01 UX-34, audit UX-28 | 17 | `UXPR-02/03` riešia iba `aria-label`/`aria-hidden` |
| `E_LOCAL_ONLY` ako komponent (README šošovka 14) | README | 14 | po D5 je to `HubScopeNote` + `hub_inline_state` — šošovka 14 ich má prevziať, nie navrhnúť tretí |

Auditové nálezy bez šošovky v tomto bloku (UX-08, 09, 10, 12, 13, 14, 19, 20, 21, 22, 24, 25, 27, 28)
ostávajú v README §5 tabuľke, s dvoma opravami: UX-12 doplniť k šošovke 8; pri UX-13 poznámka z
iterácie 02.

## 6. Metriky bloku

| Metrika | Hodnota |
|---|---|
| Nálezy v registri | **67** (28 audit + 39 nové) |
| Auditové nálezy pod šošovkou | 14 z 28 |
| — potvrdené bez zmeny | 9 (UX-01, 02, 03, 05, 07, 11, 15, 16, 26) |
| — potvrdené s korekciou premisy („opravené“) | 5 (UX-04, 06, 17, 18, 23 — z toho 23 čiastočne vyvrátené) |
| — vyvrátené úplne | 0 (vyvrátené boli iba položky plánu FE-7 a „FE-8/C3 otvorené“, nie UX-ID) |
| — neoverené (čakajú na šošovky 6–17) | 14 |
| Nové nálezy podľa závažnosti | C 2 (UX-42, 49) · H 5 (35, 43, 50, 59, 60) · M 15 · L 16 · bez sev 1 (UX-58) |
| Duplikáty/vzory zlúčené | 9 riadkov v tabuľke prekryvov (§1) |
| Rozhodnutia „raz“ | 6 (D1–D6) |
| PR v pôvodných reportoch | 17 (PR-1, 1b, 2, 3 · A, A2, B · 3a, 3b1, 3b2 · 4c, 4a1, 4a2, 4b · 5a1, 5a2, 5b1, 5b2 — po odštepoch) |
| PR vo fronte | **20** `UXPR` (18 hlavná + 2 odložené); +1 nový (`UXPR-07`), zlúčenia: budget, UX-48, `HubScopeNote` |
| Súčet diffu | ≈ **3 300** riadkov bez testov/generovaných (hlavná fronta ≈ 3 070) · ≈ 2 200 testov · + generované goldeny/reference |
| Najväčší PR | `UXPR-04` ~280, `UXPR-10` ~260 — oba pod limitom |
| Kritická cesta | 5 sekvenčných PR (09 → 10 → 11 → 12 → 13), potom 16 → 17 a 18 |
| `BUDGET_BYTES` | 57 700 → **63 800** (+6 100; meranie 57 603 → ≈63 600, +10,4 %); variant B 62 400 |
| Verdikty | LocalOnly 70 → **51**, Routed 39 → **58** (zo 130 príkazov; −2 Settings, −10 git, −7 Assets) |
| Nové hub tools | 14 (2 Settings + 10 git + 2 Assets) + 1 rozšírenie (`list_assets.last_sync`) |
| Nové migrácie | 1 (040 `friendly_name_source`) |
| Nové Svelte komponenty | 5 (`HubScopeNote`, `SettingsNav`, `HubStatusCard`, `Skeleton`, + `icons.ts` modul) |
| Otázky pre vlastníka | 35 → **28** (A 3 · B 7 · C 7 · D 8 · E 3) |

## Nezrovnalosti medzi reportmi (na vedomie kontrolórovi)

1. **05 vs 01 — tvar ikony:** 05 §Ikony píše `<Icon name="refresh-cw">`; 01 generický komponent so string
   registry výslovne odmieta. Platí 01 (`IconRefresh` z `icons.ts`). Dotýka sa `UXPR-16/17`.
2. **05 — chybná citácia:** nadpis *Loading skeleton (UX-26, UX-67)* má byť *(UX-26, UX-59)*; UX-67 je
   `REASONS.repo_write`.
3. **04 — UX-58 bez záznamu:** citovaný dvakrát, nikdy definovaný v tabuľke nálezov; register ho vedie
   s navrhnutou závažnosťou L.
4. **03 vs README — počty:** 03 správne uvádza 130 príkazov / 70 LocalOnly (generovaný súbor); README
   §2 „74 zo 123“ a CLAUDE.md „123“ sú zastarané (UX-48).
5. **04 vs README §4 — mobile:** README tvrdí, že telefón zdedí fix hub parity pre UX-16…21; pre UX-17
   to neplatí (UX-55) — mobile spec nemá asset obrazovku.
6. **Rozpočet:** tri rôzne cieľové čísla (58 400 / 58 800–58 900 / 63 800) — zjednotené v D4.
7. **Poradie hub parity:** iterácie predpokladali 3 → 4 → 5; fronta dáva 3 → 5 → 4 (hodnota a nálezy H
   pri git). Rozpočtový škrt z 4c (`UXPR-08`) ide aj tak prvý, takže D4 platí v oboch poradiach.

## Navrhované úpravy README (`docs/ux/2026-09-21-audit/README.md`)

Kontrolór aplikuje; tento dokument README nemení.

| Miesto | Dnes | Zmeniť na |
|---|---|---|
| §2 úvod nad hub-parity tabuľkou | „74 zo 123 príkazov je v hub režime `LocalOnly`“ | „70 zo 130 príkazov (podľa `src/lib/hub_verdicts.generated.json`); po konsolidácii-01 cieľ 51“ |
| §2 UX-04 | „plnofarebná ikona na jeden klik“ | „kôš otvára `ConfirmDialog`; trvalo viditeľný iba v hub režime, lebo `disabled` prebije `opacity: 0` (iter. 01, ↔ UX-31)“; Sev ostáva L |
| §2 UX-06 | „vrátane operátora (UX agenta), ktorý ‚working‘ beží“ | „sú to `kind='external'` riadky (agenti bez tmux nároku), nie operátor; operátor je session ‚fleet operator‘ (iter. 02)“ |
| §2 UX-13 | — | doplniť poznámku „iter. 02: operátor na screenshote 01/03 nebežal — panel hlásil pravdu; závažnosť prehodnotí šošovka 11“ |
| §2 UX-17 | „hub pritom `list_assets`, … servíruje“ | „hub servíruje iba definície toolov — katalóg nikdy nenačíta (UX-49, iter. 04); 5 príkazov je routovateľných na existujúce tools“ |
| §2 UX-18 | „sú LocalOnly bez `instead` textu — tlačidlá sú disabled alebo zlyhajú s `E_LOCAL_ONLY`“ | „majú `instead` text (`NO_GIT_WRITE_TOOL`), ktorý sa zobrazí iba v tooltipe; nič nezlyhá, gate je pre-emptívny (UX-59, iter. 05)“ |
| §2 UX-23 | „ukotvená vľavo od stredu (nie centrovaný modal)“ | vypustiť — natívny `<dialog>` + `showModal()` je centrovaný; ostáva „jedna dlhá strana bez záložiek, próza pri každom nastavení“ (iter. 03) |
| §3 klaster 3 | „`catalog_list_*`/`resolve_preview` (hub ich má)“ | „`catalog_list_*`/`plan_sync`/`scan_assets` (hub má tools, ale katalóg musí najprv načítať — UXPR-08)“; doplniť „prístupová politika T0–T3 v `consolidation-01.md` §2 D3“ |
| §4 tretia odrážka | „telefónny klient má rovnaký problém s paritou (UX-16…21) a zdedí fix na hub strane“ | „… zdedí fix na hub strane pre Settings a git; Assets UI v mobile spece nie je (UX-55)“ |
| §5 tabuľka šošoviek, riadok 8 | „Zasahuje: UX-08“ | „UX-08, UX-12“ |
| §5 tabuľka šošoviek, riadok 14 | „`E_LOCAL_ONLY` ako komponent“ | „prevziať `HubScopeNote` + `hub_inline_state` (consolidation-01 D5), nie navrhovať nový“ |
| §5 Rozhodnutia (a) | „**Lucide** (`lucide-svelte`)“ | „**Lucide** (`@lucide/svelte` — `lucide-svelte` je deprecated; subpath importy, sémantický `src/lib/icons.ts`, žiadny `<Icon name>`)“ |
| §5 Rozhodnutia — nový bod (d) | — | „(d) rozpočet MCP popisov `BUDGET_BYTES` sa zdvíha **raz** na 63 800 v `UXPR-09`; ďalšie PR ho nemenia“ |
| §5 Rozhodnutia — nový bod (e) | — | „(e) prístup hub tools: T0 readonly · T1 full · T2 full+trusted (`set_fleet_setting`, `repo_push`) · T3 master; `confirm: false` na hube“ |
| §5 za protokolom | — | odkaz „Konsolidácia po iteráciách 1–5: `iterations/consolidation-01.md` (register UX-01…67, fronta UXPR-01…20)“ |

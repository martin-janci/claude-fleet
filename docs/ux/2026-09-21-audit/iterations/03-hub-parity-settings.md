# Iterácia 03 — Hub parita: Settings dialóg

**Šošovka:** hub-client parita v Settings · **Zasahuje:** UX-16, UX-23, root-cause klaster 3
(„hub režim odpovedá prózou namiesto funkcie“) · **Vstup:** `docs/ux/2026-09-21-audit/README.md`,
screenshot 04, iterácie 01 a 02, kód na `c4f3e83f` · **Režim:** read-only review, žiadne zmeny
kódu, žiadny `cargo` beh. **Prvá z troch hub-parity iterácií** (3 = Settings, 4 = Assets/Catalog,
5 = Files/git) — sekcia *Recept* je spoločný postup pre všetky tri.

Schválené rozhodnutie (README §5): hub parita = **nové hub tools a routing príkazu na hub**, nie
skrývanie UI.

## Zhrnutie

1. **UX-16 potvrdené a širšie.** Settings v hub režime vloží tú istú vetu z `hubBlock('get_fleet_settings')`
   trikrát (Projects, Automation, Limits — `SettingsDialog.svelte:546,734,740`), pod Control API
   ďalšie dve (`mcp_status`, `provision_hosts` — `:746,749`) a sekcia Hub sama má štyri odseky
   prózy (~190 slov) pred jediným tlačidlom Disconnect (`:369-417`). Používateľ nevidí ani jednu
   hodnotu nastavenia a nemá kam ísť.
2. **„Do it on the hub“ nemá cieľ.** Hub nemá žiadny tool na čítanie ani zápis fleet nastavení
   (`guard.rs:93-700` `TOOL_POLICIES` neobsahuje nič so `settings`) a `fleet-hub` CLI má
   subpríkazy `init · serve · token · agent-token · host-token-mode · pair · client · demo-seed`
   (`crates/fleet-hub/src/main.rs:24-108`) — žiadny `settings`. Jediná cesta k `gc.enabled` na
   hube je `sqlite3 state.db` v kontajneri. Aj hubov vlastný warning „turn the setting off“
   (`serve.rs:885-895`) ukazuje do prázdna. Toto je nový nález **UX-42 (C)** a zároveň dôvod,
   prečo je routing (a nie lepší text) jediné správne riešenie.
3. **UX-23 čiastočne vyvrátené.** Dialóg **je** centrovaný natívny `<dialog>` so `showModal()`
   (focus trap, Escape, obnova fokusu — `Modal.svelte:43-56,123-133`); FE-8/C3 z plánu je
   landed. Na screenshote 04 je stred dialógu (x≈900) totožný so stredom okna; „vľavo od stredu“
   je optický efekt 630 px sidebaru. Potvrdená ostáva podstata: 11 sekcií na jednej scrollovanej
   strane širokej 640 px bez navigácie, s dlhými odsekmi pri každom nastavení.
4. **Návrh: dva hub tools** — `get_fleet_settings` (Client, readonly) a `set_fleet_setting`
   (Client `full`; alternatíva Master — otázka 1) — s **rovnakými menami ako Tauri príkazy**, aby
   verdikt `Routed { tool: "get_fleet_settings" }` nezaviedol tretí slovník. Oba sú čisté
   `Store` operácie (`service::settings::read_all`/`set`), `Deadline::Quick`, bez `confirm`.
   Rozpočet popisov treba zdvihnúť o ~600 B (dnes 97 B rezervy).
5. **Zo 74 LocalOnly (dnes 70) sa v Settings menia dva riadky na Routed**; `mcp_status`,
   `mcp_configure`, `install_fleet_hook`, `provision_hosts` ostávajú refused — ale UI ich má
   nahradiť **jednou kartou s faktami z `hubStatus`** (hub je control API; ty si klient `<name>`),
   nie dvoma odsekmi.
6. **IA:** ľavá navigácia so 7 skupinami (Hub & fleet · Automation · Limits · Control API ·
   Notifications · Terminal & composer · Setup & diagnostics), fleet-scoped skupiny označené a
   v hub režime s **jedným** bannerom „Fleet settings are the hub's — read from and written to
   `<url>` (paired as `<name>`)“. Dialóg ostáva na `Modal.svelte`, šírka `min(860px, 92vw)`.
7. Sedem nových nálezov UX-42…UX-48; dva PR (3a Rust + minimálny TS kontrakt, 3b IA), oba **M**.

## Mapa sekcií → príkazy → verdikt

Poradie podľa `SettingsDialog.svelte`. „Dnes (hub)“ = čo dialóg vykreslí, keď `$hubStatus.remote`.
Lokálne preferencie idú cez `prefs.ts` (localStorage, `readPref`/`writePref`) a hubu sa netýkajú.

| # | Sekcia (`data-testid`) | Príkazy / úložisko | Verdikt dnes | Dnes (hub) | Má byť |
|---|---|---|---|---|---|
| 1 | Hosts (`settings-hosts-line`) `:354-362` | `list_hosts` cez `hosts` store | Routed | „N configured · M offline“ + Open Hosts | **OK.** Bez zmeny |
| 2 | Hub (`hub-section`) `:364-540` | `hub_status`, `hub_pair`, `hub_disconnect`, `hub_stranded_token`, `hub_connection` | SameInBoth (×5) | 4 odseky prózy (klient čoho, `confirm_destructive` trap, untrusted marker, čo Disconnect nerobí) + Disconnect | **Karta faktov** (URL · klient · mód · stav spojenia · contract rev) + Disconnect; odseky do `<details>` „What is different as a client“; `confirm_destructive` odsek iba keď hub hlási, že je zapnuté (pozri tool 1, derived kľúč) |
| 3 | Projects (`projects-section`) `:550-607` | `get_fleet_settings`, `set_fleet_setting` (`projects.base_path`, `projects.layout`), `refresh_projects` | LocalOnly, LocalOnly, **Routed** | odsek `hubBlock('get_fleet_settings')` `:546` | **Editovateľné cez hub tool.** Hodnoty sú hubove (koreň projektov na každom hoste), `refresh_projects` už routuje — „Save & rescan“ funguje celý, len čo `set` routuje. `projects.local_env_base` je env **hubovho** procesu — popis v UI to má povedať |
| 4 | Setup guide (`onboarding-section`) `:609-635` | `onboardingWelcomed/Dismissed`, `hintsEnabled` (prefs) | lokálne | zobrazené celé | **Lokálne, ale „Replay setup guide“ v hub režime skryť/disable:** karta volá `check_local_prereqs`, `tunnel_status`, `mcp_status` (`OnboardingCard.svelte:33-37`), všetky LocalOnly — replay ukáže iba vetu odmietnutia (UX-46) |
| 5 | Copy on select `:636-644` | `copyOnSelect` (prefs) | lokálne | zobrazené | **Lokálne, OK** — ale presunúť pod *Terminal* (dnes pod Setup guide, UX-46) |
| 6 | Notifications + Idle (`notifications-section`) `:647-693` | `notifyStuckToast`, `notifyStuckOs`, `attentionIdleMinutes` (prefs) | lokálne | zobrazené | **Lokálne, OK.** Stuck prechody prichádzajú aj po hubovom event streame, takže toasty/OS notifikácie fungujú |
| 7 | Conversation composer (`composer-section`) `:695-728` | `composerPresets` (prefs) | lokálne | zobrazené | **Lokálne, OK** |
| 8 | Automation (`automation-section`) `:753-864` | `get_fleet_settings`, `set_fleet_setting` (`playbooks.*`, `gc.*`, `repair.*`, `reconcile.interval_secs`) | LocalOnly | odsek `hubBlock('get_fleet_settings')` `:734` | **Editovateľné cez hub tool.** Presne tieto kľúče riadia, čo hub urobí so sessions používateľa (GC ich zabije, playbook pošle Enter). Poznámka pri `tick`: „restart the hub to apply“ |
| 9 | Limits (`limits-section`) `:866-971` | `set_fleet_setting` (`tasks.*`, `sessions.lost_ttl_secs`, `move.*`, `usage.*`) | LocalOnly | odsek `:740` | **Editovateľné cez hub tool.** `move_session` routuje na hub → platia **hubove** `move.*` limity; `usage.*` poll robí hub (desktop v hub režime nepolluje) |
| 10 | Control API (MCP) (`McpSettings.svelte`) | `mcp_status`, `mcp_configure`, `install_fleet_hook`, `provision_hosts` | LocalOnly (×4) | dva odseky `:746,749` | **Read-only karta bez nového toolu:** „Control API = hub's, `<url>/mcp`; you are client `<name>` (`<mode>`)“ z `hubStatus`; provisioning/hook/tokeny = hub operator (`Access::Master`, `fleet-hub`) — jedna veta. `install_fleet_hook` (lokálny hook na *toto* API) je v hub režime N/A → nezobrazovať |
| 11 | Diagnostics (`diagnostics-section`) `:979-1007` | `collect_diagnostics`, `open_log_folder` | SameInBoth | zobrazené | **OK** (o tomto procese) |

Sekcie 3, 8 a 9 sú **jedna** vec — dvojica príkazov `get_fleet_settings`/`set_fleet_setting` nad
registrom `service::settings::SPECS` (22 kľúčov, `settings.rs:142-270`). Parita Settings = dva
riadky vo `verdicts.rs`.

Čo v Settings **nie je**, hoci sem patrí (iterácia 6): `list_account_usage`/`refresh_account_usage`
(LocalOnly; usage v pätičke).

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-16 | **Potvrdené, rozšírené.** (a) `hubBlock` skladá vetu ako `${REASONS[action]}. Do it on the hub (${url}).` (`hub.ts:239-246`); Projects, Automation a Limits volajú ten istý kľúč `get_fleet_settings` → identický odsek 3×. Control API volá `mcp_status` + `provision_hosts` → 2 odseky. (b) Sekcia Hub v `isRemote` vetve pridáva 4 odseky (`hub-connected`, `hub-confirm-note`, `hub-untrusted-note`, `hub-disconnect-note`), z nich `hub-confirm-note` opisuje stav, ktorý desktop nevie zistiť (`mcp.confirm_destructive` na hube nemá read). (c) Test `SettingsDialog.hub.test.ts:267-278` **pinuje** dnešné správanie: „replaces each of them with the reason“ a `expect(queryByTestId('gc-enabled')).toBeNull()` — PR musí tento test prepísať, nie obísť. (d) URL v „Do it on the hub (https://fleet.rlt.sk)“ je MCP endpoint bez webového UI — človek tam nemá kam kliknúť (→ UX-42). | screenshot 04; `SettingsDialog.svelte:82-86` komentár („four of the sections… replaced by the reason“), `:542-548,730-751`, `hub.ts:155-185` |
| UX-23 | **Čiastočne vyvrátené, podstata potvrdená.** *Vyvrátené:* „nie centrovaný modal“ — `Modal.svelte` je natívny `<dialog>` + `showModal()` (top layer, focus trap, `cancel` → Escape, obnova fokusu na otvárač `:56-67`), `.modal[open]:not(:modal)` má fallback `inset:0; margin:auto`. Na screenshote 04 dialóg x≈588–1213, okno x≈52–1748 → oba stredy ≈900. Dojem „vľavo“ vzniká, lebo stred *obsahovej* plochy (bez sidebaru) je ≈1215. FE-8/C3 z `docs/plans/2026-09-10-fleet-improvement-plan.md:97,207` je landed. *Potvrdené:* 11 sekcií, `width="min(640px, 92vw)"`, `.body { max-height: 85vh; overflow: auto }` → scroll cez ~2 obrazovky; každé nastavenie má 1–3 riadky `hook-desc` prózy (Limits: 10 polí × ~25 slov); žiadne záložky ani navigácia; nadpisy nekonzistentné (UX-47). | `Modal.svelte:123-168`, `SettingsDialog.svelte:351`, screenshot 04 |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-42 | C | **„Read and change them on the hub“ nemá kde.** Hub neservíruje žiadny tool pre fleet nastavenia (`TOOL_POLICIES` nemá `*settings*`; `docs/control-api-reference.md` 79 toolov, žiadny), `fleet-hub` CLI nemá `settings` subpríkaz, `hub.md` §Configuration pozná iba `hub.*`/`mcp.port`/`operator.host` cez flagy pri `serve`. `gc.*`, `playbooks.*`, `reconcile.*`, `repair.*`, `tasks.*`, `sessions.lost_ttl_secs`, `move.*`, `usage.*`, `projects.*` sa na hube dajú zmeniť iba `sqlite3` v kontajneri. To isté platí pre `mcp.confirm_destructive`: hub pri starte varuje „turn the setting off“ (`serve.rs:885-895`), ale nemá čím. `instead` vety pre `get_fleet_settings`/`set_fleet_setting` (`verdicts.rs:333-347`) a `REASONS.get_fleet_settings` (`hub.ts:181`) sú teda inštrukcia bez cieľa. | `crates/fleet-hub/src/main.rs:24-108`, `guard.rs:93-700`, `docs/hub.md:785-853` |
| UX-43 | H | **Klient je slepý k politike, ktorá zabíja jeho sessions.** `gc.work_idle_secs`, `gc.bg_idle_secs`, `playbooks.oom_recreate`, `sessions.lost_ttl_secs`, `move.max_*` sú fakty hubu, ktoré rozhodujú o sessions zobrazených v tomto okne (GC zabije, playbook recreate-ne, move odmietne `E_MOVE_TOO_LARGE`). `read_all` je čistý `Store` read (`settings.rs:398-413`), `list_projects` už routuje — pre odmietnutie *čítania* niet dôvodu. | `verdicts.rs:333-339`, `settings.rs:398` |
| UX-44 | M | **Hub sekcia = 4 odseky prózy pred jedným tlačidlom.** `hub-connected` (čo hub vlastní), `hub-confirm-note` (hypotetický `E_CONFIRM_REQUIRED` — relevantné iba ak hub má `confirm_destructive` zapnuté, čo desktop nevie), `hub-untrusted-note` (marker), `hub-disconnect-note`. Fakty (URL, meno klienta, mód, stav spojenia, contract revision z `hubConnection`) nie sú v jednom riadku, ale roztrúsené vo vetách. | `SettingsDialog.svelte:369-417`, `hub.ts:31-56` |
| UX-45 | M | **Desktop po reštarte nevie, či je `readonly` klient.** `HubStatus.client_mode` je `Some` iba hneď po spárovaní, `None` po reštarte („nothing stores it“ — `commands/hub.rs:48-51`), hoci `/pair` odpoveď `mode` nesie (`pairing.rs:37`). UI preto nemôže vopred vypnúť zápisy pre `readonly` klienta; každá routovaná mutácia (a navrhnutý `set_fleet_setting`) padne až po kliku s `E_FORBIDDEN`. | `commands/hub.rs:42-58`, `pairing.rs:30-40`, `docs/hub.md:586-589` |
| UX-46 | L | **Zle zaradené a v hub režime slepé prvky v Setup guide.** „Copy on select“ (terminálová preferencia) je pod nadpisom *Setup guide*; „Replay setup guide“ v hub režime znovu zobrazí kartu, ktorá volá tri LocalOnly príkazy a ukáže iba vetu „the setup checklist is about running a fleet from this machine“. | `SettingsDialog.svelte:609-645`, `OnboardingCard.svelte:33-37`, `hub_disabled.test.ts:174-195` |
| UX-47 | L | **Nekonzistentné nadpisy sekcií.** „Conversation composer“ je `<h4>` mimo `.section-header` (`:696`) → vykreslí sa ako veľký tučný titul, kým ostatné sekcie majú uppercase small-caps (`settings_dialog.css:13-24`); `hosts-line h4` má vlastný štýl (`:1031-1040`). Vidno na screenshote 04. | `SettingsDialog.svelte:696,1031-1040` |
| UX-48 | L | **Počty príkazov sa rozišli.** CLAUDE.md „all 123 commands“ (`:110`), README „74 zo 123“; generovaná tabuľka hovorí „130 commands, 39 route, 1 routes except…, 70 refuse, 20 same“ a `hub_verdicts.generated.json` má 70+39+1+20 = 130. Pravda je generovaný súbor. | `CLAUDE.md:110`, `docs/hub.md` generovaný blok, `src/lib/hub_verdicts.generated.json` |

## Návrh hub tools

Zásady: (1) meno toolu = meno Tauri príkazu (žiadny tretí slovník, `check` v `tests_routing.rs`
porovnáva tool z požiadavky s riadkom vo `VERDICTS`); (2) obe operácie sú `Store`-only → `Quick`;
(3) popis = jedna klauzula (rozpočet `the_served_definition_budget_stays_bounded`, dnes 57 603 z
57 700 B); (4) próza do `docs/hub.md` a `docs/control-api.md`, nie do popisu.

### Tool 1 — `get_fleet_settings`

| | |
|---|---|
| Súbor | `crates/fleet-core/src/mcp/tools/fleet.rs` (`fleet_router`) |
| Popis (návrh) | `"Every operator setting — gc, playbooks, reconcile/repair cadence, task and move limits, projects roots, usage — with its effective value; JSON map key → string."` |
| Params | žiadne |
| Výsledok | `BTreeMap<String,String>` = `service::settings::read_all(&store)` — **totožné s Tauri príkazom**, takže `remote.rs` deserializuje priamo do návratového typu a `hub_contract.golden.json` sa nemení (kontrakt pokrýva pomenované structy, nie mapy) |
| Policy | `access: Access::Client, readonly: true, confirm: false, deadline: Deadline::Quick` |
| Prečo Client/readonly | Je to čítanie faktov, ktoré určujú osud sessions klienta (UX-43). `readonly` token má vidieť to, čo mu hub urobí. Bez tajomstiev: `read_all` nevracia `mcp.token` ani `hub.*` |
| Derived kľúč (voliteľné, otázka 2) | pridať do výsledku `mcp.confirm_destructive` (read-only, mimo `SPECS`, ako `projects.resolved_base`) → Hub karta ukáže „destructive confirmations: off“ namiesto hypotetického odseku; `validate()` ho ako neznámy kľúč odmietne pri zápise |

### Tool 2 — `set_fleet_setting`

| | |
|---|---|
| Súbor | `fleet.rs`; params v `mcp/tools/params.rs` |
| Popis (návrh) | `"Validate and persist one operator setting by key; returns the whole effective map."` |
| Params | `SetFleetSettingParams { key: String /// Setting key, one of the registered ones (gc.enabled, gc.work_idle_secs, playbooks.press_enter, projects.base_path, …). , value: String /// New value as the setting stores it: "true"/"false", integer seconds/units, or a JSON object for the map kinds. }` — každé pole s `///` (test `every_tool_parameter_is_documented`) |
| Výsledok | `BTreeMap<String,String>` (po `set`, cez `read_all`) |
| Chyby | `E_INVALID` z `settings::validate` (neznámy kľúč, rozsah, tvar JSON) — rovnaká správa, akú dnes ukazuje `limitsError` |
| Policy (odporúčanie) | `access: Access::Client, readonly: false, confirm: false, deadline: Deadline::Quick` |
| Prečo Client `full`, nie Master | (a) `hub.md` definuje fleet admin ako členstvo hostov, credentials, `apply_sync` (zápis na disky hostov), párovanie — nastavenia tam nie sú; `full` = „whole-fleet session control“ a GC/playbooky sú presne hromadná session kontrola, ktorú `full` klient už má per session (`kill_session`, `recreate_session`). (b) Každý kľúč je ohraničený `SPECS` (`Kind::Secs ≤ 10 rokov`, `Int{min,max}`, `Choice`, validovaný `PathMap`). (c) Master-only by nastavenia nechalo **nedosiahnuteľné odkiaľkoľvek** (UX-42), kým nevznikne `fleet-hub settings` CLI. (d) Desktop je typicky *trusted* klient („keyboard that is yours“). Protiargument: `projects.base_path` rozhoduje o layoute disku na hostoch — analógia so `set_host_layers` (Master, „decides what apply_sync writes“). Rozhodnutie vlastníka — otázka 1 |
| `confirm` | **nie**: hub nemá approvera (`serve.rs:739-749`), `confirm: true` by tool na hube zablokoval |

### Čo sa mení vo `VERDICTS`

| Príkaz | Dnes | Po PR-3a | Poznámka |
|---|---|---|---|
| `get_fleet_settings` | `LocalOnly { instead: "these settings drive the reconcile tick… read and change them on the hub" }` | `Routed { tool: "get_fleet_settings" }` | `REASONS.get_fleet_settings` v `hub.ts` **musí zmiznúť** (test „every REASONS key that is a command name is local_only“) |
| `set_fleet_setting` | `LocalOnly { … }` | `Routed { tool: "set_fleet_setting" }` | zmizne z `gatedBySettingsDialog` allowlistu v `hub_verdicts.test.ts:176` |
| `mcp_status`, `mcp_configure` | LocalOnly | **ostáva** | hub *je* control API; UI to ukáže ako fakt z `hubStatus`, nie ako odmietnutie |
| `install_fleet_hook` | LocalOnly | ostáva | hook na *toto* API — v hub režime N/A, UI prvok nezobrazovať |
| `provision_hosts` | LocalOnly | ostáva | hubov `provision_hosts` je `Access::Master`; klient nikdy nie je master — správne odmietnutie; skrátiť `REASONS.provision_hosts` na jednu vetu |
| `check_local_prereqs`, `tunnel_status` | LocalOnly | ostáva (iterácia 6/19) | poznámka: `fleet_health` už nesie per-host tunnel health hubu — kandidát na náhradu `tunnel_status` bez nového toolu |

`instead` texty ostávajúcich riadkov sa nemenia → `local_only.golden.json` sa mení iba
odstránením dvoch záznamov (`REGEN_LOCAL_ONLY=1`).

### Rozpočet popisov

Odhad: `get_fleet_settings` ≈ 18 (meno) + ~150 (popis) + ~40 (prázdna schéma) ≈ 210 B;
`set_fleet_setting` ≈ 17 + ~90 + ~330 (dve dokumentované polia) ≈ 440 B. Spolu ~650 B nad
57 603 → **zdvihnúť `BUDGET_BYTES` na 58 400** s odsekom v doc-komentári podľa vzoru
(`tests.rs:2331-2356`: čo pribudlo, koľko meralo pred, prečo sa nedalo ušetriť). Readonly plocha
ostáva < ½ (pribúda tam len tool 1).

### Starší hub

Hub, ktorý tieto tools ešte neservíruje, odpovie `E_FORBIDDEN` (gate zlyhá „closed“ na mene
toolu) alebo `E_HUB_PROTOCOL`. `SettingsDialog` má tieto kódy (plus `E_HUB_CONTRACT`,
`E_HUB_UNREACHABLE`, `E_HUB_UNAVAILABLE`) čítať ako „this hub cannot serve settings yet — update
the hub“ **jedným riadkom v sekcii, bez toastu** — vzor `NewSessionDialog.svelte:180-205`
(`HUB_CANNOT_SCAN`). Porovnávať kód, nikdy text správy.

## Návrh IA Settings

### Navigácia (7 skupín, ľavý stĺpec)

Jedna scrollovaná strana → **ľavá navigácia** (nie horné taby: 7 položiek s dlhšími názvami sa do
640 px nezmestí; ľavý stĺpec 180 px navyše prezradí rozsah dialógu na prvý pohľad). Dialóg ostáva
`<Modal>`; `width="min(860px, 92vw)"`; `.body` → grid `180px 1fr`, nav `position: sticky`.

| # | Skupina | Obsah (dnešné sekcie) | Rozsah |
|---|---|---|---|
| 1 | **Hub & fleet** | Hub (karta faktov + Pair/Disconnect), Hosts riadok, Projects (roots + layout) | fleet |
| 2 | **Automation** | playbooks, GC, repair, tick | fleet |
| 3 | **Limits** | tasks, lost sessions, move caps, usage | fleet |
| 4 | **Control API** | `McpSettings` standalone / read-only karta v hub režime | fleet |
| 5 | **Notifications** | toast, OS notification, idle threshold | local |
| 6 | **Terminal & composer** | Copy on select (presun z Setup guide), composer chips | local |
| 7 | **Setup & diagnostics** | Replay guide, hints, Copy diagnostics, Open log folder | local |

- Nav ako `role="tablist"` s `<button role="tab" aria-selected aria-controls>`, obsah
  `role="tabpanel"`; šípky ↑/↓ prepínajú, Home/End; prvý fokus na aktívny tab (`data-autofocus`
  z `Modal.svelte:47-49`). Posledná skupina si pamätá cez `prefs` (`settings.tab`).
- Fleet-scoped skupiny (1–4) nesú malý badge `fleet` v nav; lokálne (5–7) `this app`. To je
  hierarchia, ktorá dnes chýba: používateľ vidí, čo mení pre celý fleet a čo iba pre toto okno.
- Deep-link: `settingsOpen` store rozšíriť na `false | true | SettingsTab`; pätičkový hub badge a
  „hub unavailable“ banner (`App.svelte:810,824`) otvoria priamo *Hub & fleet*.
- Próza: každý `hook-desc` skrátiť na ≤ 12 slov vedľa poľa; dlhé vysvetlenie (repair, carry
  limity, prices JSON) do `<details>`/`title`. Nadpisy zjednotiť cez `.section-header` (UX-47).

### Hub režim (`$hubStatus.remote`)

- **Jeden banner** navrchu každej fleet-scoped skupiny (1–4), spoločný komponent
  `HubScopeNote.svelte` (nový, `src/lib/`): „Fleet settings are the hub's — read from and written
  to `https://fleet.rlt.sk` (paired as `mac-desktop`).“ Props `{ what: 'settings' | 'catalog' |
  'git' }` — **iterácie 4 a 5 ho znovu použijú** pre Assets a Files.
- Skupiny 1–3: rovnaké ovládacie prvky ako standalone, hodnoty z `get_fleet_settings` (hub),
  zápis cez `set_fleet_setting` (hub). Pri `tick` a `repair` poznámka „restart the hub to apply“.
  Po `E_FORBIDDEN` z zápisu (readonly klient — UX-45): inline `err` „this client is readonly on
  the hub“, polia ostávajú viditeľné.
- **Hub karta** (skupina 1) namiesto 4 odsekov: riadky `Hub · Client · Access · Connection ·
  Contract` z `hubStatus`/`hubConnection`; `Disconnect` + jedna veta „forgets the token here,
  revokes nothing on the hub“; `<details>` „What is different as a client“ s dnešnými troma
  odsekmi. Odsek o `E_CONFIRM_REQUIRED` iba ak `get_fleet_settings` vráti
  `mcp.confirm_destructive = "true"` (otázka 2), inak riadok „Destructive confirmations: off“.
- **Control API karta** (skupina 4): „The control API is the hub's: `<url>/mcp`. This window is
  client `<name>`. Provisioning hosts, installing hooks and rotating host tokens are the hub
  operator's (`fleet-hub`, master token).“ Žiadny `mcp_status` call, žiadne dve vety odmietnutia.
- Skupina 7: „Replay setup guide“ disabled s tooltipom „the checklist is about running a fleet from
  this machine“ (UX-46).
- Standalone: nič z uvedeného sa nezobrazí; `SettingsDialog.hub.test.ts:286` „standalone is
  untouched“ ostáva zelený.

## Recept — ako dostať `LocalOnly` príkaz na `Routed`

Kontrolný zoznam pre iterácie 4 a 5 (Assets, Files). Cesty sú relatívne ku koreňu repa. Poradie
je záväzné: testy sú písané tak, že každý krok bez nasledujúceho je červený.

1. **Over, či hub tool existuje.** `grep -n 'name: "<tool>"' crates/fleet-core/src/mcp/guard.rs`.
   Ak áno a jeho parametre mapujú **pole na pole** na argumenty príkazu, preskoč na krok 4.
   Ak mapujú iba čiastočne — **nerobiť lossy mapping** (pravidlo *parity or refusal*,
   `src-tauri/src/backend/routing.rs:38-60`): rozšíriť tool o chýbajúce polia (`#[serde(default)]`,
   voliteľné) alebo použiť `RoutedUnless` ako `repair_session`.
2. **Nový tool (ak treba).**
   a. Params struct v `crates/fleet-core/src/mcp/tools/params.rs` — `#[derive(serde::Deserialize,
   schemars::JsonSchema)]`, **každé pole s `///`** (test `every_tool_parameter_is_documented`).
   b. Metóda v doménovom súbore (`fleet.rs`, `repo.rs`, `assets.rs`, …) v jeho `#[tool_router]`
   bloku: `#[tool(description = "<jedna klauzula>")] pub(super) async fn <tool>(&self, …)`,
   volá **tú istú `service::*` funkciu ako Tauri príkaz**; výsledok `ok_json(&…)`. Meno toolu =
   meno príkazu, ak nie je dôvod inak.
   c. Riadok v `guard::TOOL_POLICIES` (`crates/fleet-core/src/mcp/guard.rs:93-700`): `access`
   (Master = fleet admin; Client = session control), `readonly` (iba čisté čítanie), `confirm`
   (na hube **nie** — nemá approvera), `deadline` (`Quick` store/1× SSH, `Lifecycle` viac SSH,
   `LongPoll` čakacie). Testy: `every_router_tool_has_exactly_one_tool_policy_row`,
   `readonly_tools_are_client_tools_or_…`, `annotations_follow_the_policy_table`.
   d. Rozpočet: `the_served_definition_budget_stays_bounded` (`mcp/tools/tests.rs:2325`) padne —
   zdvihnúť `BUDGET_BYTES` **s odsekom v komentári** (čo, koľko, prečo sa nedalo trimovať).
   e. `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (beh s regen hlási FAILED —
   spustiť druhýkrát bez env). Ručne doplniť meno do indexu v `docs/control-api.md:238+`
   („Index by area“). Ak sa tool týka workflowu agentov, jedna veta do
   `skills/claude-fleet-control/SKILL.md`.
3. **Verdikt.** `src-tauri/src/backend/verdicts.rs`: `LocalOnly { instead }` →
   `Routed { tool: "<tool>" }` (alebo `RoutedUnless`). Test `every_routed_tool_is_a_tool_the_hub_serves`
   overí, že tool je v `TOOL_POLICIES`.
4. **Hub klient.** `src-tauri/src/backend/remote.rs`, `impl HubBackend`: `pub async fn <command>(&self,
   …) -> Result<T, IpcError> { self.route("<command>", &args).await }`. `args` = vlastný args struct
   príkazu (`Serialize`) tam, kde ide na drôt pole za poľom; `json!` literál tam, kde nie (default,
   clamp, kľúč len keď je nastavený) — rozdiel zapísať do doc-komentára pri volaní. Nepísať
   `route("…")` do komentára v inom tvare (testy blankujú `//` riadky).
5. **Telo príkazu.** `src-tauri/src/commands/<area>.rs`: v `pub(crate) mod routed` pridať
   `pub async fn <command>(backend: &FleetBackend, …) { match backend.hub() { Some(hub) =>
   hub.<command>(…).await, None => <lokálna service cesta> } }`; telo `#[tauri::command]` volá
   `routed::<command>(&backend, …)` a **odstráni `backend.refuse_local_only("<command>")?`**.
   Sync `fn` → `async fn`, ak awaituje. Test `every_commands_body_does_what_its_row_says`
   vyžaduje reťazec `routed::` v tele. Nikdy nedržať `Store` guard cez `.await`.
6. **Routing test.** `src-tauri/src/backend/tests_routing.rs`: `Case` do `routed_read_cases()`
   (`:285`) alebo `routed_mutation_cases()` (`:695`): `(command, tool, json!(<presne odoslané args,
   NEdefaultné hodnoty>), "<payload deserializovateľný do návratového typu>", Box::new(|b, s, h|
   block_on(commands::<area>::routed::<command>(b, …, s)).map(|_| ())))`. Test
   `every_routed_row_is_driven_by_a_case` inak padne. Ak sa mení standalone cesta, doplniť aj
   sekciu 2 (`standalone_reads_still_come_from_the_local_store`).
7. **Golden odmietnutí.** `REGEN_LOCAL_ONLY=1 cargo test -p claude-fleet --lib local_only`
   (súbor `src-tauri/src/backend/local_only.golden.json` stratí záznam). Prečítať diff.
8. **Publikovanie verdiktov.** `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`
   (prepíše `src/lib/hub_verdicts.generated.json` a tabuľku v `docs/hub.md`; beh s regen padne
   raz úmyselne — spustiť znovu bez env).
9. **Wire typy.** Nový návratový struct / nové pole: `Serialize` + `Deserialize`,
   `#[serde(default)]` na všetko, čo starší hub nepošle (inak výpadok proti staršiemu hubu),
   sample do `src-tauri/src/backend/tests_contract.rs::the_whole_contract`,
   `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (dvakrát). Mapy a primitíva
   (`BTreeMap`, `String`) kontrakt nevyžaduje.
10. **Frontend kontrakt.** `src/lib/hub.ts`: **zmazať** kľúč z `REASONS` (test „every other REASONS
    key that is a command name is local_only“); ak UI gate-uje mutáciu na stav spojenia, pridať
    do `ROUTED_ACTIONS` a používať `hubActionBlocked`. `src/lib/hub_verdicts.test.ts`: odstrániť
    z `LOCAL_ONLY_WITH_NO_DIRECT_REASONS_ENTRY`. `npx vitest run src/lib/hub_verdicts.test.ts`.
11. **Komponent.** Odstrániť `{#if !ownsFleet}` vetvu pre danú sekciu; `ownsTheFleet` guardy
    nechať len pri stále-LocalOnly volaniach; starší hub (`E_FORBIDDEN`, `E_HUB_PROTOCOL`,
    `E_HUB_CONTRACT`, `E_HUB_UNREACHABLE`, `E_HUB_UNAVAILABLE`) riešiť inline jedným riadkom, bez
    toastu; `hubBlock(...)` volania na zmazaný kľúč prepísať (inak `svelte-check` padne na
    `HubAction`).
12. **Frontend testy.** `<Komponent>.hub.test.ts`: zoznam „does not call the local-only commands on
    mount“ zúžiť; pridať „calls `<command>` in remote mode and renders the hub's values“; „standalone
    is untouched“ ostáva. `src/App.hub.test.ts` „the commands the UI calls unprompted“, ak sa
    volanie deje pri bootstrape.
13. **Docs.** `docs/hub.md` → *What is different from standalone* (bullet o danej ploche),
    prípadne *Known limitations*; `CLAUDE.md` počet príkazov (UX-48). `docs/control-api.md`
    index (krok 2e).
14. **Celý suite, nepipe-ovaný.** `export CARGO_TARGET_DIR=…` (memory: worktree zdieľa target),
    `scripts/ci-local.sh`; frontend `npx vitest run` + `npx svelte-check`. Nefiltrovať.

Kroky 1–9 sú **PR-a (Rust)**, 10–13 **PR-b (TS)**; krok 10 ale PR-a nevyhnutne rozbije (`REASONS`
kľúč prestane byť `local_only`) → PR-a musí niesť **minimálny** TS diff z krokov 10–11, PR-b robí
IA. Toto rozdelenie platí aj pre iterácie 4 a 5.

## PR plán

### PR-3a — Rust: hub tools + verdikty + minimálny TS kontrakt (**M**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `crates/fleet-core/src/mcp/tools/params.rs` | `SetFleetSettingParams { key, value }` s `///` na oboch poliach |
| 2 | `crates/fleet-core/src/mcp/tools/fleet.rs` | `get_fleet_settings` (bez params, `settings::read_all`), `set_fleet_setting` (`settings::set` → `read_all`), v `fleet_router`; voliteľne derived `mcp.confirm_destructive` (otázka 2) |
| 3 | `crates/fleet-core/src/mcp/guard.rs` | dva riadky `TOOL_POLICIES` (Client/readonly/Quick; Client/!readonly/Quick alebo Master — otázka 1) |
| 4 | `crates/fleet-core/src/mcp/tools/tests.rs:2357` | `BUDGET_BYTES` 57 700 → ~58 400 + odsek |
| 5 | `docs/control-api-reference.md` | **generované**: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (2×) |
| 6 | `docs/control-api.md:238-245` | dve mená do indexu *Fleet & hosts* |
| 7 | `src-tauri/src/backend/verdicts.rs:333-347` | dva riadky → `Routed` |
| 8 | `src-tauri/src/backend/remote.rs` | `get_fleet_settings(&self)`, `set_fleet_setting(&self, key, value)` cez `route("…", &json!({…}))` |
| 9 | `src-tauri/src/commands/sessions.rs:276-301,454+` | `routed::get_fleet_settings` / `routed::set_fleet_setting`; telá `async`, bez `refuse_local_only` |
| 10 | `src-tauri/src/backend/tests_routing.rs` | Case do `routed_read_cases` (`get_fleet_settings`, `json!({})`, payload `{"gc.enabled":"true"}`) a `routed_mutation_cases` (`set_fleet_setting`, `json!({"key":"gc.enabled","value":"true"})`); standalone test v sekcii 2 |
| 11 | `src-tauri/src/backend/local_only.golden.json` | **generované**: `REGEN_LOCAL_ONLY=1 …` |
| 12 | `src/lib/hub_verdicts.generated.json`, `docs/hub.md` tabuľka | **generované**: `REGEN_HUB_VERDICTS=1 …` (2×) |
| 13 | `src/lib/hub.ts:181-182` | zmazať `REASONS.get_fleet_settings` |
| 14 | `src/lib/hub_verdicts.test.ts:176` | zmazať `gatedBySettingsDialog` |
| 15 | `src/lib/SettingsDialog.svelte:82-86,143-172,542-549,730-752` | `onMount`: `loadFleetSettings()` aj v hub režime (iba `mcpStatus()` ostáva za `ownsFleet`); Projects/Automation/Limits bez `!ownsFleet` vetvy; MCP sekcia dočasne ostáva s `hubBlock('mcp_status')`; starší hub → jeden riadok |
| 16 | `src/lib/SettingsDialog.hub.test.ts:249-299` | prepísať „replaces each of them with the reason“ (pozri Akceptačné testy) |
| 17 | `docs/hub.md` *What is different from standalone*, `CLAUDE.md:110` | bullet „Fleet settings are read and written on the hub“; 123 → generovaný počet |

Poradie: 1–6 (fleet-core, zelené samostatne) → 7–12 (src-tauri) → 13–17 (TS + docs).

### PR-3b — Svelte: IA Settings (**M**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `src/lib/SettingsNav.svelte` (nový) | tablist so 7 skupinami, badge `fleet`/`this app`, klávesnica, `aria-*` |
| 2 | `src/lib/HubScopeNote.svelte` (nový) | jednoriadkový banner `{ what }` — zdieľaný s iteráciami 4/5 |
| 3 | `src/lib/HubStatusCard.svelte` (nový) | karta faktov + Disconnect + `<details>`; nahrádza `:369-417` |
| 4 | `src/lib/SettingsDialog.svelte` | grid `180px 1fr`, `width="min(860px, 92vw)"`, sekcie → panely podľa skupín, „Copy on select“ do skupiny 6, „Replay“ disabled v hub režime, `.section-header` všade, skrátené `hook-desc`, `settings.tab` v `prefs` |
| 5 | `src/lib/McpSettings.svelte` | `{#if remote}` read-only karta (URL, klient, mód) bez `mcp_status`; provisioning veta |
| 6 | `src/lib/settings_dialog.css` | nav, grid, badge, `<details>` |
| 7 | `src/lib/app_views.ts` / `App.svelte:41,496,606,810,824` | `settingsOpen` prijíma cieľovú skupinu; pätička otvára *Hub & fleet* |
| 8 | `src/lib/SettingsDialog.test.ts`, `SettingsDialog.hub.test.ts`, `src/App.hub.test.ts` | pozri Akceptačné testy |

## Akceptačné testy

### Rust — `crates/fleet-core/src/mcp/tools/tests.rs`

- `every_router_tool_has_exactly_one_tool_policy_row`, `every_tool_parameter_is_documented`,
  `annotations_follow_the_policy_table` zelené po pridaní oboch toolov.
- `the_served_definition_budget_stays_bounded`: nová konštanta, `ro_bytes < bytes / 2` platí.
- `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` (`:356`): doplniť
  `get_fleet_settings` medzi povolené a `set_fleet_setting` medzi odmietnuté pre `readonly`.
- Nový: `set_fleet_setting_rejects_unknown_key_and_out_of_range_with_e_invalid` — `key:
  "nope"` → `E_INVALID "unknown setting nope"`; `gc.sweep_interval_secs = "-1"` → `E_INVALID`;
  `gc.enabled = "true"` → mapa obsahuje `"gc.enabled": "true"`.

### Rust — `src-tauri/src/backend/`

- `tests_routing.rs`: `every_routed_row_is_driven_by_a_case` (dva nové Case),
  `every_commands_body_does_what_its_row_says`, `every_routed_tool_is_a_tool_the_hub_serves`,
  `every_local_only_message_is_the_one_the_fixture_records` po `REGEN_LOCAL_ONLY`.
- `a_routed_read_answers_the_hub_and_not_the_local_database` rozšíriť o `get_fleet_settings`:
  lokálny store s `gc.enabled=false`, fake hub odpovie `{"gc.enabled":"true"}` → príkaz vráti
  `"true"`.
- `tests_verdict_gen.rs`: `generated_json_is_current`, `doc_table_is_current` po regen.

### Frontend — Vitest (`npx vitest run src/lib/SettingsDialog.hub.test.ts src/lib/SettingsDialog.test.ts src/lib/hub_verdicts.test.ts src/App.hub.test.ts`)

`SettingsDialog.hub.test.ts` (paired, `hubStatus = remote`):

- „calls `get_fleet_settings` in remote mode and renders the hub's values“ — fake `invoke` vráti
  `{"gc.enabled":"true","gc.work_idle_secs":"7200"}` → `gc-enabled` je `checked`,
  `gc-work-hours` má hodnotu `2`.
- „toggling a playbook writes through `set_fleet_setting` to the hub“ — klik na
  `playbook-press-enter` → `invoke('set_fleet_setting', {key:'playbooks.press_enter', value:'true'})`.
- „an older hub answers `E_FORBIDDEN` and the section says so in one line, without a toast“ —
  `get_fleet_settings` odmietne `E_FORBIDDEN` → `settings-hub-unsupported` obsahuje „update the
  hub“, `pushError` nezavolaný, `gc-enabled` neexistuje.
- „a readonly client's write is refused inline“ — `set_fleet_setting` → `E_FORBIDDEN` → `err`
  pri sekcii, pole ostáva.
- „does not call `mcp_status` on mount“ (zúžený zoznam z `:256-265`); „shows one hub banner per
  fleet group and no repeated reason“ — `queryAllByTestId('hub-scope-note').length === 4`,
  `queryByTestId('projects-remote')` je `null`.
- „Replay setup guide is disabled with the reason“; „standalone is untouched“ ostáva.

`SettingsDialog.test.ts` (standalone):

- „renders the seven groups in the nav and opens Automation on click“; „arrow keys move between
  groups“; „remembers the last group across opens“ (prefs).
- Existujúce testy (`:144-330`) po presune sekcií do panelov musia najprv kliknúť na skupinu —
  jeden helper `openGroup('Automation')`.

`hub_verdicts.test.ts`: zelený po regen bez ďalších zmien (obe mená sú v `routed`).

`App.hub.test.ts`: „the hub badge opens Settings on the Hub & fleet group“.

### Manuálne (screenshot podľa README §1)

Hub režim, Settings → *Automation*: jeden banner, hodnoty hubu, zmena `gc.enabled` sa prejaví v
`docker exec fleet-hub sqlite3 … "select value from settings where key='gc.enabled'"`.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| PR-3a položky 1–6 (fleet-core tools, policy, budget, reference) | S | ~90 riadkov + generované |
| PR-3a položky 7–12 (verdikty, remote, commands, routing testy, goldeny) | S | ~80 riadkov + testy ~60 + generované |
| PR-3a položky 13–17 (TS kontrakt, dialóg bez odmietnutí, testy, docs) | S | ~60 riadkov + testy ~80 |
| **PR-3a spolu** | **M** | ≈230 riadkov bez testov a generovaných súborov |
| PR-3b položky 1–3 (nové komponenty) | S | ~180 riadkov |
| PR-3b položky 4–7 (dialóg, McpSettings, CSS, deep-link) | M | ~220 riadkov, prevažne presuny |
| PR-3b položka 8 (testy) | S | ~150 riadkov |
| **PR-3b spolu** | **M** | ≈400 riadkov — nad limitom ~300 z README §5; prirodzený odštep: PR-3b1 (nav + panely + testy), PR-3b2 (HubStatusCard + McpSettings karta + skrátená próza) |

## Otázky pre vlastníka

1. **`set_fleet_setting`: `Access::Client` (`full`) alebo `Access::Master`?** Odporúčam Client —
   inak sú nastavenia na hube nedosiahnuteľné, kým nevznikne `fleet-hub settings get|set` CLI
   (ktoré by som pri Master voľbe pridal do PR-3a ako položku 18, ~S). Alternatíva: viazať zápis
   na `trusted` klienta (`set_client_trust` — „keyboard that is yours“), čo ale rozširuje význam
   trustu z „nemarkuj prompty“ na „smie meniť politiku“.
2. **`mcp.confirm_destructive` ako derived read-only kľúč v `get_fleet_settings`?** Umožní Hub
   karte ukázať skutočný stav namiesto hypotetického odseku. Ísť ďalej a pridať ho do `SPECS`
   (`Kind::Bool`), aby sa dal z klienta vypnúť (hub dnes hovorí „turn it off“, ale nemá čím)?
   Riziko: na standalone desktope ho zapisuje aj `mcp_configure` — dva zapisovatelia jedného kľúča.
3. **Persistovať `client_mode` pri párovaní** (`hub.client_mode`, UX-45), aby UI mohlo vopred
   označiť zápisy ako nedostupné pre `readonly` klienta? Mód sa mení iba revoke + re-pair, takže
   hodnota z `/pair` je presná.
4. **Ľavá navigácia so 7 skupinami** vs. horné taby (max 5, zlúčiť Notifications + Terminal &
   composer + Setup do „This app“)? Odporúčam ľavú nav — badge `fleet`/`this app` je hlavný
   prínos.
5. **`reconcile.interval_secs` a `repair.tick_interval_secs` na hube:** hub ich číta pri starte
   („restart to apply“). Stačí kópia „restart the hub to apply“, alebo má hub tick prečítať
   nastavenie pri každom prechode (backend zmena, mimo tejto šošovky)?
6. **Rozpočet popisov 57 700 → ~58 400 B** akceptovateľný, alebo radšej trimovať existujúci popis
   (`fleet_health` má 480 B — najdlhší kandidát)?
7. **Rozdelenie PR-3b na dva** (nav + panely / karty + próza), aby každý ostal pod ~300 riadkami?

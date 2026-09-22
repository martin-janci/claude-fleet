# Iterácia 06 — Hub parita: Hosts view, účty, projekty

**Šošovka:** hub-client parita v Hosts view (⌘1) — zoznam hostov, stĺpec účtov s usage, detail
hosta (USAGE blok, Integration, Danger), pätička `usage …`, pridávanie projektov ·
**Zasahuje:** UX-19, UX-20, usage časť pätičky (UX-14 iba okrajovo), root-cause klaster 3
(„hub režim odpovedá prózou namiesto funkcie“) · **Vstup:** `docs/ux/2026-09-21-audit/README.md`,
`iterations/consolidation-01.md` (záväzné D1–D6, §5 položky pre šošovku 6), screenshot 03,
iterácie 03 (sekcia *Recept*), 04 a 05, `docs/hub.md`,
`docs/superpowers/specs/2026-09-13-hosts-view-and-account-usage-design.md`,
`docs/superpowers/specs/2026-09-18-host-agent-design.md`, kód na `6c8488ca` · **Režim:**
read-only review, žiadne zmeny kódu, žiadny `cargo` beh (`npx vitest` nespúšťaný — čísla
testov sú z čítania). **Štvrtá hub-parity iterácia** (3 = Settings, 4 = Assets, 5 = Files/git,
6 = Hosts/účty/projekty). Postup implementácie je **recept z iterácie 03**
(`03-hub-parity-settings.md` → *Recept — ako dostať LocalOnly príkaz na Routed*, kroky 1–14);
tento dokument ho neopakuje, iba naň odkazuje krokom.

Platia rozhodnutia konsolidácie-01: D3 rebrík T0 readonly · T1 full · T2 full+trusted · T3
master, `confirm: false` na hube; D4 `BUDGET_BYTES` sa zdvíha raz v UXPR-09 na 63 800 (táto
iterácia iba uvádza svoju deltu); D5 `HubScopeNote` + `hub_inline_state` z UXPR-07; D6 pravidlá
routing PR. Fronta končí UXPR-20 → nové PR začínajú **UXPR-21**; nálezy od **UX-68**.

## Zhrnutie

1. **Hub usage polluje — desktop ho iba nesmie čítať.** `fleet-hub serve` spúšťa ten istý
   `spawn_account_usage_tick` s tou istou `UsageCache` ako desktop (`crates/fleet-hub/src/serve.rs:801-811`,
   tick každých 60 s, floor 5 min na účet — `service/tick.rs:19`) a každý zmenený snapshot
   emituje `account_usage:updated`, ktoré desktop cez `GET /events` dostáva a merguje
   (`src-tauri/src/backend/events.rs:583`, `src/lib/events.ts:177,268`,
   `account_usage_store.ts:95-99`). Chýba **iba čítanie**: `list_account_usage` je LocalOnly s vetou
   „the cache is empty; read usage on the hub“ (`verdicts.rs:520-526`) a doc-komentár príkazu
   tvrdí „there is nothing to route to“, lebo `usage_report` má iný tvar
   (`src-tauri/src/commands/account_usage.rs:5-10`). Premisa neplatí: hub má presne
   `AccountUsageSnapshot` v pamäti, len bez toolu. To je koreň `?` v pätičke aj v stĺpci účtov
   (UX-69) a dôvod, prečo je T0 tool `list_account_usage` najlacnejšia parita celej fronty
   (~185 B).
2. **`?` má dve vrstvy.** (a) Po štarte je store prázdny (App.svelte:241-244 `loadAccountUsage`
   v remote režime preskočí) — každý účet ukazuje `…`/„usage checking…“ až kým hub nepošle
   *zmenený* snapshot, čo je pri 5-minútovom floor-e až 5 min po každom spustení; po výpadku
   spojenia sa `FleetResync` re-listuje sessions/hosts/tasks/accounts, **nie usage**
   (`backend/events.rs:617-630`). (b) `?` v stĺpci znamená „snapshot prišiel, ale `usage: null`“
   — účet, ktorého posledný fetch zlyhal (`login_expired`, `no_credentials`, `host_unsupported`,
   `no_online_host`…) a ktorý nikdy nemal `ok`; `freshnessMark` vráti generické „No usage known
   for this account yet“ a `status`/`detail` zo snapshotu zahodí (`src/lib/hosts_view.ts:264-284`).
   V hub režime je navyše `u`/refresh disabled, takže `?` je slepá ulička (UX-68). Na screenshote
   sú to `m.janci@32bit.sk` (host `claude-fleet-htz` online → chyba na hoste) a
   `mj.janci@gmail.com` (hosty `local` skrytý hubom + `mac`).
3. **UX-19 potvrdené a presnejšie:** riadok *Integration → Token* vypíše ako *hodnotu* celú vetu
   odmietnutia (`HostDetail.svelte:256-258`), token-mode `<select>` a *Rotate token…* používajú
   dôvod `remove_host` (`:81,:246-247,:271`) — jeden ovládač, dve rôzne vety (UX-72); *Danger*
   tlačidlá sú disabled s tooltipom (`:283-299`), nie viditeľne funkčné, ako tvrdí audit.
   **UX-20 potvrdené:** `＋ Add project…` disabled iba s `title` (`Sidebar.svelte:366,809-813`).
4. **Šesť `instead` viet ukazuje na CLI, ktoré neexistuje** (rodina UX-42): `add_host`,
   `remove_host`, `hide_host`, `provision_hosts` hovoria „with `fleet-hub`“, ale `fleet-hub` má
   subpríkazy `init · serve · token · agent-token · host-token-mode · pair · client · demo-seed ·
   ssh-key · healthcheck` (`crates/fleet-hub/src/main.rs:22-127`) — registrácia hosta ide cez
   ľubovoľného MCP klienta s master tokenom (`docs/hub.md:144-176`). Naopak pre tokeny CLI
   existuje (`host-token-mode`, `agent-token --rotate`), a vety ho nemenujú (UX-70).
5. **Matica: 24 príkazov — 7 už Routed (bez zmeny), 4 RE/RN v tomto bloku, 2 RN odložené
   (UXPR-24), 11 ostáva refused** (8 z nich s opraveným `instead`). Nové tools:
   `list_account_usage` (T0), `refresh_account_usage` (T0, precedens `probe_host`),
   `set_account_nickname` (T1); `tunnel_status` → existujúci `fleet_health` (projekcia v `routed::`).
   Fleet admin (`add_host`, `remove_host`, `hide_host`, `provision_hosts`) ostáva T3: hub tools
   existujú, ale sú `Access::Master` (`guard.rs:140-146,156-176`) — UI = disabled s jedným krátkym
   dôvodom, nie skryť (D3 pravidlo „existuje na hube, token nesmie“). Token trio a `discover_hosts`/
   `probe_ssh_alias` = skryť (nemajú hubový ekvivalent pre klienta a sú o inom stroji).
6. **`client_mode` (UX-45) → UXPR-21 (S):** pairing uloží `hub.client_mode` do `state.db` vedľa
   `hub.client_name` (`commands/hub.rs:348-360`), `status()` ho číta namiesto `None`
   (`:201-205`), `verdict_gen.rs` pridá do generovaného JSON zoznam `routed_readonly` (z
   `guard::is_readonly_tool`), `hubActionBlocked` dostane vetvu „this client is readonly on the
   hub“ **pred** klikom. Šošovky 7+ sa na to môžu spoľahnúť.
7. **Rozpočet:** delta tejto iterácie **+835 B** (tri tools). Earmarky D4 dávajú 63 603 z 63 800
   → nezmestí sa bez rezervných trimov; **s oboma rezervnými trimami z D4** (`fleet_health` −480,
   `plan_sync` −530) je meranie ≈ 63 430 a konštanta ostáva 63 800. `add_project` +
   `list_github_repos` (+≈1 300 B) idú do odloženého UXPR-24 s vlastným zdvihom podľa výnimky D4.
8. Deväť nových nálezov UX-68…UX-76 (H 2 · M 5 · L 2); tri PR: UXPR-21 (S), UXPR-22 (M, lane D
   po UXPR-13), UXPR-23 (M, lane E po UXPR-07 a 22); voliteľný UXPR-24 (M, odložený).

## Matica parity

24 príkazov v `generate_handler!` poradí (`verdicts.rs`). „UI“ = komponent a riadok, kde sa
príkaz volá. Akcia: **OK** už routované · **RE** route na existujúci tool · **RN** route na nový
tool · **R** ostáva refused (s opraveným `instead` a tým, čo UI ukáže **namiesto vety**).
Tier podľa D3. Hub-side tools a ich policy: `crates/fleet-core/src/mcp/guard.rs` `TOOL_POLICIES`,
definície `mcp/tools/fleet.rs` a `repo.rs`.

### Projekty

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia · tier |
|---|---|---|---|---|
| `list_projects` | `loadProjects` pri boote (`App.svelte:190-195`), sidebar | Routed (`verdicts.rs:122-127`) | `list_projects` Client/ro/Quick (`guard.rs:509-515`), `remote.rs:638-644` posiela `{summary:false}` | **OK** |
| `refresh_projects` | Refresh v sidebare | Routed (`:128-133`) | `refresh_projects` Client/ro/Lifecycle (`:516-522`) — popis vraví „no local projects directory to scan“ na hube; hub skenuje hostov cez SSH | **OK** |
| `add_project` | `＋ Add project…` (`Sidebar.svelte:809-813`) → `AddProjectDialog` (clone / My GitHub / folder / new) | LocalOnly „clones or adopts a checkout using this machine's SSH and GitHub credentials; add the project on the hub“ (`:134-140`) | žiadny; služba `service::add_project` (klon na hoste cez SSH, `gh repo create` pri `new`+`create_remote`, `call_id` cancel) | **RN odložené → UXPR-24** `add_project { host_alias, source{kind: clone\|folder\|new,…} }` · **T1**; `create_remote: true` iba master (analógia `force`, otázka 2). Dovtedy **R**: tlačidlo disabled s krátkym dôvodom (dnes OK), `instead` bez „on the hub“ (hub nemá kam) |
| `list_github_repos` | `GithubRepoBrowser` v dialógu (`:42`) | LocalOnly „runs `gh` over this machine's SSH“ (`:141-147`) | žiadny; `gh repo list` na hoste cez SSH = čítanie externého stavu ako `probe_host` | **RN odložené → UXPR-24** `list_github_repos { host_alias }` · **T0** (precedens `probe_host` ro). Bez `add_project` nemá zmysel |
| `purge_project` | kôš v hlavičke skupiny (`Sidebar.svelte:367`, UX-04) | LocalOnly „…the hub exposes no tool for it; purge from the hub“ (`:325-331`) | žiadny; maže Claude stav na každom hoste — nevratné | **R · T3** (D3: nevratné = master). `instead` → „…and the hub exposes no tool for it (a standalone app does)“ — bez „purge from the hub“. UI: **skryť** v hub režime (D3 pravidlo: bez hubového ekvivalentu) — koordinovať s UXPR-02, ktorý kôš rieši (UX-04/31). Šošovka 7 (UX-21) môže navrhnúť Master tool |

### Hosty a účty

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia · tier |
|---|---|---|---|---|
| `discover_hosts` | `AddHostPicker` pri otvorení (`:19`) | LocalOnly „reads this machine's ~/.ssh/config“ (`:469-475`) | **existuje** `discover_hosts` Client/ro/Quick (`guard.rs:126-132`, `fleet.rs:80-85`) — číta `~/.ssh/config` **hubového procesu** (`service/hosts.rs:17-19`, bez store) | **R** — správne (klientovi je hubov ssh config nanič, `add_host` je Master). Skryté za disabled `+ Add host`. **Pozn. UX-76:** hubov tool je Client/readonly → paired telefón vidí operátorove SSH aliasy (hostname, user, port — `ssh_config.rs:13-18`) bez možnosti čokoľvek pridať → navrhnúť **Master** |
| `list_hosts` | `loadHosts` boot, `HostsList`, `FleetResync` po výpadku (`backend/events.rs:727`) | Routed (`:476`) | `list_hosts` Client/ro/Quick (`:112-118`), `json!({})` (`remote.rs:628-631`) | **OK** — `HostRow` ide celý (`transport` vrátane, `rows.rs:463-480`) |
| `list_accounts` | `loadAccounts` boot, skupiny účtov, re-list (`:759`) | Routed (`:477-482`) | `list_accounts` Client/ro/Quick (`:133-139`) | **OK** — nesie `nickname`, `seat_tier`, `has_extra_usage` |
| `add_host` | `+ Add host` (`HostsView.svelte:381-387`, disabled s `title`) | LocalOnly „…add it there with `fleet-hub`“ (`:483-489`) | **existuje** `add_host` **Master**/Lifecycle (`guard.rs:140-146`, `fleet.rs:94-109`, aj `transport: agent`) | **R · T3**. `instead` → „registering a host is fleet administration — the hub's operator does it with the master token (`add_host` tool; `docs/hub.md` → *Add and provision hosts*)“. UI: disabled + krátky tooltip; veta o admin roli je **raz** v `HubScopeNote` |
| `probe_host` | `r` / Re-probe (`HostsView:204`, `HostDetail:87`) | Routed (`:490`) | `probe_host` Client/ro/Lifecycle (`:149-155`) — „re-reads external (SSH) state … readonly like `refresh_projects`“ (`:147-148`) | **OK** — a **precedens** pre `refresh_account_usage` ako readonly |
| `probe_ssh_alias` | náhľad v Add-host dialógu (`accounts.ts:89`) | LocalOnly „SSHes from this machine to preview“ (`:491-497`) | žiadny; hub by musel probe-ovať svoj ssh alias — má `probe_host` pre registrované | **R** — správne, skryté za `+ Add host` (allowlist `gatedByAddHostDialog`, `hub_verdicts.test.ts:189`) |
| `remove_host` | *Danger → Remove host…* (`HostDetail:292-299`) | LocalOnly „…remove it there with `fleet-hub`“ (`:498-504`) | **existuje** `remove_host` **Master**/Quick (`guard.rs:156-162`, `fleet.rs:124-133`) | **R · T3**. `instead` → „…the hub's operator removes it with the master token (`remove_host`)“. UI: disabled + krátky tooltip |
| `hide_host` | *Danger → Hide host* (`:283-291`) | LocalOnly „…hide it there with `fleet-hub`“ (`:505-511`) | **existuje** `hide_host` **Master**/Quick (`:163-169`, `fleet.rs:135-146`) | **R · T3**, rovnako |
| `set_account_nickname` | `AccountNickname` v skupine aj detaile (`HostsList:19`, `HostDetail:84`, `e`) | LocalOnly „nickname lives in the hub's database and there is no tool to set it“ (`:512-518`) | žiadny; `hosts::set_account_nickname` je Store-only (`service/hosts.rs:318-329`) | **RN** `set_account_nickname { uuid, nickname? }` · **T1** (D3: mutácia v registri hubu, vratná, validovaná) · Quick |
| `list_account_usage` | `loadAccountUsage` po boote (`App.svelte:236-244`, v remote preskočené) | LocalOnly „does not poll … cache is empty; read usage on the hub“ (`:520-526`) | žiadny; hub má `UsageCache` (`serve.rs:801-811`) a `account_usage_poll::list_account_usage(&store,&cache)` (`account_usage_poll.rs:158-172`) | **RN** `list_account_usage {}` · **T0** (čisté čítanie cache, bez tajomstiev — status, %, `resets_at`, `source_host`) · Quick. **Priorita bloku** |
| `refresh_account_usage` | `u`/refresh (`HostsView:210-224`, `UsageBlock:91-102`), Retry v outage banneri (`:425-433`), otvorenie Hosts/New-session (`HostsView:165-168`, `NewSessionDialog:419-423`) | LocalOnly „reads the account's usage over this machine's SSH“ (`:527-533`) | žiadny; `account_usage_poll::refresh_account_usage` (`:174-196`) rešpektuje floor, `E_NOTFOUND` | **RN** `refresh_account_usage { account_uuid }` · **T0** (`readonly: true` — precedens `probe_host`; floor 5 min ohraničuje) · Lifecycle (SSH + HTTPS na Anthropic) |

### Control API, tokeny, provisioning (Integration sekcia)

| Príkaz | UI | Verdikt dnes | Hub tool / CLI | Akcia · tier |
|---|---|---|---|---|
| `install_fleet_hook` | Settings (šošovka 3) | LocalOnly (`:549-555`) | žiadny; hook mieri na *toto* API | **R** — UXPR-15 už disabluje; v Hosts sa nevolá |
| `provision_hosts` | Settings → Control API (šošovka 3); *Hooks* v detaile hosta iba číta (`HostDetail:261-264`) | LocalOnly „…provision from the hub with `fleet-hub`“ (`:556-562`) | **existuje** `provision_hosts` **Master**/Lifecycle (`guard.rs:170-176`, `fleet.rs:150-176`); CLI subpríkaz **neexistuje** | **R · T3**. `instead` → „…the hub's operator runs `provision_hosts` with the master token“ |
| `list_host_tokens` | `loadHostTokens` pri mount (v remote preskočené, `HostsView:165-166`) → *Integration → Token* | LocalOnly „these are this app's own per-host tokens; list them on the hub“ (`:563-569`) | žiadny tool; hubove per-host tokeny existujú, ale hub ich nevystavuje ani cez `list_clients` | **R**. UI: **skryť** riadok Token v hub režime (D3: bez hubového ekvivalentu pre klienta); ostáva *Hooks* (z `sessions`, funguje). `instead` → „per-host tokens belong to the process that provisioned the host; on the hub see `fleet-hub agent-token <host>` (agent hosts) or re-run `provision_hosts`“ |
| `set_host_token_mode` | `<select>` Token mode (`:243-253`) | LocalOnly „change the mode on the hub“ (`:570-576`) | **CLI existuje:** `fleet-hub host-token-mode <host> full\|readonly` (`main.rs:58-70`) | **R**. `instead` → menovať CLI. UI: skryté s Token riadkom |
| `rotate_host_token` | *Rotate token…* (`:265-275`) | LocalOnly „rotate the token on the hub“ (`:577-583`) | **CLI existuje** pre agent hosty: `fleet-hub agent-token <host> --rotate` (`main.rs:42-57`); SSH hosty: `provision_hosts rotate=true` (Master) | **R**. `instead` → obe cesty. UI: skryté |
| `check_local_prereqs` | `OnboardingCard` (`:37-42`, gate `ownsTheFleet`) | LocalOnly (`:659-665`) | žiadny; je o `claude`/`tmux` na **tomto** stroji | **R** — správne; onboarding v hub režime = šošovka 19 (karta má ukázať hubové fakty, nie checklist) |
| `tunnel_status` | `OnboardingCard` (`:42`) | LocalOnly „tunnels belong to the process that owns the fleet; check them on the hub“ (`:666-672`) | **existuje ako pole** `fleet_health.tunnels: BTreeMap<alias, TunnelHealth>` + `tunnels_flapping` (`service/health.rs:51-56`, `fleet.rs:8-25`); `onboarding::map_tunnel_states(&hosts, &health)` je čistá projekcia (`service/onboarding.rs:64-70`) | **RE** `Routed { tool: "fleet_health" }` — `routed::tunnel_status` = `hub.fleet_health()` + `hub.list_hosts()` → `map_tunnel_states` (tá istá funkcia ako lokálne; nie lossy). 0 B rozpočtu. Konzument v hub režime zatiaľ žiadny → nízka priorita, ale lacné (~25 riadkov) |

**Súčty:** OK 7 · **RE 1** (`tunnel_status`) · **RN 3** (`list_account_usage`, `refresh_account_usage`,
`set_account_nickname`) · RN odložené 2 (`add_project`, `list_github_repos` → UXPR-24) · **R 11**
(z toho 8 s opraveným `instead`: `add_host`, `remove_host`, `hide_host`, `provision_hosts`,
`list_host_tokens`, `set_host_token_mode`, `rotate_host_token`, `purge_project`). LocalOnly po
UXPR-22: 51 (cieľ konsolidácie) − 4 = **47**; Routed 58 + 4 = **62**. S UXPR-24: 45 / 64.

**Ako hub vykoná `refresh_account_usage`:** rovnako ako desktop dnes — `source_hosts(account,
hosts, sticky)` vyberie online host účtu (`account_usage.rs:621`), `usage_script` beží cez hubov
`ssh::run_bounded` (`fetch_account_usage_with`, `:1023`), výsledok do hubovej `UsageCache`, event
na bus → **všetci** klienti dostanú `account_usage:updated`. Nič nové nevzniká; mení sa iba kto má
SSH — definícia hub režimu (iterácia 05, ten istý odsek).

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-19 | **potvrdené s korekciou** | *Token* riadok vypíše vetu odmietnutia ako hodnotu (`HostDetail.svelte:256-258`: `{hostTokensBlocked ?? …}` — jediné miesto v Hosts, kde próza nahrádza fakt). *Danger* tlačidlá **nie sú** „viditeľné, hoci LocalOnly“ v zmysle funkčné — sú `disabled` s dôvodom v `title` (`:283-299`, test `hub_disabled.test.ts:108-119`); problém je, že dôvod je v tooltipe a text patrí `remove_host` aj pre token-mode a Rotate (`:81` → `:246-247,:271`). Chýbajúci usage refresh potvrdený (`:85`, `UsageBlock:96-100` label „refresh“ disabled). Sev ostáva **M**; koreň je UX-69 (H) |
| UX-20 | **potvrdené** | `addProjectBlocked = hubBlock('add_project')` (`Sidebar.svelte:366`), tlačidlo `disabled` iba s `title` (`:809-813`); dialóg sa neotvorí, `list_github_repos` gate je iba allowlist (`hub_verdicts.test.ts:186`). Hub tool neexistuje (`TOOL_POLICIES` nemá `add_project`). Sev **M** ostáva; routing odložený (UXPR-24) — dôvod: rozpočet a `create_remote` |
| UX-27 (časť `?`) | **potvrdené, spresnené** | `?` v hlavičke je *keyboard legend* (`aria-label="Keyboard legend"`, `HostsView:388-395`), nie pomocník k `?` v stĺpci účtov — dva významy toho istého glyfu na jednej obrazovke (↔ UX-33). Šošovka 16 ostáva vlastníkom; tu iba poznámka pre UXPR-23 (tooltip `?` v stĺpci musí niesť dôvod, aby si ich používateľ neplietol) |
| README §3 „`list_account_usage` kandidát na routing“ | **potvrdené a lacnejšie, než README čaká** | hub má cache aj poller; tool je 1 funkcia + 1 policy riadok (~185 B) |
| verdikt `list_account_usage` „this app does not poll … so the cache is empty“ | **čiastočne vyvrátené** | store **nie je** prázdny — plní ho hubov event stream (`events.ts:268`, `App.svelte:224`), prázdny je iba pri štarte a po výpadku; „read usage on the hub“ nemá cieľ (žiadny tool, žiadny CLI) — rodina UX-42 |
| `commands/account_usage.rs:5-10` „nothing to route to“ | **vyvrátené** | `usage_report` má iný tvar (per-session tokeny), ale `UsageCache` na hube má **ten istý** `AccountUsageSnapshot` — chýba len `#[tool]` |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz | Návrh |
|---|---|---|---|---|
| **UX-68** | **H** | **`?` bez dôvodu a bez východiska.** `freshnessMark` vráti `?` s titulkom „No usage known for this account yet“ vždy, keď `fetched_at === null \|\| !usage`, a `snapshot.status`/`detail` (`login_expired`, `no_credentials`, `host_unsupported`, `no_online_host`…) zahodí; `compactWindow` dá `—`. Dôvod vidno až v detaile hosta (`UsageBlock` `generalLines`). V hub režime je `u`/refresh disabled → `?` sa nedá ani skúsiť obnoviť | `src/lib/hosts_view.ts:264-284,236-254`; `UsageBlock.svelte:91-102`; screenshot 03 (`m.janci@32bit.sk`, `mj.janci@gmail.com`) | `freshnessMark(snapshot, now)` → titulok z `status` + `detail` + `source_host` (`login expired on claude-fleet-htz · checked 5 min ago`; pre `no_online_host`: `no online host is logged in`), `data-status` na `.fresh`; refresh cez routovaný T0 tool (UXPR-22/23) |
| **UX-69** | **H** | **Usage v hub režime je iba event-driven.** Bez bootstrap čítania (`loadAccountUsage` preskočené) sú všetky účty `…`/„usage checking…“ až do prvého *zmeneného* snapshotu — pri 5-min floor-e až 5 min po každom spustení; `FleetResync` po výpadku re-listuje sessions/hosts/tasks/accounts, **usage nie** → po `reconnecting` stav ostáva starý až do ďalšej zmeny; „Hosts view opening“ a „New-session opening“ ako fetch triggery zo specu (`:165-168`, `NewSessionDialog:419-423`) sú vypnuté. Premisa „nothing to route to“ je nepravdivá (hub má `UsageCache`) | `App.svelte:236-244`; `backend/events.rs:617-630,706-767`; `commands/account_usage.rs:5-10`; `serve.rs:801-811`; `tick.rs:19` | T0 tool `list_account_usage` + `refresh_account_usage`; `FleetResync` pridá `list_account_usage` (tabuľka `:627-630` dostane piaty riadok, event `account_usage:updated`); `App.svelte:244` gate preč (UXPR-22) |
| **UX-70** | M | **`instead` ukazuje na `fleet-hub` subpríkazy, ktoré neexistujú** (`add_host`, `remove_host`, `hide_host`, `provision_hosts`: „with `fleet-hub`“), kým registrácia ide cez MCP s master tokenom; naopak token trio hovorí „on the hub“ a **nemenuje** existujúce `fleet-hub host-token-mode` / `agent-token --rotate`. `REASONS` v `hub.ts` končia „Do it on the hub (url)“ — pre Master tools bez CLI je to prázdny pokyn | `verdicts.rs:483-511,556-583`; `main.rs:22-127`; `docs/hub.md:144-176,1049,1089,1101`; `hub.ts:147-165,239-245` | 8 opravených `instead` (matica); `REGEN_HUB_VERDICTS` prepíše tabuľku v `hub.md`; `hubBlock` sufix „Do it on the hub“ pre admin akcie nahradiť „(hub operator, master token)“ — jedna veta v `HubScopeNote`, tooltipy krátke |
| **UX-71** | M | **`local offline hidden` na hube bez lokálneho hosta.** Hub s `hub.local_host=false` skryje a odpojí skopírovaný `local` riadok (`serve.rs:123-136`, `hub.md:873-876`), desktop však skryté hosty **lístuje** (`groupHostsByAccount` bez filtra `hidden`), počíta ich v „Hosts 6“ (`$hosts.length`) a `local` pripne účet `mj.janci@gmail.com` so `?`. Používateľ vidí offline problém, ktorý je dizajnové rozhodnutie hubu. Žiadny „show hidden“ prepínač neexistuje ani standalone | `hosts_view.ts:48-73`; `HostsView.svelte:378`; `HostsList.svelte:159-170` | skryté hosty zložiť do riadku `1 hidden ▸` na konci zoznamu (oba režimy), summary počíta iba nezakryté (`5 · 5 online`); v remote režime `local` s `hidden && !reachable` vôbec nelístovať (je to hubov stroj, nie klientov) — otázka 3 |
| **UX-72** | M | **Jeden ovládač, dve vety.** Token-mode `<select>` a *Rotate token…* sú disabled s `adminBlocked` (dôvod `remove_host`: „a client is never the fleet's administrator“), prázdny Token riadok s `host_tokens` („fleet owner's per-host tokens; this desktop … has none“); `hide_host`/`remove_host`/`add_host` majú v `REASONS` **identickú** vetu 3× | `HostDetail.svelte:81,84-92,246-247,256-258,271`; `hub.ts:147-152` | Integration → Token riadok v hub režime skryť (matica); `REASONS.add_host/remove_host/hide_host` zlúčiť do jednej hodnoty `FLEET_ADMIN` (test `REASONS_KEYS_THAT_ARE_NOT_COMMANDS` to dovolí len ako kľúč, nie hodnotu — hodnoty sa smú zdieľať); `HubScopeNote what='hosts'` nesie vetu raz |
| **UX-73** | M | **Falošné „usage every 5 min“ a „via“ v hub režime.** Hlavička tvrdí kadenciu tohto procesu; v hub režime polluje hub (60 s tick, 5 min floor), desktop iba prijíma. Pätička `via mefistos · checked 5 min ago` nehovorí, že číslo prišlo z hubu | `HostsView.svelte:379`; `UsageBlock.svelte:181-184` | `usage from <hub host> · every 5 min` v remote; footer bloku `via mefistos · checked 5 min ago · hub` — jeden token, nie odsek |
| **UX-74** | M | **`readonly` klient zlyhá až po kliku** aj pri routovaných mutáciách (Re-probe je `readonly:true` → OK; nickname po routingu T1, Kill, New session… → `E_FORBIDDEN`), lebo `HubStatus.client_mode` je po reštarte `None` („nothing stores it“). Rovnaké ako UX-45/57 — tu **špecifikované** ako UXPR-21, nie nový nález; číslo pridelené, aby register mal riadok pre Hosts kontext | `commands/hub.rs:48-51,201-205,335-337`; `hub.ts:37-38` | UXPR-21 (§ nižšie) |
| **UX-75** | L | **Fetch triggery zo specu v hub režime mlčia:** „Hosts view opening“, „New-session dialog opening“, outage *Retry* — všetky gate-ované `ownsTheFleet`/`hubBlock('refresh_account_usage')`; po routingu treba gate zmeniť na `hubActionBlocked` (spojenie), inak spec pravidlo „opening = fetch trigger“ v hub režime neplatí | `HostsView.svelte:165-168,211,227,343`; `NewSessionDialog.svelte:419-423`; spec § *Staleness and failure* „Fetch triggers“ | UXPR-22 min. TS / UXPR-23 |
| **UX-76** | L (bezpečnosť) | **Hubov `discover_hosts` je Client/readonly**, číta `~/.ssh/config` hubového procesu (`load_user_config`) a vracia alias/hostname/user/port — paired `readonly` telefón vidí operátorove SSH ciele, hoci `add_host` (jediný konzument) je Master | `guard.rs:126-132`; `fleet.rs:80-85`; `service/hosts.rs:17-19`; `ssh_config.rs:13-18` | `Access::Master` (0 B pre klienta; `-1` riadok v client surface); `hub.md` *Clients* doplniť do zoznamu master-only |

Číslovanie: konsolidácia-01 končí UX-67; táto iterácia používa **UX-68…UX-76**.

## Návrh hub tools / rozšírení

Zásady z iterácie 03 platia: meno toolu = meno príkazu, jedna klauzula popisu, próza do
`docs/hub.md`/`control-api.md`, `confirm: false`, `Deadline::Quick` pre Store/cache.

### Tool 1 — `list_account_usage` (T0)

| | |
|---|---|
| Súbor | `crates/fleet-core/src/mcp/tools/fleet.rs` (`fleet_router`, za `list_accounts`) |
| Popis (návrh, ~120 B) | `"Cached Claude account usage (5-hour and weekly windows, % used, resets_at, status) for every account seen on a host; never fetches. JSON rows."` |
| Params | žiadne |
| Výsledok | `Vec<AccountUsageSnapshot>` = `account_usage_poll::list_account_usage(&self.store, &self.usage_cache)` — **totožné s Tauri príkazom**; `FleetTools` potrebuje pole `usage_cache: Arc<Mutex<UsageCache>>` (dnes ho drží iba `serve.rs:801` lokálne a `lib.rs:170-175` ako Tauri state → odovzdať do `FleetTools::new` na oboch stranách) |
| Policy | `access: Access::Client, readonly: true, confirm: false, deadline: Deadline::Quick` |
| Prečo T0 | čisté čítanie cache; žiadne tajomstvo (žiadny token, iba %, časy, `source_host`, `status`, `detail`); `readonly` klient má vidieť, či má účet headroom — presne ako `list_accounts` |
| Wire | `AccountUsageSnapshot`, `AccountUsage`, `Window` derivujú iba `Serialize` (`account_usage.rs:210-232,895-909`), `UsageOutcomeKind` tiež (`:290-303`) → pridať `Deserialize` (+ `#[serde(default)]` na `Option` polia a `next_try_at`, `Default = NeverFetched` pre kind); sample `sample_usage_snapshot()` do `tests_contract.rs` → `REGEN_HUB_CONTRACT`. Frontend `AccountUsageSnapshot` už tvar pozná (`account_usage_store.ts:44-54`) |

### Tool 2 — `refresh_account_usage` (T0)

| | |
|---|---|
| Súbor | `fleet.rs`; params `mcp/tools/params.rs` |
| Popis (~130 B) | `"Fetch one account's usage now over SSH to an online host of that account, unless polled within the 5-minute floor (then the cached snapshot). JSON."` |
| Params | `RefreshAccountUsageParams { account_uuid: String /// Account uuid from list_accounts. }` |
| Výsledok | `AccountUsageSnapshot` = `account_usage_poll::refresh_account_usage(&uuid, &store, &*ssh, &cache, &*bus)` — emituje `account_usage:updated` iff zmena (všetci klienti) |
| Chyby | `E_NOTFOUND` (neznámy účet) — rovnaké ako dnes |
| Policy | `access: Access::Client, readonly: true, confirm: false, deadline: Deadline::Lifecycle` |
| Prečo `readonly: true` | precedens `probe_host`: „re-reads external (SSH) state without touching sessions — readonly like `refresh_projects`“ (`guard.rs:147-148`); floor 5 min/účet a `MAX_CONCURRENT_FETCHES = 2` (`account_usage_poll.rs:31`) ohraničujú, čo `readonly` telefón môže spôsobiť. Alternatíva T1 — otázka 1 |

### Tool 3 — `set_account_nickname` (T1)

| | |
|---|---|
| Súbor | `fleet.rs`; args `service/hosts.rs:318-321` `SetAccountNicknameArgs` doplniť `Serialize + rmcp::schemars::JsonSchema` + `///` na obe polia (test `every_tool_parameter_is_documented`), `#[schemars(rename = "SetAccountNicknameParams")]` ako `HideHostArgs` (`:302-307`) |
| Popis (~110 B) | `"Set or clear (null) the display nickname of a Claude account; returns the account row."` |
| Výsledok | `AccountRow` (má `Deserialize`, `rows.rs:540-542`; sample existuje `tests_contract.rs:107`) |
| Policy | `access: Access::Client, readonly: false, confirm: false, deadline: Deadline::Quick` |
| Prečo T1, nie T3 | D3: „mutácia v registri hubu, vratná, chránená service vrstvou“ — meno je kozmetika viditeľná všetkým klientom, ale nič nemení na hostoch ani credentials; `readonly` klient odmietnutý gate-om (`enforce_mode`) |

### Rozšírenie — `tunnel_status` → `fleet_health` (RE, 0 B)

`routed::tunnel_status(backend, store, tunnels)`: `Some(hub) => { let h = hub.fleet_health().await?;
let hosts = hub.list_hosts().await?; Ok(onboarding::map_tunnel_states(&hosts, &h.tunnels.into_iter().collect())) }`.
`Health.tunnels` je `BTreeMap<String, TunnelHealth>` (`health.rs:51`), `map_tunnel_states` berie
`HashMap` (`onboarding.rs:64-67`) — jeden `collect()`. `TunnelHealth` má `Deserialize` (kontrakt
`tests_contract.rs:229`). Verdikt `Routed { tool: "fleet_health" }` — test
`every_routed_tool_is_a_tool_the_hub_serves` prejde (`health_check` už tak routuje, `verdicts.rs:102-107`).

### `discover_hosts` → `Access::Master` (UX-76, 0 B pre klienta)

Jeden riadok `guard.rs:128`; test `readonly_tools_are_client_tools_or_…` (ak existuje pre Master+ro,
vzor `list_clients` `:199-205`) — `readonly: true` ostáva, lebo číta. `hub.md:590-596` doplniť do
zoznamu master-only.

### Čo sa mení vo `VERDICTS` (recept krok 3)

| Príkaz | Dnes | Po UXPR-22 |
|---|---|---|
| `list_account_usage` | LocalOnly | `Routed { tool: "list_account_usage" }` |
| `refresh_account_usage` | LocalOnly | `Routed { tool: "refresh_account_usage" }` |
| `set_account_nickname` | LocalOnly | `Routed { tool: "set_account_nickname" }` |
| `tunnel_status` | LocalOnly | `Routed { tool: "fleet_health" }` |
| `add_host` / `remove_host` / `hide_host` | LocalOnly „…with `fleet-hub`“ | LocalOnly, `instead` = `FLEET_ADMIN_IS_THE_OPERATORS` konštanta: „fleet administration — the hub's operator does it with the master token (`add_host` / `remove_host` / `hide_host` tools, see docs/hub.md → Add and provision hosts)“ (jedna `const` ako `CATALOG_IS_A_CHECKOUT`, `verdicts.rs:90-92`) |
| `provision_hosts` | „…with `fleet-hub`“ | „…the hub's operator runs `provision_hosts` with the master token“ |
| `list_host_tokens` / `set_host_token_mode` / `rotate_host_token` | „…on the hub“ | menovať `fleet-hub host-token-mode <host> <mode>`, `fleet-hub agent-token <host> --rotate` (agent), `provision_hosts rotate=true` (SSH) |
| `purge_project` | „…purge from the hub“ | „…the hub exposes no tool for it; a standalone app can“ |
| `add_project`, `list_github_repos` | LocalOnly „…add the project on the hub“ / „browse from the hub“ | LocalOnly, bez „on the hub“ (hub nemá kam) — do UXPR-24 |

`local_only.golden.json` stratí 4 záznamy a zmení 8 textov → `REGEN_LOCAL_ONLY`; tabuľka v
`hub.md` → `REGEN_HUB_VERDICTS`; diff **prečítať** (D6 bod 3).

### Starší hub a readonly klient (D5 `hub_inline_state`)

- Hub bez `list_account_usage`: `E_FORBIDDEN` (gate padá closed na neznámom mene) alebo
  `E_HUB_PROTOCOL` → **jeden riadok** pod hlavičkou Hosts `usage — this hub cannot serve account
  usage yet (update the hub)`, stĺpec účtov `—` namiesto `…`, pätička muted `usage — hub too
  old` (nový `FooterState 'unsupported'`, tón `muted`); bez toastu; porovnávať **kód**.
- `readonly` klient a `set_account_nickname`: po UXPR-21 disabled **vopred** („this client is
  readonly on the hub“); bez UXPR-21 prvý `E_FORBIDDEN` → `AccountNickname.blocked` s tou istou
  vetou (`hubInlineState(err).text`).
- `refresh_account_usage` mimo floor-u: hub vráti nezmenený snapshot s `next_try_at` → dnešný
  `refreshCountdown` (`HostsView:86,217`) funguje bez zmeny.

## Návrh Hosts view a usage v hub režime

Princíp D5: **jeden** `HubScopeNote` na pohľad (nie na sekciu), fakty namiesto viet, disabled
s krátkym dôvodom pre to, čo na hube existuje (Master), skryť to, čo nemá hubový ekvivalent.

### Hlavička (`HostsView.svelte:376-396`)

```
Hosts  5 · 5 online · 1 hidden ▸        usage from fleet.rlt.sk · every 5 min      [+ Add host] [?]
Hosts and accounts are the hub's (https://fleet.rlt.sk, paired as mac-desktop) — registering,
hiding, removing and provisioning hosts is its operator's, with the master token.     ← HubScopeNote what='hosts'
```

- Summary počíta nezakryté hosty; skryté idú do `1 hidden ▸` (UX-71); v remote režime sa
  `local` s `hidden && !reachable` nelístuje vôbec (otázka 3).
- Kadencia: `usage from <hub host> · every 5 min` (remote) / `usage every 5 min` (standalone) — UX-73.
- `+ Add host`: disabled, `title` = krátky dôvod „Hub operator only (master token)“; dlhá veta je
  v note. `?` legend ostáva (šošovka 16 rozhodne o glyfe).
- `HubScopeNote` je **jediný** odsek v celom pohľade; Integration a Danger žiadny ďalší text.

### Stĺpec účtov (`HostsList.svelte:107-150`)

- Bary a `% left · resets …` nezmenené (dáta z hubu majú ten istý tvar).
- `…` iba do prvého bootstrap čítania (po UXPR-22 sekundy, nie minúty); `?` vždy s dôvodom v
  `title` a `data-status` (UX-68): `login expired on claude-fleet-htz · checked 5 min ago — run
  claude /login there`; `no online host is logged in to this account`; `usage endpoint
  unavailable since 13:10` (outage banner ostáva jedinou hlásnou pre endpoint).
- `AccountNickname` editovateľný (T1) — `readonly` klient disabled vopred (UXPR-21).

### Detail hosta (`HostDetail.svelte`)

| Sekcia | Dnes (hub) | Návrh |
|---|---|---|
| Hlavička, ssh/transport/last ping/claude/tmux, Re-probe | funguje (routed) | bez zmeny |
| Account + USAGE blok | refresh disabled „refresh“; footer `via mefistos · checked 5 min ago` | `u refresh` funkčný (T0), floor countdown z hubového `next_try_at`; footer `via mefistos · checked 5 min ago · hub`; správy z `usageMessages` nezmenené |
| Sessions | funguje | bez zmeny |
| **Integration** | Token = veta odmietnutia; Hooks OK | riadok **Token skrytý** v remote (nemá hubový ekvivalent pre klienta); **Hooks** ostáva (`hookHealthLabel` zo `sessions`); pod ním jeden fakt `provisioned · hooks report to the hub` z `host.provisioned` — bez `Rotate token…` |
| **Danger** | Hide/Remove disabled, dlhý `title` ×2 | tlačidlá ostávajú **disabled** (tools existujú, Master — D3 pravidlo), `title` = „Hub operator only (master token)“; žiadna ďalšia veta; `local` vetva („can't be hidden or removed“) v remote nenastane, lebo `local` sa nelístuje |

### Pätička (`App.svelte:836-844`, `usage_glance.ts:227-334`)

- Po bootstrap čítaní sa `usage checking…` skráti z minút na sekundy; `attention`/`ok`/
  `unavailable` logika nezmenená (dáta majú ten istý tvar).
- Nový stav `unsupported` (starší hub) — muted `usage — hub too old`, `aria-label` s vetou
  z `hub_inline_state`; klik otvorí Hosts, kde je ten istý riadok.
- `off` po 24 h nezmenené.

### Kadencia a `?` sémantika (jedna tabuľka pre oba režimy)

| Kedy | Standalone | Hub klient (po UXPR-22) |
|---|---|---|
| poll | tick 60 s, floor 5 min/účet, tento proces | hub: tick 60 s, floor 5 min; desktop nič nepolluje |
| bootstrap | `list_account_usage` po subscribe (`App.svelte:236-244`) | **to isté cez hub** (gate `:244` preč) |
| po výpadku spojenia | — | `FleetResync` + `list_account_usage` (piaty riadok tabuľky `events.rs:627-630`) |
| Hosts / New-session open, `u`, Retry | `refresh_account_usage` per účet | to isté (T0), gate `hubActionBlocked` (spojenie), nie `ownsTheFleet` |
| `…` | žiadny snapshot | žiadny snapshot (sekundy) alebo starší hub (`—` + riadok) |
| `?` | číslo zadržané: expired alebo bez `ok`; **dôvod v title** | to isté |
| `—` | okno chýba v odpovedi | to isté |

### Add project (`Sidebar.svelte:809-813`)

Do UXPR-24: disabled ostáva, `title` skrátený na „Hub client: adding projects is not served by
this hub yet“ (bez „Do it on the hub“); kôš (`purge_project`) v remote **skrytý** — v koordinácii
s UXPR-02, ktorý mení `.purge-btn` (UX-04/31). Po UXPR-24: dialóg sa otvorí, `create_remote`
checkbox skrytý (master-only).

## `client_mode` (UXPR-21)

**Problém (UX-45/57/74):** `HubStatus.client_mode` je `Some(mode)` iba v odpovedi `hub_pair`
(`commands/hub.rs:335-337`), `status()` vracia `None` s odôvodnením „the mode is the hub's to
know, and nothing stores it“ (`:201-205`, doc `:48-51`). Pairing však už ukladá tri hodnoty do
`state.db` (`write_settings`, `:348-360`: `hub.client_name`, `hub.client_plaintext_token`,
`hub.remote_url`) a `PairedClient.mode` je k dispozícii (`backend/pairing.rs:37-38`). Mód
klienta sa na hube nemení inak než revoke + nové párovanie (žiadny `set_client_mode` tool), takže
uložená hodnota nestarne. **Trust** (`trusted_at`) v `/pair` odpovedi nie je (`PairedClient` má
`token, name, mode`) → T2 stav sa naďalej učí z prvého `E_FORBIDDEN` cez `hub_inline_state`
(UXPR-07); rozšírenie `/pair` o `trusted` je hubová zmena mimo tohto PR (otázka 5).

**Backend (S):**

| # | Súbor | Zmena |
|---|---|---|
| 1 | `src-tauri/src/backend/mod.rs:47-75` | `pub const CLIENT_MODE_KEY: &str = "hub.client_mode";` s doc-komentárom (uložené pri párovaní, `full`\|`readonly`, informatívne — gate je hubov `enforce_mode`) |
| 2 | `commands/hub.rs::logic::write_settings` (`:348-360`) | nový parameter `client_mode: &str`; `s.set_setting(CLIENT_MODE_KEY, mode)` **pred** `REMOTE_URL_KEY` (URL ostáva posledná — invariant z `:229-338`) |
| 3 | `pair()` (`:229-338`) | odovzdať `&client.mode`; riadok `out.client_mode = Some(client.mode)` ostáva (rovnaká hodnota, `status()` ju už prečíta) |
| 4 | `read_settings`/`restore_settings` (`:363-390`) | pole `[(&str, String); 4]` — nový kľúč |
| 5 | `status()` (`:179-212`) | `client_mode: setting(&s, CLIENT_MODE_KEY).filter(|m| store::validate_client_mode(m).is_ok())` — neplatná/prázdna hodnota = `None` (starší `state.db`) |
| 6 | `disconnect()` (`:426-443`) | `s.set_setting(CLIENT_MODE_KEY, "")?` vedľa ostatných troch |
| 7 | `HubStatus.client_mode` doc (`:48-51`) | „as the hub recorded it at pairing time; `None` for a pairing older than this field“ |
| 8 | `backend/verdict_gen.rs:77-108` | `VerdictLists` dostane `routed_readonly: Vec<String>` — routované príkazy, ktorých tool je `guard::is_readonly_tool` (`guard.rs:712`); `RoutedUnless` sa do zoznamu **nepridáva**. Generuje sa do `src/lib/hub_verdicts.generated.json` (nový kľúč) — `REGEN_HUB_VERDICTS` |
| 9 | `tests_pairing.rs` | `paired_status_keeps_client_mode_across_status_calls` (pair → `status()` bez `pair` výsledku → `Some("readonly")`), `disconnect_forgets_client_mode`, `unknown_stored_mode_reads_as_none` |

**Frontend (S):**

| # | Súbor | Zmena |
|---|---|---|
| 10 | `src/lib/hub.ts:37-38` | doc `client_mode`: „`full` \| `readonly` as stored at pairing; `null` for an older pairing“ |
| 11 | `hub.ts` | `export function clientIsReadonly(status = get(hubStatus)): boolean` (`status.remote && status.client_mode === 'readonly'`); `hubActionBlocked(action, status, conn)` (`:323`) dostane **pred** kontrolou spojenia vetvu: `if (clientIsReadonly(status) && !ROUTED_READONLY.has(action)) return 'This client is readonly on the hub — <action> is refused. Ask the hub operator for a full pairing.'`; `ROUTED_READONLY` číta `verdicts.routed_readonly` z generovaného JSON (jediný import JSON v `hub.ts` — alternatíva: literál + cross-check test ako `ROUTED_ACTIONS`, `:257-266`; odporúčam literál kvôli union typu, test ho drží) |
| 12 | `hub_verdicts.test.ts` | `routed_readonly ⊆ routed`; každý `ROUTED_ACTIONS` prvok je buď v `routed_readonly` (probe_host) alebo mutácia; `clientIsReadonly` blokuje `kill_session`, nie `probe_host` |
| 13 | `hub_disabled.test.ts` | „a readonly client sees Kill/New session/nickname disabled before the click, Re-probe enabled“; „full client untouched“; „older pairing (`client_mode: null`) behaves as today“ |
| 14 | `App.hub.test.ts` | `HubStatus` fixture (`:13`) dostane `client_mode: 'full'`/`'readonly'` varianty |

Veľkosť: ~90 riadkov (+~80 testov) + generovaný JSON. **Bez** `guard.rs`, `tests.rs`, `verdicts.rs`,
`remote.rs` → nekoliduje s lane D okrem generovaného JSON (regen je idempotentný; poradie:
UXPR-21 **pred** UXPR-09, alebo hocikedy s jedným re-regen). Odblokuje stavy `files-forbidden`
(UXPR-16), `SyncPlanDialog.applyBlocked` (UXPR-18) a nickname/kill v Hosts (UXPR-23).

## Rozpočet

`the_served_definition_budget_stays_bounded` (`crates/fleet-core/src/mcp/tools/tests.rs:2325-2400`),
`BUDGET_BYTES = 57_700` (`:2357`), meranie 57 603; D4 zdvíha **raz** na 63 800 v UXPR-09.

| Položka | Odhad B | Priebežne |
|---|---|---|
| východisko (meranie) | 57 603 | 57 603 |
| earmark 03 (`get_fleet_settings`, `set_fleet_setting`) | +650 | 58 253 |
| earmark 04 (2 tools, `last_sync`, −4 vety) | +450 | 58 703 |
| earmark 05 variant A (10 git toolov) | +4 900 | **63 603** (197 B pod stropom) |
| **UXPR-22:** `list_account_usage` 18 + ~120 + ~45 (prázdna schéma) | +185 | |
| `refresh_account_usage` 21 + ~130 + ~140 (1 pole s `///`) | +290 | |
| `set_account_nickname` 20 + ~110 + ~230 (2 polia) | +360 | |
| `tunnel_status` → `fleet_health`, `discover_hosts` → Master | 0 | |
| **delta iterácie 06** | **+835** | **64 438** → **nad 63 800** |
| rezervný trim D4: `fleet_health` popis (dnes ≈500 B na 3 riadkoch, `fleet.rs:8-10`; tunnel/usage vysvetlenie do `control-api.md`) | −480 | |
| rezervný trim D4: `plan_sync` popis (≈830 B vrátane schémy) | −530 | |
| **po trimoch** | | **≈ 63 430** (370 B pod 63 800) |
| UXPR-24 (odložené): `add_project` (11 + ~180 + tagged enum `source` 3 varianty ≈ 700) + `list_github_repos` (17 + ~120 + ~130) | ≈ +1 300 | ≈ 64 730 → výnimka D4: zdvih na **64 800** s odsekom |

Záver: **UXPR-22 sa zmestí pod 63 800 iba s oboma rezervnými trimami z D4** (boli označené ako
rezerva, nie podmienka — teraz sa spotrebujú; konštantu **nedotýkať**). Ak by meranie po
UXPR-13 bolo vyššie než odhad, platí výnimka D4 (najbližšia stovka nad meranie + odsek).
`ro_bytes < bytes / 2`: dva nové tools sú readonly (+475 B do readonly plochy), `set_account_nickname`
nie; pomer sa mierne zhorší, ale readonly plocha je ďaleko pod polovicou (05 pridalo 4 900 B
mutujúcich). Čísla `master / host full / host readonly / client full` vypísať do popisu PR (D4).
`discover_hosts` → Master **zníži** `client full` plochu o ~180 B.

## PR plán (podľa receptu z iterácie 03)

Recept `03-hub-parity-settings.md` → *Recept*, kroky 1–14; D6 pravidlá. Lane D (sekvenčná hub
Rust) · lane E (Svelte) · lane C (zdieľané UI, UXPR-07).

### UXPR-21 — `client_mode` persistovaný + `routed_readonly` (**S**, lane C/∥)

Položky 1–14 z § `client_mode` vyššie. Súbory: `backend/mod.rs`, `commands/hub.rs`,
`backend/tests_pairing.rs`, `backend/verdict_gen.rs`, `hub_verdicts.generated.json` (gen),
`hub.ts`, `hub_verdicts.test.ts`, `hub_disabled.test.ts`, `App.hub.test.ts`. Regen:
`REGEN_HUB_VERDICTS`. Závisí: nič. Paralelnosť: ∥ so všetkým; ideálne **pred UXPR-09**
(jediný spoločný súbor je generovaný JSON). ~90 (+80).

### UXPR-22 — Rust: usage + nickname tools, routing, opravené `instead`, kontrakt (**M**, lane D po UXPR-13)

| # | Krok receptu | Súbor | Zmena |
|---|---|---|---|
| 1 | 2a | `mcp/tools/params.rs` | `RefreshAccountUsageParams { account_uuid /// }` |
| 2 | 2a | `service/hosts.rs:318-321` | `SetAccountNicknameArgs`: `Serialize, Deserialize, JsonSchema`, `///` ×2, `#[schemars(rename)]` |
| 3 | 9 | `service/account_usage.rs:210-232,290-303,895-909` | `Deserialize` na `Window`, `AccountUsage`, `UsageOutcomeKind` (+`Default = NeverFetched`), `AccountUsageSnapshot` (`#[serde(default)]` na `Option`/`next_try_at`) |
| 4 | 2b | `mcp/tools/mod.rs` (`FleetTools`), `crates/fleet-hub/src/serve.rs:801`, `src-tauri/src/lib.rs:170-175` + miesto, kde vzniká `FleetTools` | pole `usage_cache: Arc<Mutex<UsageCache>>` zdieľané s tickom |
| 5 | 2b | `mcp/tools/fleet.rs` | `list_account_usage`, `refresh_account_usage`, `set_account_nickname` (`audit(...)`, `ok_json_compact`) |
| 6 | 2c | `mcp/guard.rs` | 3 riadky `TOOL_POLICIES` (T0/T0/T1 podľa § tools); `discover_hosts` → `Access::Master` (UX-76) |
| 7 | 2d | `mcp/tools/tests.rs` | budget: **nedvíhať**; trim `fleet_health` (`fleet.rs:8-10`) a `plan_sync` popisov (rezerva D4), text do `docs/control-api.md`; `a_readonly_client_is_refused_mutating_tools_but_allowed_reads`: `list_account_usage`/`refresh_account_usage` povolené, `set_account_nickname` odmietnuté |
| 8 | 2e | `docs/control-api.md:238-244` | index *Fleet & hosts*: `list_account_usage`, `refresh_account_usage`, `set_account_nickname`; `REGEN_DOCS` ×2 |
| 9 | 3 | `backend/verdicts.rs` | 4× `Routed`; `const FLEET_ADMIN_IS_THE_OPERATORS`; 8 opravených `instead` (tabuľka § *Čo sa mení*) |
| 10 | 4 | `backend/remote.rs` | `list_account_usage()` (`json!({})`), `refresh_account_usage(&RefreshAccountUsageArgs)` (struct `Serialize`, pole za poľom), `set_account_nickname(&SetAccountNicknameArgs)`, `tunnel_status()` = `fleet_health` + `list_hosts` + `map_tunnel_states` (doc-komentár: prečo dve volania) |
| 11 | 5 | `commands/account_usage.rs`, `commands/hosts.rs`, `commands/onboarding.rs` | `mod routed` ×4; `refuse_local_only` ×4 preč; doc-komentár `:5-10` prepísať (premisa UX-69) |
| 12 | 6 | `backend/tests_routing.rs` | 3 Case do `routed_read_cases()` (`:285`; `list_account_usage` `"[]"`, `refresh_account_usage` `json!({"account_uuid":"acc-1"})` + snapshot payload, `tunnel_status` → `fleet_health` s `HEALTH_PAYLOAD` + `list_hosts`), 1 Case do `routed_mutation_cases()` (`:695`; `set_account_nickname` `json!({"uuid":"acc-1","nickname":"work"})`); sekcia 2 standalone: `list_account_usage` z lokálnej cache |
| 13 | 9 | `backend/tests_contract.rs` | `sample_usage_snapshot()` (každé pole neprázdne, `status: Ok`), do `the_whole_contract`; `REGEN_HUB_CONTRACT` ×2 |
| 14 | 7–8 | goldeny | `REGEN_LOCAL_ONLY`, `REGEN_HUB_VERDICTS` (×2 každý), diff prečítať |
| 15 | 9 | `backend/events.rs:617-630,700-770` | `FleetResync`: piaty re-list `list_account_usage` → `account_usage:updated` per riadok (UX-69) |
| 16 | 10 (min. TS) | `src/lib/hub.ts:145-204` | zmazať `REASONS.list_account_usage`, `refresh_account_usage`, `set_account_nickname`, `tunnel_status`; `add_host/remove_host/hide_host` → jedna hodnota; token trio text menuje CLI |
| 17 | 10 | `hub_verdicts.test.ts` | nič v allowlistoch (žiadny zo 4 tam nie je); test „every other REASONS key is local_only“ vynúti krok 16 |
| 18 | 11 (min.) | `App.svelte:236-244`, `HostsView.svelte:165-168,211,227,242,343`, `HostDetail.svelte:84-85`, `NewSessionDialog.svelte:419-423` | gate `ownsTheFleet` → bez gate (bootstrap) alebo `hubActionBlocked` (spojenie); `hubBlock('refresh_account_usage'\|'set_account_nickname')` volania prepísať (inak `svelte-check` padne na `HubAction`) |
| 19 | 12 | `App.hub.test.ts:286-313` | „does not poll account usage against a hub“ → „loads account usage from the hub after subscribing“; `HostsView.test.ts:480-500` „r and u do nothing while blocked“ ostáva pre `offline`, pribudne „u calls refresh_account_usage on a connected hub client“ |
| 20 | 13 | `docs/hub.md:975-1000,1181-1204` | bullet *What is different*: „Account usage comes from the hub's poller; the desktop reads and refreshes it through the hub“; *Known limitations*: per-host tokens a fleet admin (s CLI menami); *Clients*: `discover_hosts` master-only |

~280 riadkov (+~200 testov, + generované). Blízko limitu — **prirodzený odštep** ako 10/11:
**22a** položky 1–8 (fleet-core: tools, derives, policy, budget, reference; ~130) → **22b**
položky 9–20 (routing, goldeny, min. TS, docs; ~150). Závisí: UXPR-13 (`guard.rs`, `tests.rs`,
`verdicts.rs`, `remote.rs`, goldeny), UXPR-21 nie je nutné (min. TS nepotrebuje `client_mode`).

### UXPR-23 — Svelte: Hosts view v hub režime + usage plochy (**M**, lane E po UXPR-07 a 22)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `HostsView.svelte:376-396` | `HubScopeNote what='hosts'` pod hlavičkou (jediný odsek); kadencia `usage from <hub> · every 5 min`; summary bez skrytých + `N hidden ▸`; `+ Add host` `title` krátky |
| 2 | `hosts_view.ts:48-73,264-284` | `groupHostsByAccount(hosts, accounts, { hideHidden })`; `freshnessMark(snapshot, now)` → `title` zo `status`/`detail`/`source_host`, `data-status`; helper `usageReason(snapshot)` zdieľaný s `UsageBlock` |
| 3 | `HostsList.svelte:107-150,159-170` | `title` a `data-status` na `.fresh`; `hidden` skupina zložená |
| 4 | `HostDetail.svelte:238-300` | Integration: Token riadok/`Rotate` iba `{#if ownsTheFleet}`, fakt `provisioned` z `host.provisioned`; Danger `title` krátky; `adminBlocked` text z `HubScopeNote` slovníka |
| 5 | `UsageBlock.svelte:91-102,181-184` | refresh label podľa `hubActionBlocked`; footer `· hub` v remote |
| 6 | `usage_glance.ts:227-334` | `FooterState 'unsupported'` z `hubInlineState` (starší hub); `App.svelte:836-844` tón `muted` |
| 7 | `Sidebar.svelte:366,809-813` | `title` bez „Do it on the hub“; kôš `{#if ownsTheFleet}` (koordinácia s UXPR-02) |
| 8 | testy | `HostsView.hub.test.ts` (nový): „renders the hub's usage rows and one scope note“, „`?` carries the reason“, „hidden local is not listed on a hub client“, „Token row hidden, Hooks shown“, „Danger disabled with short reason“, „older hub: one line, no toast“, „standalone untouched“ (legend, Add host enabled, Token row shown); `hosts_view.test.ts` pre `freshnessMark` dôvody a `groupHostsByAccount` skryté; `usage_glance.test.ts` `unsupported`; `hub_disabled.test.ts:104-138,204-219` upraviť tooltipy |

~220 riadkov (+~180 testov). Závisí: UXPR-07 (`HubScopeNote`, `hub_inline_state`), UXPR-22
(routing), mäkko UXPR-21 (readonly disabled vopred), UXPR-02 (kôš). ∥ s UXPR-14–18.

### UXPR-24 — `add_project` + `list_github_repos` (**M**, lane D, **odložený**, otázka 2)

`AddProjectArgs`/`AddProjectSource` (`service/add_project.rs:62-84`) → `Serialize + JsonSchema +
///` (tagged enum `kind`), `call_id` **nie** na drôt (hubový cancel nie je v rozsahu — tool beží
do `Deadline::Lifecycle`); tools `add_project` (T1; `create_remote: true` → `E_FORBIDDEN`
ne-master v tele, alebo pole vypustiť ako `force`), `list_github_repos { host_alias }` (T0);
verdikty, remote, routed, 2 Case, kontrakt (`ProjectTreeRow` už má sample?), `REASONS.add_project`
von, `AddProjectDialog` `create_remote` checkbox `{#if ownsTheFleet}`, Cancel v dialógu disabled v
remote (žiadny `call_id`). Rozpočet +≈1 300 B → výnimka D4 (64 800). ~230 (+150). Po UXPR-22.

### Lanes

```
lane C  zdieľané UI   07 ∥ 21
lane D  hub Rust      { 08 ∥ 09 } → 10 → 11 → 12 → 13 → 22(a→b) → [24]
lane E  hub Svelte    14 → 15 · 16 → 17 · 18 · 23 (po 07, 22; mäkko 21, 02)
```

## Akceptačné testy

### Rust — `crates/fleet-core/src/mcp/tools/tests.rs`

- `every_router_tool_has_exactly_one_tool_policy_row`, `every_tool_parameter_is_documented`,
  `annotations_follow_the_policy_table` zelené po troch tooloch.
- `the_served_definition_budget_stays_bounded`: konštanta **63 800 nezmenená**, meranie ≤ 63 800
  po trimoch; `ro_bytes < bytes / 2`.
- `a_readonly_client_is_refused_mutating_tools_but_allowed_reads`: `list_account_usage`,
  `refresh_account_usage` povolené `readonly`; `set_account_nickname` → `E_FORBIDDEN`;
  `discover_hosts` → `E_FORBIDDEN` pre každého klienta (Master).
- Nové: `list_account_usage_reads_the_shared_cache_never_fetches` (FakeSsh bez volaní; snapshot
  `never_fetched` pre účet bez záznamu, `ok` po `cache.record`); `refresh_account_usage_within_floor_is_a_cache_read`
  (0 SSH volaní, `next_try_at > 0`), `refresh_account_usage_unknown_account_is_e_notfound`;
  `set_account_nickname_clears_with_null_and_returns_the_row`.

### Rust — `src-tauri/src/backend/`

- `tests_routing.rs`: `every_routed_row_is_driven_by_a_case` (4 nové Case),
  `every_commands_body_does_what_its_row_says` (`routed::` v 4 telách),
  `every_routed_tool_is_a_tool_the_hub_serves` (`fleet_health` pre `tunnel_status`),
  `every_local_only_message_is_the_one_the_fixture_records` po `REGEN_LOCAL_ONLY` (8 textov).
- `a_routed_read_answers_the_hub_and_not_the_local_database` rozšíriť o `list_account_usage`:
  lokálna cache prázdna, fake hub vráti jeden `ok` snapshot → príkaz vráti `ok`.
- `tests_events.rs`: `resync_relists_account_usage_and_emits_account_usage_updated`.
- `tests_contract.rs`: `the_whole_contract` s `sample_usage_snapshot()`; golden po
  `REGEN_HUB_CONTRACT`.
- `tests_pairing.rs` (UXPR-21): tri testy z položky 9.
- `tests_verdict_gen.rs`: `generated_json_is_current` (nový kľúč `routed_readonly`),
  `doc_table_is_current`.

### Frontend — Vitest (`npx vitest run src/lib/hub_verdicts.test.ts src/lib/hub_disabled.test.ts src/lib/HostsView.test.ts src/lib/HostsView.hub.test.ts src/lib/hosts_view.test.ts src/lib/usage_glance.test.ts src/App.hub.test.ts`)

- `hub_verdicts.test.ts`: „every other REASONS key that is a command name is local_only“ po
  zmazaní 4 kľúčov; `routed_readonly ⊆ routed`; `ROUTED_ACTIONS` ∩ `routed_readonly` = `{probe_host}`.
- `App.hub.test.ts:286-313`: remote → `list_account_usage` **volané** po `subscribeToRowEvents`
  (poradie: `hub_status` < `health_check` < `list_account_usage`); `unavailable` → nevolané
  (`:381-392` ostáva).
- `HostsView.hub.test.ts` (nový, fixtures `hosts_fixture.ts`: `ADMIN/WORK/GMAIL`, `fleetHosts()`,
  `fleetUsage()`): (1) presne jeden `[data-testid="hub-scope-note"]`; (2) `group-freshness` s
  `?` má `title` obsahujúci `login expired` a alias hosta pre snapshot `status: 'login_expired',
  usage: null`; (3) `local` s `hidden && !reachable` nie je v `hosts-list`, summary `5 · 5 online`;
  (4) `detail-token-mode`/`detail-token-empty`/`detail-rotate` neexistujú, `detail-hooks` áno;
  (5) `detail-hide`/`detail-remove` disabled, `title` bez „Do it on the hub“; (6) `u` na
  `connected` hube volá `refresh_account_usage`, na `offline` nie; (7) `list_account_usage`
  odpovie `E_FORBIDDEN` → jeden riadok `hosts-usage-unsupported`, žiadny toast, pätička
  `data-state="unsupported"`; (8) `readonly` klient (UXPR-21): `group-label` nickname disabled
  s „readonly“, Re-probe enabled; (9) standalone: legend, Add host enabled, Token riadok, kôš.
- `hosts_view.test.ts`: `freshnessMark` pre každý `UsageStatus` bez `usage` vráti `?` s textom
  `describe`-u (`no credentials file`, `login expired`, …) a `source_host`; `groupHostsByAccount`
  so `hideHidden`.
- `usage_glance.test.ts`: `footerUsage(..., unsupported: true)` → `state 'unsupported'`, tón `muted`.
- `hub_disabled.test.ts:104-138`: „Hide, Remove disabled, each saying why“ — `title` neobsahuje
  `Do it on the hub`, obsahuje `master`; Rotate/token-mode testy presunuté do „hidden on a hub
  client“; `:204-219` Add host nezmenené; `:413-440` Add project `title` bez „Do it on the hub“,
  kôš neexistuje v remote.

### Manuálne (screenshot podľa README §1)

Hub režim, Hosts ⌘1: do 5 s po štarte žiadny `…`; `m.janci@32bit.sk` má `?` s tooltipom
menujúcim host a dôvod, `u` na ňom vráti countdown alebo nový snapshot; `local` nie je v zozname;
detail `claude-fleet-trn`: Integration = iba Hooks, Danger dve disabled tlačidlá s krátkym
tooltipom; jeden banner pod hlavičkou; pätička bez „checking…“. Nový screenshot `03b-hosts-view-hub.jpg`.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| UXPR-21 backend (kľúč, pairing, status, disconnect, verdict_gen) | S | ~50 riadkov + testy ~50 + generovaný JSON |
| UXPR-21 frontend (`clientIsReadonly`, `hubActionBlocked`, testy) | S | ~40 riadkov + testy ~30 |
| **UXPR-21 spolu** | **S** | ≈90 (+80) |
| UXPR-22a fleet-core (params, derives, cache do `FleetTools`, 3 tools, policy, trimy, reference) | S/M | ~130 riadkov + testy ~80 + generované |
| UXPR-22b routing (verdikty + 8 textov, remote ×4, routed ×4, Case ×4, resync, kontrakt, goldeny, min. TS, docs) | M | ~150 riadkov + testy ~120 + generované |
| **UXPR-22 spolu** | **M** | ≈280 — na hrane; odštep 22a/22b pripravený |
| UXPR-23 komponenty (HostsView, HostsList, HostDetail, UsageBlock, Sidebar) | M | ~150 riadkov |
| UXPR-23 `hosts_view.ts`, `usage_glance.ts` | S | ~70 riadkov |
| UXPR-23 testy | M | ~180 riadkov |
| **UXPR-23 spolu** | **M** | ≈220 (+180) |
| UXPR-24 (odložený) | M | ≈230 (+150) + rozpočet 64 800 |

Bez UXPR-24: ≈ **590** riadkov diffu (+≈460 testov) — v súčte fronty 3 070 → ≈ 3 660.
Verdikty po bloku: LocalOnly **47**, Routed **62**, RoutedUnless 1, SameInBoth 20 (zo 130).

## Otázky pre vlastníka

Iba to, čo konsolidácia-01 nerozhodla; každá s odporúčaním („default“ = prijať odporúčania).

1. **`refresh_account_usage` T0 alebo T1?** Tool spustí SSH na host a HTTPS na Anthropic v mene
   účtu, ohraničené floor-om 5 min/účet a 2 súbežnými fetchmi. Precedens `probe_host`
   (`readonly: true`, „re-reads external state“) hovorí T0; konsolidácia §5 označila oba usage
   príkazy za T0 kandidátov. *Odporúčanie: T0 — `readonly` telefón má vidieť čerstvé číslo, keď
   ho potrebuje; floor je mantinel.*
2. **`add_project` + `list_github_repos` (UXPR-24):** routovať (rozhodnutie b) s rozpočtom
   +≈1 300 B a zdvihom na 64 800 podľa výnimky D4, alebo nechať refused, kým telefón (šošovka 20)
   nepotrebuje pridávať projekty? Ak routovať: `create_remote` vypustiť zo schémy (ako `force`,
   otázka C3) alebo master-only v tele? *Odporúčanie: routovať v odloženom UXPR-24 po šošovke 20;
   `create_remote` **vypustiť** zo hubovej schémy (GitHub repo vytvára iba standalone).*
3. **Skryté hosty:** (a) v remote režime `local` s `hidden && !reachable` nelístovať vôbec a
   ostatné skryté zložiť do `N hidden ▸`; (b) iba zložiť (oba režimy), `local` ostáva v skrytých;
   (c) nič nemeniť. *Odporúčanie: (a) + zloženie v oboch režimoch — hubov `local` nie je klientov
   host, a standalone získa menej šumu; S v UXPR-23.*
4. **`discover_hosts` na hube → `Access::Master`** (UX-76): tool vracia operátorove SSH aliasy
   každému paired klientovi; jediný konzument (`add_host`) je Master. *Odporúčanie: áno, v
   UXPR-22a (1 riadok), `hub.md` *Clients* doplniť.*
5. **`trusted` v `/pair` odpovedi a `hub.client_trusted`:** desktop by po UXPR-21 poznal aj T2
   stav vopred (Push, `set_fleet_setting`) — vyžaduje zmenu hubu (`pairing.rs` redeem + `/pair`
   handler) a novšiu verziu hubu. *Odporúčanie: nie teraz; T2 sa učí z prvého `E_FORBIDDEN` cez
   `hub_inline_state` (UXPR-07); zapísať do backend backlogu ako follow-up UXPR-21.*
6. **`purge_project` v hub režime:** skryť kôš (D3 pravidlo „bez hubového ekvivalentu“) alebo
   disabled s dôvodom (konzistentne s Danger)? Šošovka 7 (UX-21) môže navrhnúť Master tool.
   *Odporúčanie: skryť teraz (UXPR-23 v koordinácii s UXPR-02); ak šošovka 7 tool navrhne, vráti
   sa ako disabled T3.*
7. **`FooterState 'unsupported'`** pre starší hub — nový stav v pätičke, alebo zložiť do
   `unavailable` s iným textom? *Odporúčanie: nový stav (tón `muted`, bez alarmu) — `unavailable`
   znamená „Anthropic endpoint“, čo by zavádzalo.*

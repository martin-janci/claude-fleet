# Iterácia 04 — Hub parita: Assets / Catalog panel

**Šošovka:** hub-client parita v Assets tabe · **Zasahuje:** UX-17, root-cause klaster 3 („hub režim
odpovedá prózou namiesto funkcie“) · **Vstup:** `docs/ux/2026-09-21-audit/README.md`, iterácia 03
(sekcia *Recept*), asset specy `docs/superpowers/specs/2026-09-1{4,5,7,8}-*asset*`, `docs/hub.md`,
kód na `9bd46cb2` · **Režim:** read-only review, žiadne zmeny kódu, žiadny `cargo` beh. **Druhá z
troch hub-parity iterácií** (3 = Settings, 4 = Assets, 5 = Files/git). Postup implementácie je
**recept z iterácie 03** (`03-hub-parity-settings.md` → *Recept — ako dostať LocalOnly príkaz na
Routed*, kroky 1–14); tento dokument ho neopakuje, iba naň odkazuje krokom.

Schválené rozhodnutie (README §5): hub parita = **routing na hub tools**, nie skrývanie UI.

## Zhrnutie

1. **UX-17 potvrdené, ale s opačným znamienkom, než README predpokladá.** Hub *dnes* neservíruje
   ani `list_assets`. Všetkých desať hubových asset toolov číta process-globálny
   `catalog::CATALOG` (`service/catalog/mod.rs:27-29`), ktorý napĺňa **iba** Tauri príkaz
   `catalog_load` (`src-tauri/src/commands/assets.rs:169`) a authoring zápisy (`author.rs:631`).
   `crates/fleet-hub/src/` neobsahuje slovo `catalog` ani raz; `fleet-hub` CLI nemá subpríkaz na
   konfiguráciu katalógu; MCP nemá tool `catalog_configure`/`catalog_load`. Na hube preto každý
   asset tool odpovie `E_CATALOG_NOT_CONFIGURED` („configure the catalog repo first“, resp.
   „catalog not loaded; call catalog_load“ — `mod.rs:69-72,127-138`). Deväť `instead` viet vo
   `verdicts.rs` a odsek v `hub.md:1020-1024` („the hub does serve list_assets“) ukazujú na tool,
   ktorý nemá čo odpovedať. To je nový nález **UX-49 (C)** a predpoklad všetkého ostatného.
2. **Päť príkazov je na routing pripravených bez jediného nového toolu:** `catalog_list_assets`,
   `catalog_list_layers`, `catalog_propose_layers`, `assets_scan_hosts`, `catalog_plan_sync`
   volajú **tú istú `service::catalog` funkciu** ako hubov tool a argumenty mapujú pole na pole
   (`mcp/tools/assets.rs` vs `commands/assets.rs`). Verdikt „built on the catalog checkout, which
   only the machine that owns the fleet has“ nie je pravda o týchto piatich — checkout potrebuje
   hub, nie klient.
3. **Dva nové Client/readonly tools stačia na celý prehliadací režim:** `catalog_config`
   (`CatalogConfigRow | null`, gate panelu a zdroj `repo_path @ head`) a `catalog_get_asset`
   (detail s preview per harness). Plus rozšírenie výsledku `list_assets` o `last_sync`
   (`#[serde(default)]`), čím `catalog_last_sync` nepotrebuje vlastný tool. Odhad rozpočtu popisov
   **+~450 B netto** (dva tools ≈ +620 B, škrt štyroch viet „Requires catalog_configure +
   catalog_load in the app“ ≈ −210 B) → 57 603 → ~58 050; s iteráciou 03 ~58 700 →
   `BUDGET_BYTES` 58 800.
4. **Master-only tier ostáva odmietnutý, ale s funkciou, nie prózou:** `apply_sync`,
   `set_host_layers`, `set_secret` sú `Access::Master` (`guard.rs:648-699`) a klient nikdy nie je
   master. UI ich ukáže **disabled s dôvodom** (kľúče `REASONS.apply_sync`/`REASONS.set_secret`
   existujú a nik ich nevolá — `hub_verdicts.test.ts:70-74`), plán zo `plan_sync` je pritom
   viditeľný celý.
5. **Authoring tier (13 príkazov) ostáva odmietnutý správne** — git checkout je na stroji hubu a
   „import z local“ na hube znamená `$HOME` hubového procesu (`import.rs:36-43`, UX-54). Tlačidlá
   Edit/Delete/New/Lint/Commit/Push/Import/Pull/Open in session sa v hub režime **nevykreslia**;
   jedna veta v scope banneri povie, kde sa autoruje.
6. **Matica: 5 route-existing · 2 route-new · 1 zložený do rozšíreného toolu · 25 refused**
   (z toho 3 Master-tier s existujúcim toolom, 1 sémantický nesúlad, 1 shape gap bez UI,
   1 mŕtve volanie, 19 authoring/checkout). Deväť nových nálezov UX-49…UX-57.
7. **Tri PR:** PR-4c (hub načíta katalóg: boot + `fleet-hub catalog` CLI, **S/M**) → PR-4a (Rust
   routing + 2 tools + kontrakt + minimálny TS, **M**) → PR-4b (Svelte panel v hub režime, **M**).
   PR-4c ide prvý — bez neho routing vráti pravdivé, ale prázdne „hub has no catalog“.

## Matica parity

33 príkazov v `generate_handler!` poradí (`verdicts.rs:674-907`). „UI“ = kto príkaz volá
(`src/lib/assets.ts` wrapper → komponent). „Hub tool“ = existujúci tool v `guard::TOOL_POLICIES`
(`guard.rs:605-700`) a čo mu chýba. Akcia: **RE** route na existujúci tool · **RN** route na nový
tool · **W** zložiť do rozšíreného výsledku iného toolu · **R** ostáva refused.

### Tier 1 — prehliadanie katalógu (musí fungovať pre každého spárovaného klienta, aj `readonly`)

| Príkaz | UI | Verdikt dnes | Hub tool dnes (medzera) | Akcia |
|---|---|---|---|---|
| `catalog_config` | `loadCatalogConfig` → `AssetsPanel` onMount `:81`, gate `:77` | LocalOnly (`CATALOG_IS_A_CHECKOUT`) | **žiadny**; `catalog::config` je čistý `Store` read (`mod.rs:65-67`) | **RN** `catalog_config` — Client, readonly, Quick |
| `catalog_list_assets` | `loadAssets` → `AssetList` (skupiny, chipy stavov, unmanaged) | LocalOnly („hub does serve this… but the panel is built on the checkout“) | `list_assets` = **tá istá funkcia** `catalog::list_assets(&store)` (`assets.rs:11-17`), výsledok `AssetListing` bajt za bajtom (`ok_json_compact` iba strihá `null` — `AssetSummary.install_as` má aj tak `skip_serializing_if`) | **RE** `list_assets`; rozšíriť o `last_sync` (pozri W nižšie) |
| `catalog_get_asset` | `getAsset` → `AssetDetail` (`:42-50`): titul, desc, tagy, matica hostov, Preview per harness; `AssetEditor` pri edite | LocalOnly | **žiadny**; `resolve_preview` má komentár, prečo hub *neposiela* celé `Asset` (`assets.rs:188-195`: body + base64 `bytes` každého resource) — platí pre celý katalóg, nie pre **jeden** asset | **RN** `catalog_get_asset { kind, name }` — Client, readonly, Quick; výsledok `AssetDetail` (jeden asset je ohraničený; UI z `resources[].bytes` číta iba dĺžku — `assets.ts:399-405`) |
| `assets_inventory` | `loadInventory` → store `inventory` v `assets.ts:94,136-140` | LocalOnly (`CATALOG_IS_A_CHECKOUT`) | žiadny; **Store-only** read | **R + zmazať volanie** — store `inventory` nemá čitateľa (UX-53); `list_assets` už nesie `hosts` aj `unmanaged` |
| `catalog_last_sync` | `lastSync` → `assets-last-sync` v toolbare `:251` | LocalOnly (`CATALOG_IS_A_CHECKOUT`) | žiadny; `sync::last_sync` je **Store-only** (`sync/mod.rs:531-535`) | **W** — `AssetListing.last_sync: Option<SyncRunSummary>` (`skip_serializing_if none`), TS `last_sync?: SyncRunSummary \| null`; v hub režime panel číta z listingu, príkaz sa nevolá; verdikt ostáva LocalOnly s opraveným `instead` („the hub folds it into list_assets“) |
| `catalog_list_layers` | **nik** (`noUiControl`, `hub_verdicts.test.ts:113-124`) | LocalOnly | `list_layers` = tá istá `catalog::list_layers` (`assets.rs:168-175`), `LayerListing` identický | **RE** `list_layers` — nulová cena, budúca Layers UI funguje rovno |
| `catalog_resolve_preview` | nik | LocalOnly (shape gap správne opísaný) | `resolve_preview` vracia projekciu `{provenance, excluded, assets:[{kind,name,version}]}` (`assets.rs:178-214`), Tauri vracia `Resolution` s celým `Catalog` | **R** kým nie je Layers UI (UX-52); voliteľne `full: bool` param (`#[serde(default)]`, +~130 B) — potom RE |
| `catalog_propose_layers` | nik | LocalOnly | `propose_layers` identický (`assets.rs:214-222`) | **RE** `propose_layers` |
| `catalog_repo_status` | `repoStatus` → strip `assets-repo-status` `:259-264`, po každom zápise | LocalOnly | žiadny; `git status` **hubovho** checkoutu — dirty/ahead sú fakty o stroji, kde klient nemôže commitnuť ani pushnuť | **R**; strip sa v hub režime nevykreslí, `@ head` ide z `catalog_config.head_commit` |
| `catalog_lint_asset`, `catalog_lint_all` | `AssetDetail` Lint `:163`, `LintAllDialog` | LocalOnly | žiadny; `author::lint_everything(&store)` číta `CATALOG` (mohol by byť Client/readonly), ale lint je spätná väzba autorovi | **R** (tier 3 — bez zápisu nemá komu slúžiť); tlačidlá skryť |
| `catalog_template`, `catalog_layer_template` | `assetTemplate` → `NewAssetDialog`; layer nik | LocalOnly (`CATALOG_IS_A_CHECKOUT`) | žiadny; **čisté funkcie** (`author.rs:38,117`) — ani store, ani checkout | **R** — šablóna je prvý krok zápisu; **opraviť `instead`** (UX-50) |

### Tier 2 — priradenie vrstiev a sync (Client `full` číta a plánuje; Master zapisuje)

| Príkaz | UI | Verdikt dnes | Hub tool dnes (medzera) | Akcia |
|---|---|---|---|---|
| `assets_scan_hosts` | `scanHosts` → toolbar `assets-scan` `:235`, výsledok `assets-scan-result` | LocalOnly („feeds an inventory panel built on the checkout“) | `scan_assets` = **tá istá** `inventory::scan_hosts(&store,&ssh,host_alias)` (`assets.rs:19-34`), `ScanAssetsParams.host_alias` ↔ `ScanArgs.host_alias` 1:1, `Vec<HostScanResult>` identický; Client, **readonly: true**, Lifecycle | **RE** `scan_assets` — funguje aj `readonly` klientovi; scan robí hub cez svoje SSH |
| `catalog_plan_sync` | `planSync` → toolbar `Sync` `:237`, `AssetDetail` „Sync this asset“ `:160`, bunky matice `cell-sync-*` `:200` → `SyncPlanDialog` | LocalOnly („plan is shown in a sync panel built on the checkout“) | `plan_sync` = tá istá `sync::plan_sync(PlanArgs,…)` (`assets.rs:63-92`); `PlanSyncParams {host_alias, kind: Option<String>, name}` ↔ `PlanArgs {host_alias, kind: Option<Kind>, name}` — `Kind` serializuje snake_case, ktorý `parse_kind` prijme; Client, readonly: **false**, Lifecycle | **RE** `plan_sync`; `PlanArgs` potrebuje `Serialize` (memory: hub-routed args); plán sa parkuje v **hubovom** registri na 10 min (UX-57) |
| `catalog_apply_sync` | `applySync` → `SyncPlanDialog` Apply `:147` | LocalOnly (správne: master-only) | `apply_sync` **Master + confirm** (`guard.rs:648-653`) | **R** — Apply **disabled** s `hubBlock('apply_sync')`, `force-partial` skrytý; otázka 2 |
| `catalog_set_host_layers` | nik | LocalOnly (správne) | `set_host_layers` Master (`guard.rs:694-699`) | **R** (Master); Layers UI raz ukáže priradenie read-only |
| `catalog_set_secret` | `setSecret` → `SecretsPanel` Set `:120` | LocalOnly (správne) | `set_secret` Master (`guard.rs:658-663`) | **R** — Set disabled s `hubBlock('set_secret')` |
| `catalog_delete_secret` | `SecretsPanel` Delete | LocalOnly (`CATALOG_IS_A_CHECKOUT` — **nepravda**, Store-only) | žiadny | **R** (Master tier ako `set_secret`); opraviť `instead` (UX-50) |
| `catalog_list_secrets` | `listSecrets` → `SecretsPanel` `:23` (mená, host, `updated_at` — **nikdy hodnoty**, `SecretRow` `rows.rs:713-717`) | LocalOnly (`CATALOG_IS_A_CHECKOUT` — nepravda, Store-only) | žiadny | **R dnes**; alternatíva RN `list_secrets` (Client/readonly — mená sú aj tak v pláne ako `secrets`/`missing_secrets`) — otázka 3. Hub variant `SecretsPanel` ukáže mená z posledného plánu (`secretNames`, `AssetsPanel:201-208`) |
| `catalog_load` | `loadCatalog(pull)` → Pull `:234`, po importe, pri mounte | LocalOnly (`CATALOG_IS_A_CHECKOUT`) | žiadny — a **hub ho potrebuje sám** (UX-49) | **R pre klienta**; na hube: load pri `serve` boote + `fleet-hub catalog load [--pull]` CLI (PR-4c); voliteľne Master tool (+~280 B). Klient dostane `catalog:loaded` event (hub posiela všetky druhy, `events_route.rs:152-170`) → `App.svelte:229` znovu načíta listing |
| `catalog_configure` | `configureCatalog` → setup formulár `:222-229` | LocalOnly | žiadny; klonuje na disk hubu | **R pre klienta**; `fleet-hub catalog set --path … [--remote …]` (PR-4c) |
| `catalog_import_host` | `importHost` → `ImportDialog`; Import link pri unmanaged riadku (`AssetList.svelte:55`) | LocalOnly („call import_assets on the hub“) | `import_assets` existuje (Client, mutating), ale `ImportSources::for_local()` číta `$HOME/.claude` **hubového procesu** (`import.rs:36-43`) — v Dockeri kontajner, nie používateľov Mac | **R** — sémantika `local` sa líši (UX-54); `instead` prepísať; Import link skryť |

### Tier 3 — authoring a git (iba stroj s checkoutom)

| Príkazy | UI | Akcia |
|---|---|---|
| `catalog_create_asset`, `catalog_update_asset`, `catalog_delete_asset`, `catalog_add_resource`, `catalog_remove_resource` | `NewAssetDialog`, `AssetEditor`, `AssetDetail` Edit/Delete `:165-167` | **R** — tlačidlá sa v hub režime nevykreslia; `add_resource` navyše číta **lokálny** súbor (`check_local_path`, `commands/assets.rs:66-79`), ktorý by na hube neexistoval |
| `catalog_write_layer`, `catalog_delete_layer` | nik | **R** |
| `catalog_commit_pending`, `catalog_push` | toolbar `assets-commit-pending`, `assets-push` `:242-253` | **R** — skryť aj strip repo statusu |
| `catalog_spawn_author_session` | `AuthorSessionDialog` z `AssetDetail` „Open in session“ `:162` | **R** — hubov `new_session` by potreboval katalógový repo ako projekt na hubovom hoste (UX-58 mimo rozsahu); skryť |

**Súčty:** RE 5 · RN 2 · W 1 · R 25 (3× Master s existujúcim toolom, 1× sémantický nesúlad,
1× shape gap bez UI, 1× mŕtve volanie, 19× checkout/authoring). Zo 70 LocalOnly riadkov
(`hub_verdicts.generated.json`) ubudne **7** (5 RE + 2 RN); `catalog_last_sync` ostáva
LocalOnly, ale UI ho v hub režime nevolá.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-17 | **Potvrdené, rozšírené — a premisa README opravená.** (a) Panel v hub režime vykreslí dva odseky (`AssetsPanel.svelte:212-221`): `hubBlock('catalog_config')` (veta `REASONS.catalog_config`, `hub.ts:179-180`, ~40 slov + „Do it on the hub (https://fleet.rlt.sk)“) a pevný odsek o sync a secrets. Žiadny fakt: ani či hub katalóg má, ani head, ani posledný sync. (b) Test `hub_disabled.test.ts:153-163` pinuje „shows the reason instead“ a `assets-sync`/`assets-secrets` = null — PR-4b ho musí prepísať. (c) README tvrdí „hub pritom list_assets… servíruje“: **servíruje iba tool definíciu**; volanie končí `E_CATALOG_NOT_CONFIGURED`, lebo hub katalóg nikdy nenačíta (UX-49). Parita Assets teda nie je iba routing — najprv musí hub katalóg *mať*. (d) `list_assets`, `list_layers`, `propose_layers`, `scan_assets`, `plan_sync` volajú tú istú service funkciu ako Tauri príkazy a ich výsledky sú s TS typmi v `assets.ts` kompatibilné bez zmeny — routing je päť riadkov vo `verdicts.rs` + päť metód v `remote.rs`. | `mod.rs:27-29,97-125,127-138`, `assets.rs` (mcp) `:11-17,19-34,63-92,168-175,214-222`, `commands/assets.rs:172-176,193-210,284-305`, `verdicts.rs:693-701,709-716,728-735,779-800` |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-49 | C | **Hub katalóg nikdy nenačíta, takže žiadny z jeho 10 asset toolov nefunguje.** `CATALOG` je `LazyLock<RwLock<Option<Catalog>>>`, `None` kým `catalog::load` neprebehne; volajú ho len Tauri `catalog_load` a authoring commit-reload. `crates/fleet-hub/src/` nemá ani jeden výskyt `catalog`; `fleet-hub` CLI (`main.rs:24-108`) má `init · serve · token · agent-token · host-token-mode · pair · client · demo-seed`; MCP nemá `catalog_configure`/`catalog_load`. Aj keby `state.db` niesol `catalog_config` riadok (napr. skopírovaný z desktopu), `with_catalog` odpovie „catalog not loaded; call catalog_load“ — a nie je čím. Popisy piatich toolov hovoria „Requires catalog_configure + catalog_load **in the app**“ — na hube žiadna „app“ nie je. Deväť `instead` viet („call list_assets on the hub“, „call plan_sync on the hub“ …) a `hub.md:1020-1024` sú inštrukcie bez cieľa — rovnaký vzor ako UX-42 pri Settings. | `mod.rs:27-29,69-72,127-138`, `commands/assets.rs:169`, `author.rs:631`, `crates/fleet-hub/src/*` (grep prázdny), `assets.rs` (mcp) `:12,169,180,226` |
| UX-50 | H | **`CATALOG_IS_A_CHECKOUT` je pripnutý na sedem príkazov, ktoré checkout nepotrebujú.** `catalog_config`, `assets_inventory`, `catalog_last_sync`, `catalog_list_secrets`, `catalog_delete_secret` sú čisté `Store` operácie; `catalog_template` a `catalog_layer_template` sú čisté funkcie bez store aj bez disku. Veta „the asset catalog is a git checkout… and the hub has no tool for this“ je pri nich zavádzajúca — správny dôvod je buď „hub nemá tool“ (config, inventory, last_sync, list_secrets), „master-only tier“ (delete_secret) alebo „šablóna bez zápisu nemá zmysel“ (template). Test `every_local_only_message_is_the_one_the_fixture_records` pinuje text → zmena = `REGEN_LOCAL_ONLY`. | `verdicts.rs:88-91`, `mod.rs:65-67,183-185`, `sync/mod.rs:531`, `commands/assets.rs:336-341,358-365`, `author.rs:38,117` |
| UX-51 | M | **Hub režim panelu = 2 odseky prózy bez jediného faktu**; druhý odsek („Sync and secrets are fleet administration besides…“) je natvrdo v šablóne, nie z `REASONS`. Žiadne: URL hubu ako fakt, či hub katalóg má, `repo_path @ head`, počet assetov, posledný sync. Používateľ, ktorý chce vidieť, čo je na jeho hostoch drifted, nemá kam. | `AssetsPanel.svelte:212-221`, `hub_disabled.test.ts:153-163` |
| UX-52 | M | **Sedem príkazov vrstiev (layers) nemá žiadnu UI**, hoci spec `2026-09-17-asset-layers-and-profiles-design.md:255-267` sľubuje Tauri príkazy „mirrored as MCP tools“ a `hub_verdicts.test.ts:113-124` to zapisuje ako `noUiControl`. Verdikt `catalog_resolve_preview` pedantne opisuje shape gap (summary vs. `Resolution`), ktorý dnes nikoho neblokuje. Parita vrstiev je preto lacná (dva RE riadky) a zároveň zbytočná, kým Layers UI nevznikne — mimo tejto šošovky, poznamenať pre konsolidáciu. | `hub_verdicts.test.ts:113-124`, `verdicts.rs:718-726`, `assets.rs` (mcp) `:178-214` |
| UX-53 | L | **`assets_inventory` je mŕtve volanie.** `AssetsPanel.refresh()` ho volá paralelne s `loadAssets` pri každom refreshi, výsledok ide do store `inventory` (`assets.ts:94,136-140`), ktorý **nikto nečíta** (jediný výskyt mimo `assets.ts` je komentár v `AssetDetail.svelte:136`); `AssetList` aj `AssetDetail` čítajú `hosts`/`unmanaged` z listingu. Zmazať volanie = o jeden príkaz menej na paritu a o jeden IPC round-trip menej v standalone. `mergeInventoryRow`/`clearInventoryFor` (`App.svelte:227-228`) tiež zapisujú do mŕtveho store. | `AssetsPanel.svelte:47-51`, `assets.ts:94,136-160`, `App.svelte:227-228` |
| UX-54 | M | **`catalog_import_host` má na hube inú sémantiku, než `instead` sľubuje.** Veta hovorí „call import_assets on the hub“, ale `import_host` podporuje iba `host_alias = local` a `ImportSources::for_local()` číta `$HOME/.claude` **procesu, ktorý tool beží** — na hube kontajner `fleet-hub`, nie Mac používateľa. Routing by teda „importoval z local“ niečo iné, než tlačidlo *Import from host* sľubuje (porušenie *parity or refusal*, `routing.rs:38-60`). Odmietnutie je správne; text nie. | `import.rs:36-43`, `mod.rs:247-269`, `verdicts.rs:771-777`, `ImportDialog.svelte` |
| UX-55 | L | **Telefónny klient z Assets parity nezdedí nič** — spec `fleet-mobile-design.md:86-110` má päť obrazoviek (pairing, Sessions, detail, prompt, Hosts) a slovo *asset*/*catalog* sa v ňom nevyskytuje. README §4 („telefónny klient… zdedí fix na hub strane“) pre UX-17 neplatí; platí iba to, že hub s načítaným katalógom (UX-49) je predpoklad pre *akéhokoľvek* budúceho klienta. | `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md` (grep prázdny) |
| UX-56 | L | **Rozpočet popisov platí za vetu, ktorá je na hube nepravdivá.** „Requires catalog_configure + catalog_load in the app.“ sa opakuje v `list_assets`, `list_layers`, `resolve_preview`, `set_host_layers` (~52 B × 4 ≈ 210 B) a „in the app“ nemá na hube referent. Po PR-4c (hub načíta katalóg pri boote) veta zmizne; uvoľnené bajty zaplatia tretinu nových toolov. | `assets.rs` (mcp) `:12,169,180,226`, `tests.rs:2357` |
| UX-57 | M | **Plán zo `plan_sync` má na hube 10-minútovú platnosť a klient ho nikdy neaplikuje.** `plan_sync` parkuje plán v registri hubu; `apply_sync` je Master. Po routingu klient uvidí plán s `plan_id`, ktorý pre neho nemá pokračovanie — dialóg to musí povedať vopred (Apply disabled s dôvodom, nie po kliku `E_FORBIDDEN`). Navyše `plan_sync` má `readonly: false` → `readonly` klient dostane `E_FORBIDDEN` už pri plánovaní, a desktop nevie, či je `readonly` (UX-45 z iterácie 03) — Sync tlačidlo musí odmietnutie zobraziť inline, nie toastom. | `guard.rs:633-653`, `assets.rs` (mcp) `:63-92,97-131`, `SyncPlanDialog.svelte:124-147` |

## Návrh hub tools / rozšírení

Zásady z iterácie 03 platia: meno toolu = meno príkazu; `Store`/`CATALOG` read = `Quick`;
popis = jedna klauzula; próza do `docs/hub.md` a `docs/control-api.md`. Nič nejde na `confirm`
(hub nemá approvera). Poradie: najprv hub katalóg **má** (PR-4c), potom ho **servíruje**.

### 0 — Hub načíta katalóg (PR-4c, predpoklad; UX-49)

| | |
|---|---|
| `serve` boot | V `crates/fleet-hub/src/serve.rs::serve` po `persist(&store,&r)` a pred štartom control API: ak `catalog::config(&store)?` je `Some`, `catalog::load(false, &store)` — chyba **loguje a pokračuje** (`E_CATALOG_GIT`/`E_CATALOG_PARSE` nesmú zabiť hub; `problems` sa aj tak objavia v `list_assets`). `load` volá `repo::ensure_repo` (klon, ak chýba) — v Docker obraze treba `git` a prístup k remote (otázka 4) |
| CLI | `fleet-hub catalog set --path <dir> [--remote <url>]` (= `catalog::configure`), `fleet-hub catalog load [--pull]` (= `catalog::load`, vyžaduje bežiaci hub? — nie: `load` zapisuje `CATALOG` **vlastného** procesu, takže one-shot CLI hub proces nenaplní. Preto `load` cez CLI = zapísať do store a poslať signál, alebo jednoducho **MCP tool** `catalog_load` volaný cez `fleet-hub client …`? Najjednoduchšie: CLI `set` + reštart hubu, alebo Master tool — otázka 1) |
| Voliteľný Master tool `catalog_load { pull }` | `Access::Master, readonly: false, confirm: false, Lifecycle` (pull = git nad sieťou); popis `"Re-read the hub's catalog checkout, optionally git-pulling first; answers the CatalogSummary."`; ≈ +280 B. Bez neho: `catalog:loaded` po zmene remote nastane až po reštarte hubu. Alternatíva bez toolu: nastavenie `catalog.pull_interval_secs` (do `SPECS`, tick v hube) |
| Docs | `hub.md` → *Configuration* riadok `catalog`, *What is different from standalone* bullet prepísať (dnes „hub does serve list_assets“) |

### 1 — `catalog_config` (nový, Client/readonly)

| | |
|---|---|
| Súbor | `crates/fleet-core/src/mcp/tools/assets.rs` (`assets_router`) |
| Popis (návrh) | `"The hub's catalog checkout — repo path, remote, HEAD and when it was last loaded — or null when none is configured."` |
| Params | žiadne |
| Výsledok | `Option<CatalogConfigRow>` = `catalog::config(&store)` — totožné s Tauri (`commands/assets.rs:144-151`) |
| Policy | `Access::Client, readonly: true, confirm: false, Deadline::Quick` |
| Prečo | Gate panelu a jediný zdroj „má hub katalóg?“ (empty state) a `repo_path @ head` v toolbare; bez tajomstiev (`remote_url` je git URL, ktorú klient aj tak vidí v pláne) |
| Wire | `CatalogConfigRow` (`rows.rs:685-690`) potrebuje `Deserialize`; sample do `tests_contract.rs::the_whole_contract` |

### 2 — `catalog_get_asset` (nový, Client/readonly)

| | |
|---|---|
| Popis (návrh) | `"One catalog asset in full — header, body, resources — with its per-harness render preview and per-host state."` |
| Params | `GetAssetParams { kind: String /// Asset kind: skill, agent, hook, mcp_server or plugin_ref. , name: String /// Asset name as list_assets lists it. }` — dve `///` polia (test `every_tool_parameter_is_documented`); `kind` cez `parse_kind` (`assets.rs:258-264`) |
| Výsledok | `AssetDetail { asset, previews, hosts }` = `catalog::get_asset(kind,&name,&store)` — totožné s Tauri; `ok_json` (nie compact — `Preview.plan: Option` a `unsupported: Option` sú sémantické `null`, ktoré `AssetDetail.svelte:217-222` rozlišuje) |
| Policy | `Access::Client, readonly: true, confirm: false, Deadline::Quick` |
| Veľkosť odpovede | Jeden asset: `body` + `resources[].bytes` base64. Riziko z komentára pri `resolve_preview` sa týka **celého katalógu**; jeden skill s resources je rádovo desiatky kB. Ak vlastník chce strop: `with_resources: bool` (`#[serde(default)]`), pri `false` `bytes: ""` — UI číta iba `resourceSize` (`assets.ts:399-405`), takže by ukázalo 0 B — radšej nie; poznámka v popise stačí |
| Wire | `AssetDetail`, `Preview`, `RenderPlan`, `FileWrite`, `ConfigMerge`, `HostState` — `Deserialize` + sample v kontrakte; `Asset` má ručný serializer (`model.rs::merge_header_and_spec`) a `Deserialize` už existuje (`UpdateArgs.asset`) |

### 3 — rozšírenie `list_assets` o `last_sync` (W)

`AssetListing` (`mod.rs:155-162`) dostane `#[serde(default, skip_serializing_if = "Option::is_none")]
pub last_sync: Option<SyncRunSummary>`, naplnené `sync::last_sync(store)?` v `list_assets`. Popis
toolu +klauzula „…and the last sync run“ (~+40 B). Tauri `catalog_list_assets` ho ponesie tiež →
`AssetsPanel` môže v **oboch** režimoch čítať `$catalog.last_sync` a `catalog_last_sync` volať už
len po `applySync` (kde `onSyncApplied` beztak nastaví `lastSyncRun` zo summary). TS:
`AssetListing.last_sync?: SyncRunSummary | null`.

### 4 — čo sa mení vo `VERDICTS` (krok 3 receptu)

| Príkaz | Dnes | Po PR-4a | Poznámka |
|---|---|---|---|
| `catalog_config` | LocalOnly `CATALOG_IS_A_CHECKOUT` | `Routed { tool: "catalog_config" }` | `REASONS.catalog_config` (`hub.ts:179-180`) **musí zmiznúť** → gate panelu prepísať (PR-4a nesie minimálny TS) |
| `catalog_list_assets` | LocalOnly | `Routed { tool: "list_assets" }` | jediný RE, kde meno toolu ≠ meno príkazu — ako `health_check → fleet_health` |
| `catalog_get_asset` | LocalOnly | `Routed { tool: "catalog_get_asset" }` | |
| `catalog_list_layers` | LocalOnly | `Routed { tool: "list_layers" }` | bez UI, nulová cena |
| `catalog_propose_layers` | LocalOnly | `Routed { tool: "propose_layers" }` | bez UI |
| `assets_scan_hosts` | LocalOnly | `Routed { tool: "scan_assets" }` | |
| `catalog_plan_sync` | LocalOnly | `Routed { tool: "plan_sync" }` | `PlanArgs` + `Serialize` |
| `catalog_last_sync` | LocalOnly `CATALOG_IS_A_CHECKOUT` | LocalOnly, `instead: "the hub folds the last run into list_assets; the panel reads it there"` | text → `REGEN_LOCAL_ONLY` |
| `assets_inventory` | LocalOnly `CATALOG_IS_A_CHECKOUT` | LocalOnly, `instead: "the inventory is already folded into list_assets (hosts, unmanaged); nothing reads this on its own"` | + zmazať volanie (UX-53) |
| `catalog_list_secrets`, `catalog_delete_secret` | `CATALOG_IS_A_CHECKOUT` | LocalOnly, text „secrets are the hub's master-tier store rows, like set_secret; the hub has no tool for this yet“ | otázka 3 |
| `catalog_template`, `catalog_layer_template` | `CATALOG_IS_A_CHECKOUT` | LocalOnly, text „a template is the first step of a write into the checkout, which only the hub's machine has“ | |
| `catalog_import_host` | „call import_assets on the hub“ | LocalOnly, text „the hub's import_assets reads the HUB process's own ~/.claude, not this machine's; import from the machine whose config it is“ | UX-54 |
| `catalog_apply_sync`, `catalog_set_secret`, `catalog_set_host_layers` | LocalOnly (master-only) | **ostáva**; texty OK | UI: disabled s dôvodom |
| `catalog_resolve_preview` | LocalOnly (shape gap) | ostáva (alebo `full` param → Routed) | otázka 5 |
| ostatných 12 (authoring, load, configure, repo_status, lint×2, commit, push, spawn) | `CATALOG_IS_A_CHECKOUT` | ostáva; `CATALOG_IS_A_CHECKOUT` prepísať na „the catalog checkout is on the hub's machine, and authoring it is the hub operator's — the panel shows the catalog read-only here“ | jedna konštanta, jeden regen |

Zo 70 LocalOnly → **63**; Routed 39 → **46**.

### 5 — rozpočet popisov (`the_served_definition_budget_stays_bounded`, `tests.rs:2325-2400`)

| Položka | Odhad B |
|---|---|
| východisko (iterácia 03 meranie) | 57 603 z 57 700 |
| `catalog_config`: 14 (meno) + ~115 (popis) + ~40 (prázdna schéma) | +170 |
| `catalog_get_asset`: 17 + ~110 + ~320 (dve dokumentované polia) | +450 |
| `list_assets` klauzula `last_sync` | +40 |
| škrt „Requires catalog_configure + catalog_load in the app.“ ×4 (UX-56) | −210 |
| **netto iterácia 04** | **+450 → ~58 050** |
| s PR-3a (iterácia 03, +650) | ~58 700 |
| voliteľný Master `catalog_load` | +280 → ~58 980 |
| voliteľný `resolve_preview.full` | +130 |

Odporúčanie: `BUDGET_BYTES` **58 800** (bez voliteľných; ak PR-3a už zdvihol na 58 400, zdvihnúť
na 58 900) s odsekom v doc-komentári podľa vzoru. Oba nové tools sú `readonly` → rastie aj
`ro_bytes`; podmienka `ro_bytes < bytes / 2` musí ostať pravdivá — beh testu to vypíše
(`host readonly: N tools / B bytes`), pri páde trimovať `catalog_get_asset` popis, nie polia.

### 6 — starší hub

Hub bez `catalog_config` toolu odpovie `E_FORBIDDEN` (gate zlyhá „closed“) alebo
`E_HUB_PROTOCOL`; hub *s* toolom, ale bez katalógu odpovie `null` (prázdny stav), a hub s katalógom,
ktorý sa nenačítal (PR-4c chyba pri boote) odpovie na `list_assets` `E_CATALOG_NOT_CONFIGURED`
„catalog not loaded“. Panel má tri rôzne jednoriadkové stavy — porovnávať **kód**, nie text
(vzor `NewSessionDialog.svelte:180-205`).

## Návrh panelu v hub režime

Podmienka `$hubStatus.remote` (cez `ownsTheFleet`). Standalone vetva sa nemení
(`hub_disabled.test.ts:165-172` „standalone is untouched“ ostáva zelený).

### Stavy panelu

| Stav | Kedy | Čo sa vykreslí |
|---|---|---|
| **Načítavam** | `catalog_config` letí | skeleton toolbaru + „Loading…“ |
| **Hub nevie katalóg servírovať** | `E_FORBIDDEN`/`E_HUB_PROTOCOL`/`E_HUB_CONTRACT` z `catalog_config` | `HubScopeNote` + jeden riadok `assets-hub-unsupported`: „This hub cannot serve its catalog yet — update the hub.“ Bez toastu |
| **Hub nemá katalóg** (`data-testid="assets-hub-empty"`) | `catalog_config` → `null` | `HubScopeNote` + karta: „No asset catalog on `<url>`. The hub's operator configures one with `fleet-hub catalog set --path <dir> --remote <git url>` and restarts the hub.“ **Bez** formulára Local path/Remote URL (`assets-setup` sa nevykreslí — je o disku tohto Macu) |
| **Katalóg konfigurovaný, nenačítaný** | `list_assets` → `E_CATALOG_NOT_CONFIGURED` | toolbar s `repo_path @ head` z configu + riadok „The hub has this catalog configured but has not loaded it (`<message>`) — check the hub's log.“ |
| **Katalóg** | inak | plný read-only panel nižšie |

### Scope banner

Jeden `HubScopeNote` (komponent navrhnutý v iterácii 03, `{ what: 'catalog' }`) navrchu:
„The catalog is the hub's — read from `https://fleet.rlt.sk` (paired as `mac-desktop`).
Authoring happens in its checkout on the hub's machine; sync and secrets need the hub's master
token.“ Jediné miesto s prózou; nahrádza oba dnešné odseky (`AssetsPanel.svelte:214-220`).

### Toolbar (`:231-257`) v hub režime

| Prvok | Standalone | Hub | Prečo |
|---|---|---|---|
| `repo_path` + `@ head` | z `$catalogConfig` | rovnako (z routovaného `catalog_config`) | fakt |
| Pull | áno | **skryť** | `catalog_load` je hubova operácia; `catalog:loaded` event listing obnoví sám |
| Scan hosts (`assets-scan`) | áno | áno — routed `scan_assets`; `hubActionBlocked('assets_scan_hosts')` pri offline | readonly tool, funguje každému klientovi |
| Import from host | áno | **skryť** | UX-54 |
| Sync (`assets-sync`) | plán + Apply | plán áno (routed `plan_sync`), Apply disabled (nižšie); `E_FORBIDDEN` pri `readonly` klientovi inline pod toolbarom („this client is readonly on the hub“) | UX-57 |
| Secrets (`assets-secrets`) | panel so Set/Delete | **disabled s `title = hubBlock('set_secret')`** — alebo read-only mená (otázka 3) | Master tier |
| New asset, Lint all | áno | **skryť** | authoring |
| Commit pending, Push, strip `assets-repo-status` | áno | **skryť** | git na stroji hubu |
| `assets-last-sync` | z `catalog_last_sync` | z `$catalog.last_sync` | W |
| `N problems` badge, filter | áno | áno | z listingu |

Pravidlo *zmizne vs. disabled*: **disabled s dôvodom** pre to, čo na hube existuje, ale tento token
nesmie (Apply, Set secret, budúce Set layers — môže sa zmeniť rozhodnutím vlastníka); **skryť** to,
čo nemá hubový ekvivalent a je o inom stroji (Pull, Import, New, Edit, Delete, Lint, Commit, Push,
Open in session). Skryté prvky nie sú „dočasne nedostupné“, sú inou rolou.

### Telo

- `AssetList` bez zmeny (skupiny, chipy `in sync/drifted/missing`, unmanaged). Pri unmanaged
  riadku **skryť** link *Import* (`AssetList.svelte:55`) — nová prop `readonly` alebo `onimport`
  = `undefined` → link sa nevykreslí; `orphan` badge ostáva.
- `AssetDetail` (`:155-167`): titul, desc, `install_as`, tagy, **matica hostov s bunkovým Sync**
  (routed plán) a **Preview** taby ostávajú; **skryť** Open in session, Lint, Edit, Delete;
  *Sync this asset* ostáva (plán). Nová prop `readonly: boolean` (z `AssetsPanel`), aby komponent
  nečítal `hubStatus` sám — testovateľnejšie.
- `SyncPlanDialog` (`:124-147`): tabuľka plánu ako dnes; **Apply disabled** s
  `title = hubBlock('apply_sync')`, checkbox `plan-force-partial` skrytý, pod tabuľkou jeden riadok
  „Plan `<id>` is valid on the hub for 10 minutes; applying needs the hub's master token.“;
  *Open secrets* → disabled ako Secrets v toolbare. `sync:progress` sa nikdy nevyskytne (klient
  neaplikuje) — nič netreba.
- `SecretsPanel` hub variant (ak ostane R): zoznam `names` z posledného plánu (`secretNames`) s
  označením `missing` (z `missing_secrets`), bez inputu hodnoty, bez Add/Set/Delete; riadok
  „Set on the hub with its master token (`set_secret`).“
- `App.svelte:229` `onCatalogLoaded` volá aj `repoStatus()` → v hub režime **preskočiť**
  (`ownsTheFleet`), inak každý `catalog:loaded` z hubu vyvolá `E_LOCAL_ONLY` toast.
- Mount v hub režime (`AssetsPanel.svelte:79-87`): `loadCatalogConfig()` → ak `Some`: `loadAssets()`
  (nie `reload(false)` — ten volá `catalog_load`); **nie** `repoStatus()`, **nie** `lastSync()`,
  **nie** `loadInventory()`.

## PR plán (podľa receptu z iterácie 03)

Poradie: **PR-4c → PR-4a → PR-4b.** Každý pod ~300 riadkov bez generovaných súborov a testov.

### PR-4c — hub má katalóg (**S/M**, mimo receptu; UX-49, UX-56)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `crates/fleet-hub/src/main.rs` | subpríkaz `Catalog { cmd: CatalogCmd }` — `set --path --remote`, `show`; (`load` pozri otázku 1) |
| 2 | `crates/fleet-hub/src/serve.rs::serve` | po `persist`: `if catalog::config(&store)?.is_some() { if let Err(e) = catalog::load(false,&store) { warn!(…) } }` |
| 3 | `crates/fleet-core/src/mcp/tools/assets.rs` | škrtnúť „Requires catalog_configure + catalog_load in the app.“ ×4 (uvoľní ~210 B) |
| 4 | `docs/control-api-reference.md` | **generované** `REGEN_DOCS=1` (2×) |
| 5 | `docs/hub.md` | *Configuration*: odstavec „Asset catalog“ + CLI; *What is different from standalone* `:1020-1024` prepísať |
| 6 | `Dockerfile`/`docs/hub.md` | `git` v obraze + poznámka o deploy kľúči pre remote (otázka 4) |
| 7 | testy | `serve` boot s configom bez repo → hub beží a loguje (`serve.rs` testy `:940+` vzor); CLI `catalog set` zapíše `catalog_config` riadok |

### PR-4a — Rust: 2 tools + 7 routingov + kontrakt + minimálny TS (**M**; kroky 1–11 receptu)

| # | Krok | Súbor | Zmena |
|---|---|---|---|
| 1 | 2a | `crates/fleet-core/src/mcp/tools/params.rs` | `GetAssetParams { kind, name }` s `///` |
| 2 | 2b | `crates/fleet-core/src/mcp/tools/assets.rs` | `catalog_config`, `catalog_get_asset`; `list_assets` popis +`last_sync` |
| 3 | — | `crates/fleet-core/src/service/catalog/mod.rs:158-222` | `AssetListing.last_sync` + naplnenie |
| 4 | 2c | `crates/fleet-core/src/mcp/guard.rs:605-700` | dva riadky `TOOL_POLICIES` (Client/readonly/Quick) |
| 5 | 2d | `crates/fleet-core/src/mcp/tools/tests.rs:2357` | `BUDGET_BYTES` → 58 800 + odsek; `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` +2 povolené |
| 6 | 2e | `docs/control-api-reference.md`, `docs/control-api.md:279-294` | regen (2×); index *Asset catalog* +2 mená |
| 7 | 3 | `src-tauri/src/backend/verdicts.rs:674-907` | 7× `Routed`; texty podľa tabuľky 4; `CATALOG_IS_A_CHECKOUT` prepísať |
| 8 | 4 | `src-tauri/src/backend/remote.rs` | `catalog_config()`, `catalog_list_assets()`, `catalog_get_asset(kind,name)`, `catalog_list_layers()`, `catalog_propose_layers()`, `assets_scan_hosts(host_alias)`, `catalog_plan_sync(&PlanArgs)` cez `route(...)`; `PlanArgs` + `Serialize` (`sync/mod.rs:45`) |
| 9 | 5 | `src-tauri/src/commands/assets.rs` | `pub(crate) mod routed` so 7 funkciami (vzor `commands/projects.rs:69-90`); telá bez `refuse_local_only`, sync → `async fn` |
| 10 | 6 | `src-tauri/src/backend/tests_routing.rs:285,695` | 5 Case do `routed_read_cases` (`catalog_config` payload `null` aj `{repo_path…}`, `catalog_list_assets` `{"head":"h","loaded_at":1,"assets":[],"unmanaged":[],"problems":[]}`, `catalog_get_asset` `json!({"kind":"skill","name":"x"})`, `catalog_list_layers`, `catalog_propose_layers`), 2 do `routed_mutation_cases` (`assets_scan_hosts` `json!({"host_alias":"h"})`, `catalog_plan_sync` `json!({"host_alias":"h","kind":"skill","name":"x"})` — **nedefaultné hodnoty**); sekcia 2 standalone pre `catalog_config` |
| 11 | 7 | `src-tauri/src/backend/local_only.golden.json` | `REGEN_LOCAL_ONLY=1` — 7 záznamov ubudne, ~14 textov sa zmení; **prečítať diff** |
| 12 | 8 | `src/lib/hub_verdicts.generated.json`, `docs/hub.md` tabuľka | `REGEN_HUB_VERDICTS=1` (2×) |
| 13 | 9 | `src-tauri/src/backend/tests_contract.rs` | `Deserialize` + samples: `CatalogConfigRow`, `AssetListing`, `AssetSummary`, `HostState`, `Problem`, `AssetInventoryRow`, `AssetDetail`, `Preview`, `RenderPlan`, `FileWrite`, `ConfigMerge`, `LayerListing`, `Layer`, `HostLayerRow`, `LayerProposal`, `ProposedLayer`, `ProposedSingleton`, `HostScanResult`, `SyncPlan`, `HostPlan`, `SyncAction`, `SyncRunSummary`; `REGEN_HUB_CONTRACT=1` (2×). `last_sync` s `#[serde(default)]` — starší hub ho nepošle |
| 14 | 10 | `src/lib/hub.ts:179-180,267-284` | zmazať `REASONS.catalog_config`; `ROUTED_ACTIONS` + `'assets_scan_hosts'`, `'catalog_plan_sync'` |
| 15 | 10 | `src/lib/hub_verdicts.test.ts:113-157` | z `noUiControl` von `catalog_list_layers`, `catalog_propose_layers`; z `gatedByAssetsPanel` von `catalog_list_assets`, `catalog_get_asset`, `assets_scan_hosts`, `catalog_plan_sync`; komentár skupiny prepísať (gate už nie je `catalog_config`) |
| 16 | 11 | `src/lib/AssetsPanel.svelte:71-87,212-221` | `catalogBlocked` → `remote = $derived(!ownsTheFleet($hubStatus))`; onMount v hub režime `loadCatalogConfig` + `loadAssets`; remote vetva dočasne = `HubScopeNote`-text + listing read-only (tlačidlá za `{#if !remote}`) — IA robí PR-4b |
| 17 | 12 | `src/lib/hub_disabled.test.ts:140-172` | prepísať „shows the reason instead“ (pozri Akceptačné testy) |
| 18 | 13 | `docs/hub.md` *What is different*, `CLAUDE.md:110` | bullet „the catalog is read from the hub; authoring, sync and secrets stay the hub operator's“; 123 → generovaný počet (UX-48) |

Položka 13 je najväčšia (samples). Ak PR-4a presiahne ~300 riadkov, odštep: **PR-4a1** = položky
1–6 (fleet-core, zelené samostatne), **PR-4a2** = 7–18.

### PR-4b — Svelte: panel v hub režime (**M**; krok 11–12 receptu)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `src/lib/HubScopeNote.svelte` | ak ho PR-3b ešte nezaviedol: vytvoriť tu (`{ what: 'catalog' }`) |
| 2 | `src/lib/AssetsPanel.svelte` | stavy *unsupported / empty / configured-not-loaded / catalog*; toolbar podľa tabuľky (skryť Pull/Import/New/Lint/Commit/Push/strip; Secrets disabled); `lastSync()` → `$catalog.last_sync`; zmazať `loadInventory` (UX-53); `E_FORBIDDEN` pri Sync inline (`assets-forbidden`) |
| 3 | `src/lib/AssetDetail.svelte` | prop `readonly`; skryť Open in session/Lint/Edit/Delete |
| 4 | `src/lib/AssetList.svelte` | `onimport?` voliteľný → bez Import linku |
| 5 | `src/lib/SyncPlanDialog.svelte` | prop `applyBlocked: string \| null` (z `hubBlock('apply_sync')`); Apply disabled + `title`, `force-partial` skrytý, riadok o plán_id |
| 6 | `src/lib/SecretsPanel.svelte` | prop `readonly` (mená + missing, bez Set/Delete) — alebo len disabled tlačidlo v toolbare, podľa otázky 3 |
| 7 | `src/App.svelte:229` | `onCatalogLoaded`: `repoStatus()` len pri `ownsTheFleet` |
| 8 | `src/lib/assets.ts` | `AssetListing.last_sync?`; zmazať `inventory`, `loadInventory`, `mergeInventoryRow`, `clearInventoryFor` **alebo** ich nechať a iba nevolať (bezpečnejšie: nechať, `App.svelte:227-228` ich používa) |
| 9 | testy | `AssetsPanel.hub.test.ts` (nový), `SyncPlanDialog.test.ts`, `AssetDetail.test.ts`, `hub_disabled.test.ts` — pozri nižšie |

## Akceptačné testy

### Rust — `crates/fleet-core/src/mcp/tools/tests.rs`

- `every_router_tool_has_exactly_one_tool_policy_row`, `every_tool_parameter_is_documented`,
  `annotations_follow_the_policy_table` zelené s dvoma novými toolmi.
- `the_served_definition_budget_stays_bounded`: nová konštanta; `ro_bytes < bytes / 2` platí
  (vypísať čísla do PR popisu).
- `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` (`:356`): `catalog_config`,
  `catalog_get_asset` medzi povolené pre `readonly`; `plan_sync` ostáva medzi odmietnutými.
- Nový `catalog_get_asset_answers_the_full_asset_and_e_asset_not_found`: katalóg s jedným skillom
  (vzor existujúcich catalog testov pod `CATALOG_TEST_LOCK`) → JSON má `asset.body`,
  `previews[].harness`, `hosts`; neznáme meno → `E_ASSET_NOT_FOUND`; `kind: "nope"` → `E_INVALID`.
- Nový `catalog_config_answers_null_without_a_catalog`: prázdny store → `null`.
- Nový `list_assets_carries_the_last_sync_run`: po zápise `sync_runs` riadku → `last_sync.plan_id`.

### Rust — `crates/fleet-hub` (PR-4c)

- `serve_loads_a_configured_catalog_at_boot` (fake repo v tempdir) → `CATALOG` je `Some`;
  `serve_survives_a_catalog_that_fails_to_load` (config na neexistujúci path bez remote) → hub
  štartuje, log obsahuje `E_CATALOG_GIT`.
- CLI `catalog set` → `get_catalog_config()` má path; `catalog show` vypíše ho.

### Rust — `src-tauri/src/backend/`

- `tests_routing.rs`: `every_routed_row_is_driven_by_a_case` (7 nových Case),
  `every_commands_body_does_what_its_row_says`, `every_routed_tool_is_a_tool_the_hub_serves`
  (`list_assets`, `list_layers`, `propose_layers`, `scan_assets`, `plan_sync`, `catalog_config`,
  `catalog_get_asset` sú v `TOOL_POLICIES`), `every_local_only_message_is_the_one_the_fixture_records`
  po regen.
- `a_routed_read_answers_the_hub_and_not_the_local_database` + `catalog_config`: lokálny store bez
  configu, fake hub odpovie `{"repo_path":"/hub/catalog",…}` → príkaz vráti hubovu cestu.
- `standalone_reads_still_come_from_the_local_store` + `catalog_config`.
- `tests_contract.rs::the_whole_contract` po regen; `tests_verdict_gen.rs` po regen.

### Frontend — Vitest

`npx vitest run src/lib/hub_disabled.test.ts src/lib/AssetsPanel.hub.test.ts src/lib/AssetsPanel.test.ts src/lib/hub_verdicts.test.ts src/lib/SyncPlanDialog.test.ts src/lib/AssetDetail.test.ts src/App.hub.test.ts`

`hub_disabled.test.ts:140-172` (prepísané):

- „asks the hub for its catalog config and list, and for nothing local“ — v `remote`: `invoke`
  volaný s `catalog_config` a `catalog_list_assets`; **nie** s `catalog_load`, `catalog_last_sync`,
  `assets_inventory`, `catalog_repo_status`.
- „standalone is untouched“ ostáva.

`AssetsPanel.hub.test.ts` (nový, `hubStatus = remote`):

- „renders the hub's catalog read-only“ — `catalog_config` → `{repo_path:'/hub/catalog',
  head_commit:'abcdef1…'}`, `catalog_list_assets` → listing s 2 assetmi → `asset-row-skill-*`
  existujú, `assets-head` = `@ abcdef1`, `hub-scope-note` presne 1×, `assets-remote` = null.
- „hides authoring and git controls, keeps scan and sync“ — `assets-scan`, `assets-sync` existujú;
  `assets-new`, `assets-lint-all`, `assets-push`, `assets-commit-pending`, `assets-repo-status`
  sú null; Pull a Import (podľa textu tlačidla) null.
- „Secrets is disabled with the master-only reason“ — `assets-secrets` `disabled`, `title`
  obsahuje „master“ / `REASONS.set_secret`.
- „shows the empty state when the hub has no catalog“ — `catalog_config` → `null` →
  `assets-hub-empty` obsahuje `fleet-hub catalog set`; `assets-setup` (formulár) null;
  `assets-setup-path` null.
- „an older hub answers E_FORBIDDEN and the panel says so in one line, without a toast“ —
  `catalog_config` odmietne `{code:'E_FORBIDDEN'}` → `assets-hub-unsupported` obsahuje „update the
  hub“; `pushError` nezavolaný.
- „a configured but unloaded catalog is reported, not toasted“ — `catalog_list_assets` odmietne
  `E_CATALOG_NOT_CONFIGURED` → riadok s „not loaded“.
- „Sync plans through the hub and a readonly client's refusal is inline“ — klik `assets-sync` →
  `invoke('catalog_plan_sync', {args:{host_alias:null,kind:null,name:null}})`; ak odmietne
  `E_FORBIDDEN` → `assets-forbidden` obsahuje „readonly“, `pushError` nezavolaný.
- „the last sync line comes from the listing“ — listing s `last_sync` → `assets-last-sync`
  vykreslený bez volania `catalog_last_sync`.
- „a catalog:loaded event from the hub reloads the listing and does not ask for repo status“
  (`App.hub.test.ts` alebo tu cez `subscribeToRowEvents` mock).

`SyncPlanDialog.test.ts`: „Apply is disabled with the reason and force-partial is hidden when
`applyBlocked` is set“ — `plan-force-partial` null, Apply `disabled`, `title` = dôvod; existujúce
testy s `applyBlocked = null` nezmenené.

`AssetDetail.test.ts`: „readonly hides Edit, Delete, Lint and Open in session but keeps Sync“ —
`asset-edit`, `asset-delete`, `asset-lint`, `asset-open-session` null; `asset-sync` a
`cell-sync-*` (pri `missing`) existujú; `preview-tab-claude` existuje.

`AssetsPanel.test.ts` (standalone): existujúce testy zelené; „refresh no longer asks for
assets_inventory“ (UX-53) — `byCmd` bez `assets_inventory` nesmie padnúť.

`hub_verdicts.test.ts`: zelený po regen bez ďalších zmien okrem položky 15 PR-4a.

### Manuálne (screenshot podľa README §1)

Hub režim, tab Assets: jeden banner, zoznam assetov s chipmi drift stavov, detail s maticou a
Preview, `Scan hosts` prebehne (výsledok per host), `Sync` ukáže plán s disabled Apply. Na hube:
`docker exec fleet-hub fleet-hub catalog show` vypíše path a head zhodný s `assets-head`.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| PR-4c (CLI `catalog set/show`, boot load, popisy, docs, Dockerfile) | **S/M** | ~150 riadkov + testy ~60 + generované |
| PR-4a položky 1–6 (tools, listing, policy, budget, reference) | S | ~110 riadkov + generované |
| PR-4a položky 7–12 (verdikty, remote, commands, routing Case, goldeny) | S/M | ~140 riadkov + testy ~90 + generované |
| PR-4a položka 13 (kontrakt: `Deserialize` + ~22 samples) | S/M | ~40 riadkov derive + ~120 samples |
| PR-4a položky 14–18 (TS kontrakt, panel minimum, test, docs) | S | ~50 riadkov + testy ~40 |
| **PR-4a spolu** | **M → odštep 4a1/4a2** | ≈340 riadkov bez testov/generovaných — nad limitom; 4a1 = 1–6, 4a2 = 7–18 |
| PR-4b položky 1–8 (panel, detail, list, dialógy, App) | M | ~220 riadkov, prevažne `{#if}` a props |
| PR-4b položka 9 (testy) | S | ~180 riadkov |
| **PR-4b spolu** | **M** | ≈220 + testy |

Mimo rozsahu, zapísané pre konsolidáciu: Layers UI (UX-52), `resolve_preview.full`, `list_secrets`
tool, otvorenie `apply_sync`/`set_host_layers` `trusted` klientom, `catalog_spawn_author_session`
cez hubov `new_session` (UX-58 — katalógový repo ako projekt na hubovom hoste).

## Otázky pre vlastníka

1. **Ako sa má hubov katalóg (re)načítať?** (a) iba pri `serve` boote + reštart po zmene remote;
   (b) Master MCP tool `catalog_load { pull }` (+~280 B; volateľný operátorovým asistentom cez
   master token, nie desktopom); (c) nastavenie `catalog.pull_interval_secs` a tick v hube
   (`git pull` periodicky, `catalog:loaded` event klientom). Odporúčam **(a) + (c)** — bez toolu,
   bez rozpočtu; (b) až keď bude potrebný ručný zásah.
2. **`apply_sync` pre desktop-klienta:** ostať Master (odporúčam — sync píše na disky všetkých
   hostov, `hub.md` ho definuje ako fleet admin), alebo otvoriť `trusted` `full` klientovi
   (rozšírenie významu trustu, rovnaká dilema ako otázka 1 iterácie 03)? Pri Master ostáva Apply
   disabled s dôvodom a operátor aplikuje cez master token (dnes jediná cesta — CLI `fleet-hub`
   apply nemá).
3. **Secrets v hub režime:** (a) tlačidlo disabled s dôvodom (bez toolu, 0 B); (b) nový
   `list_secrets` Client/readonly — **mená a hosty, nikdy hodnoty** (`SecretRow`) — aby
   `SecretsPanel` ukázal, čo je nastavené vs. `missing` (+~180 B). Odporúčam (a) teraz, (b) ak
   plán často končí na `missing_secrets`.
4. **Docker obraz hubu:** má `git`? Ako dostať deploy kľúč pre `remote_url` (SSH) do kontajnera —
   mount `~/.ssh` alebo HTTPS token? Bez toho `catalog set --remote` na NAS (memory:
   `fleet-hub-deploy-nas`) neklonuje. Alternatíva: bind-mount už naklonovaného repa a `set --path`
   bez remote (`ensure_repo` ho iba otvorí).
5. **`catalog_resolve_preview`:** nechať refused, kým nevznikne Layers UI (odporúčam), alebo
   pridať `full: bool` do `ResolvePreviewParams` už teraz (+~130 B), aby aj sedem layer príkazov
   malo hotovú paritu?
6. **`catalog_get_asset` a veľkosť odpovede:** stačí poznámka v popise („one asset, body in full“),
   alebo chcete `with_resources` prepínač (potom UI ukáže 0 B pri resources — kompromis)?
7. **`BUDGET_BYTES` 58 800** (resp. 58 900 po PR-3a) akceptovateľné? Alternatíva: trimovať
   `plan_sync` popis (dnes ~530 B, najdlhší v `assets.rs` — polovica je história `plugin_update`
   a `orphan`, ktorá patrí do `control-api.md`).
8. **Zmazať mŕtvy `inventory` store** (`assets.ts`, `App.svelte:227-228`) v PR-4b, alebo nechať
   pre budúcu Layers/inventory UI? Odporúčam zmazať — kód, ktorý nikto nečíta, iba mätie paritu.

# Iterácia 05 — Hub parita: Files tab a git operácie

**Šošovka:** hub-client parita vo Files tabe (Changed / All files / History / Branches, commit
box, Fetch/Pull/Push) · **Zasahuje:** UX-18, UX-26 (v rozsahu hub režimu), root-cause klaster 3
(„hub režim odpovedá prózou namiesto funkcie“), FE-7 a SEC-5 z
`docs/plans/2026-09-10-fleet-improvement-plan.md` · **Vstup:** `docs/ux/2026-09-21-audit/README.md`,
screenshot 02, iterácie 03 (sekcia *Recept*) a 04, `docs/hub.md`,
`docs/plans/2026-05-22-git-history-branches.md`, `docs/specs/2026-05-21-hardening-review.md`,
`docs/superpowers/specs/2026-09-20-ux-agent-fab-design.md`, kód na `5a62beed` · **Režim:**
read-only review, žiadne zmeny kódu, žiadny `cargo` beh. **Tretia z troch hub-parity iterácií**
(3 = Settings, 4 = Assets, 5 = Files/git). Postup implementácie je **recept z iterácie 03**
(`03-hub-parity-settings.md` → *Recept — ako dostať LocalOnly príkaz na Routed*, kroky 1–14);
tento dokument ho neopakuje, iba naň odkazuje krokom.

Schválené rozhodnutie (README §5): hub parita = **nové hub tools a routing**, nie skrývanie UI.

Číslovanie nálezov: iterácia 04 už použila **UX-58** (v sekcii *Tier 3* a *Mimo rozsahu* pre
`catalog_spawn_author_session`), preto táto iterácia číslu­je od **UX-59**.

## Zhrnutie

1. **UX-18 potvrdené, ale s dvoma opravami premisy.** (a) Desať git zápisov **má** `instead`
   text — konštanta `NO_GIT_WRITE_TOOL` (`verdicts.rs:95-97`): „the hub exposes no git-write tool —
   a remote client must not stage or commit under a running agent; do it in the session, or from
   a standalone app“. README tvrdí „bez `instead` textu“ — nepravda; pravda je, že text sa
   **nikde nezobrazí**. (b) V hub režime **nič nezlyhá** a žiadny `E_LOCAL_ONLY` toast
   nevznikne: `FilesPanel.svelte:33` spočíta `writeBlocked = hubBlock('repo_write', $hubStatus)` a
   rozdá ho ako `disabled` + `title` do `FileList` (checkbox, textarea, Commit), `RemoteToolbar`
   (Fetch/Pull/Push), `BranchList` (Checkout/Delete/+ New branch) a `CommitGraph`. Dôvod existuje
   **iba v tooltipe**; na screenshote 02 vyzerajú checkboxy aj textarea „Commit message…“ ako
   živé a „Commit 0 files“ je disabled rovnako ako v standalone bez stagovaných súborov. To je
   nový nález **UX-59 (H)**.
2. **Parita je routing, nie nová schopnosť.** Všetkých desať zápisov v
   `crates/fleet-core/src/service/repo_mutate.rs` beží cez `run_git → run_in_repo → ssh::run_shell`
   (`repo.rs:75-115`), **presne tou istou cestou ako osem už routovaných čítaní** — lokálne
   `bash -lc`, na hoste multiplexované SSH, na agent hoste `AgentTransport` (`agent/transport.rs:257`
   implementuje `SshExec`). Hub má SSH ku všetkým hostom; jediné, čo mu chýba, je **desať riadkov
   `#[tool]`** nad funkciami, ktoré už exportuje (`repo_mutate.rs:7-8` „reachable from the MCP
   layer“). Zdôvodnenie v `commands/mutate.rs:4-9` („a remote client staging under a running agent
   would race“) platí **rovnako pre standalone desktop**, ktorý to dnes robí — nie je to argument
   hub vs. standalone (**UX-60, H**).
3. **FE-7 je opravené.** `window.prompt`/`confirm` nahradili `PromptDialog` a `ConfirmDialog`
   (`FilesPanel.svelte:206-212,386-429`), s `validateBranchName` a checkboxom „Check out the new
   branch now“. C3 z plánu je landed v tejto časti.
4. **Návrh: desať Client `full` toolov s menami príkazov, `confirm: false`, bez jedinej zmeny
   v service vrstve.** Ochrany, ktoré service už má — `ensure_clean` → `E_DIRTY` pri checkoute,
   `pull --ff-only`, push **bez** `--force` (parameter neexistuje), `validate::git_ref /
   commit_hash / repo_rel_path`, `shell::quote` na každej hodnote — sú mantinely aj pre hub.
   Doplniť: `force` pri `repo_delete_branch` odmietnuť ne-master callerovi (UI ho aj tak nikdy
   nepošle, `FilesPanel.svelte:237`), `repo_push` viazať na **trusted** klienta (otázka 1), a
   audit už **existuje**: `persist_audit` zapíše `mcp_call` riadok „repo_push by
   client:mac-desktop“ na časovú os session **pred** enforce gate (`tools/mod.rs:151-155`),
   `Timeline.svelte` ho radí do kategórie `ops` (`timeline.ts:49`).
5. **Rozpočet popisov je tu najdrahší z troch iterácií:** 10 toolov ≈ **+4 900 B** (desať schém
   so `session_id` a 1–3 poľami). Bežný súčet: 57 603 → +650 (03) → +450 (04) = 58 703 →
   **~63 600 → `BUDGET_BYTES` 63 800**. Alternatíva B (6 toolov: `repo_checkout` prijme ref aj
   hash, `repo_stage {staged}` zlúči stage/unstage, `repo_remote {op}` zlúči fetch/pull/push)
   ≈ +3 500 B → ~62 200 → 62 400, za cenu 5 príkazov, ktorých meno ≠ meno toolu (precedens
   `catalog_list_assets → list_assets`). Odporúčam **A** (princíp z iterácie 03); B ak vlastník
   uprednostní rozpočet (otázka 2).
6. **Potvrdenia:** hub nemá approvera (`serve.rs:738-749`), takže `confirm: true` by tool na hube
   zablokoval — rovnako ako v iteráciách 03/04 **`confirm: false`** a potvrdenie robí **desktopov
   vlastný `ConfirmDialog`** (dnes ho majú checkout-commit a delete-branch, chýba pri checkout
   vetvy, Pull a Push — **UX-62**). „Confirmation channel for paired clients“, ktorý FAB spec
   odkladá na slice 2, tu **nič neblokuje**: keď vznikne, `repo_push`/`repo_delete_branch`
   prejdú na `confirm: true` pridaním voliteľného `confirm_nonce` (`#[serde(default)]`, drôtovo
   kompatibilné); dnes ho do schémy nedávať (stojí ~150 B na tool).
7. **Matica: 8 Routed (bez zmeny) · 10 Routed na nový tool · 0 ostáva refused.** Deväť nových
   nálezov UX-59…UX-67. **Dva PR:** PR-5a (Rust: 10 toolov + routing + kontrakt + minimálny TS,
   **M**, odštep 5a1/5a2) → PR-5b (Svelte: potvrdenia, inline stavy, skeleton, empty CTA,
   agent-busy hint, **M**).

## Matica parity

18 príkazov v `generate_handler!` poradí (`verdicts.rs:356-468`). „UI“ = komponent a riadok, kde sa
príkaz volá (`src/lib/files.ts`, `src/lib/history.ts` → komponent). „Ako beží“ = cesta v service
vrstve. Akcia: **RE** route na existujúci tool · **RN** route na nový tool · **R** ostáva refused.

### Čítania — už routované, bez zmeny

| Príkaz | UI | Verdikt dnes | Hub tool (policy) | Akcia |
|---|---|---|---|---|
| `repo_changes` | `loadChanges` → strip *Changed* (`FilesPanel.svelte:127-139`) | Routed | `repo_changes` Client/readonly/Quick (`guard.rs:549-554`) | **OK** |
| `repo_tree` | `loadTree` → *All files* (`:141-155`) | Routed | `repo_tree` | **OK** |
| `repo_file` | `FileViewer` obsah | Routed | `repo_file` | **OK** |
| `repo_diff` | `FileViewer` diff | Routed | `repo_diff` | **OK** |
| `repo_log` | `loadHistory` → `CommitGraph` (`:157-173`) | Routed | `repo_log` (celý `RepoLogArgs` ide na drôt aj s nulami — `commands/history.rs:72-77`) | **OK** |
| `repo_branches` | `loadBranches` → `BranchList` (`:185-195`) | Routed | `repo_branches` | **OK** |
| `repo_commit` | `openCommitDetail` (`:175-181`) | Routed | `repo_commit` | **OK** |
| `repo_commit_diff` | `FileViewer` s `commit` prop | Routed | `repo_commit_diff` | **OK** |

Všetky štyri strips (Changed, All files, History, Branches) teda v hub režime **fungujú
read-only už dnes** — screenshot 02 to ukazuje (zoznam zmenených súborov je z hubu). Tvrdenie
README „`repo_*` čítania už idú“ platí pre všetkých osem (nie sedem).

### Zápisy — LocalOnly, všetky RN

Spoločné pre všetkých desať: verdikt `LocalOnly { instead: NO_GIT_WRITE_TOOL }`
(`verdicts.rs:408-468`); Tauri telo `backend.refuse_local_only("…")?` + `repo_mutate::…`
(`commands/mutate.rs`); UI gate `writeBlocked` (`FilesPanel.svelte:33`); FE wrapper v
`history.ts:75-139`; hub tool **žiadny**; service beží cez `run_git` (`repo.rs:105-115`) =
`repo_script` (tmux cwd → `git rev-parse --show-toplevel`, sentinel `E_NO_WORKTREE`) +
`ssh::run_shell(host, "bash -lc <quote(script)>", 10 s)`.

| Príkaz | UI | Service (`repo_mutate.rs`) a ochrany dnes | Návrh tool (meno = príkaz) | Access · readonly · confirm · Deadline |
|---|---|---|---|---|
| `repo_stage` | `FileList` checkbox `.stage` (`:137-146`) → `stageToggle` (`FilesPanel:254-257`) | `validate::repo_rel_path` na každú cestu; `git add -- <quoted…>`; prázdny zoznam = no-op (`:127-146`) | **RN** `repo_stage { session_id, paths[] }` — „Stage worktree paths (git add --) in a session's worktree.“ | Client · false · false · Quick |
| `repo_unstage` | ten istý checkbox, odškrtnutie | `git restore --staged -- <quoted…>` (`:149-165`) | **RN** `repo_unstage { session_id, paths[] }` | Client · false · false · Quick |
| `repo_commit_create` | commit box: textarea + „Commit N files“ (`FileList:190-204`) → `commitStaged` (`FilesPanel:259-261`) | prázdna správa → `E_INVALID`; `git commit [--amend] -m <quote(msg)>` (`:167-189`); UI nikdy neposiela `amend: true` | **RN** `repo_commit_create { session_id, message, amend }` — „Commit the staged changes with `message`; `amend` rewrites HEAD.“ | Client · false · false · Quick |
| `repo_create_branch` | `+ New branch` (`BranchList:28`), „Create branch from here“ v `CommitGraph:118` → `PromptDialog` → `doCreateBranch` (`FilesPanel:245-252`) | `git_ref(name)`; `start_point` = hash **alebo** ref; `checkout -b` alebo `branch` (`:61-99`) | **RN** `repo_create_branch { session_id, name, start_point?, checkout }` | Client · false · false · Quick |
| `repo_delete_branch` | `Delete` pri lokálnej vetve (`BranchList:47`) → `ConfirmDialog` → `doDeleteBranch` s `force=false` (`FilesPanel:235-238`) | `git_ref`; `-d`, alebo `-D` pri `force` (`:101-118`) | **RN** `repo_delete_branch { session_id, name, force }` — **`force: true` odmietnuť ne-master callerovi `E_FORBIDDEN`** (UX-65) alebo pole vynechať zo schémy | Client · false · false · Quick |
| `repo_checkout` | `Checkout` pri vetve (`BranchList:46,58`) → `confirmCheckout` **bez dialógu** (`FilesPanel:202-204`) | `git_ref`; **`ensure_clean` → `E_DIRTY`** ak worktree špinavý; nikdy `--force` (`:19-36`) | **RN** `repo_checkout { session_id, branch }` — „Check out a branch; refuses E_DIRTY when the worktree has uncommitted changes.“ | Client · false · false · Quick |
| `repo_checkout_commit` | „Checkout this commit (detached)“ v `CommitGraph:123` → `ConfirmDialog` (`FilesPanel:386-397`) | `commit_hash` (4–40 lower-hex); `ensure_clean`; detached HEAD (`:39-56`) | **RN** `repo_checkout_commit { session_id, hash }` | Client · false · false · Quick |
| `repo_fetch` | `RemoteToolbar` Fetch (`:25-31`) | `git fetch --all --prune` (`:191-198`); **10 s timeout** (UX-63) | **RN** `repo_fetch { session_id }` | Client · false · false · **Lifecycle** (sieť) |
| `repo_pull` | `RemoteToolbar` Pull | `git pull --ff-only` — nikdy tichý merge (`:200-207`) | **RN** `repo_pull { session_id }` | Client · false · false · Lifecycle |
| `repo_push` | `RemoteToolbar` Push s `set_upstream=false` (`:42`) | `git push`, alebo `push -u origin <HEAD branch>`; **žiadny `--force` parameter** (`:209-231`) | **RN** `repo_push { session_id, set_upstream }` — **Client `full` + trusted** (otázka 1) | Client · false · false · Lifecycle |

**Súčty:** RE 0 (8 už routovaných bez zmeny) · **RN 10** · R 0. Zo 70 LocalOnly riadkov
(`hub_verdicts.generated.json`) ubudne **10** → 60; Routed 39 → **49**. Po iteráciách 03 (−2), 04
(−7) a 05 (−10) je LocalOnly **51** zo 130.

**Ako hub vykoná zápis na vzdialenom hoste:** rovnako ako desktop dnes. `session_target` vezme
z `state.db` hubu `(host_alias, tmux_name)` session, `repo_script` nájde cwd panelu cez `tmux
display-message` a koreň cez `git rev-parse`, `ssh::run_shell` pošle skript ako `bash -lc
<quote(script)>` cez hubov `ControlMaster` k hostu (`ssh.rs:733-749`); pre `host == "local"` je to
stroj hubu (`ensure_local_allowed`, `hub.local_host`); pre agent host `AgentTransport`. Nič nové
nevzniká — mení sa iba **kto** má SSH (hub namiesto desktopu), čo je definícia hub režimu.

## Potvrdené / vyvrátené nálezy

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-18 | **Potvrdené v podstate, dve detaily vyvrátené.** *Vyvrátené:* (a) „bez `instead` textu“ — text existuje (`NO_GIT_WRITE_TOOL`, `verdicts.rs:95-97`) a je v tabuľke `docs/hub.md:1103-1112` desaťkrát; (b) „zlyhajú s `E_LOCAL_ONLY`“ — nezlyhajú, gate je pre-emptívny: `hubBlock('repo_write')` (`hub.ts:191-192`) → `disabled` + `title` na každom ovládači (`FileList:142-143,196-201`, `RemoteToolbar:26-27,33-34,40-41`, `BranchList:28,46-47,58`, `CommitGraph:118-124`). `E_LOCAL_ONLY` by prišiel iba pri obídení UI. *Potvrdené:* používateľ nemá **žiadny viditeľný** dôvod (iba hover tooltip), textarea má normálny placeholder „Commit message…“, checkboxy nemajú disabled štýl (screenshot 02: vyzerajú živé), „Commit 0 files“ je v hub režime disabled z **dvoch** dôvodov naraz a nepovie, z ktorého. Text `REASONS.repo_write` končí „do it in the session“ — bez odkazu kam; backendová verzia pridáva „or from a standalone app“, čo je presne „two brains for one fleet“, ktoré `hub.md:985-988` označuje za zlyhanie, ktorému hub režim bráni. Test `hub_disabled.test.ts:252-345` toto správanie **pinuje** (`expect(checkbox).toBeDisabled(); expect(checkbox.title).toBe(REASON)`) — PR-5a ho musí prepísať. | screenshot 02; `FilesPanel.svelte:28-33`; `commands/mutate.rs:4-9` |
| UX-26 (hub časť) | **Potvrdené.** Štyri nezávislé reťazce „Loading…“ (`FilesPanel:321`, `FileList:128`, `BranchList:33`, `FileViewer:188`) bez skeletu; „Select a file to view it.“ (`FileViewer:166`) bez CTA; „No changes.“ (`FileList:133`) bez ďalšieho kroku. V hub režime je každé načítanie **dva skoky** (desktop → hub HTTP → SSH na host), takže holý text je viditeľný dlhšie než standalone. | `FilesPanel.svelte:320-323`, `FileList.svelte:126-134`, `FileViewer.svelte:164-166,186-190` |
| FE-7 (plán) | **Vyvrátené ako otvorený problém — opravené.** `PromptDialog` pre *New branch* (s `validateBranchName`, checkbox „Check out the new branch now“ namiesto druhého confirmu) a `ConfirmDialog` pre *Checkout commit* a *Delete branch*; komentár `FilesPanel.svelte:206-208` opisuje pôvodný WKWebView problém. `grep window.prompt src/lib/*.svelte` nájde iba tento komentár. | `FilesPanel.svelte:206-252,386-429`, `src/lib/PromptDialog.svelte`, `ConfirmDialog.svelte` |

## Nové nálezy

| ID | Sev | Zistenie | Dôkaz |
|---|---|---|---|
| UX-59 | H | **Odmietnutie git zápisov v hub režime je neviditeľné.** Jediný nosič dôvodu je atribút `title`; disabled checkbox a textarea nemajú odlišný vzhľad (`FileList.svelte` štýly `.commit-footer textarea` nemajú `:disabled` pravidlo; `.stage` checkbox natívny), `RemoteToolbar button:disabled { opacity: .5 }` je jediný vizuálny signál. Nikde nie je riadok „Git writes are refused from this window because …“. Používateľ na screenshote 02 vidí funkčný commit box, ktorý nikdy nič neurobí. | screenshot 02; `FileList.svelte:137-146,190-204,361-395`; `RemoteToolbar.svelte:56` |
| UX-60 | H | **Verdikt LocalOnly stojí na argumente, ktorý nerozlišuje hub od standalone.** `commands/mutate.rs:4-9`: „a remote client staging or committing under it would race whatever it is doing … They run over this machine's SSH connection, which a hub-client desktop has no reason to have.“ Prvá polovica platí pre standalone desktop rovnako (aj on staguje pod bežiacim agentom cez SSH); druhá je pravdivá — a presne preto má zápis bežať **na hube**, ktorý SSH má. `repo_mutate.rs:7-8` sám hovorí „reachable from the MCP layer“. Pravidlo *parity or refusal* (`routing.rs:38-60`) je tu splnené iba formálne: hub schopnosť má, tool chýba. | `commands/mutate.rs:1-9`, `repo_mutate.rs:1-8`, `repo.rs:75-115` |
| UX-61 | M | **`FilesPanel` nemá vlastný test; hub správanie je pinnuté na sub-komponentoch.** Existujú `files.test.ts`, `files_view.test.ts`, `FileViewer.test.ts`; `FilesPanel.test.ts` **neexistuje** — `runAction`, tri dialógy, `applyFailure` (`E_NO_WORKTREE` → `worktreeGone`), reset pri zmene session (`:78-94`) a `openPathRequest` (`:99-109`) sú netestované. `hub_disabled.test.ts:252-345` testuje `FileList`/`RemoteToolbar`/`BranchList` s `writeBlocked: 'no git-write tool'` — nie `FilesPanel`, ktorý dôvod počíta. | `ls src/lib \| grep -i file`, `hub_disabled.test.ts:256-345` |
| UX-62 | M | **Potvrdenia sú nekonzistentné.** *Checkout commit* a *Delete branch* majú `ConfirmDialog` (`danger`); *Checkout branch* (`confirmCheckout`, `FilesPanel:202-204`) beží **okamžite**, hoci mení vetvu agenta rovnako („The agent's branch will change“ hovorí iba dialóg pre commit); *Pull* a *Push* bežia okamžite bez potvrdenia a bez náhľadu (ktorá vetva, kam, koľko commitov — `Branch.ahead` je pritom v `BranchList` k dispozícii). Po routingu bude Push z hubu **vzdialená operácia s credentials hubovho hosta** — potvrdenie s náhľadom je minimum. | `FilesPanel.svelte:202-238`, `RemoteToolbar.svelte:14-45` |
| UX-63 | M | **Sieťové git operácie bežia pod 10 s timeoutom.** `REPO_TIMEOUT_SECS = 10` (`repo.rs:21`) platí pre `git status` aj pre `fetch --all --prune`, `pull`, `push` — pomalý push skončí `E_SSH` timeoutom, kým git na hoste dobehne; UI ukáže červený `!`. `ssh::run_shell_bounded` (`ssh.rs:753-771`) existuje presne pre skripty, ktoré „legitimately outlive a reasonable connect budget“. Platí aj standalone; na hube k tomu pribúda `Deadline` — preto Lifecycle pre trojicu. | `repo.rs:21,75-87`, `repo_mutate.rs:191-231`, `support.rs:938-948` |
| UX-64 | L | **Jediné skutočné riziko z `mutate.rs` — commit pod agentom uprostred editu — nemá UI signál v žiadnom režime.** `SessionRow.claude_status` (`sessions.ts:42`, `working \| idle \| …`) je vo frontende, ale commit box ho nečíta. Refused verdikt to nerieši (standalone commituje bez varovania); routing to nezhorší. Riešenie je UX: hint pri Commit/Checkout, keď `claude_status === 'working'`, nie backendové odmietnutie (to by zmenilo aj standalone). | `sessions.ts:6-42`, `FileList.svelte:190-204` |
| UX-65 | L | **`repo_delete_branch.force` je na drôte, ale UI ho nikdy nepoužije.** `doDeleteBranch` posiela `false` (`FilesPanel:237`), `repoDeleteBranch(…, force = false)` (`history.ts:107-115`); service robí `-D` pri `true` (`repo_mutate.rs:113`). Hub tool by `-D` sprístupnil každému `full` klientovi zadarmo. | `FilesPanel.svelte:235-238`, `repo_mutate.rs:101-118` |
| UX-66 | L | **Chyby Fetch/Pull/Push sú červený `!` s textom v `title`.** `RemoteToolbar.svelte:46`: `<span class="err" title={err}>!</span>`. Po routingu sem prídu `E_FORBIDDEN` (readonly klient — UX-45 z iterácie 03: desktop nevie vopred, že je readonly), `E_HUB_UNREACHABLE`, `E_SSH` timeout — všetko ako `!`. Porovnávať **kód**, ukázať jeden riadok pod toolbárom. | `RemoteToolbar.svelte:14-21,46` |
| UX-67 | L | **`REASONS.repo_write` je syntetický kľúč mimo slovníka príkazov.** `hub_verdicts.test.ts:75-80` ho vedie v `REASONS_KEYS_THAT_ARE_NOT_COMMANDS` a `gatedByFilesPanel` (`:169-180`) allowlistuje desať príkazov. Po routingu **musí zmiznúť** kľúč, allowlist aj komentár `:65-67`, inak test „every other REASONS key that is a command name is local_only“ neplatí obrátene (allowlist „every allowlisted command really is local_only today“ padne). Poznámka pre recept krok 10. | `hub.ts:191-192`, `hub_verdicts.test.ts:65-80,167-180` |

## Návrh hub tools

Zásady z iterácie 03: (1) meno toolu = meno príkazu; (2) tool volá **tú istú** `repo_mutate::*`
funkciu ako Tauri príkaz, bez zmeny service vrstvy; (3) popis = jedna klauzula, chyby v popise len
kódom; (4) `confirm: false` — hub nemá approvera (`serve.rs:738-749`), `confirm: true` = tool na
hube nepoužiteľný pri zapnutom `mcp.confirm_destructive`; (5) `Deadline::Quick` (60 s cap) pre
lokálne git operácie, **`Lifecycle`** (300 s) pre fetch/pull/push spolu s opravou UX-63
(`run_shell_bounded`, wall clock 60–120 s).

### Params — kde vzniknú

Args structy v `repo_mutate.rs` (`CheckoutArgs`, `CheckoutCommitArgs`, `CreateBranchArgs`,
`DeleteBranchArgs`, `StageArgs`, `CommitCreateArgs`, `PushArgs`) majú dnes iba `#[derive(Deserialize)]`.
Vzor `SessionIdArgs`/`RepoFileArgs` (`repo.rs:39-44`, `repo_read.rs:417-419`): pridať
`Serialize` (memory: hub-routed args idú na drôt celé) a `rmcp::schemars::JsonSchema` s
`#[schemars(crate = "rmcp::schemars", rename = "<X>Params")]` a **`///` na každom poli** (test
`every_tool_parameter_is_documented`). Tak `remote.rs` posiela `&args` pole za poľom a
`tests_routing.rs` Case používa ten istý struct. Alternatíva (samostatné `*Params` v `params.rs` +
mapovanie) pridáva kód bez prínosu — struct-level `//` komentár namiesto `///` je jediná pasca
(`repo.rs:35-37`).

### Variant A — 10 toolov 1:1 (odporúčaný)

Súbor `crates/fleet-core/src/mcp/tools/repo.rs`, `repo_router`, za `repo_commit_diff`.

| Tool | Popis (návrh, jedna klauzula) | Params (`///`) | Výsledok | Odhad B |
|---|---|---|---|---|
| `repo_stage` | „Stage worktree paths (git add --) in a session's worktree; paths are repo-relative.“ | `session_id` /// Fleet session id. · `paths` /// Repo-relative paths to stage. | `"staged"` text (`Ok(())` ↔ `null` na Tauri strane — `remote.rs` deserializuje `()` z textového výsledku ako `route_text` → **použiť `route_text` a zahodiť**, alebo tool vráti `ok_json(&serde_json::Value::Null)`; zvoliť druhé, aby `route::<()>` fungoval jednotne) | ~470 |
| `repo_unstage` | „Unstage worktree paths (git restore --staged --).“ | ako `repo_stage` | `null` | ~470 |
| `repo_commit_create` | „Commit the staged changes with `message`; `amend` rewrites HEAD. Errors: E_INVALID (empty message), E_REPO.“ | `session_id` · `message` /// Commit message; must not be empty. · `amend` /// Rewrite HEAD instead of adding a commit. | `null` | ~615 |
| `repo_create_branch` | „Create a branch from HEAD or `start_point` (branch or hash), optionally checking it out.“ | `session_id` · `name` /// New branch name. · `start_point` /// Branch name or commit hash to start from; HEAD when omitted. · `checkout` /// Check the new branch out. | `null` | ~770 |
| `repo_delete_branch` | „Delete a local branch (git branch -d); `force` (-D) is the master token's alone.“ | `session_id` · `name` · `force` /// -D instead of -d; refused E_FORBIDDEN to a client. | `null`; v tele: `if p.force && !caller.is_master() → E_FORBIDDEN` | ~600 |
| `repo_checkout` | „Check out a branch; refuses E_DIRTY while the worktree has uncommitted changes.“ | `session_id` · `branch` /// Local or remote branch name. | `null` | ~465 |
| `repo_checkout_commit` | „Check out a commit as a detached HEAD; same E_DIRTY guard.“ | `session_id` · `hash` /// Commit hash, 4–40 lowercase hex. | `null` | ~470 |
| `repo_fetch` | „git fetch --all --prune in a session's worktree.“ | `session_id` | `null` | ~285 |
| `repo_pull` | „git pull --ff-only; refuses rather than merging.“ | `session_id` | `null` | ~295 |
| `repo_push` | „git push the current branch; `set_upstream` adds -u origin. Never --force. Needs a trusted client or the master.“ | `session_id` · `set_upstream` /// Also set origin/<branch> as upstream. | `null`; v tele: `if !(caller.is_master() \|\| caller.is_trusted_client()) → E_FORBIDDEN` (otázka 1) | ~460 |

Policy riadky (`guard.rs`, za `repo_commit_diff` `:599-604`): všetkých desať
`access: Access::Client, readonly: false, confirm: false`; `deadline: Quick` okrem
`repo_fetch`/`repo_pull`/`repo_push` `Lifecycle`. Každý tool volá `audit("<tool>", "session_id=…
<identifikátory>")` — **nikdy `message`** (`redact_args` ho aj tak skráti na dĺžku, `guard.rs:1236`).

`Extension(caller): Extension<Caller>` treba iba v `repo_delete_branch` a `repo_push`
(vzor `delete_worktree`, `repo.rs` mcp `:128-133`).

### Variant B — 6 toolov (rozpočtová alternatíva)

| Tool | Zlučuje príkazy | Diskriminátor | Úspora |
|---|---|---|---|
| `repo_checkout { session_id, target }` | `repo_checkout`, `repo_checkout_commit` | `target` validovaný ako hash **alebo** ref (vzor `start_point`, `repo_mutate.rs:70-74`); service rozlíši volaním správnej funkcie | ~470 |
| `repo_stage { session_id, paths, staged }` | `repo_stage`, `repo_unstage` | `staged: bool` (UI ho už má: `stageToggle(path, staged)`) | ~345 |
| `repo_remote { session_id, op, set_upstream }` | `repo_fetch`, `repo_pull`, `repo_push` | `op: "fetch" \| "pull" \| "push"` | ~580 |
| `repo_commit_create`, `repo_create_branch`, `repo_delete_branch` | 1:1 | — | — |

Netto ≈ **+3 500 B**. `remote.rs` posiela `json!` literál s diskriminátorom (rozdiel zapísať pri
volaní — recept krok 4); `verdicts.rs` má `Routed { tool: "repo_checkout" }` pri dvoch príkazoch
(precedens `catalog_list_assets → list_assets`, `health_check → fleet_health`). Cena: päť príkazov
s iným menom než tool, `docs/control-api.md` opisuje tri tools s `op`.

### Čo sa mení vo `VERDICTS` (recept krok 3)

| Príkaz | Dnes | Po PR-5a |
|---|---|---|
| desať `repo_*` zápisov | `LocalOnly { instead: NO_GIT_WRITE_TOOL }` | `Routed { tool: "<meno príkazu>" }` (A) / podľa tabuľky B |
| `NO_GIT_WRITE_TOOL` (`verdicts.rs:95-97`) | konštanta | **zmazať** (žiadny riadok ju nepoužíva); komentár sekcie `:407` prepísať na „the Files tab's git writes — each has a tool of the same name“ |
| `local_only.golden.json` | 10 záznamov | `REGEN_LOCAL_ONLY=1` — 10 záznamov ubudne (`grep -c repo_` dnes 11, jedenásty je `catalog_repo_status`) |

### Rozpočet popisov (`the_served_definition_budget_stays_bounded`, `tests.rs:2325-2400`)

| Položka | Odhad B |
|---|---|
| východisko (meranie v `d417be9f`) | 57 603 z 57 700 |
| + iterácia 03 (`get_fleet_settings`, `set_fleet_setting`) | +650 → 58 253 |
| + iterácia 04 (`catalog_config`, `catalog_get_asset`, `last_sync`, −4 vety) | +450 → 58 703 |
| **+ iterácia 05 Variant A** (10 toolov, tabuľka vyššie) | **+4 900 → ~63 600** |
| + iterácia 05 Variant B (6 toolov) | +3 500 → ~62 200 |

Odporúčanie: **`BUDGET_BYTES` 63 800** (A) resp. 62 400 (B), s odsekom v doc-komentári podľa
vzoru (`tests.rs:2331-2356`: čo pribudlo, koľko meralo pred, prečo sa nedalo trimovať —
tu: každá schéma nesie `session_id` s popisom, ktorý `every_tool_parameter_is_documented`
vyžaduje; degenerovaná verzia s jednoznakovými popismi by stále merala ~3 800 B). Ak PR-3a/4a už
konštantu zdvihli, zdvihnúť **z ich hodnoty** o +4 900/+3 500. Podmienka `ro_bytes < bytes / 2`
sa **zlepší** (pribúdajú iba mutujúce bajty; readonly plocha ostáva).

Poznámka mimo receptu: `present::visible_to` (`present.rs:63-68`) pozná iba osi
readonly/master — neexistuje „iba pre paired klientov“. Desať write toolov teda uvidí aj agent
v session cez per-host token, hoci **on** má git v shelli a tools nepotrebuje. Tier
`Visibility::ClientOnly` (tools, ktoré master vidí, per-host token nie) by agentovu plochu
nechal plochú a rozpočtový test by ju mohol merať zvlášť — návrh pre konsolidáciu, nie pre tento
PR (otázka 3).

### Starší hub a readonly klient

- Hub bez write toolov: `E_FORBIDDEN` (gate zlyhá „closed“ na neznámom mene) alebo
  `E_HUB_PROTOCOL` → **jeden riadok** `files-hub-unsupported` pod hlavičkou panelu: „This hub
  cannot write to worktrees yet — update the hub.“; ovládače disabled; bez toastu; porovnávať
  **kód** (vzor `NewSessionDialog.svelte:180-205`).
- `readonly` klient: prvý zápis vráti `E_FORBIDDEN` (`enforce_mode`) → riadok
  `files-forbidden` „This client is readonly on the hub — writes are refused.“ a **zapamätať si
  to** v `FilesPanel` (`writeRefused = $state<string|null>`) — ďalšie ovládače disabled s tým
  istým textom, kým sa session/hub nezmení. UX-45 (desktop nevie mód vopred) platí naďalej —
  otázka 3 iterácie 03 (persistovať `client_mode`) by tento stav dala vopred.
- Netrusted klient a `repo_push` (ak otázka 1 = trusted): `E_FORBIDDEN` s textom
  „push from a paired client needs the operator's trust (fleet-hub client trust <name>)“ →
  Push disabled + riadok; ostatné zápisy fungujú.

## Bezpečnostné mantinely (SEC-5, blast radius `full` klienta)

Východisko: `hub.md:583-585` definuje `full` ako „whole-fleet *session* control“; zápis do
worktree session je session control (agent tam robí to isté). Push z hubu ale používa
**credentials hubovho hosta** voči vzdialenému git serveru — to je rozšírenie dosahu klienta na
systém mimo fleetu. Odporúčané mantinely, zoradené podľa toho, čo už existuje:

| # | Mantinel | Stav | Kde |
|---|---|---|---|
| 1 | **Žiadny force push, nikdy.** `PushArgs` nemá `force`; skript je `git push` / `push -u origin <b>` | **existuje** — držať; test `repo_push_never_carries_force` (grep na `--force`/`-f` v skripte) | `repo_mutate.rs:209-231` |
| 2 | **Pull iba fast-forward** (`--ff-only`) — klient nevie z hubu vyrobiť merge commit | **existuje** | `repo_mutate.rs:200-207` |
| 3 | **Checkout iba na čistom worktree** (`ensure_clean` → `E_DIRTY`), nikdy `--force`/`--discard` | **existuje** | `repo.rs:127-141` |
| 4 | **Validácia vstupov** pred quotovaním: `git_ref` (bez `..`, bez `-` na začiatku, bez whitespace/control), `commit_hash` (4–40 lower-hex), `repo_rel_path` (relatívna, ≤4096) — a `shell::quote` na každej hodnote (hardening review §shell) | **existuje**; hub tool ich dedí, lebo volá tú istú funkciu | `validate.rs:133-215`, `repo_mutate.rs` každé `quote(` |
| 5 | **Audit na časovej osi session.** `persist_audit` beží pre **každý** tool call pred `enforce_mode`, riadok `session_events` kind `mcp_call` „repo_push by client:mac-desktop: session_id=12 set_upstream=false“ (argumenty cez `redact_args`, `message` len ako dĺžka; `scrub_line` proti CR/LF v mene klienta); `session_history` tool a `Timeline.svelte` (kategória `ops`) ho ukážu. Zamietnuté volania sú na osi tiež („Audit first so refused calls are on the timeline too“) | **existuje** — bez práce; PR-5b môže v Timeline zvýrazniť `repo_push`/`repo_commit_create` ikonou git | `tools/mod.rs:151-155`, `support.rs:360-385`, `timeline.ts:49` |
| 6 | **`force` pri `delete_branch` iba master** | **nové** (UX-65): tool body odmietne `force && !caller.is_master()`; alebo pole vynechať zo schémy a UI ho nemať | `mcp/tools/repo.rs` |
| 7 | **Push iba trusted klient alebo master** | **nové, odporúčané** (otázka 1): `caller.is_trusted_client()` (`auth.rs:103-105`) je presne „keyboard that is yours“ — `hub.md:599-609` odporúča trustovať desktop, ktorý si spároval sám; netrusted telefón vidí Push disabled s dôvodom. Rozširuje význam trustu z „nemarkuj prompty“ na „smie hovoriť s git remote v mene hubu“ — rovnaká dilema ako otázka 1 iterácie 03. Bez toho: `full` = push (rovnako ako `full` = `kill_session` každej session) | `auth.rs:103-105`, `guard.rs:215-220` |
| 8 | **Bez `--amend` z klienta?** `amend` prepisuje HEAD; na pushnutej vetve vyžaduje force push, ktorý (1) neexistuje → škoda ohraničená lokálnym HEAD. UI ho neposiela. Nechať, ale audit ho zapíše (`amend=true` je identifikátor, `redact_args` ho ponechá) | ponechať | — |
| 9 | **Rate limit** — `RateLimiter` je dnes iba pre `broadcast_prompt`; push z klienta nie je fan-out. Netreba | — | `guard.rs` |
| 10 | **Confirm** — dnes `false` (hub nemá approvera). Slice 2 (paired-client confirmation channel): keď hub bude vedieť doručiť `E_CONFIRM_REQUIRED` klientovi cez `/events` a klient odpovedať routovaným `mcp_confirm`, `repo_push`, `repo_delete_branch` a `repo_checkout_commit` prejdú na `confirm: true` + voliteľné `confirm_nonce` (`#[serde(default)]`); `hubNextStep('E_CONFIRM_REQUIRED')` (`hub.ts:354-362`) už má vetu pre medzistav. Dnes: **in-app `ConfirmDialog`** (sekcia nižšie) | odložené na slice 2 | `docs/superpowers/specs/2026-09-20-ux-agent-fab-design.md:56,291-299` |

Nič z toho nevyžaduje zmenu `repo_mutate.rs` okrem UX-63 (`run_shell_bounded` pre trojicu).

## Návrh Files tabu v hub režime

Princíp: po PR-5a je Files tab v hub režime **totožný so standalone** — parita znamená, že
`writeBlocked` z `hubBlock('repo_write')` zmizne. Prvky nižšie riešia (a) stavy, ktoré ostávajú
iné (starší hub, readonly klient, netrusted push), (b) potvrdenia, ktoré po presune na hub
potrebuje **každý** režim (UX-62), (c) UX-26/UX-59 loading a prázdne stavy.

### Stavy panelu

| Stav | Kedy | Čo sa vykreslí (`data-testid`) |
|---|---|---|
| **Načítavam** | prvé `repo_changes`/`repo_tree`/`repo_log`/`repo_branches` letí | skeleton (nižšie), `aria-busy="true"` na `.rows` |
| **Worktree gone** | `E_NO_WORKTREE` | dnešný `worktree-gone` (`FilesPanel:305-312`) — bez zmeny |
| **Hub nevie zapisovať** (`files-hub-unsupported`) | `E_FORBIDDEN`/`E_HUB_PROTOCOL` na **prvom** zápise, hub režim | jeden riadok pod hlavičkou: „This hub cannot write to worktrees yet — update the hub.“; všetky write ovládače disabled s tým istým `title`; čítania fungujú |
| **Readonly klient** (`files-forbidden`) | `E_FORBIDDEN` s textom `readonly` na zápise | riadok „This client is readonly on the hub — writes are refused.“; ovládače disabled |
| **Push potrebuje trust** (`files-push-untrusted`) | `E_FORBIDDEN` iba z `repo_push` | riadok pod `RemoteToolbar`: „Pushing from a paired client needs the hub operator's trust (`fleet-hub client trust <name>`).“; iba Push disabled |
| **Odpojený hub** | `hubConnection.state !== 'connected'` | `hubActionBlocked` pre každý zo `ROUTED_ACTIONS` (desať `repo_*` zápisov pribudne do `ROUTED_ACTIONS`, `hub.ts:267-284`) → disabled + offline veta; čítania zlyhajú `E_HUB_UNREACHABLE` → dnešný `error` riadok |
| **Bežný** | inak | plný panel ako standalone |

Bez scope bannera v bežnom stave: po routingu nie je čo vysvetľovať (na rozdiel od Assets, kde
authoring ostáva na hube). `HubScopeNote` (komponent z iterácie 03/04) sa použije **iba** pre tri
odmietnuté stavy vyššie s `{ what: 'git' }`.

### Strips

| Strip | Hub režim po PR-5a | Poznámka |
|---|---|---|
| Changed | zoznam z `repo_changes` (dnes), staging checkbox → `repo_stage`/`repo_unstage` routed, commit box aktívny | agent-busy hint (nižšie) |
| All files | bez zmeny (read) | — |
| History | `CommitGraph` + „Create branch from here“ / „Checkout this commit“ routed; `RemoteToolbar` routed | „Load more“ bez zmeny |
| Branches | `BranchList` Checkout/Delete/+ New branch routed; `RemoteToolbar` routed | `ahead/behind` využiť v Push náhľade |

### Commit box (`FileList.svelte:190-204`)

- Textarea a tlačidlo ako dnes, **bez `writeBlocked`** v bežnom stave.
- Label tlačidla podľa stavu: `stagedCount === 0` → „Stage files to commit“ (disabled, nie
  „Commit 0 files“ — UX-59: dva dôvody jedného disabled); inak „Commit N files“.
- V odmietnutých stavoch **textarea zmizne** a na jej mieste je riadok stavu (`files-forbidden`
  / `files-hub-unsupported`) — nie disabled textarea s normálnym placeholderom.
- **Agent-busy hint (UX-64, oba režimy):** ak `session.claude_status === 'working'`, pod
  tlačidlom `files-agent-working` „The agent is working in this worktree — a commit now may
  capture a half-finished edit.“ Tlačidlo ostáva aktívne (agentov stav nie je zámok; standalone
  to dnes robí bez varovania). Rovnaký hint v `ConfirmDialog` pre checkout.
- `E_INVALID` (prázdna správa) sa nestane — tlačidlo je disabled pri prázdnom `commitMsg`.

### Tok potvrdení (UX-62) — jedna tabuľka pre oba režimy

| Akcia | Dnes | Po PR-5b | Text dialógu |
|---|---|---|---|
| Stage / Unstage | bez potvrdenia | bez potvrdenia (vratné) | — |
| Commit | bez potvrdenia | bez potvrdenia; agent-busy hint | — |
| New branch | `PromptDialog` | bez zmeny | — |
| Checkout branch | **okamžite** | `ConfirmDialog` (nie `danger`): „Check out `<name>`? The agent's branch will change. Refused if the worktree has uncommitted changes.“ | zjednotiť s checkout-commit |
| Checkout commit | `ConfirmDialog danger` | bez zmeny | — |
| Delete branch | `ConfirmDialog danger` | bez zmeny; `force` nikdy | — |
| Fetch | okamžite | okamžite (neškodné) | — |
| Pull | okamžite | `ConfirmDialog`: „Pull `<upstream>` into `<branch>` (fast-forward only, ↓N behind)?“ — `behind` z `repo_branches` | — |
| Push | okamžite | **`ConfirmDialog danger`**: „Push `<branch>` → `origin/<branch>` (↑N ahead)? In hub mode this runs on `<host>` with its git credentials and is recorded on the session timeline as `client:<name>`.“; `set_upstream` checkbox v dialógu, keď `upstream === null` (dnes UI posiela vždy `false` → push bez upstreamu zlyhá `E_REPO` „no upstream“) | hub veta iba pri `$hubStatus.remote` |

Toto **je** dnešná „confirmation“ pre paired klienta — na desktopu, pred volaním. Slice 2 FAB
specu ju môže presunúť za hub (`confirm: true` + `E_CONFIRM_REQUIRED` doručený klientovi); dialógy
z PR-5b sa vtedy stanú odpoveďou na `mcp:confirm-required` namiesto pre-emptívneho kroku — layout
a texty ostávajú.

### Loading skeleton (UX-26, UX-67)

Nový komponent `src/lib/Skeleton.svelte` (`{ rows = 6, variant: 'list' | 'graph' | 'viewer' }`):
`rows` pruhov výšky riadku (`.row` má ~1.6 rem), šírky 55–85 % striedavo, `background:
color-mix(in srgb, var(--fg-muted) 12%, transparent)`, `@keyframes shimmer` s
`prefers-reduced-motion: reduce` → bez animácie; `aria-hidden="true"`, rodič `aria-busy`. Nahradí
štyri „Loading…“ (`FilesPanel:321`, `FileList:128`, `BranchList:33`, `FileViewer:188`). Ukázať až
po **150 ms** (`setTimeout`), aby rýchle standalone odpovede nepreblikli; v hub režime (dva skoky)
sa ukáže takmer vždy. `CommitGraph` variant: 5 pruhov s bodkou vľavo (lane).

### Prázdne stavy (UX-26)

| Miesto | Dnes | Návrh |
|---|---|---|
| `FileViewer` bez výberu (`:166`) | „Select a file to view it.“ | Changed s N>0: „Pick a changed file on the left to see its diff.“; Changed s N=0: „Nothing changed in this worktree yet. See **All files** or **History**.“ (odkazy volajú `onMode`); All files: „Pick a file to read it.“ |
| `FileList` Changed prázdny (`:133`) | „No changes.“ | „Clean worktree — nothing to stage.“ + agentov posledný prompt, ak `session.last_prompt` (kontext, prečo nič nie je) |
| `BranchList` bez remote | (nič) | riadok „No remotes — Fetch/Push need `origin`.“ ak `remotes.length === 0` a `branches.length > 0` |
| `CommitGraph` prázdny | (nič) | „No commits yet.“ |

### Ikony

`↻` Refresh (`FilesPanel:301`) → `<Icon name="refresh-cw">` podľa iterácie 01; `!` chyba v
`RemoteToolbar:46` → `<Icon name="alert-circle">` + viditeľný riadok (UX-66).

## PR plán (podľa receptu z iterácie 03)

Poradie: **PR-5a → PR-5b.** Žiadny predpoklad typu PR-4c — hub SSH už má.

### PR-5a — Rust: 10 toolov + routing + kontrakt + minimálny TS (**M → odštep 5a1/5a2**)

| # | Krok | Súbor | Zmena |
|---|---|---|---|
| 1 | 2a | `crates/fleet-core/src/service/repo_mutate.rs:19-23,39-43,58-64,101-106,120-124,167-172,209-213` | `Serialize + JsonSchema` + `#[schemars(rename)]` + `///` na každom poli siedmich args structov (`SessionIdArgs` už má) |
| 2 | — | `repo_mutate.rs:191-231`, `repo.rs` | UX-63: `run_git_bounded` (alebo param) cez `ssh::run_shell_bounded(connect 10 s, wall 90 s)` pre fetch/pull/push |
| 3 | 2b | `crates/fleet-core/src/mcp/tools/repo.rs` | 10 `#[tool]` metód (Variant A) volajúcich `repo_mutate::*`; `Extension(caller)` v `repo_delete_branch` (force → master) a `repo_push` (trusted \| master); `audit(...)` bez `message` |
| 4 | 2c | `crates/fleet-core/src/mcp/guard.rs:599-604+` | 10 riadkov `TOOL_POLICIES` (Client/false/false; Quick ×7, Lifecycle ×3) |
| 5 | 2d | `crates/fleet-core/src/mcp/tools/tests.rs:2357` | `BUDGET_BYTES` → 63 800 (A) + odsek; `a_readonly_client_is_refused_mutating_tools_but_allowed_reads` +10 odmietnutých pre readonly; nové testy (nižšie) |
| 6 | 2e | `docs/control-api-reference.md`, `docs/control-api.md:275-277` | regen (2×); index „Worktree files & git“ → odstrániť „(read-only)“, +10 mien s vetou o mantineloch |
| 7 | 3 | `src-tauri/src/backend/verdicts.rs:95-97,407-468` | 10× `Routed`; zmazať `NO_GIT_WRITE_TOOL`; komentár sekcie |
| 8 | 4 | `src-tauri/src/backend/remote.rs` | 10 metód `route("<cmd>", &args)` — args struct celý (pole za poľom) |
| 9 | 5 | `src-tauri/src/commands/mutate.rs` | `pub(crate) mod routed` s 10 funkciami (vzor `commands/files.rs:66-114`); telá bez `refuse_local_only`; hlavičkový komentár `:4-9` prepísať (UX-60) |
| 10 | 6 | `src-tauri/src/backend/tests_routing.rs:695+` | 10 Case do `routed_mutation_cases` s **nedefaultnými** hodnotami (`json!({"session_id":7,"paths":["a/b.rs"]})`, `{"session_id":7,"message":"m","amend":true}`, `{"session_id":7,"name":"f/x","start_point":"abc123","checkout":true}`, `{"session_id":7,"name":"f/x","force":true}`, `{"session_id":7,"branch":"main"}`, `{"session_id":7,"hash":"abc123"}`, `{"session_id":7}` ×3, `{"session_id":7,"set_upstream":true}`); payload `null` |
| 11 | 7 | `src-tauri/src/backend/local_only.golden.json` | `REGEN_LOCAL_ONLY=1` — 10 záznamov ubudne; prečítať diff |
| 12 | 8 | `src/lib/hub_verdicts.generated.json`, `docs/hub.md:1045,1103-1112` | `REGEN_HUB_VERDICTS=1` (2×) — 10 riadkov tabuľky zmizne, počty 60/49 |
| 13 | 9 | `src-tauri/src/backend/tests_contract.rs` | **nič** — výsledok `()`/`null`, args idú von; kontrakt pokrýva pomenované návratové structy (iterácia 03, krok 9) |
| 14 | 10 | `src/lib/hub.ts:191-192,267-284` | zmazať `REASONS.repo_write`; `ROUTED_ACTIONS` +10 `repo_*` zápisov |
| 15 | 10 | `src/lib/hub_verdicts.test.ts:65-80,167-180` | z `REASONS_KEYS_THAT_ARE_NOT_COMMANDS` von `repo_write` (+ komentár „the five“ → „the three“, `:91`); zmazať `gatedByFilesPanel` |
| 16 | 11 | `src/lib/FilesPanel.svelte:28-33` | `writeBlocked = $derived(hubActionBlocked('repo_commit_create', $hubStatus, $hubConnection))` — jeden kľúč za všetky (offline veta), plus `writeRefused` z prvého `E_FORBIDDEN`/`E_HUB_PROTOCOL` (kód, nie text) → `files-hub-unsupported`/`files-forbidden` riadok (minimum; IA v PR-5b) |
| 17 | 12 | `src/lib/hub_disabled.test.ts:252-345` | prepísať: `FileList`/`RemoteToolbar`/`BranchList` s `writeBlocked: null` v remote; nový `FilesPanel.hub.test.ts` (nižšie) |
| 18 | 13 | `docs/hub.md` *What is different from standalone* (`:985-1005`), `CLAUDE.md:110` | bullet „Git writes from the Files tab run on the hub's host over its SSH and land on the session timeline as `client:<name>`; push needs a trusted client“; 123 → generovaný počet (UX-48) |

Odštep: **PR-5a1** = položky 1–6 (fleet-core, zelené samostatne, ~260 riadkov), **PR-5a2** =
7–18 (src-tauri + TS kontrakt, ~200 riadkov + testy).

### PR-5b — Svelte: potvrdenia, stavy, skeleton, empty CTA (**M**)

| # | Súbor | Zmena |
|---|---|---|
| 1 | `src/lib/Skeleton.svelte` (nový) | tri varianty, shimmer, reduced-motion, 150 ms oneskorenie cez prop `delay` |
| 2 | `src/lib/FilesPanel.svelte` | `ConfirmDialog` pre checkout branch, Pull, Push (`FilesDialog` union +3 druhy; Push dialóg s `set_upstream` checkboxom keď `upstream === null`); stavy `files-hub-unsupported`/`files-forbidden`/`files-push-untrusted`; `HubScopeNote { what: 'git' }` len v nich; `Skeleton` namiesto `Loading…` `:321` |
| 3 | `src/lib/FileList.svelte` | label „Stage files to commit“; textarea → stavový riadok v odmietnutých stavoch (prop `writeRefused: string \| null`); `files-agent-working` hint (prop `agentWorking`); `Skeleton` `:128`; empty „Clean worktree…“ |
| 4 | `src/lib/FileViewer.svelte` | empty CTA podľa `mode` a `changesCount` (nové props); `Skeleton variant="viewer"` `:188` |
| 5 | `src/lib/BranchList.svelte` | `Skeleton` `:33`; „No remotes“ riadok; `ahead/behind` exportované pre Push/Pull dialóg (cez `branches` v `FilesPanel`, už k dispozícii) |
| 6 | `src/lib/RemoteToolbar.svelte` | `!` → viditeľný riadok pod toolbárom s kódom → textom (`E_FORBIDDEN` readonly / untrusted, `E_HUB_UNREACHABLE`, `E_SSH` timeout „still running on the host — refresh in a moment“); Pull/Push volajú `onconfirm` callback namiesto priameho `run` |
| 7 | `src/lib/CommitGraph.svelte` | `Skeleton variant="graph"`; „No commits yet.“ |
| 8 | `src/lib/Timeline.svelte` / `timeline.ts` | `mcp_call` s `repo_push`/`repo_commit_create`/`repo_checkout*` → ikona git (Lucide `git-commit`, `git-branch`, `upload`) — voliteľné |
| 9 | testy | `FilesPanel.test.ts` (nový, standalone), `FilesPanel.hub.test.ts`, `FileList.test.ts` (nový), `RemoteToolbar.test.ts` (nový), `Skeleton.test.ts` — nižšie |

Ak PR-5b presiahne ~300 riadkov (pravdepodobné s testami): **PR-5b1** = potvrdenia + stavy
(položky 2, 3-časť, 6), **PR-5b2** = skeleton + empty CTA (1, 3-časť, 4, 5, 7).

## Akceptačné testy

### Rust — `crates/fleet-core/src/mcp/tools/tests.rs`

- `every_router_tool_has_exactly_one_tool_policy_row`, `every_tool_parameter_is_documented`,
  `annotations_follow_the_policy_table` (žiadny z desiatich nemá `readOnlyHint` ani
  `destructiveHint`) zelené.
- `the_served_definition_budget_stays_bounded`: nová konštanta; vypísať `master/host full/host
  readonly/client full` čísla do PR popisu; `ro_bytes < bytes / 2` platí.
- `a_readonly_client_is_refused_mutating_tools_but_allowed_reads`: všetkých 10 medzi
  odmietnutými pre `readonly`; `repo_changes` ostáva povolené.
- Nový `repo_push_is_refused_to_an_untrusted_client_and_allowed_to_a_trusted_one`:
  `client_caller("phone", Full)` → `E_FORBIDDEN` s textom „trust“; `trusted` klient a
  `Caller::master()` prejdú gateom (FakeSsh zachytí `git -C "$root" push`).
- Nový `repo_delete_branch_force_is_the_masters_alone`: `force: true` od klienta → `E_FORBIDDEN`;
  `force: false` → skript obsahuje `branch -d 'x'`; master `force: true` → `-D`.
- Nový `repo_write_tools_quote_every_argument`: mená vetiev/cesty s `'`, `$(`, medzerou → skript
  obsahuje `quote(...)` tvar a **nikdy** surovú hodnotu (vzor `repo_script_embeds_quoted_name_and_body`,
  `repo.rs:147-153`).
- Nový `repo_push_script_never_carries_force`: pre `set_upstream` oba tvary → `!script.contains("--force") && !script.contains(" -f ")`.
- Nový `repo_write_audit_row_names_the_client_and_hides_the_message`: `persist_audit` pre
  `repo_commit_create` s `message: "secret words"` → riadok obsahuje `repo_commit_create by
  client:phone`, `session_id=7`, `message=<12 chars>`, nie „secret“.
- `repo_mutate.rs` unit: `fetch/pull/push` používajú `run_shell_bounded` s wall ≥ 60 s (UX-63).

### Rust — `src-tauri/src/backend/`

- `tests_routing.rs`: `every_routed_row_is_driven_by_a_case` (10 Case),
  `every_commands_body_does_what_its_row_says` (telá obsahujú `routed::`),
  `every_routed_tool_is_a_tool_the_hub_serves`, `every_local_only_message_is_the_one_the_fixture_records`
  po regen; `every_refusal_names_a_command_the_table_can_refuse` — `NO_GIT_WRITE_TOOL` zmazaný,
  žiadny `refuse_local_only("repo_*")` neostal.
- Nový `a_routed_git_write_never_touches_this_machines_ssh`: fake hub prijme `repo_stage`,
  lokálny `FakeSsh` **nedostane** žiadne volanie (parita = hub SSH, nie desktop).
- `tests_verdict_gen.rs`: `generated_json_is_current`, `doc_table_is_current` po regen.

### Frontend — Vitest

`npx vitest run src/lib/hub_disabled.test.ts src/lib/hub_verdicts.test.ts src/lib/FilesPanel.test.ts src/lib/FilesPanel.hub.test.ts src/lib/FileList.test.ts src/lib/RemoteToolbar.test.ts src/lib/FileViewer.test.ts src/App.hub.test.ts`

`hub_verdicts.test.ts`: zelený po regen + položka 15 PR-5a (`repo_write` už nie je kľúč;
`gatedByFilesPanel` zmazaný; „every local_only command is covered“ prejde, lebo 10 príkazov už nie
je `local_only`).

`hub_disabled.test.ts:252-345` (prepísané): „the git-write panel on a hub client is enabled“ —
`FileList`/`RemoteToolbar`/`BranchList` s `writeBlocked: null` majú checkbox, Commit, Fetch/Pull/Push,
Checkout/Delete/+ New branch **enabled**; standalone vetva bez zmeny.

`FilesPanel.hub.test.ts` (nový, `hubStatus = remote`, `hubConnection = connected`):

- „stages through the hub and reloads changes“ — klik na `.stage` → `invoke('repo_stage',
  {args:{session_id, paths:['src/lib.rs']}})`, potom `repo_changes` znovu.
- „commits through the hub“ — textarea + klik → `invoke('repo_commit_create', {args:{session_id,
  message:'m', amend:false}})`; `historyLoaded` reset → ďalší vstup do History volá `repo_log`.
- „an older hub answers E_FORBIDDEN on the first write and the panel says so in one line, without
  a toast“ — `repo_stage` → `{code:'E_FORBIDDEN', message:'unknown tool'}` →
  `files-hub-unsupported` obsahuje „update the hub“; `pushError` nezavolaný; Commit disabled s tým
  istým `title`.
- „a readonly client's write is refused inline and remembered“ — `E_FORBIDDEN` s „readonly“ →
  `files-forbidden`; druhý klik na checkbox **nevolá** `repo_stage` znovu.
- „an untrusted client's push is refused inline; stage still works“ — `repo_push` →
  `E_FORBIDDEN` „trust“ → `files-push-untrusted`; Push disabled; checkbox enabled.
- „writes are disabled while the hub is offline, with the offline sentence“ — `hubConnection =
  {state:'offline', attempt:2}` → Commit `disabled`, `title` obsahuje „unreachable“;
  `repo_changes` chyba `E_HUB_UNREACHABLE` v `error` riadku.
- „Push asks first and names the hub host“ — klik Push → `ConfirmDialog` s textom obsahujúcim
  `origin/` a `client:`; potvrdenie → `invoke('repo_push', …)`; zrušenie → žiadne volanie.

`FilesPanel.test.ts` (nový, standalone):

- „checkout branch asks first“ — klik Checkout → `confirm-checkout-branch`; potvrdiť →
  `invoke('repo_checkout', {args:{session_id, branch:'feature/x'}})`.
- „Pull asks with the behind count“; „Push with no upstream offers set_upstream“ (`upstream: null`
  → checkbox → `set_upstream: true`).
- „E_NO_WORKTREE from a write switches to worktree-gone“ (existujúca logika `applyFailure`,
  netestovaná — UX-61).
- „shows the agent-working hint when claude_status is working“ — `files-agent-working` existuje;
  Commit **nie** je disabled.
- „switching sessions resets mode, selection and history“ (`:78-94`).

`FileList.test.ts` (nový): „Commit button reads *Stage files to commit* with nothing staged and
*Commit 2 files* with two“; „a refused state replaces the textarea with the reason“; „skeleton
shows while loading and the list has aria-busy“.

`RemoteToolbar.test.ts` (nový): „errors are a visible line, keyed by code“ — `E_SSH` → „still
running on the host“; `E_FORBIDDEN` → „readonly“/„trust“ podľa správy.

`FileViewer.test.ts` (rozšíriť): „empty state names the mode: diff hint in Changed, read hint in
All files, All files/History links when nothing changed“.

`App.hub.test.ts` „the commands the UI calls unprompted“: Files tab v hub režime pri mounte volá
**iba** `repo_changes` (žiadny zápis, žiadny `E_LOCAL_ONLY`).

### Manuálne (screenshot podľa README §1)

Hub režim, tab Files, session na `claude-fleet-trn`: staging checkbox funguje, „Commit 1 file“
commitne; `docker exec fleet-hub fleet-hub client …` / `session_history` ukáže `mcp_call
repo_commit_create by client:mac-desktop`; Push otvorí dialóg s `origin/<branch>` a
`client:mac-desktop`; po `fleet-hub client untrust mac-desktop` Push zobrazí `files-push-untrusted`
a checkbox ostáva funkčný. Screenshot Changed strip so skeletonom (throttling: `hubConnection`
reconnecting) a s „Stage files to commit“.

## Odhad

| Časť | Veľkosť | Diff |
|---|---|---|
| PR-5a1 položky 1–6 (derives + `///`, `run_shell_bounded`, 10 toolov, policy, budget, reference) | **M** | ~260 riadkov + generované; z toho ~120 sú `#[tool]` bloky (10 × 12) |
| PR-5a2 položky 7–13 (verdikty, remote ×10, `mod routed` ×10, 10 Case, goldeny) | **M** | ~200 riadkov + testy ~120 + generované |
| PR-5a2 položky 14–18 (TS kontrakt, `FilesPanel` minimum, `hub_disabled` prepis, docs) | S | ~40 riadkov + testy ~60 |
| **PR-5a spolu** | **L → odštep 5a1/5a2** | ≈500 riadkov bez testov/generovaných; Variant B ušetrí ~4 tools ≈ −90 riadkov, nie o triedu |
| PR-5b1 (potvrdenia ×3, stavy, RemoteToolbar chyby) | M | ~180 riadkov |
| PR-5b2 (Skeleton, empty CTA, labely, agent hint) | S/M | ~150 riadkov |
| PR-5b testy (5 súborov) | M | ~300 riadkov |
| **PR-5b spolu** | **M → odštep 5b1/5b2** | ≈330 + testy |

Mimo rozsahu, zapísané pre konsolidáciu: `Visibility::ClientOnly` tier pre tools, ktoré agent
nepotrebuje (otázka 3); persistovať `client_mode` pri párovaní (UX-45, iterácia 03 Q3); slice 2
confirmation channel → `confirm: true` pre `repo_push`/`repo_delete_branch`; `repo_stash` /
`repo_discard` (dnes neexistujú ani standalone — správne, discard je agentova vec); Timeline ikony
pre git `mcp_call` riadky.

## Otázky pre vlastníka

1. **`repo_push`: Client `full`, alebo `full` + trusted (`caller.is_trusted_client()`)?**
   Odporúčam **trusted**: push používa credentials hubovho hosta voči systému mimo fleetu; trust
   je per zariadenie, revokovateľný, a `hub.md` už radí trustovať vlastný desktop. Cena: význam
   trustu sa rozšíri z „nemarkuj prompty“ na „smie pushovať“ — rovnaká dilema ako otázka 1
   iterácie 03 (`set_fleet_setting`). Alternatíva: `full` bez trustu (push = session control,
   ako `kill_session`), s dialógom a auditom ako jedinými mantinelmi.
2. **Variant A (10 toolov 1:1, `BUDGET_BYTES` ~63 800) alebo B (6 toolov, ~62 400)?** A drží
   princíp „meno toolu = meno príkazu“ z iterácie 03; B ušetrí ~1 400 B za päť príkazov s iným
   menom. Odporúčam A, pokiaľ rozpočet nie je tvrdý strop.
3. **`Visibility::ClientOnly`** — má zmysel tier toolov, ktoré per-host token (agent) nevidí, aby
   agentova plocha nerástla o git tools, ktoré má v shelli? Zmena `present::visible_to` +
   `TOOL_POLICIES` pole + samostatné meranie v budget teste. Mimo tejto iterácie, ale rozhoduje o
   tom, ako vážne brať otázku 2.
4. **`repo_delete_branch.force`:** odmietnuť ne-master callerovi v tele toolu (pole ostáva na
   drôte, ~150 B), alebo pole zo schémy a z Tauri args vypustiť (UI ho nepoužíva)? Odporúčam
   vypustiť — menej plochy, menej bajtov.
5. **Agent-busy:** iba UI hint (odporúčam), alebo backendové `E_BUSY` pri `claude_status ==
   working` pre commit/checkout? Backend verzia by zmenila aj standalone a spoľahla sa na
   `pane_intel`, ktorý je heuristika.
6. **Potvrdenie pri checkout vetvy a Pull** — zaviesť (UX-62, odporúčam, zjednotí s
   checkout-commit), alebo nechať okamžité a spoľahnúť sa na `E_DIRTY`/`--ff-only`?
7. **UX-63 wall clock** pre fetch/pull/push: 60 s, 90 s, alebo konfigurovateľné? Odporúčam
   pevných 90 s (`run_shell_bounded(connect 10 s, wall 90 s)`), `Deadline::Lifecycle` na hube.
8. **Skeleton oneskorenie 150 ms** a `prefers-reduced-motion` — akceptovateľné, alebo skeleton
   vždy (bez oneskorenia) pre konzistentný pocit?

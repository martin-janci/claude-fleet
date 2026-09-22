# Iterácia 11 — kritická revízia troch expertov + prvá vlna opráv

**Dátum:** 2026-09-22 · **Vetva:** `feature/ux-expert-review-fix-95377f` ·
**Režim:** read-only revízia (3 paralelní subagenti), potom implementácia.

Na rozdiel od iterácií 1–10 táto neprodukuje len návrhy: overila tvrdenia
auditu proti **aktuálnemu kódu** a opravila to, čo sa dalo opraviť
samostatne a otestovať.

## 1. Vyvrátené / neplatné nálezy auditu

Toto sú tvrdenia z `README.md`, ktoré v kóde **neobstáli**. Netreba ich
riešiť a nemajú sa ďalej citovať.

| ID | Verdikt | Dôkaz |
|---|---|---|
| UX-24 | **Neexistuje.** Appka nemá updater vôbec — žiadny `@tauri-apps/plugin-updater`, žiadny `tauri-plugin-updater`, žiadny kľúč `updater` v `tauri.conf.json`, a reťazce „Update installed" / „Restart to update" nie sú nikde v `src/`, `src-tauri/src` ani `crates/`. To, čo bolo na screenshote 05, vykreslil **Claude Code do tmux panelu**, nie toast fleetu. Skutočný nález pod tým je opačný: appka sa distribuuje ručným DMG a **nemá ani kontrolu verzie**. | `package.json`, `src-tauri/Cargo.toml`, `tauri.conf.json` |
| UX-28 | **Zlá diagnóza.** „Pravdepodobne chýbajú `role`/`aria-*`" je nepravda: všetky štyri klikateľné `div`/`span` v 100 `.svelte` súboroch majú korektný pár `role="button" tabindex="0"` + `onkeydown`. Bare clickable div v repe nie je. Príčina zlyhania AX je iná — **interaktívne prvky vnorené v `role="button"`** (checkbox + 5 tlačidiel v riadku session, `+`/kôš v riadku projektu). To je nevalidná ARIA a presne to láme `app_click`. | `SessionRowItem.svelte:155-186,308-352`, `Sidebar.svelte:747-778` |
| UX-14 | **Zastarané.** Panel už nie je 300×110 px — je 360 px × 60vh a montuje plnohodnotný `ConversationPanel`. Prežíva len časť „fixed overlay v pravom dolnom rohu". | `AgentPanel.svelte:145,172-188` |
| UX-13 | **Nereprodukovateľné z kódu.** Stav panela sa odvodzuje z `$operatorState` s copy aj akciou per stav; `blockedAction` je kľúčované stavom, nie reťazcom. Zatvoriť, kým to niekto neuvidí naživo. | `AgentPanel.svelte:62-76` |
| FE-8 | **Opravené, vyhodiť z backlogu.** `Modal.svelte` je natívny `<dialog>` + `showModal()` s `[data-autofocus]`, obnovou fokusu pri unmounte, Escape cez `cancel` a reopen guardom. Používa ho 22 komponentov. | `Modal.svelte:50-95` |
| UX-08 (časť) | **Polovica nepravda.** `Needs you` **mení farbu** (`class:hot` → `#e64a4a`). Skutočná chyba je užšia: `⚠` a `(0)` sa kreslili aj pri nule. A riadkov je päť, nie štyri. | `SidebarFilters.svelte:67,88,123,135,141,184,271` |
| UX-27 | **Zastarané**, Hosts je dnes najlepší pohľad v appke: `?` legenda s `aria-expanded`, plný klávesový model, korektný `role="listbox"` + `aria-activedescendant`. | `HostsView.svelte:388-395`, `HostsList.svelte:101-104` |
| UX-05 (formulácia) | **Mechanizmus je iný, než audit tvrdí.** Nie „každý prompt premenuje". Po prvom prompte je meno `yes`, to sa **nerovná** `default_friendly_name`, takže `replaceable` je false a **každý ďalší, lepší prompt sa ignoruje**. Chyba je: *prvý prompt cez fleet vyhráva natrvalo.* Dôsledok: stačí filter na prompte, žiadna migrácia ani nový stĺpec. | `prompt.rs:322-327` |

## 2. Nové nálezy (nad rámec auditu)

Závažnosť: **C** kritické, **H** vysoká, **M** stredná.

| ID | Sev | Zistenie |
|---|---|---|
| UX-124 | **C** | **Schvaľovací dialóg pre control-API mal `data-autofocus` na *Approve*.** Je to brána pred `kill_session` / `delete_worktree` / `broadcast_prompt` / `set_clipboard`, je globálne namontovaný a `showModal()` kradne fokus — takže stlačenie Space/Enter už v lete schválilo agentovi deštruktívny call. Vlastné pravidlo appky (`ConfirmDialog.svelte:20`) hovorí opak. |
| UX-125 | H | **Zamietnutie, ktoré nedorazí na backend, sa tvári ako úspech.** `await mcpConfirm(...)` zahadzoval `Result` a request sa z fronty odstránil bezpodmienečne. |
| UX-126 | H | **Restart claude je jednoklikový deštruktívny úkon bez potvrdenia**, so **zhodným glyfom `↻`** ako neškodný Refresh. Susedia v tom istom pruhu (Recreate, Kill) potvrdzujú. To isté `onRepair`. |
| UX-127 | H | **Checkout vetvy nepotvrdzoval** — napriek tomu, že handler sa volá `confirmCheckout`. Checkout *commitu* o tri riadky nižšie pritom dvíha `danger` dialóg pre ten istý následok. |
| UX-128 | H | **Tri nesúvisiace fixed vrstvy v pravom dolnom rohu.** FAB (48 px, `bottom:20px`) prekrýva pravých ~56 px 24 px status lišty vrátane tlačidla usage — jediného vstupu do Hosts na prekvótovaný účet. A každý toast (`bottom:2rem`, z-index 50) prekryl FAB, takže jediný dokumentovaný vstup k agentovi bol počas toastu neklikateľný. |
| UX-129 | H | **Tri komponenty, tri politiky mena tej istej session.** `attention.ts:303` už exportuje `displayName()` a používa ho iba stuck announcer. Riadok rešpektuje `$showFriendlyNames`, **hlavička terminálu kreslila `tmux_name` bezpodmienečne**, `agent_context.ts:23` naopak friendly name bezpodmienečne. Pri default nastaveniach si sidebar a hlavička priamo nad ním protirečili. |
| UX-130 | H | **AssetsPanel ukazoval `Loading…` navždy**, keď katalóg zlyhal: `catalog` sa `.set()` len pri úspechu, takže sa súčasne vykreslil červený error **aj** „Loading…", bez retry. |
| UX-131 | H | **Žiadny focus indikátor na primárnej navigácii.** Riadok session, riadok projektu a riadok commitu sú `tabindex="0" role="button"` a nemajú `:focus`/`:focus-visible`. Klávesnicový používateľ pri tabovaní zoznamu sessions nevidí **nič**. WCAG 2.4.7. |
| UX-132 | H | **Riadkové akcie sú len na hover**, `display:none` ich vyradí z tab poradia a `:focus-within` pravidlo neexistuje — Restart/Edit/Rename/Recreate/Kill boli z klávesnice nedosiahnuteľné. |
| UX-133 | M | **Kôš pri projekte je viditeľný presne naopak.** `.icon-btn:disabled { opacity: 0.6 }` prebíja `.purge-btn { opacity: 0 }` — neviditeľný, keď funguje; trvalo viditeľný, keď nie (hub režim). (= mechanizmus UX-04, teraz s dôkazom.) |
| UX-134 | M | **Vnorené live regióny.** `role="alert"` vnorený v `role="status" aria-live="polite"` je nedefinované správanie; čítačky buď ohlásia dvakrát, alebo jedno zahodia. |
| UX-135 | M | **Toast stack je neohraničený a bez „zrušiť všetko".** Chyby sú sticky, dedup je podľa `code+message`, takže N zlyhaných sessions = N sticky toastov a jediná cesta von bolo N klikov na `×`. |
| UX-136 | M | **Dve súperiace natvrdo zapísané červené, každá nečitateľná v jednej téme.** `#e64a4a` ×55 v 29 súboroch (3.86:1 na light) a `#dc2626` ×15 v rodine Assets (3.91:1 na dark). `src/app.css` nemá token `--danger` vôbec. Test `conversation_theme.test.ts:15` ten problém **už pozná** a zakazuje `#e64a4a` — ale len pre 4 z 33 súborov. |
| UX-137 | M | **`☑` znamená dve veci v jednom 32 px pruhu**: Tasks (navigácia) a Select mode (prepínač). |
| UX-138 | M | **`×` je tri rôzne príkazy** v riadku session — dismiss ghost, remove from list a **kill session** — deštruktívny sa nedá odlíšiť. Navyše `↺` a `♻` sú dva glyfy pre jeden Recreate. |
| UX-139 | M | **Conversation hlavička ticho odreže fakty**: `.facts` má `overflow:hidden`, žiadny wrap, žiadne per-tag ellipsis a `title`. Status je v zdroji posledný, takže padá prvý. |

## 3. Čo táto iterácia opravila

| Nález | Oprava | Test |
|---|---|---|
| UX-05 | `friendly_name_from_prompt` → `label_from_prompt` s filtrom: prvý neprázdny riadok; odmietnuť prefixy `/ ! # [ < >`; odmietnuť, ak prvé slovo je potvrdenie (EN+SK stop-list); min. 3 slová; aspoň jedno písmeno. Odmietnutý prompt **nezapíše nič**, riadok si necháva branch default a pomenuje ho až ďalší skutočný prompt. Plus jednorazové samoliečenie starých mien (`yes`, `clear`) porovnaním so starým pravidlom nad `last_prompt`. Plus `label: bool` v `send_prompt_inner` a nové `send_system_prompt` — safe-kill, doručenie správy, review seed a broadcast už nepomenúvajú. | 9 testov v `sessions/tests.rs` |
| UX-124, UX-125 | `data-autofocus` presunutý na **Deny**; zlyhaný verdikt necháva request vo fronte a hlási sa toastom. | 2 testy v `McpConfirmDialog.test.ts` |
| UX-126 | Restart (riadok aj detail) ide cez `ConfirmDialog` s `danger`. | `Sidebar.test.ts` |
| UX-127 | Checkout vetvy ide cez `ConfirmDialog` s `danger`. | nový `FilesPanel.checkout.test.ts` |
| UX-128 | Tokeny `--status-h` / `--fab-size` / `--layer-gap` v `app.css`; FAB nad lištou, toasty nad FAB-om. Jeden stĺpec, jeden zdroj mier. | vizuálne (jsdom nemá layout) |
| UX-129 | Hlavička terminálu ide cez `displayName()` rovnako ako riadok; tmux meno ostáva v tooltipe. | — |
| UX-130 | Pri chybe sa namiesto `Loading…` vykreslí hláška + **Retry**. | `AssetsPanel.test.ts` |
| UX-131, UX-132 | `:focus-visible` ring (kreslený dovnútra) na riadku session, projektu aj commitu; `:focus-within` odkrýva riadkové akcie. | `svelte-check` |
| UX-133 | `.purge-btn:disabled { opacity: 0 }` + jemné `0.35` na hover. | — |
| UX-134, UX-135 | Vnorený `role="alert"` odstránený; „Dismiss all (N)" pri viac než jednom toaste (`clearToasts`). | — |
| UX-08 (zvyšok) | `⚠` a `(0)` sa pri nule nekreslia; pill ostáva, aby filter zostal dosiahnuteľný. | — |

## 4. Čo sa vedome NEurobilo

- **Ikonový systém (Lucide, rozhodnutie (a)).** Je to správny smer a stále
  neimplementovaný, ale je to PR, ktorý sa dotkne každého iného UX PR v lete
  (`TransferSheet.svelte:414-418` má glyfy v CSS `content:`, label stringy v
  `attention.ts:39-43` sa asertujú menom). Musí ísť **sám** a **pred**
  prepisom sidebar IA. UX-138 (deštruktívne `×`) a UX-137 patria doň.
- **Token `--danger` a prebratie farieb (UX-136).** 140 riadkov mechanickej
  zmeny; test naň už existuje, stačí rozšíriť glob v
  `conversation_theme.test.ts:5` zo 4 súborov na `./*.svelte`. Vlastný PR,
  nič iné v ňom.
- **Migrácia 040 / `friendly_name_source`.** Správny dlhodobý tvar, ale je to
  wire field, riziko výpadku proti staršiemu hubu, `REGEN_HUB_CONTRACT` a
  ~13 súborov — a na zastavenie `yes`/`clear` netreba.
- **Rozdelenie riadku session na `<button>` + súrodenecký checkbox** (koreň
  UX-28). Správne, ale `10-select-mode-bulk-actions.md` to už scopuje ako
  UXPR-37; mimo poradia to garantuje konflikt.
- **Prestavba tab stripu podľa ARIA APG (UX-140).** `role="tablist"` má ako
  priame dieťa `div` s `role="radiogroup"`, `role="tabpanel"` nie je v repe
  ani raz a žiadny tab nemá `aria-controls`. Zisk pre čítačku je okrajový,
  cena je najfrekventovanejší layout v `App.svelte`. Jediné, čo sa oplatí:
  vytiahnuť `.tab-tail` von z `role="tablist"`.
- **Klientske IPC timeouty.** `ssh.rs:293-310` už ohraničuje každý príkaz
  (`ConnectTimeout × 3`, floor 30 s, upload 300 s). Druhý, kratší časovač na
  fronte by opustil prácu, ktorá stále beží, a klamal o nej. UX-130 bola
  chyba stavového automatu, nie timeoutu — tak sa aj opravila.
- **Skeletony pre Files (UX-26).** Husté vývojárske UI, git volania sa lokálne
  vracajú v desiatkach ms — skeleton by viac blikal, než upokojil. Čo naozaj
  chýba, je **retry** na chybových vetvách (`FilesPanel.svelte:322`,
  `FileList.svelte:129`), teda vzor z UX-130.
- **Postaviť updater.** Podpisové kľúče, feed, rollback — produktové
  rozhodnutie, nie UX oprava. Lacná náhrada, ak sa bude chcieť: pätička
  porovná `health.version` s posledným GitHub tagom a ukáže odkaz.

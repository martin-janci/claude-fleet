# Claude Fleet v obrazoch

Claude Fleet riadi dlhobežiace sessiony Claude Code v tmux na viacerých
strojoch cez SSH. Rovnaký fleet sa dá prevádzkovať tromi spôsobmi: len z
desktopu, cez vždy zapnutý hub, a k hubu pripojiť aj mobil. Podrobnosti sú v
[concepts.md](concepts.md) a [hub.md](hub.md).

Farby: modrá je desktop, oranžová `fleet-hub`, tyrkysová host s Claude Code,
zelená `fleet-agent`, ružová mobil, fialová trackery.

## A · Desktop bez hubu

![Desktop bez hubu](images/fleet-standalone.svg)

Všetko žije v jednej Tauri aplikácii. Aplikácia sa sama pripája na hosty cez
SSH (jeden `ControlMaster` na host), sama ukladá stav do `state.db` a sama
robí reconcile. Hooky Claude Code sa vracajú reverzným tunelom `ssh -R` na
lokálny port Control API, ak je zapnuté. Keď je počítač vypnutý, sessiony v
tmux bežia ďalej, ale fleet o nich nevie.

## B · S hubom

![Fleet s hubom](images/fleet-hub.svg)

`fleet-hub` je to isté jadro bez okna, spustené 24/7 na serveri. Hub vlastní
stav, robí reconcile, synchronizuje trackery a hosty mu hlásia hooky priamo
na jeho verejnú URL. Host bez verejnej adresy spustí `fleet-agent`, ktorý sa
k hubu pripojí sám (`wss://<hub>/agent`). Spárovaný desktop je iba okno do
hubu: príkazy posiela cez `/mcp`, zmeny sleduje cez `GET /events` a terminál
otvára priamo (`ssh` + `tmux attach`), bez hubu v ceste. Ak hub nie je
dostupný, desktop sa nevráti do režimu A.

## C · S mobilom

<img src="images/fleet-mobile.svg" alt="Blokovaná session na mobile" width="280">

Mobil je klient hubu s vlastným tokenom (`full` alebo `readonly`, voliteľne
viazaný na jednu organizáciu):

1. `fleet-hub pair --name phone` vypíše QR s jednorazovým kódom (platí 10 minút).
2. Aplikácia kód naskenuje a raz zavolá `POST /pair`.
3. Dostane vlastný token; hub si ukladá iba jeho SHA-256.
4. Príkazy idú cez `/mcp`, zmeny cez `/events` (keep-alive každých 15 s).

Keď session čaká na povolenie, hub pošle jej voľby (`pending_input`) a mobil
z nich spraví tlačidlá. Prompt z mobilu dostane agent označený ako
*untrusted*, kým operátor klientovi nedôveruje (`fleet-hub client trust`).
Stratený telefón: `fleet-hub client revoke phone`.

## Tok udalosti

![Tok udalosti od hooku po obrazovky](images/fleet-event-flow.svg)

Hook na hoste pošle zmenu hubu, hub ju zapíše a cez SSE ju pošle všetkým
klientom naraz. V režime A je v strede diagramu samotný desktop.

## Porovnanie

| | A · Desktop bez hubu | B · Desktop s hubom | C · Mobil |
|---|---|---|---|
| Kto vlastní stav fleetu | desktop (`state.db` na disku) | hub | hub |
| Čo musí byť zapnuté | tvoj počítač | server s hubom | server s hubom |
| Kto sa pripája na hosty | desktop cez SSH | hub cez SSH alebo `fleet-agent` | nikto, všetko ide cez hub |
| Host za NAT | nie | áno, `fleet-agent` | áno, cez hub |
| Kam idú hooky | `ssh -R` tunel na laptop | priamo na URL hubu | na hub |
| Živé zmeny | lokálny event bus | `GET /events` | `GET /events` |
| Terminál | lokálny PTY | lokálny PTY priamo na host (nie agent host) | výstup, transkript, prompty |
| Pridanie hostu, provisioning | áno | operátor hubu (`fleet-hub`) | nie |
| Viac zariadení naraz | nie | áno | áno |
| Windows | funguje, bez SSH multiplexu | odporúčané | – |

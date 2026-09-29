# Fleet: AI riadený systém práce — brainstorming

Dátum: 29. september 2026
Stav: otvorený pracovný zápis; brainstorming pokračuje. Nie je to schválená špecifikácia ani implementačný plán.

## Vízia používateľa

Fleet má poskytovať AI riadený, inteligentný systém riadenia práce. Má spolupracovať s existujúcim interným task systémom, Jirou, Asanou alebo inými nástrojmi a dopĺňať schopnosti, ktoré im chýbajú. Interné úlohy sú plnohodnotnou súčasťou: pri práci na tickete môžu vzniknúť pomocné úlohy, ktoré nepotrebujú vlastný externý ticket.

Cieľ sa neobmedzuje na jednoduchý zoznam To do / Doing / Done. Agent má postupne vedieť podstatnú časť práce pripraviť, delegovať a vykonať za používateľa.

## 1. Stručná história a výsledky práce

Každá úloha má mať svoj zrozumiteľný „životopis“: čo sa robilo, aký bol postup, aké podstatné nástroje sa použili a aké výsledky vznikli. Výstup nemá byť román ani neprehľadný výpis všetkých udalostí.

Používateľ chce vedieť zobraziť stručný súhrn práce na úlohe a tento súhrn ďalej použiť, napríklad v inom prehľade alebo ako podklad na odpoveď do externého ticketu. Automatické odosielanie odpovedí zatiaľ nebolo dohodnuté.

## 2. Inteligentné riadenie pracovného postupu

Úloha má mať riadený súbor krokov zodpovedajúcich vývojovému cyklu. Inšpiráciou je Paperclip, ale riešenie má byť jednoduchšie a prirodzene použiteľné vo Fleet.

Už teraz majú existovať skilly a API na správu tejto práce a možnosť využiť AI pri rozhodovaní. Neskôr sa na ten istý systém majú napojiť automatickí agenti.

Postup sa prispôsobuje konkrétnej úlohe. Nemá byť nevyhnutne rovnakým checklistom pre každý ticket. Medzi uvažované činnosti patria analýza zadania, posúdenie potreby výskumu, plánovanie a diagnostika chyby.

## 3. Spolupráca človeka a agentov

Príklad používateľa: má na seba priradené tickety. AI zanalyzuje, čo treba urobiť, posúdi vhodný postup a pripraví prácu podľa workflowu. Časť vykoná programátor, pri časti mu AI pomôže a časť môže delegovať.

Neskôr môže agent prevziať kroky, ktoré dnes vykonáva človek. Riadenie úlohy má podporovať tento postupný prechod, nie vyžadovať okamžitú úplnú autonómiu.

## Otvorené témy

- Vzťah externého ticketu, internej úlohy, delegovaného jobu a natívnych taskov či krokov jednotlivých AI nástrojov vrátane Claude.
- Ako reprezentovať workflowy a módy vývojového cyklu a ako vyberať vhodný postup.
- Ktoré rozhodnutia vykonáva AI samostatne a ktoré zostávajú používateľovi.
- Ktorý systém vlastní jednotlivé údaje a ako sa riešia zmeny medzi nástrojmi.
- Aké udalosti a výsledky zachytávať a ako z nich zostavovať krátke, dôveryhodné súhrny.
- Konkrétny rozsah prvej implementácie zatiaľ nebol dohodnutý.

## Vzťah k pôvodnému návrhu

Diskusia nadviazala na návrh interného zoznamu úloh v repozitári martin-janci/claude-fleet, vetva `docs/internal-task-list-spec`. Pôvodná špecifikácia a plán riešili vlastné úlohy, zrkadlenie dispatch jobov a zjednodušenie Work tabu.

Používateľ počas brainstormingu spresnil širší produktový cieľ uvedený vyššie. Skoršie odporúčanie asistenta zúžiť produkt na jednoduchý zoznam a malé MVP nie je rozhodnutím používateľa. Technické námietky k pôvodnému plánu ostávajú samostatným review, nie obmedzením tejto vízie.

Tento zápis zachytáva doterajšiu diskusiu. Ďalšie ciele ešte môžu pribudnúť.

## Pracovná roadmapa

Nasledujúce poradie je návrh asistenta odvodený z brainstormingu, nie schválený harmonogram ani pevný rozsah vydaní. Jednotlivé oblasti sa môžu rozvíjať súbežne. Používateľ otvoril potrebu roadmapy; konkrétne priority a termíny zostávajú otvorené.

1. **Spoločný kontext práce.** Prepojiť externé tickety, interné úlohy, session a delegované joby. Určiť vzťah k natívnym taskom AI nástrojov a vlastníctvo údajov. Používateľ vie, k akému zadaniu práca patrí, aj keď prechádza medzi nástrojmi.
2. **Pamäť úlohy.** Zachytávať podstatné kroky, rozhodnutia a výsledky. Poskytovať stručný súhrn s dostupnými podkladmi, použiteľný aj pri príprave odpovede do ticketu.
3. **Riadené workflowy.** Zaviesť postupy vývojového cyklu ovládateľné cez rozhranie, skilly a API. Človek aj agent môžu vykonávať jednotlivé kroky a odovzdávať výsledky.
4. **AI príprava a koordinácia.** Z priradeného ticketu vyhodnotiť potrebnú analýzu, výskum, plánovanie alebo debugovanie; navrhnúť ďalšie kroky a delegovanie podľa kontextu a výsledkov.
5. **Postupná autonómia.** Umožniť agentom preberať dohodnuté časti workflowu, sledovať ich výsledky a v definovaných situáciách odovzdať rozhodnutie človeku. Rozsah samostatnosti musí byť ešte dohodnutý.

Roadmapa zachováva širšiu víziu inteligentného riadenia práce. Nie je návratom k obmedzeniu produktu na jednoduchý interný zoznam.

## Kontra review a protiopatrenia

Používateľ požiadal zaradiť protiopatrenia k trom produktovým rizikám. Nižšie sú navrhnuté zásady na ďalšie rozpracovanie; konkrétne technické riešenia a prahy nie sú schválené.

### Riziko: ďalší systém, ktorý treba ručne udržiavať

- Pre každú prepojenú informáciu určiť autoritatívny zdroj. Externý ticket môže vlastniť zadanie a tímový stav; Fleet interné kroky, vykonanie a pracovnú históriu. Presné rozdelenie dohodnúť pre každý typ integrácie.
- Interné kroky nemusia vytvárať externé tickety. Prepojenie na existujúci ticket nesmie automaticky znamenať duplikovanie každej podúlohy.
- Synchronizovať iba dohodnuté údaje. Zobraziť oneskorenú synchronizáciu a konflikty; neprepisovať potichu súbežné zmeny. Opakované spracovanie nesmie vytvárať duplikáty.
- Overenie úžitku: používateľ nemusí ten istý údaj ručne aktualizovať na dvoch miestach; výpadok integrácie sa prejaví zrozumiteľne.
- Zaradiť do oblasti roadmapy „Spoločný kontext práce“, pred automatickými zápismi do externých systémov.

### Riziko: AI vytvorí viac procesu než úžitku

- Workflow prispôsobiť zložitosti a neistote úlohy. Research, rozsiahle plánovanie ani delegovanie nie sú povinné pri každej úlohe.
- Používateľ môže postup skrátiť alebo upraviť. AI má vedieť stručne zdôvodniť, prečo pridáva ďalší krok.
- Pre automatickú prácu zaviesť dohodnuté limity času, nákladov a opakovaní; pri chýbajúcom pokroku postup prehodnotiť alebo odovzdať človeku.
- Overenie úžitku: sledovať čas do užitočného výsledku a potrebnú pozornosť človeka, nie počet vytvorených taskov či spustených agentov.
- Zaradiť do „Riadených workflowov“ a „AI prípravy a koordinácie“.

### Riziko: presvedčivý súhrn bez overeného výsledku

- Oddeliť plánované kroky, vykonané akcie, pozorované výsledky a overené závery. Dokončenie agentovho checklistu samo osebe nedokazuje splnenie zadania.
- Podstatné tvrdenia v súhrne prepojiť na dostupné podklady, napríklad zmenu kódu, výsledok testu alebo vytvorený dokument. Nevydávať samotné použitie nástroja za dôkaz úspechu.
- Pri úlohe určiť primerané podmienky dokončenia. Chýbajúce overenie a neistotu uviesť priamo, nie schovať do podrobností.
- Súhrn pre externý ticket pripravovať ako návrh; pravidlá automatického publikovania dohodnúť osobitne.
- Overenie úžitku: pri tvrdení o dokončení sa dá dohľadať podklad a súhrn prizná zlyhané alebo nevykonané kontroly.
- Zaradiť už do „Pamäte úlohy“ a zachovať vo všetkých stupňoch autonómie.

## Využitie natívnych schopností agentov

Používateľ otvoril možnosť využiť existujúcu natívnu funkcionalitu agentov. Návrh na ďalšie rozpracovanie:

- Fleet zadáva cieľ, kontext, podmienky dokončenia a hranice samostatnosti. Agent používa vlastné plánovanie, tasky, skilly, nástroje a delegovanie tam, kde ich skutočne podporuje.
- Fleet prepája vykonanie s používateľskou úlohou alebo externým ticketom, uchováva podstatné výsledky a umožňuje pokračovanie naprieč session a nástrojmi.
- Natívny task agenta možno pripojiť ako krok alebo vykonanie pod úlohou Fleet. Nie každý dočasný krok musí vytvoriť samostatnú trvalú úlohu alebo externý ticket.
- Integrácia má poznať dostupné schopnosti konkrétneho agenta. Ak podporuje štruktúrované udalosti alebo stav taskov, môže ich prevziať; ak nie, použije explicitné hlásenie výsledku cez dostupné rozhranie. Konkrétne možnosti jednotlivých agentov treba ešte overiť.
- Detailný plán nemajú nezávisle meniť dva systémy. Určiť, ktorý systém ho vlastní a ako sa zmeny od človeka alebo Fleet odovzdajú agentovi.
- Natívne označenie „done“ je signál o dokončení kroku podľa agenta; splnenie celej úlohy sa posudzuje podľa dohodnutých kritérií a podkladov.

Cieľom je využiť silné stránky agentov a doplniť spoločný kontext, kontinuitu a koordináciu, nie budovať druhú implementáciu každej ich schopnosti.

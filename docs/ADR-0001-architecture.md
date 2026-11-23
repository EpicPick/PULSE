# ADR-0001 · Mise en œuvre du premier jalon

La décision et le cahier des charges originaux sont conservés dans le [dossier consolidé](../PULSE-ADR-DESIGN-SYSTEM-complet.md), sections ADR-001-A à F. Le présent fichier consigne les choix effectivement appliqués.

| Crate | Responsabilité |
|---|---|
| `pulse-domain` | Identité PID + FILETIME de création, instantanés, unités, disponibilité et provenance |
| `pulse-platform-windows` | Collecte native read-only ; exécution des demandes terminées par Win32 |
| `pulse-actions` | Contrat, réponses et canal borné des actions explicites |
| `pulse-runtime` | Worker interrompable et boîte « dernier instantané » de capacité 1 |
| `pulse-history` | Ring borné, schéma SQLite versionné, rétention et relecture |
| `pulse-insights` | Règle de croissance de la mémoire privée, déterministe et indépendante de l’UI |
| `pulse-testkit` | Fixtures déterministes, indépendantes du système |
| `pulse-design` / `pulse-ui` | Tokens, widgets Rune, projection triée/filtrée et interactions |
| `pulse-app` | CLI, amorçage Rune et câblage des modes et des workers |

**Flux.** Le worker collecte une fois par seconde environ et ralentit à quatre secondes lorsque Rune indique que la fenêtre est masquée. Il publie des snapshots immuables par `Arc`. Rune se réveille à l’arrivée d’un échantillon, prend seulement le dernier et ne lance aucun appel de collecte depuis le rendu. Les captures désactivent le worker et s’appuient sur les scènes déterministes. La rétention mémoire est de 900 points par série agrégée et 900 points pour l’unique processus suivi (cadence ~6 s) ; aucun historique complet de tous les processus n’est conservé.

**Permissions.** Collecte avec `PROCESS_QUERY_LIMITED_INFORMATION`, sans privilège de débogage. La terminaison n’est disponible qu’en mode réel, après confirmation, et n’est jamais déclenchée par un diagnostic. Elle vérifie le PID, le FILETIME et l’état critique via le même handle que celui utilisé pour terminer. Un échec de vérification entraîne un refus. Le succès indique l’acceptation de la demande, pas la preuve de sortie. La réussite d’une terminaison réelle n’est pas testée automatiquement.

**Connexions.** Les tables IP Helper ne sont interrogées que tant que l’onglet Connexions est demandé, que la fenêtre est visible et que l’affichage n’est pas en pause. Échantillonnage à environ cinq secondes, quatre tables indépendantes, 1 024 lignes retenues par table. Le PID est affiché tel que fourni au moment de la collecte ; pas de jointure avec des noms d’une autre photographie. Aucun DNS, trafic injecté, stockage ou export d’adresse.

**GPU par processus.** La requête PDH WDDM locale est créée uniquement pendant l’inspection. Les mesures sont rapprochées par PID, identifiant LUID de l’adaptateur et index physique, puis seules les générations de processus observées dans les deux relevés encadrant la mesure sont conservées. La charge retenue est le maximum des moteurs disponibles. Mémoire dédiée et partagée sont indépendantes ; une valeur manquante n’est pas imputée à zéro. L’identifiant WDDM est distinct de l’inventaire NVIDIA et n’est pas assimilé à une carte NVML non vérifiée.

**PresentMon.** Worker séparé du collecteur système et une opération à la fois. Le paquet portable intègre le CLI officiel x64 épinglé et vérifié par SHA-256 au packaging ; sur demande, le worker l’extrait dans un répertoire temporaire unique, supprimé après usage. Pas d’installateur, de service permanent, de recherche dans le PATH ni de téléchargement à l’exécution. La capture reste sans console ni élévation, ciblée par PID et heure de création vérifiée. En cas d’annulation, seul l’auxiliaire lancé par PULSE est arrêté et sa session ETW nommée est nettoyée au mieux. L’analyse est locale et bornée (32 Mio, 180 000 lignes, 512 chaînes). Les rapports ne sont pas stockés en SQLite.

**Sonde CPU HWiNFO.** Lecteur SM2 facultatif, en lecture seule. Le verrou du producteur est essayé sans attente sur le collecteur ; les sections sont bornées avant copie et le décodage s’effectue hors verrou. Seules des températures CPU identifiées et récentes sont affichées, avec leur source. PULSE ne démarre pas HWiNFO et ne contourne aucune limite de partage.

**Stockage par disque.** Worker séparé, actif dans l’onglet Stockage et jamais pendant la relecture. WMI inventorie les disques et les volumes avec lettre toutes les 15 s ; les associations de volumes proviennent de Windows, les données de santé Windows sont rapprochées uniquement par numéro de série non ambigu. PDH relève les débits, l’activité (100 − % Idle Time) et les latences par disque. Le journal de santé NVMe standard est consulté en lecture seule via `IOCTL_STORAGE_QUERY_PROPERTY`, sans commande propriétaire et sans droits supplémentaires. Les courbes en RAM sont bornées à 61 points / 60 s et réinitialisées au changement d’identité PnP observé. Aucun numéro de série ou détail par disque n’est écrit en session.

**Ciblage auto.** Après armement explicite, la première application admissible stable 750 ms est ciblée, hors PULSE et fenêtres usuelles du shell. Windows ne fournit pas ici de classification des jeux. L’observation du premier plan à environ 50 ms et les horodatages QPC PresentMon permettent d’exclure les intervalles non encadrés par des observations continues du processus au premier plan. La durée murale de la capture n’est pas prolongée.

**Historique.** SQLite WAL facultatif depuis le sélecteur natif de l’interface ou la CLI. Le fichier est créé exclusivement à la demande, jamais écrasé, puis écrit par le worker. Il contient l’ordre, UTC, le temps monotone depuis le premier point de la session, les pourcentages agrégés et, depuis le schéma v2, les débits disque/réseau en octets/s, sans noms ni identités de processus. Les valeurs indisponibles sont stockées en `NULL` ; la raison détaillée n’est pas conservée. La relecture v1 reste disponible sans migration du fichier. Limite SQL de pages et rétention transactionnelle ; le WAL peut croître temporairement si une autre application maintient un lecteur actif. La relecture est limitée à 900 points et suspend la collecte de nouveaux échantillons jusqu’au retour au direct.

**Schéma v3.** Option désactivée par défaut : jusqu’à douze contributeurs par échantillon (union des six premiers en CPU et des six premiers en mémoire privée). Les lignes sont écrites dans la même transaction que le point agrégé et supprimées avec lui. Sont enregistrés uniquement le nom de l’exécutable sans chemin, le PID et le temps de création de l’instance, ainsi que les deux mesures disponibles. Le CSV ne lit pas la table des contributeurs. Les anciens fichiers v1/v2 restent lisibles ; ils n’ont pas de détail par processus. La base est plafonnée à 8 192 pages (environ 32 Mio pour une page de 4 Kio) ; une erreur d’écriture arrête l’enregistrement et s’affiche dans l’interface.

**Arbitrages.** L’ordre explicite de la mission prime sur le plan consolidé : prototype sur fixtures, puis collecte et actions Windows, puis historique et règle mémoire. L’exécutable de production démarre en mode réel et utilise le sous-système graphique Windows, sans console. Le monitoring avancé couvre les débits disque/réseau et les premiers compteurs NVIDIA NVML. Les fixtures restent disponibles uniquement dans les tests et les captures de développement. Aucun changement de framework.

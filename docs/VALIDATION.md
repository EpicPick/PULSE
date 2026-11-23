# Validation locale · Windows 11 x64

Machine : Windows 11 Pro 10.0.26200, Ryzen 9 3900X (24 processeurs logiques), Rust 1.99.1 `x86_64-pc-windows-msvc`.

| Vérification | Résultat |
|---|---|
| `cargo fmt --all -- --check` | Réussi |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Réussi |
| `cargo test --locked --workspace` | 58 tests, dont 20 scénarios d’interaction et de projection Rune |
| `cargo build --locked --release -p pulse-app` | Réussi |
| `scripts/smoke-windows.ps1` | Fenêtre release ouverte en mode réel, fermeture normale, 4 points réels après purge, relecture ouverte puis fermée |
| `scripts/capture.ps1` | 14 PNG générés hors écran, dont le rapport FPS, les connexions et les variantes compactes ou à 150 % |

Le test d’intégration a obtenu des mesures natives pour les deux nouveaux collecteurs sur ce poste : environ **30,8 Mo/s** sur le compteur disque agrégé pendant le test et **0 o/s** sur les interfaces admissibles au cours de l’intervalle réseau observé. Ce n’est pas une mesure de capacité ni une validation comparative sous charge. Les tests vérifient aussi que la disparition d’une interface, une remise à zéro du compteur ou un intervalle trop long ne produisent pas de faux débit.

Le binaire release incluant les collecteurs I/O a passé le test de démarrage et relecture. Sur cette exécution de 8 s : **1,56 % d’un cœur logique**, **149,4 Mio** privés et **113,1 Mio** résidents. La différence avec la mesure précédente ne suffit pas à établir un gain reproductible ni à valider les objectifs sur 30 min.

Le test d’intégration NVML a détecté la **GeForce RTX 3090** réelle de ce poste : **30 %** d’activité, **1,56 Gio** de VRAM occupée et **47 °C** au moment de l’échantillon. Ces valeurs ne sont ni un benchmark ni des valeurs figées de l’application.

Jalon suivant : export CSV (cellules indisponibles vides, refus d’écraser), arborescence (parent nécessairement plus ancien), calcul du placement secondaire et latences disque vérifiés sans ouvrir de fenêtre sur le bureau. Le binaire a été recompilé et son sous-système Windows GUI vérifié ; le comportement d’ouverture inactive sur deux écrans reste à confirmer en usage réel.

Le scénario de priorité modifie uniquement un processus de test enfant sans fenêtre. Il vérifie le refus d’une heure de création erronée, puis relit la classe retournée par Windows. Le test d’interface confirme qu’aucune commande n’est envoyée au seul choix de la classe.

Les sessions intégrées sont testées via les réponses de dialogue scriptées de Rune : annulation, choix du fichier, démarrage/arrêt, lecture seule et retour au direct. Le service SQLite vérifie la rétention, le chronomètre d’une session et l’absence d’écrasement. Le sélecteur Windows n’a pas été ouvert sur le bureau pour cette validation.

Les contributeurs v3 sont vérifiés par le clic sur une courbe en relecture alors que le processus historique n’existe pas dans l’instantané courant. Les tests SQLite vérifient la purge des contributeurs avec leur point, la distinction entre un point vide et un détail non enregistré, ainsi que l’absence des noms dans le CSV.

Les trois compteurs WDDM par processus ont répondu sur ce poste et le test a observé 31 couples processus/adaptateur lors d’un relevé. La navigation sans adaptateur NVML est testée avec Rune headless ; aucune machine AMD ou Intel n’a été utilisée pour ce jalon.

PresentMon 2.3.1 x64 (SHA-256 vérifié) a décodé la trace officielle `Tests/Gold/test_case_0.etl` sans fenêtre. Le parseur a mesuré 197 intervalles affichés, 41,0 FPS moyens et un P99 de 283,58 ms. Les tests automatisés couvrent également le choix du processus, l’envoi explicite de la capture, son annulation, l’absence de mélange entre chaînes et les métriques de repli. Un essai en direct ciblant le shell de test (aucune fenêtre) a reçu « access denied » d’ETW ; les droits du compte n’ont pas été modifiés. Pas de test en jeu sur le bureau.

Mode auto : test Rune sans processus sélectionné, tests des transitions de premier plan et des intervalles chevauchant une perte de focus. La trace officielle a également été décodée avec `--qpc_time_ms` et traitée par le parseur du mode auto.

Refonte Essentiel : les interactions Rune vérifient les quatre destinations principales, l’accès aux détails matériels depuis Direct et le passage au diagnostic CPU sans déplacer le curseur d’une session. L’accueil est capturé hors écran avec la fixture de développement ; le binaire release reste en mesures réelles.

Refonte Graphite : ouverture sur Processus et recherche en haut ; les mesures GPU manquantes restent vides. Un test du tampon de l’inspecteur vérifie que les mesures d’un PID réutilisé ne sont pas ajoutées à l’ancienne instance et que la fenêtre reste bornée à 60 secondes. Les tests d’actions continuent à vérifier que la terminaison et la priorité ne sont envoyées qu’après confirmation.

Moniteur : un test Rune vérifie les six blocs, les capacités RAM/VRAM et le changement d’adaptateur avec capteur absent. Les mesures réelles Windows ont confirmé la cohérence RAM utilisée/total, la progression de la durée de fonctionnement et la disponibilité des capteurs NVIDIA sur la RTX 3090 de ce poste.

Détail CPU : les tests vérifient l’absence de résumé GPU et l’affichage de la température CPU fournie par un capteur de fixture. Le lecteur HWiNFO SM2 est testé sur des données synthétiques (CPU, groupe GPU exclu, données périmées et bornes incorrectes) ; la sonde physique reste à valider avec HWiNFO en cours d’exécution.

Stockage : les trois disques de ce poste et leurs volumes associés ont été détectés hors écran. PDH a répondu pour chacun. Les deux NVMe ont exposé leur journal de santé, température et total écrit ; le SATA n’a pas publié de température via les interfaces utilisées. Le scénario Rune vérifie la sélection d’un autre disque et l’état non exposé ; le tampon par disque est testé lors d’un remplacement d’identité.

Langue : les tests hors écran vérifient le passage français → anglais → système sans perdre le disque sélectionné, la traduction d’une confirmation de terminaison sans modifier le nom du processus, et la relecture de la préférence dans un dossier temporaire.

**Empreinte de la préversion avant les collecteurs I/O.** En mode release, vue d’ensemble et enregistrement SQLite en cours, après 3 s de stabilisation : **12,86 % d’un cœur logique** sur une fenêtre de 8 s, **211,5 Mio** privés, **126,4 Mio** résidents. Ce résultat est une mesure ponctuelle, pas un benchmark de 30 min ; il ne valide ni la cible de 2 % d’un cœur ni celle de 200 Mio privés. Le premier rendu du tableau dans l’outil headless **debug** était d’environ 482 ms, GPU initialisé séparément ; cette mesure à froid ne représente pas la latence d’interaction ou la cadence à l’écran.

**Restant :** benchmark de 30 min et p95 d’interaction, comparatif Task Manager/PerfMon, contrôle de la capture et du DPI sur un second poste, Windows Narrator, scénario de terminaison réelle testé manuellement sur un processus jetable et exécution de GitHub Actions sur un dépôt distant.

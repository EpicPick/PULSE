# Jalon et backlog PULSE

## État livré

- **Étape 0 :** fenêtre native Rune, cinq captures démo et variantes DPI, table paresseuse de 500 processus, graphiques vectoriels et tests d’interaction. Les budgets de fluidité restent à qualifier sur une charge longue.
- **Étape 1 :** cinq vues interactives. GPU et une partie des diagnostics restent limités aux scénarios démo ; il n’existe pas de session gaming native.
- **Étape 2 :** CPU/RAM système ; charge et fréquence par processeur logique via PDH ; CPU, mémoire, threads, handles, chemin, icône, E/S agrégées et priorité des processus accessibles. Arborescence conditionnée à l’identité du parent ; terminaison et changement de priorité après confirmation. Restent notamment l’éditeur et la suspension.
- **Étape 3 partielle :** ring borné et SQLite WAL avec rétention, relecture limitée, inspection temporelle agrégée et règle mémoire v1. Pas de comparaison ou export de session, ni de persistance par processus. La règle a des fixtures de croissance, plateau et libération ; pas de calibration sur charges réelles.
- **Étape 4 partielle :** débits agrégés disque/réseau, latences disque, inventaire TCP/UDP, capteurs NVIDIA NVML et détail WDDM par processus en direct. PresentMon : capture ciblée et import CSV, rapport FPS/frametimes à la fin. Restent notamment les comparaisons de captures, l’historique GPU par processus, le détail des E/S et les autres règles Smart Diagnostics.

## Sémantique Windows et limites

- `GetSystemTimes` : `(Δkernel + Δuser − Δidle) / (Δkernel + Δuser)`. `GetProcessTimes` : temps CPU du processus divisé par la même capacité cumulée, approximée car les appels ne sont pas simultanés. Deltas invalides ou premier échantillon : en attente. Machines avec plusieurs groupes processeur : CPU indisponible, jamais un faux total.
- `GlobalMemoryStatusEx` : pourcentage de RAM physique non disponible calculé à partir de `ullTotalPhys` et `ullAvailPhys`. Les valeurs peuvent différer du Gestionnaire des tâches selon la définition et la fenêtre.
- `K32GetProcessMemoryInfo` : `WorkingSetSize` et `PrivateUsage` ; échec d’accès distinct de zéro. `PrivateUsage` correspond à la mémoire de commit privée, pas à la mémoire résidente. Le PID parent et le nombre de threads viennent de Toolhelp ; le parent n’est pas résolu en instance. Les entrées sans identité fiable sont comptées séparément.
- La latence de collecte et les timestamps des compteurs ne sont pas atomiques. La mesure UTC identifie l’échantillon ; l’axe temporel est fondé sur le temps monotone. La cadence ralentit à l’arrière-plan ; les valeurs non disponibles ne sont pas interpolées.
- Disque : `PdhAddEnglishCounterW` sélectionne `PhysicalDisk(_Total)\Disk Bytes/sec` sans dépendre de la langue Windows. La requête PDH reste sur le worker. Premier point, intervalle trop long ou compteur invalide : pas de débit affiché.
- Réseau : `GetIfTable2` donne les compteurs d’octets cumulés. PULSE mesure les deltas sur une horloge monotone et ne somme que les interfaces matérielles Ethernet/Wi-Fi actives. Changement d’interfaces, recul de compteur ou interruption de plus de 15 s : intervalle écarté. Une interface absente n’est pas assimilée à un débit nul.
- GPU : NVML donne la fraction de temps pendant laquelle le GPU exécute du travail, pas une somme WDDM de moteurs. La mémoire occupée peut inclure les réservations du pilote. Le ventilateur donne une consigne en %, pas des tours/minute. Les droits et le pilote peuvent masquer un capteur indépendamment des autres.
- Détail CPU : compteurs PDH `% Processor Time` et `Processor Frequency` par groupe et processeur logique. La fréquence déclarée par le compteur n’est pas une mesure de fréquence effective. Les icônes sont extraites en lecture seule depuis le fichier `.exe` local, puis mises en cache ; elles ne sont pas stockées dans les sessions.
- MEM-01 : fenêtre minimale de 30 min et 20 points ; rejet des trous de plus de deux minutes ; pente médiane de type Theil–Sen sur une projection d’au plus 41 points ; hausse ≥256 Mio et pente ≥64 Mio/h, avec contrôles de plateau et de retour. Seuils non calibrés. Un cache ou un changement de charge peuvent imiter le signal.

## Prochaines validations et travaux

1. Comparer les rendus aux deux PNG de référence lorsqu’ils seront disponibles. Automatiser les comparaisons visuelles avec une tolérance documentée.
2. Mesurer la fluidité (p95) et l’empreinte du release pendant 30 min, ainsi que l’écart de métriques avec Task Manager/PerfMon. Valider DPI natif multi-écran, clavier et Narrateur.
3. Ajouter les groupes de processus sécurisés par leur génération, l’éditeur, la priorité et les historiques individuels rejouables.
4. Étendre SQLite aux événements, apporter l’export et tester les quotas et la récupération après interruption.
5. Intégrer WDDM et les capteurs ; ajouter latence/disques, connexions réseau et attribution lorsque les API le permettent, puis ETW/PresentMon à la demande.

Plan d’origine : [dossier consolidé](../PULSE-ADR-DESIGN-SYSTEM-complet.md), section « Plan de réalisation ».

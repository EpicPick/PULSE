# PULSE · Périmètre produit de la préversion

La spécification fonctionnelle originale figure dans le [dossier consolidé](../PULSE-ADR-DESIGN-SYSTEM-complet.md), section « Spécification fonctionnelle (Product Spec) ». Ce résumé décrit les parcours effectivement exposés.

1. **Performances** : trois métriques prioritaires (CPU, mémoire, GPU) choisissent une courbe principale ; les détails matériels restent dans cette rubrique. La barre latérale relie **Processus, Performances, Jeux et Sessions**.
2. **Processus** : écran de démarrage. Recherche par nom/PID, tri des colonnes (dont le maximum GPU par moteur), filtre CPU, détails à droite et jusqu’à 60 s de mesures depuis la sélection. Le suivi mémoire s’ouvre depuis l’onglet RAM. Terminaison confirmée, sous réserve de droits et de protection Windows.
3. **GPU Inspector** : adaptateurs NVIDIA détectés, VRAM, activité et capteurs disponibles par adaptateur, encodeur et décodeur vidéo. Les éléments non exposés par NVML restent indisponibles, sans valeurs de remplacement.
4. **Memory Leak Hunter** : suivi d’une instance choisie, historique mémoire privé/résident et règle MEM-01 v1. L’onglet Interprétation donne les causes alternatives et rappelle la nécessité d’un profilage.
5. **Sessions** : enregistrement, relecture, export et chronologie. Le diagnostic CPU peut être affiché sans déplacer le curseur de la chronologie. La persistance des noms et identités est optionnelle, et l’export CSV reste agrégé.
6. **Stockage et réseau** : inventaire des disques physiques et volumes avec lettre, activité, débits et latences par disque, espace par volume et données de santé disponibles. Historique live d’une minute par disque, non persisté. Débits Ethernet/Wi-Fi, connexions TCP et ports UDP IPv4/IPv6 associés au PID fourni par IP Helper. Pas de débit par processus ni de noms déduits d’un PID potentiellement recyclé.
7. **Performances** : charge par processeur logique et fréquence reportée par Windows lorsque disponible.
8. **Jeux** : mise en avant de la capture automatique, suivie du rapport ; import CSV et capture ciblée restent dans les options secondaires.

Le panneau **Processus → E/S** affiche lecture et écriture agrégées par instance (`GetProcessIoCounters`). Dans **Diagnostics**, `CPU-01` analyse une suite de mesures valides ; un pic isolé, une mesure absente ou une longue interruption ne déclenchent pas d’alerte de charge soutenue.

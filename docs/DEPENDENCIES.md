# Rune et dépendances natives

Rune et Rune Kit : [`anomalyco/rune@3f0d9973526818bae62c89c483fbd144a2a19669`](https://github.com/anomalyco/rune/commit/3f0d9973526818bae62c89c483fbd144a2a19669), révision vérifiée de `dev`. Ce pin est un instantané de développement reproductible, pas une version stable publiée. `Cargo.lock` fixe la résolution testée.

Sources consultées à ce SHA : `examples/hello`, `examples/shapes`, `crates/rune/README.md`, `crates/rune-kit/README.md`, `crates/rune-kit/examples/table.rs`, les modules `el`, `tooling`, `app` et les builders de Table/TextInput. Capacités employées : layout flex, fenêtres natives winit, rendu vectoriel Rune sur wgpu, champs d’édition, clic et défilement, ressorts, focus, virtualisation paresseuse, `Runtime` headless et captures `--snap`.

Les `[patch.crates-io]` du workspace amont ne sont pas hérités par ses consommateurs. Le workspace PULSE reprend donc les six paquets vendorizés depuis **le même SHA**. Sans ce report, le build échoue dès la résolution de `parley_engine` : son feature `line-dictionaries` n’existe pas dans la version de crates.io. L’activation choisie inclut les polices embarquées et AccessKit. Les options de capture Rune sont présentes dans le build debug ; elles ne sont pas incluses dans le portable release.

`windows` 0.62.2 fournit les bindings Win32. `rusqlite` embarque SQLite compilé par la chaîne MSVC, sans SQLite à installer séparément. Les révisions Rune et les patches doivent être actualisés ensemble ; revalider le démarrage et les captures après changement.

`nvml-wrapper` charge le NVML du pilote NVIDIA à partir du répertoire système Windows. PULSE ne distribue pas cette DLL et ne recherche pas dans son dossier de travail ou dans `PATH`.

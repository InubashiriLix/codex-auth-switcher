# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

Gérez plusieurs comptes Codex depuis un point d’entrée local unique. Codex Switcher conserve des instantanés d’authentification, vérifie les quotas, change de compte en sécurité et fournit un proxy de streaming en boucle locale qui préserve les sessions actives.

## Fonctionnalités

- Importer, renommer, vérifier, activer et supprimer les instantanés locaux.
- Vérifier les fenêtres de quota sans exposer les identifiants.
- Ajouter explicitement les comptes sains au pool du proxy et conserver l’affinité d’identité.
- Changer uniquement aux limites sûres des requêtes et préserver les réglages Codex sans rapport.
- Ne conserver que des métadonnées nettoyées, jamais les prompts, le code, les réponses, les jetons ou les en-têtes d’autorisation.
- Interface en chinois, anglais, japonais, espagnol, allemand, italien et français.

## Installation et démarrage

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

Dans ACCOUNT, `a` importe la connexion actuelle, `r` la vérifie et `Enter` active l’instantané. Utilisez `codex-switcher --proxy` pour le proxy ou `codex-switcher --daemon` pour le service en arrière-plan. Avant le démarrage du proxy, ajoutez au moins un compte sain avec `Space`.

## Langue

Appuyez sur `l` dans l’écran principal ou détaillé, choisissez « Suivre le système » ou l’une des sept langues, puis enregistrez avec `Enter`. `Esc` annule.

```toml
language = "auto" # auto, zh-cn, en, ja, es, de, it, fr
```

La détection automatique consulte `LC_ALL`, `LC_MESSAGES` et `LANG` ; les langues inconnues utilisent l’anglais. L’en-tête affiche la langue effective sous la forme `🌐 [l] Français`.

## Touches principales

`m` change d’espace, `j/k` déplace la sélection, `Space/x` gère le pool et le changement sûr, `r/R` vérifie les comptes, `t` change le thème, `?` ouvre l’aide et `q` quitte.

La configuration se trouve dans `$XDG_CONFIG_HOME/codex-switcher/config.toml` ; les comptes et métadonnées sont sous `$XDG_DATA_HOME/codex-switcher/`. Linux est actuellement la plateforme vérifiée.

## Sécurité et contributions

Utilisez uniquement des comptes qui vous appartiennent ou pour lesquels vous avez une autorisation. Cet outil ne contourne pas les limites du service. Retirez jetons, e-mails, chemins privés, prompts, code et sorties des captures ou journaux partagés.

Avant toute contribution, exécutez `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` et un build release. Licence MIT.

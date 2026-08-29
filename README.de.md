# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

Verwalte mehrere Codex-Konten über einen lokalen Einstiegspunkt. Codex Switcher speichert Authentifizierungs-Snapshots, prüft Kontingente, wechselt Konten sicher und bietet einen Loopback-Streaming-Proxy, der aktive Sitzungen stabil hält.

## Funktionen

- Lokale Snapshots importieren, umbenennen, prüfen, aktivieren und löschen.
- Primäre und sekundäre Kontingentfenster ohne Offenlegung von Zugangsdaten prüfen.
- Gesunde Konten ausdrücklich zum Proxy-Pool hinzufügen und Identitätsbindung erhalten.
- Nur an sicheren Anfragegrenzen wechseln und fremde Codex-Einstellungen bewahren.
- Ausschließlich bereinigte Metadaten speichern; keine Prompts, Quelltexte, Antworten, Tokens oder Authorization-Header.
- Oberfläche auf Chinesisch, Englisch, Japanisch, Spanisch, Deutsch, Italienisch und Französisch.

## Installation und Start

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

Im ACCOUNT-Bereich importiert `a` die aktuelle Anmeldung, `r` prüft sie und `Enter` aktiviert den Snapshot. `codex-switcher --proxy` öffnet den Proxy-Bereich, `codex-switcher --daemon` startet den Hintergrunddienst. Vor dem Proxy-Start muss mindestens ein gesundes Konto mit `Space` in den Pool aufgenommen werden.

## Sprache

Drücke `l` in der Haupt- oder Detailansicht, wähle „Systemeinstellung“ oder eine der sieben Sprachen und speichere mit `Enter`. `Esc` verwirft die Änderung.

```toml
language = "auto" # auto, zh-cn, en, ja, es, de, it, fr
```

Die automatische Erkennung prüft `LC_ALL`, `LC_MESSAGES` und `LANG`; unbekannte Sprachen fallen auf Englisch zurück. Der Kopf zeigt die wirksame Sprache als `🌐 [l] Deutsch`.

## Wichtige Tasten

`m` wechselt den Bereich, `j/k` bewegt die Auswahl, `Space/x` verwaltet Pool und sicheren Wechsel, `r/R` prüft Konten, `t` wechselt das Theme, `?` öffnet Hilfe und `q` beendet.

Die Konfiguration liegt in `$XDG_CONFIG_HOME/codex-switcher/config.toml`, Konten und Laufzeitdaten unter `$XDG_DATA_HOME/codex-switcher/`. Linux ist derzeit die verifizierte Plattform.

## Sicherheit und Beiträge

Nur mit eigenen oder autorisierten Konten verwenden. Dienstlimits werden nicht umgangen. Entferne Tokens, E-Mails, private Pfade, Prompts, Code und Ausgaben aus geteilten Bildern und Protokollen.

Vor Beiträgen `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` und einen Release-Build ausführen. MIT-Lizenz.

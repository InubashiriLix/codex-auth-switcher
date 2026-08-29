# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

Gestisce più account Codex da un unico punto locale. Salva istantanee di autenticazione, controlla le quote, cambia account in sicurezza e offre un proxy streaming su loopback che mantiene stabili le sessioni attive.

## Funzioni

- Importazione, rinomina, controllo, attivazione e rimozione delle istantanee locali.
- Controllo delle finestre di quota senza esporre credenziali.
- Inserimento esplicito degli account sani nel pool proxy con affinità per identità.
- Cambio solo ai confini sicuri delle richieste e conservazione delle impostazioni Codex estranee.
- Memorizzazione dei soli metadati sanificati; mai prompt, codice, risposte, token o header di autorizzazione.
- Interfaccia in cinese, inglese, giapponese, spagnolo, tedesco, italiano e francese.

## Installazione e avvio

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

In ACCOUNT, `a` importa l’accesso corrente, `r` lo controlla e `Enter` attiva l’istantanea. Usa `codex-switcher --proxy` per il proxy o `codex-switcher --daemon` per il servizio in background. Prima di avviare il proxy aggiungi almeno un account sano con `Space`.

## Lingua

Premi `l` nella schermata principale o di dettaglio, scegli “Segui il sistema” o una delle sette lingue e salva con `Enter`. `Esc` annulla.

```toml
language = "auto" # auto, zh-cn, en, ja, es, de, it, fr
```

Il rilevamento automatico usa `LC_ALL`, `LC_MESSAGES` e `LANG`; le lingue non supportate usano l’inglese. L’intestazione mostra la lingua effettiva come `🌐 [l] Italiano`.

## Tasti principali

`m` cambia area, `j/k` sposta la selezione, `Space/x` gestisce pool e cambio sicuro, `r/R` controlla gli account, `t` cambia tema, `?` apre l’aiuto e `q` esce.

Su Linux la configurazione è in `$XDG_CONFIG_HOME/codex-switcher/config.toml` e i dati in `$XDG_DATA_HOME/codex-switcher/`. Windows 10/11 x86_64 usa `%APPDATA%\CodexSwitcher` e `%LOCALAPPDATA%\CodexSwitcher`, con MSI non firmato e servizio LocalService.

## Sicurezza e contributi

Usalo solo con account propri o autorizzati. Non aggira i limiti del servizio. Rimuovi token, email, percorsi personali, prompt, codice e output prima di condividere immagini o log.

Prima di contribuire esegui `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` e una build release. Licenza MIT.

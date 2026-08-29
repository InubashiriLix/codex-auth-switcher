# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

Gestiona varias cuentas de Codex desde un único punto local. Guarda instantáneas de autenticación, comprueba cuotas, cambia de cuenta de forma segura y ofrece un proxy de streaming en loopback que mantiene estables las sesiones activas.

## Funciones

- Importar, renombrar, comprobar, activar y eliminar instantáneas locales.
- Consultar las ventanas de cuota sin mostrar credenciales.
- Añadir explícitamente cuentas sanas al grupo del proxy y mantener afinidad por identidad.
- Cambiar solo en límites seguros de petición y conservar ajustes de Codex no relacionados.
- Guardar únicamente metadatos saneados, nunca prompts, código, respuestas, tokens ni cabeceras de autorización.
- Interfaz en chino, inglés, japonés, español, alemán, italiano y francés.

## Instalación y uso

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

En ACCOUNT, `a` importa el inicio de sesión actual, `r` lo comprueba y `Enter` activa la instantánea. Usa `codex-switcher --proxy` para el proxy o `codex-switcher --daemon` para ejecutarlo en segundo plano. Antes de iniciar el proxy, añade al menos una cuenta sana con `Space`.

## Idioma

Pulsa `l` en la pantalla principal o de detalles, elige “Seguir el sistema” o uno de los siete idiomas y confirma con `Enter`. `Esc` cancela.

```toml
language = "auto" # auto, zh-cn, en, ja, es, de, it, fr
```

La detección automática consulta `LC_ALL`, `LC_MESSAGES` y `LANG`; los idiomas no compatibles usan inglés. La cabecera muestra el idioma efectivo, por ejemplo `🌐 [l] Español`.

## Teclas principales

`m` cambia de espacio, `j/k` mueve la selección, `Space/x` gestiona el grupo y el cambio seguro, `r/R` comprueba cuentas, `t` cambia el tema, `?` abre la ayuda y `q` sale.

La configuración reside en `$XDG_CONFIG_HOME/codex-switcher/config.toml`; las cuentas y los metadatos están en `$XDG_DATA_HOME/codex-switcher/`. Linux es la plataforma verificada actualmente.

## Seguridad y contribuciones

Úsalo solo con cuentas propias o autorizadas. No evita límites del servicio. Elimina tokens, correos, rutas personales, prompts, código y salidas antes de compartir capturas o registros.

Antes de contribuir ejecuta `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` y una compilación release. Licencia MIT.

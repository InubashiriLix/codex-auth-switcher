# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

複数の Codex アカウントを一つのローカル画面で管理します。認証スナップショットの保存、使用量の確認、安全なアカウント切替、セッションを維持するループバック・ストリーミングプロキシを提供します。

## 主な機能

- 認証スナップショットのインポート、名前変更、確認、有効化、削除。
- 認証情報を公開せず、主要・副次の使用量ウィンドウを確認。
- 明示的に選んだアカウントだけをプロキシプールへ追加。
- 安全なリクエスト境界で切替し、無関係な Codex 設定を保持。
- プロンプト、コード、本文、Token、Authorization を保存しないメタデータ専用履歴。
- 中国語、英語、日本語、スペイン語、ドイツ語、イタリア語、フランス語に対応。

## インストールと起動

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

ACCOUNT 画面で `a` は現在のログインを取り込み、`r` は確認、`Enter` は選択したスナップショットを有効化します。プロキシ画面は `codex-switcher --proxy`、常駐実行は `codex-switcher --daemon` です。プロキシ開始前に、正常なアカウントを `Space` でプールへ追加してください。

## 言語

メイン画面または詳細画面で `l` を押し、「システムに従う」または七つの言語を選び、`Enter` で保存します。`Esc` は変更を破棄します。

```toml
language = "auto" # auto, zh-cn, en, ja, es, de, it, fr
```

自動判定は `LC_ALL`、`LC_MESSAGES`、`LANG` の順です。未対応のロケールは英語へフォールバックし、ヘッダーには `🌐 [l] 日本語` の形式で実際の言語が表示されます。

## 主なキー

`m` はワークスペース選択、`j/k` は移動、`Space/x` はプール登録と安全な切替、`r/R` は確認、`t` はテーマ、`?` はヘルプ、`q` は終了です。

設定は `$XDG_CONFIG_HOME/codex-switcher/config.toml`、アカウントと実行データは `$XDG_DATA_HOME/codex-switcher/` に保存されます。現在正式に検証済みのプラットフォームは Linux です。

## セキュリティと貢献

所有または利用許可のあるアカウントだけで使用してください。サービス制限を回避するものではありません。共有するログや画像から Token、メール、ホームパス、プロンプト、コード、出力を削除してください。

変更を送る前に `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、release build を実行してください。ライセンスは MIT です。

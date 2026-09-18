<div align="center">

# CC Switch Usage Plugin

### CC Switch に API Key ごとの使用制限（バジェット）を追加 — ローカルプロキシが強制します

**プラグインバージョン:** v1.0.1 · **ホストアプリ:** CC Switch 3.20.3

[![Release](https://img.shields.io/github/v/release/Xhoryon/ccswitch-usage-plugin?color=blue&label=release)](https://github.com/Xhoryon/ccswitch-usage-plugin/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-lightgrey.svg)](https://github.com/Xhoryon/ccswitch-usage-plugin/releases)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

[English](README.md) | [简体中文](README_ZH.md) | [繁體中文](README_ZH-TW.md) | 日本語

</div>

---

## これは何？

**CC Switch Usage Plugin** は、オープンソースのプロバイダ切り替えツール
[CC Switch](https://github.com/farion1231/cc-switch) にバジェット機能を追加したものです。
すべての API Key に**使用上限** — 金額（USD / CNY）またはトークン数 — を設定でき、
上限に達すると CC Switch のローカルプロキシが新しいリクエストを明確なエラーで
**拒否します**。統計だけの「見かけの制限」ではありません。

## 機能

- **金額制限** — USD または CNY。ローカルの USD→CNY 為替レート（デフォルト 7.2、
  オンライン API 不要）で換算
- **トークン制限** — 内蔵の使用量ダッシュボードと同じ正規化トークン計算を使用するため、
  両者の数値は常に一致します
- **本物の強制執行** — プロキシは転送**前**に予算をチェックします。上限に達すると、
  新しいリクエストは構造化された `usage_limit_reached` エラー（HTTP 429）で即座に失敗。
  すでに送信されたリクエストは最後まで完了するため、最後のリクエストで多少の超過が
  発生するのは正常な設計です
- **プロバイダ名ではなくクレデンシャルに紐付け** — 制限は API Key の SHA-256
  フィンガープリントに紐付き、平文のキーは一切保存されません。キーをローテーションすると
  予算は自動的に新規開始します
- **カードごとのライブステータス** — メーターアイコンの 4 状態
  （オフ / 通常 / 警告 ≥80% / 枯渇）、進捗バー・残額・使用量リセット付きの設定ダイアログ
  （統計ウィンドウを進めるだけで、履歴は削除されません）
- **自動リセット期間** — リセットしない、N 時間ごと、N 日ごとから選択できます。
  リセットは各 Switch 内で遅延実行され、使用量の履歴は削除されません
- **誤解を招かない設計** — トラフィックがプロキシを経由しない場合やモデルの価格が
  未登録の場合、UI が明示します。制限が機能していないふりはしません
- **対応言語** — 简体中文、繁體中文、English、日本語

## インストール

1. [Releases](https://github.com/Xhoryon/ccswitch-usage-plugin/releases) ページから
   `CC-Switch-3.20.3-Usage-Plugin-v1.0.1-macOS-arm64.dmg`（macOS、Apple Silicon）をダウンロード。
2. DMG をマウントし、**CC Switch.app** をアプリケーションにドラッグします。

## クイックスタート

1. CC Switch を開き、**Claude / Codex / Gemini / Grok Build** に切り替えます。
2. プロバイダカードにカーソルを合わせ、**メーターアイコン**（使用制限）をクリック。
3. トグルをオンにし、**金額**または**トークン**を選択、上限を入力して保存。
4. 該当アプリのローカルルート接管を有効にすると、制限の強制が始まります。
5. 「リセットしない」「N 時間ごと」「N 日ごと」から自動リセット期間を選択できます。
   手動の「使用量をリセット」も統計の起点を現在に進めるだけで、ダッシュボードの履歴は
   完全に保持されます。

## 重要な境界線（必ずご確認ください）

- 制限は **CC Switch ローカルプロキシを経由するトラフィック**にのみ適用されます。
  直接接続は観測も遮断もできません（強制できない場合は UI が明示します）。
- 使用量は**ローカルでの観測値**です。プロバイダアカウントの全デバイス合計の
  グローバル割り当てではありません。
- 価格が未登録のモデルではダイアログに明示的な警告が表示され、リクエストを安全な残額
  として黙って扱いません。CC Switch でカスタムモデル価格を設定できます。

## リリースノート

- [v1.0.1 — English](docs/release-notes/v1.0.1-en.md)
- [v1.0.1 — 简体中文](docs/release-notes/v1.0.1-zh.md)
- [v1.0.1 — 繁體中文](docs/release-notes/v1.0.1-zh-TW.md)
- [v1.0.1 — 日本語](docs/release-notes/v1.0.1-ja.md)

本リリースは署名なしの macOS Apple Silicon ビルドです。アプリのバージョンは CC Switch
3.20.3 のままで、v1.0.1 は Usage Plugin のリリースバージョンです。

## ソースからビルド

```bash
pnpm install
pnpm build        # 利用可能な Rust ツールチェーン（stable）が必要です
```

アーキテクチャと既知の制限（中国語）：[docs/development_log.md](docs/development_log.md)。

## クレジット

本プロジェクトは [@farion1231](https://github.com/farion1231) によるオープンソース
プロジェクト [**CC Switch**](https://github.com/farion1231/cc-switch)（MIT）を
ベースに構築されています — Claude Code / Codex / Gemini CLI などのための優れた
オールインワンマネージャーです。切り替え・プロキシ・使用量の基盤インフラの功績は
上流の作者とコントリビュータに帰属します。公式サイト：[ccswitch.io](https://ccswitch.io)。

使用制限機能自体はこのリポジトリで開発されました。

## ライセンス

[MIT](LICENSE) © 2026 Jiayi Huang — オリジナル CC Switch の MIT 表示を含みます。

# CC Switch 使用者手冊（Usage Plugin 補充）

> 本頁提供 CC Switch Usage Plugin v1.0.1 的繁體中文使用說明；宿主應用程式版本為 CC Switch 3.20.3。

## 安裝

從 GitHub Releases 下載：

`CC-Switch-3.20.3-Usage-Plugin-v1.0.1-macOS-arm64.dmg`

掛載 DMG 後，將 **CC Switch.app** 拖入「應用程式」。本版本為未簽名且未公證的 macOS Apple Silicon 建置，
系統可能會顯示一般的 Gatekeeper 安全提示。

## 設定使用限額

1. 開啟 CC Switch，切換至 Claude、Codex、Gemini 或 Grok Build。
2. 將游標停留在供應商卡片上，點擊儀表板圖示（Usage Limit）。
3. 開啟限額，選擇金額（USD/CNY）或 Token，輸入上限後儲存。
4. 開啟該應用程式的本地路由接管，限額才會對經過本地代理的流量強制執行。

## 自動重置

在「自動重置週期」中可選擇：

- 永不重置
- 每 N 小時
- 每 N 天

重置只會在每台 Switch 本地惰性推進統計視窗，不會刪除用量歷史，也不會同步到其他 Switch。

## Usage Duo 邊界

- 同一 API Key 身分可在獨立 Switch 之間穩定識別，但這不是節點配對或雲端同步。
- 每台 Switch 維護自己的本地預算帳本；直連 Provider 的流量無法被本地代理觀測或攔截。
- OAuth/託管憑證沒有靜態 API Key，不會進入 API Key 預算帳本。

## 其他語言與 Release Notes

- [繁體中文 README](../../../README_ZH-TW.md)
- [v1.0.1 Release 說明](../../release-notes/v1.0.1-zh-TW.md)
- [English](../en/README.md) · [简体中文](../zh/README.md) · [日本語](../ja/README.md)

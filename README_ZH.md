<div align="center">

# CC Switch Duo Usage Plugin

### 为开源的 [CC Switch](https://github.com/farion1231/cc-switch) 增加本地预算守卫

**插件版本：** v1.0.2 · **宿主应用：** CC Switch 3.20.3

[![Release](https://img.shields.io/github/v/release/Xhoryon/ccswitch-usage-plugin?color=blue&label=release)](https://github.com/Xhoryon/ccswitch-usage-plugin/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-lightgrey.svg)](https://github.com/Xhoryon/ccswitch-usage-plugin/releases)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

[English](README.md) | 简体中文 | [繁體中文](README_ZH-TW.md) | [日本語](README_JA.md)

</div>

---

## 这是什么？

**CC Switch Duo Usage Plugin** 是为开源供应商切换工具
[CC Switch](https://github.com/farion1231/cc-switch) 提供的轻量、本地扩展。它增加了预算功能：
每个 API Key 都可以设置**使用上限**——金额（USD / CNY）或 Token 数量。达到上限后，
CC Switch 本地代理会**直接拒绝新的请求**并返回明确错误，而不是只做统计。

## 功能特性

- **金额限额** — USD 或 CNY，通过可修改的本地 USD→CNY 汇率换算（默认 7.2，不联网）
- **Token 限额** — 与内置用量面板相同的标准化 Token 统计口径，两边数字永远一致
- **真正的强制执行** — 代理在**转发前**检查预算；达到限额后新请求快速失败并返回结构化的
  `usage_limit_reached` 错误（HTTP 429）。已发出的请求允许正常完成，因此最后一个请求
  存在少量超出属正常设计
- **绑定凭证而非供应商名称** — 限额绑定到 API Key 的 SHA-256 指纹；绝不存储明文 Key，
  更换 Key 后预算自动重新计算
- **每张卡片的实时状态** — 仪表盘图标四态（关闭 / 正常 / 告警 ≥80% / 耗尽），
  配置对话框含进度条、剩余额度与用量重置（统计窗口前移，历史数据永不删除）
- **自动重置周期** — 可选择永不重置、每 N 小时或每 N 天；重置只在每台 Switch
  本地惰性推进统计窗口，不删除用量历史
- **诚实不误导** — 流量未经过代理或模型缺少定价时，界面会明确提示，不假装限额已生效
- **多语言** — 简体中文、繁體中文、English、日本語

## 下载安装

1. 从 [Releases](https://github.com/Xhoryon/ccswitch-usage-plugin/releases) 页面下载
   `CC-Switch-3.20.3-Usage-Plugin-v1.0.2-macOS-arm64.dmg`（macOS，Apple Silicon）。
2. 挂载 DMG，将 **CC Switch.app** 拖入"应用程序"。

## 快速上手

1. 打开 CC Switch，切换到 **Claude / Codex / Gemini / Grok Build**。
2. 悬停供应商卡片，点击**仪表盘图标**（使用限额）。
3. 打开开关，选择**金额**或 **Token**，输入上限并保存。
4. 为该应用开启本地路由接管后，限额开始强制执行。
5. 可选择「永不重置」「每 N 小时」或「每 N 天」自动重置；手动「重置使用量」
   也只是把统计起点推进到当下，用量面板中的历史数据完整保留。

## 重要边界（请务必了解）

- 限额仅对**经过 CC Switch 本地代理的流量**生效；直连流量无法观测也无法拦截
  （无法强制执行时界面会明确提示）。
- 用量是**本机观测值**——它不是 Provider 账户在你所有设备上的全局配额。
- 模型缺少定价时，对话框会显式警示，不会静默地把请求当作安全额度；可在 CC Switch
  中配置自定义模型价格。

## Release 说明

- [v1.0.2 — English](docs/release-notes/v1.0.2-en.md)
- [v1.0.2 — 简体中文](docs/release-notes/v1.0.2-zh.md)
- [v1.0.2 — 繁體中文](docs/release-notes/v1.0.2-zh-TW.md)
- [v1.0.2 — 日本語](docs/release-notes/v1.0.2-ja.md)

本版本是未签名的 macOS Apple Silicon 构建。应用版本仍为 CC Switch 3.20.3，
v1.0.2 是 Usage Plugin 的版本号。

## 从源码构建

```bash
pnpm install
pnpm build        # 需要可用的 Rust 工具链（stable）
```

架构说明与已知限制（中文）：[docs/development_log.md](docs/development_log.md)。

## 致谢

本项目基于 [@farion1231](https://github.com/farion1231) 的开源项目
[**CC Switch**](https://github.com/farion1231/cc-switch)（MIT）构建——一个优秀的
Claude Code / Codex / Gemini CLI 全能管理工具。底层的切换、代理与用量基础设施
归功于上游作者及所有贡献者。原项目官网：[ccswitch.io](https://ccswitch.io)。

使用限额功能本身由本仓库开发。

## 许可证

[MIT](LICENSE) © 2026 CC Switch Duo contributors —— 含原 CC Switch 的 MIT 声明。

# VoxKey

VoxKey 是一个面向 Windows 10/11 的本地优先语音输入工具：按住快捷键说话，松开后本地识别，并通过剪贴板 + `Ctrl+V` 把文字插入当前聚焦的输入框。可选接入任意 OpenAI 兼容接口做**极度保守**的识别纠错。

- 纯 Rust，原生 Win32 + egui 悬浮窗，无 Electron / WebView。
- 本地识别使用 sherpa-onnx SenseVoice 多语言模型，默认简体中文，支持 English、繁體中文、日本語、한국어。
- 首次运行自动下载模型到本机缓存；之后可完全离线运行。
- Release 构建为单个 `VoxKey.exe`，绿色免安装，不使用 MSIX / MSI / 安装程序。

## 快速开始

1. 从 [Releases](https://github.com/katriyda/VoxKey/releases) 下载 `VoxKey.exe`，双击运行。程序无主窗口，常驻系统托盘。
2. 首次运行会下载 SenseVoice int8 模型（约 229 MB），底部悬浮窗显示进度与速率；支持断点续传与镜像回退。网络受限时可手动导入，见 [docs/offline-model.md](docs/offline-model.md)。
3. 默认按住**右 Ctrl**说话，松开后自动识别并插入。托盘 → `设置…` 可改为 `Ctrl + Shift + 空格`、`Fn / F13`，或任意受支持的组合（如 `ctrl+alt+space`、`f13`、`right_alt`）。

> Fn 键在多数笔记本上由键盘固件消费，Windows 全局钩子无法可靠监听；右 Ctrl 是可靠性优先的默认方案。细节见 [docs/usage.md](docs/usage.md#关于-fn-键)。

## 功能概览

- 全局按住说话；`cpal` 采集后自动重采样到 16 kHz 单声道，本地 SenseVoice int8 识别。
- 录音期间约每 1.3 秒刷新一次预览文字，松键后再做最终识别。
- 插入走剪贴板快照 → 写入 Unicode 文本 → `Ctrl+V` → 延时恢复，不破坏中文输入法状态。
- 可选 LLM 纠错：只修复明显的同音/近音错误，代码侧有长度与编辑距离守卫，任何异常一律回退原文。
- API Key 以 Windows DPAPI 密文保存；托盘提供语言切换、LLM 开关、设置与检查更新。
- 应用内更新会校验大小、PE 头、SHA-256 与版本号后再替换自身。

完整功能表与实现细节见 [docs/features.md](docs/features.md)。

## 配置与数据

| 内容 | 默认位置 | 覆盖环境变量 |
| --- | --- | --- |
| 设置文件 | `%APPDATA%\VoxKey\config.json` | `VOXKEY_DATA_DIR` |
| 模型缓存 | `%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\` | `VOXKEY_MODEL_DIR` |
| 手动导入目录 | `%LOCALAPPDATA%\VoxKey\models\inbox\` | `VOXKEY_IMPORT_DIR` |
| 错误日志 | `%APPDATA%\VoxKey\voxkey.log` | `VOXKEY_DATA_DIR` |

API Key 不会以明文写入配置文件；配置字段、LLM 约束与剪贴板限制见 [docs/configuration.md](docs/configuration.md)。

## 构建

本仓库**禁止本地编译**，所有构建在 GitHub Actions 的 `windows-2022` Runner 上执行：

```text
Actions → Build VoxKey → Run workflow
```

推送 `v*` tag 会自动构建并创建 Release。工作流、`build.rs` 资源嵌入与产物要求见 [docs/build.md](docs/build.md)。

## 安全提示（未签名）

当前所有 Release 都**没有 Authenticode 签名**，SmartScreen“未知发布者”与部分杀软的启发式误报属于预期现象。只从 Releases 下载并核对发布说明中的大小与 SHA-256；误报原因与放行步骤见 [docs/security.md](docs/security.md)。

## 文档

- [docs/README.md](docs/README.md) — 完整文档索引
- [docs/offline-model.md](docs/offline-model.md) — 离线模型手动下载与导入
- [docs/known-limitations.md](docs/known-limitations.md) — 已知限制
- [docs/verification.md](docs/verification.md) — 验证记录

## 许可与隐私

- 代码：MIT，见 [LICENSE](LICENSE)。
- 字体 Noto Sans CJK SC：SIL OFL 1.1，见 [assets/OFL.txt](assets/OFL.txt)。
- OpenCC 简体→繁体映射数据：Apache-2.0；SenseVoice 模型与 sherpa-onnx 遵循上游许可证。详见 [NOTICE](NOTICE)。
- 音频只在本机识别；仅当启用 LLM 纠错时，识别后的**文本**才会发送到所配置的接口；不收集遥测。

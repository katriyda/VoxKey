# 功能与实现

VoxKey 的全部功能一览，以及每项功能对应的实现方式。

| 功能 | 实现 |
| --- | --- |
| 全局按住说话 | `WH_KEYBOARD_LL` 低级键盘钩子，可选自定义组合键 |
| 录音 | `cpal` 默认麦克风，自动重采样到 16 kHz 单声道 |
| 识别 | `sherpa-onnx` OfflineRecognizer + SenseVoice int8，运行在独立进程 `VoxKey-asr.exe`（stdin/stdout 协议） |
| 实时文字 | 录音过程中每 250 ms 刷新一次预览：按约 280 ms 的停顿切分，已结束的句子只识别一次并保留，仅对正在说的句子重新识别（最长 8 秒强制切分），使延迟不随录音变长而增长；松键后再对整段做最终识别 |
| 悬浮窗 | egui/eframe 无边框透明置顶窗口，底部居中，RMS 驱动的 5 根波形，弹性宽度，淡入淡出 |
| 插入 | 快照原剪贴板 → 写入 Unicode 文本 → 模拟 `Ctrl+V` → 延时恢复原剪贴板 |
| 纠错 | OpenAI-compatible `/chat/completions`；严格系统提示词 + 本地编辑距离/长度守卫 |
| 托盘 | 语言切换、LLM 开/关、Settings、退出、检查更新 |
| 设置 | 界面语言、识别语言、快捷键、Base URL、API Key、Model、Test、Save |
| 界面字体 | 运行时读取 `C:\Windows\Fonts`（Segoe UI + 微软雅黑/宋体），不占用 exe 体积；无中文字体时自动英文；`--features embed-cjk-font` 可回退到内嵌 Noto CJK |
| 双语界面 | `src/i18n.rs`，中文/英文逐条内联，`auto` 跟随字体可用性 |
| 密钥安全 | API Key 使用 Windows DPAPI（CurrentUser）加密后写入配置文件；清空后不保留密文 |
| 发布资源 | `build.rs` 调用 MSVC `rc.exe` 嵌入图标、`VERSIONINFO` 与 `asInvoker` 清单（`assets/voxkey.rc`、`assets/voxkey.manifest`） |

支持的语言：简体中文（默认）、English、繁體中文、日本語、한국어。

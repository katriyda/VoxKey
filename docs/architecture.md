# 架构建档

## 技术栈

- Rust 2024（MSRV 1.95，CI 与 `mise.toml` 固定 1.99.1）
- `windows` 0.62（Win32 钩子、托盘、剪贴板、SendInput、DPAPI）
- `eframe` / `egui` 0.37（悬浮窗与设置）
- `cpal` 0.18（麦克风）
- `sherpa-onnx` 2.0（本地 ASR，仅 `voxkey-asr` worker 依赖）
- `tokio` + `reqwest` 0.13（SChannel/native-tls，模型下载与 LLM 请求）
- `serde` / `serde_json`（配置、API JSON 与 worker 协议）

## 进程模型

`VoxKey.exe` 只做 UI、全局钩子、托盘、剪贴板、设置与下载；SenseVoice 推理在独立进程 `VoxKey-asr.exe` 中运行，因此 UI 本体不含 sherpa-onnx 与 onnxruntime（约 19 MB）。worker 与模型一样按需下载到 `%LOCALAPPDATA%\VoxKey\bin\`，启动方式为继承 stdin/stdout 管道并加 `CREATE_NO_WINDOW`，协议是逐行 JSON 加长度已知的 `f32` PCM 数据（16 kHz 单声道）；worker 在管道 EOF 时自行退出，UI 被杀不会留下孤儿进程。识别质量与原进程内方案完全一致：同一模型、同一解码代码。

录音期间的预览由 UI 侧增量驱动：每 250 ms 检查一次新音频，按约 280 ms 的停顿切分，已结束的句子只送 worker 识别一次并拼接保留，只对当前未结束的句子重复识别（超过 8 秒强制切分）。这样每帧的识别成本只与当前句子长度有关，不会像“整段重跑”那样随录音时长线性增长；松键后的最终识别仍使用完整录音。

## 模块结构

```text
src/main.rs        eframe 启动、窗口参数
src/app.rs         状态机、悬浮窗显示/隐藏/定位、托盘事件、设置窗口
src/engine.rs      录音线程：录音、预览识别（经 worker）、最终识别、LLM、插入
src/audio.rs       cpal 采集与 16 kHz 线性重采样
src/worker.rs      语音引擎进程：下载/校验、启动、stdin/stdout 协议
src/model.rs        SenseVoice 首次下载（HF / hf-mirror，断点续传 + 停滞超时 + SHA-256 校验）与离线导入
src/sha256.rs       模型文件的纯 Rust SHA-256 校验（不引入额外 crypto 依赖）
src/platform.rs    WH_KEYBOARD_LL、托盘菜单、隐藏窗口、NoActivate/工作区
src/clipboard.rs   剪贴板快照/恢复、SendInput Ctrl+V
src/llm.rs         OpenAI-compatible 纠错、保守守卫、连接测试
src/config.rs      配置读写（JSON）
src/secrets.rs     Windows DPAPI 密钥保护
src/overlay.rs     胶囊悬浮窗绘制与动画
src/settings.rs    Settings UI
src/fonts.rs       系统字体加载（Segoe UI + 微软雅黑/宋体），`embed-cjk-font` 可回退到内嵌
src/i18n.rs        双语字符串（自动 / 中文 / English）
src/zh.rs          简体→繁体单字转换
src/hotkey.rs      快捷键解析
src/update.rs      应用内更新：GitHub Release 检查、下载校验、替换重启
crates/voxkey-asr/ SenseVoice worker（sherpa-onnx + 静态 onnxruntime）
build.rs           Win32 资源：图标、VERSIONINFO、asInvoker 清单（调用 rc.exe）
```

## 悬浮窗窗口约束

- OS 窗口常驻可见，但胶囊内容隐藏时窗口完全透明。这是为了避开 winit 隐藏窗口在 Windows 上不再响应跨线程重绘请求的问题；否则应用空闲约 10 秒后第一次按热键可能没有反应。
- 鼠标穿透由扩展样式 `WS_EX_LAYERED | WS_EX_TRANSPARENT` 加 `SetLayeredWindowAttributes(..., 255, LWA_ALPHA)` 实现（`platform::configure_overlay_window`）。
  - **不要**退回窗口过程里返回 `HTTRANSPARENT` 的写法：系统只把鼠标消息交给同一线程内下方的窗口，其他进程的窗口（桌面、资源管理器、用户正在操作的任何程序）永远收不到落在悬浮窗上的点击，用户看到的是“胶囊被一层透明的东西挡住，点不穿”。`WS_EX_TRANSPARENT` 的命中测试跳过依赖分层窗口，所以这两个样式必须成对设置。
  - **不要**直接用 eframe 的 `with_mouse_passthrough(true)` 而不同时设置分层属性：winit 会加 `WS_EX_LAYERED` 却不调用 `SetLayeredWindowAttributes`/`UpdateLayeredWindow`，结果是窗口仍有正确尺寸与内容（`PrintWindow` 能看到胶囊），但桌面合成器完全不绘制它——悬浮窗在屏幕上彻底不可见，波浪动画与实时预览文字也就永远看不到。本仓库手动设置样式并补上分层属性后，上述合成问题不再出现（胶囊隐藏时窗口区域与桌面逐像素一致，显示时胶囊正常绘制）。
- 相关排查记录见 [verification.md](verification.md)。

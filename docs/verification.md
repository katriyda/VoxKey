# 验证记录

这里记录 CI 产物与真机验证的真实状态；未实测项目会被明确标注，不把“应该能用”写成“已验证”。

## 验证状态（2026-11-10）

最终 Release 产物由 GitHub Actions 的 `windows-2022` Runner 构建，提交 `cfec8ec`，运行
[Build VoxKey #42804184486](https://github.com/katriyda/VoxKey/actions/runs/42804184486) 全部通过：

| 检查 | 结果 |
| --- | --- |
| `cargo test --release --locked` | ✅ 17 个单元测试全部通过 |
| `cargo build --release --locked` | ✅ |
| `dumpbin /dependents` 检查 | ✅ 没有 onnxruntime / sherpa / VCRUNTIME / MSVCP 依赖 |
| `VoxKey.exe --version` 冒烟测试 | ✅ |
| 单文件 artifact 上传 | ✅ `VoxKey.exe`，约 40 MB（2026-11-11 体积优化后改为两个资产，见文末） |

最终 exe SHA-256（Actions run 42817193269，源码 commit `fdbb448`）：

```text
4b75fe6544c553b9b6aef251c843936893f3a0c47ddcb2741e197af6cdefbc5e
```

2026-11-11 的悬浮窗可见性修复与 `v0.1.0` 发布构建同样由 CI 生成，运行
[Build VoxKey #42867746307](https://github.com/katriyda/VoxKey/actions/runs/42867746307)，`v0.1.0` Release 附带的 exe SHA-256：

```text
a8d1e2bc9b6d335c783b5398b523465c11f481905abe93ec037b0c4f6670f591
```

在本机（当前受限的低完整性桌面会话）做过以下运行时验证：

- 模型首次下载完成，文件大小与预期完全一致；
- SenseVoice 离线识别：官方 zh/en/ja/ko/yue 测试音频全部识别正确；
- 真实声学链路：用扬声器播放官方中文测试音频，Realtek 麦克风阵列采集后识别为“开饭时间早上9点至下午5点。”；
- 完整热键链路：右 Ctrl → 录音 → 识别 → 剪贴板注入 → 目标文本框收到“开饭时间早上9点至下午5点。” → 原剪贴板哨兵值被恢复；
- 悬浮窗在录音时显示，扩展样式 `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`，尺寸约 329×92，底部居中，获得焦点前后均未抢走前台输入框；
- 空闲 9 秒后再按热键仍能正常触发（修复隐藏窗口丢失重绘请求的问题后）；
- 悬浮窗在真实桌面上可见：录音过程中桌面截图能直接看到胶囊、实时预览文字与波形（修复 `WS_EX_LAYERED` 导致的不可见问题后，见下文）；
- 静音/环境噪声不再插入“我。”或“。”这类幻觉结果，而是提示“没有检测到语音”；
- 错误写入 `%APPDATA%\VoxKey\voxkey.log`。

**仍未实测**：物理 Fn 键（各机型固件差异）、真实人声而非扬声器回放、托盘图标与菜单点击、Settings 窗口交互、真实 LLM 端点、干净 Windows 10 机器上的首次运行。当前会话是低完整性进程，`Shell_NotifyIconW` 返回 Win32 5（拒绝访问），且无法向普通完整性窗口注入测试按键；这些限制来自测试沙箱，不是应用逻辑。普通非提权用户会话应能正常使用托盘；若托盘不出现，请确认 Explorer 正在运行，并避免以管理员身份启动。

依赖升级与真机回归的详细记录见 [device-test-2026-11-11.md](device-test-2026-11-11.md)。

## 悬浮窗可见性修复（2026-11-11）

用户反馈“没有看到动画、文字也没有实时写上去”。排查结果是整条链路都在正常工作——录音、1.3 秒一次的预览识别、`Transcript` 事件、每帧动画状态、`PrintWindow` 内容全部正确——但**悬浮窗从未被桌面合成器绘制出来**，所以屏幕上一片空白。

根因：`main.rs` 里的 `with_mouse_passthrough(true)` 会让 winit 给窗口加上 `WS_EX_LAYERED`（`window_state.rs` 中 `IGNORE_CURSOR_EVENT` 分支），却从不设置 layered 属性。一个没有属性的 layered 窗口不会被合成，窗口存在、内容在渲染、`IsWindowVisible` 为真，但屏幕上什么都看不到。

修复：去掉 `with_mouse_passthrough`，改为在窗口过程里对 `WM_NCHITTEST` 返回 `HTTRANSPARENT`，既保留真实的逐像素透明（DWM blur-behind），又保持鼠标穿透。

## 体积优化：UI 与引擎拆分（2026-11-11）

目标是“本体 exe 要小，模型/引擎可以下载”，同时不降低识别质量（仍是 SenseVoice int8，同一套解码代码）。改动与本地实测：

| 改动 | 效果 |
| --- | --- |
| 移除内嵌 `NotoSansCJKsc-Regular.otf`，改为运行时读取 `C:\Windows\Fonts`（Segoe UI + 微软雅黑/宋体），无中文字体回退英文 | −16.4 MB |
| 关闭 `eframe` 的 `default_fonts` | −1.5 MB |
| SenseVoice 推理移入独立进程 `VoxKey-asr.exe`（sherpa-onnx + 静态 onnxruntime） | UI −19.3 MB |
| `opt-level = "z"` + `lto = "fat"` | 约 −1 MB |

本地 `cargo build --release`（仅用于体积测量，发布产物仍由 CI 生成）：

```text
target/release/VoxKey.exe       5,304,832 字节   （41,738,240 → 5.3 MB，−87%）
target/release/VoxKey-asr.exe  19,258,880 字节   （首次运行自动下载）
```

- `VoxKey.exe` 内已无 `onnxruntime` 字符串（`grep -a -c onnxruntime` = 0）；`sherpa` 仅剩模型仓库常量。
- worker 协议实测（真实 int8 模型）：`load` → `{"ok":true}`；`transcribe` 3200 个静音采样 → `{"ok":true,"text":"。"}`（UI 侧的“无有效文字”守卫会拒绝该幻觉结果）；`shutdown` → `{"ok":true}`。
- `cargo test --release --locked --workspace`：UI 42 个 + worker 3 个测试全部通过。
- 首次运行的磁盘占用：UI 5.3 MB + 引擎 19.3 MB + 模型 229.5 MB ≈ 254 MB（原单文件方案为 471 MB）。

**未实测**：真机 GUI 端到端（沙箱内无法显示悬浮窗/托盘）、CI 产物、无中文字体的英文回退界面、Windows 10 干净机首次运行。CI 已加入 UI 体积上限断言（≤16 MB）与两个 exe 的 DLL 依赖检查。

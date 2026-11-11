# VoxKey 真机测试记录：2026-11-11 依赖升级回归

> 本文件记录一次针对依赖大版本升级（Rust 1.99.1 / egui 0.37 / cpal 0.18 / reqwest 0.13 / windows 0.62 / sherpa-onnx 2.0.1）的真机回归过程与结论；README 只保留功能、用法与已知限制。

## 验证状态（2026-11-11 依赖升级真机回归）

依赖与工具链升级（edition 2024、Rust 1.99.1、egui/eframe 0.37、cpal 0.18、reqwest 0.13、
windows 0.62、sherpa-onnx 2.0.1）后，在本机 Windows 11（build 28000，Realtek 麦克风阵列）
做了一轮真机回归。测试通过 PowerShell + `SendInput` 驱动、用 SAPI 中文语音经扬声器播放，
由麦克风真实采集：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` / `cargo clippy -D warnings` | ✅ 干净，0 警告 |
| `cargo test --release --locked` | ✅ 21 项通过 |
| `cargo build --release --locked`（Rust 1.99.1） | ✅ 单文件 `VoxKey.exe`，42,086,400 字节 |
| 干净产物 SHA-256 | `869be53190e5fbf6933e78a1cd7b79026a6eae2eedfda2873ae155b2e4ae0a79` |
| `--version` 冒烟 | ✅ 退出码 0 |
| 冷启动 + SenseVoice 模型加载 | ✅ 引擎 1.6–1.9 s 就绪（模型已缓存，未触发下载） |
| 按住说话→录音→识别→插入（4 轮） | ✅ 全部识别为“今天天气不错，我们来测试一下语音输入的效果。” |
| 实时预览（1.3 s 一次） | ✅ 递进刷新：今天天气不。→…→完整句 |
| 悬浮窗（egui 0.37 渲染） | ✅ 录制时在真实桌面可见；宽度随文字弹性变化 326→706 px；5 根 RMS 波形；结束态标题/详情正确 |
| 空录音守卫 | ✅ 只显示“没有检测到语音，请按住快捷键说话”，不再输出幻觉字符 |
| 剪贴板写入 → `Ctrl+V` → 恢复哨兵 | ✅ 每轮都恢复原剪贴板 `OLD-CLIP-SENTINEL-42` |
| LLM 本地 mock（OpenAI 兼容） | ✅ 旧模型 id `gpt-4o-mini` 请求时已迁移为 `gpt-5.4-mini`；`max_tokens` 被 400 拒绝后自动以 `max_completion_tokens` + `temperature: 0` 重试成功；纠错“我们→咱们”通过保守守卫并插入 |
| 真实 TLS 端点（`https://api.openai.com/v1`，无密钥） | ✅ reqwest 0.13 + SChannel 握手与请求正常，拿到真实 HTTP 401；错误只作提示，原文照常插入 |

证据截图保存在本地 `.agents/qa/e2e/evidence/`（`listening-visible.png` 为录制中的桌面实拍，
`error-pill.png` 为静音提示）。

### 本轮真机测试顺带修掉的两个悬浮窗问题

1. **弹出后卡住一个白框（含标题栏与关闭按钮）**：`egui/eframe 0.37` 下 winit 会把悬浮窗建成
   “带标题栏的普通窗口”（`WS_CAPTION | WS_THICKFRAME | WS_SYSMENU`），于是屏幕底部常驻一个
   白底带 × 的空框。修复：`platform::configure_overlay_window` 除扩展样式外同时强制
   `GWL_STYLE` 为无边框 `WS_POPUP`（去掉 caption/thickframe/sysmenu/minmax），并把
   `WS_EX_LAYERED` 一并清掉。
2. **提示气泡淡出后又自己淡入，永不消失**：窗口改为常驻可见后，`Overlay::advance` 在淡出结束时
   只把 alpha 归零、没清空标题/详情，下一帧的淡入分支又把气泡拉回来。修复：淡出完成时清空
   `title`/`detail`，保持 alpha=0 直到下一次 `present`。

修复后实测：静置 10 秒以上，托盘错误提示淡出后不再复现白框；录制中胶囊仍然可见。

**本轮未能在自动化会话里验证的两项**（受测试沙箱限制，不是本次升级的代码问题）：

- **托盘图标**：`Shell_NotifyIconW` 仍返回 Win32 5（拒绝访问）。该会话里由工具启动的所有
  进程都处在带 UI 限制的作业对象中，向沙箱外的 Explorer / 普通窗口注入输入无效——同一套
  `SendInput` 在沙箱内窗口可用、对沙箱外窗口无效，可复现。
- **`Ctrl+V` 落到外部输入框**：剪贴板侧（写入、恢复）已实测通过，但注入的按键不会到达沙箱外
  的记事本，故“记事本收到文字”无法在本会话成立。普通用户桌面会话（双击启动）不受此限；
  这一项仍建议在目标机器上人工确认一次。

## 产物

- 提交：`583d6ec`（悬浮窗白框与气泡不复位修复）及其前的依赖升级提交 `3822407` / `064706a`
- 构建：Rust 1.99.1，`cargo build --release --locked`
- `VoxKey.exe` 42,086,400 字节，SHA-256 `869be53190e5fbf6933e78a1cd7b79026a6eae2eedfda2873ae155b2e4ae0a79`
- 本地证据：`.agents/qa/e2e/evidence/`（`listening-visible.png`、`error-pill.png`）与各轮
  `trace.log` / `report.txt`


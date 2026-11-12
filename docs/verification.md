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

修复：去掉 `with_mouse_passthrough`，改为在窗口过程里对 `WM_NCHITTEST` 返回 `HTTRANSPARENT`，既保留真实的逐像素透明（DWM blur-behind），又保持鼠标穿透。（该穿透只对同一线程内的窗口有效，对其他进程的窗口无效；完整修复见下文“悬浮窗鼠标穿透修复”。）

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

## 悬浮窗鼠标穿透修复（2026-11-12，0.2.4）

用户反馈：白线修好后胶囊区域仍“被一层透明的东西挡住”，点不到下面的窗口。根因是上一版用
`WM_NCHITTEST` 返回 `HTTRANSPARENT` 实现穿透——系统只会把鼠标消息转给**同一线程**下方的窗口，
桌面、资源管理器和其他进程的窗口收不到，悬浮窗实际吞掉了整个 360×92 区域（含透明边距）的所有点击。

修复：`platform::configure_overlay_window` 改为扩展样式 `WS_EX_LAYERED | WS_EX_TRANSPARENT`
并调用 `SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)`；不再在窗口过程里处理
`WM_NCHITTEST`（保留 `WM_NCCALCSIZE` 的整窗客户区修复）。

本机（1024×768 桌面，胶囊位于 `(283,594)-(741,686)`）用真实鼠标事件实测，同一套脚本分别跑修复前后
的两个 exe，命中目标是一块置于胶囊正下方、被提到置顶层的 WinForms 窗口：

| 检查 | 修复前 | 修复后 |
| --- | --- | --- |
| 扩展样式 `GWL_EXSTYLE` | `0x8000098`（无 LAYERED/TRANSPARENT） | `0x80800B8` |
| `WindowFromPoint(512,640)` | 返回悬浮窗自身 | 返回下方 WinForms 窗口 |
| 在胶囊中心真实点击，下方窗口收到 `MouseDown` | ❌ 未收到 | ✅ 收到（`button=Left`） |
| 胶囊隐藏时悬浮窗区域像素 | — | 与无悬浮窗基线逐像素一致（均值 `(255,0,255)`） |
| 胶囊显示时截图 | — | 胶囊、波形、实时预览文字正常；无白线、无黑框 |

补充说明：

- 本机 `SendInput` 的鼠标注入被会话丢弃（返回值成功但事件不到达），改用 `mouse_event` 注入后目标窗口
  正常收到点击；键盘注入仍会被应用自身的 `LLKHF_INJECTED` 过滤，因此测试用“模型目录缺失”触发下载胶囊，
  而不是模拟热键。
- 渲染回归在 `WS_EX_LAYERED` 生效后单独验证：胶囊隐藏时窗口完全透明（桌面像素不变），说明分层窗口没有
  破坏 DWM 的逐像素 alpha（winit 的 `DwmEnableBlurBehindWindow` 仍在起作用）。

发布后复测：下载 v0.2.4 Release 资产（`VoxKey.exe`，5,320,192 字节，SHA-256
`e4d1372b…2c450be30`，与 Release 公布的摘要一致），用同一套脚本再跑一遍——扩展样式
`0x80800B8`，`WindowFromPoint` 返回下方目标窗口，胶囊中心点击被目标窗口收到，说明 CI 产物与本地
构建行为一致。CI 运行 [#43185132758](https://github.com/katriyda/VoxKey/actions/runs/43185132758) 全部通过。

## 悬浮窗生命周期重构（2026-11-12，0.2.5）

用户反馈：0.2.2–0.2.4 的悬浮窗“常驻可见但完全透明”，桌面在空闲时也一直存在一个不可见窗口；
不用的时候应当直接隐藏。调研结论与实测如下。

为什么之前常驻可见：0.2.2 为了绕开 eframe 0.32 的缺陷——`winit` 隐藏窗口在 Windows 上收不到跨线程
重绘请求，空闲约 10 秒后第一次热键没有反应。仓库已升级到 eframe 0.37，它会为不可见窗口直接执行
pass（`run.rs` 的 `is_invisible_or_minimized` 分支，emilk/egui#5229），因此这个绕法不再需要。

改造中实测到的两个坑：

1. **窗口必须以可见状态创建。** `with_visible(false)` 创建的透明窗口，后来 `ShowWindow` 也不会被
   DWM 合成：`IsWindowVisible` 为真、分层与穿透样式正确、`App::ui` 以 alpha=1.0 持续绘制，但
   GDI 屏幕截图上什么都看不到；同一份代码改为 `with_visible(true)` 创建即正常。窗口因此先创建为
   可见，由 `VoxKeyApp::tick` 隐藏；并且在第一次隐藏前显式发送一次 `Visible(true)` 并绘制两帧
   （`startup_frames`），否则之后的显示同样不会被合成。
2. **不能在无事件循环上下文的 pass 里创建设置窗口。** 隐藏窗口的 pass 由
   `check_redraw_requests` 直接绘制，`with_current_event_loop` 为空；在其中调用
   `show_viewport_immediate` 会 panic：`egui backend is implemented incorrectly - the user
   callback was never called`。现在 `open_settings` 只置 `pending_settings`，等
   `platform::window_visible` 确认窗口真的可见后的下一个 pass 再创建。

事件处理、动画推进与显示/隐藏决策移入 `App::logic`（`ui` 只绘制），因此隐藏期间热键/托盘事件仍会
被处理。代价是 eframe 把不可见窗口的重绘请求节流到至少 100 ms，胶囊在按下热键后约 140–160 ms 出现
（见 [known-limitations.md](known-limitations.md)）。

实测（1024×768，Windows 11 26100，release 构建）：

| 检查 | 结果 |
| --- | --- |
| 空闲 25 s | `IsWindowVisible=false`；`WindowFromPoint(512,640)` 返回其它进程窗口；6 s CPU 0.000 s |
| 托盘的“打开模型目录”显示胶囊 | GDI 截图逐像素可见胶囊、文字与深色气泡；4 s 后 `Visible(false)` |
| 隐藏 → 再次显示 | 正常合成，胶囊再次出现在屏幕上 |
| 热键唤醒（测试构建临时去掉 `LLKHF_INJECTED` 过滤） | 159 ms 后窗口可见 |
| 显示时鼠标穿透 | exstyle `0x80800B8`；`WindowFromPoint` 指向下层窗口；目标窗口收到 `MouseDown button=Left` |
| 空闲隐藏态打开设置（托盘 WM_COMMAND 30） | 无 panic，设置窗口创建成功（`VoxKey 设置`，130,130–666,949） |
| 关闭设置（WM_CLOSE） | 设置窗口消失，根窗口 3 s 内 `Visible(false)` |
| `with_visible(false)` 对照实验 | 窗口可见且持续绘制，屏幕截图空白（见坑 1） |

- 本会话没有可用的麦克风，引擎报“麦克风不可用”，因此录音内容未验证；热键唤醒用测试构建
  （临时去掉注入键过滤）驱动，发布构建的过滤逻辑未改。
- 像素证据用 GDI `CopyFromScreen` 采集：本机 MCP 的 pillow 截图不包含分层窗口，不能用它判断胶囊
  是否在屏幕上（用它判断会得到相反的结论）。

# VoxKey

VoxKey 是一个面向 Windows 10/11 的本地优先语音输入工具：按住快捷键说话，松开后本地识别，并通过剪贴板 + `Ctrl+V` 把文字插入当前聚焦的输入框。可选接入任意 OpenAI 兼容接口做**极度保守**的识别纠错。

- 纯 Rust，原生 Win32 + egui 悬浮窗，无 Electron / WebView。
- 本地识别使用 sherpa-onnx SenseVoice 多语言模型，默认简体中文，支持 English、繁體中文、日本語、한국어。
- 首次运行自动下载模型到本机缓存；之后可完全离线运行。
- Release 构建为单个 `VoxKey.exe`，绿色免安装，不使用 MSIX / MSI / 安装程序。

## 功能

| 功能 | 实现 |
| --- | --- |
| 全局按住说话 | `WH_KEYBOARD_LL` 低级键盘钩子，可选自定义组合键 |
| 录音 | `cpal` 默认麦克风，自动重采样到 16 kHz 单声道 |
| 识别 | `sherpa-onnx` OfflineRecognizer + SenseVoice int8 |
| 实时文字 | 录音过程中按约 1.3 秒间隔对累计音频做一次识别，只用于界面预览；松键后再做最终识别 |
| 悬浮窗 | egui/eframe 无边框透明置顶窗口，底部居中，RMS 驱动的 5 根波形，弹性宽度，淡入淡出 |
| 插入 | 快照原剪贴板 → 写入 Unicode 文本 → 模拟 `Ctrl+V` → 延时恢复原剪贴板 |
| 纠错 | OpenAI-compatible `/chat/completions`；严格系统提示词 + 本地编辑距离/长度守卫 |
| 托盘 | 语言切换、LLM 开/关、Settings、退出 |
| 设置 | Base URL、API Key、Model、Test、Save；快捷键/语言也可在此保存 |
| 密钥安全 | API Key 使用 Windows DPAPI（CurrentUser）加密后写入配置文件；清空后不保留密文 |
| 发布资源 | `build.rs` 调用 MSVC `rc.exe` 嵌入图标、`VERSIONINFO` 与 `asInvoker` 清单（`assets/voxkey.rc`、`assets/voxkey.manifest`） |

## 使用

1. 从 [Releases](https://github.com/katriyda/VoxKey/releases) 下载 `VoxKey.exe`，双击运行。程序无主窗口，常驻系统托盘。
2. 首次运行会下载 SenseVoice int8 模型（约 229 MB），屏幕底部悬浮窗显示进度与速率。下载支持断点续传：中断后保留 `.part`，下次启动从已下载的字节继续；连接超过 30 秒没有新字节会换镜像重试（HF → hf-mirror，最多 3 轮）。模型缓存在 `%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\`。
   网络受限、卡在某个百分比不动时，可手动下载两个文件并导入，完整步骤见 [docs/offline-model.md](docs/offline-model.md)。
3. 默认按住**右 Ctrl**说话，松开后自动识别并插入。也可在托盘 `设置…` 中修改为：
   - 右 Ctrl（默认）
   - `Ctrl + Shift + 空格`
   - `Fn / F13`
   - 任意受支持的组合，例如 `ctrl+alt+space`、`f13`、`right_alt`。

### 关于 Fn 键

多数笔记本的 Fn 键在键盘固件层就被消费，根本不会产生 Windows 键盘消息，`WH_KEYBOARD_LL` 也无法拦截。VoxKey 实现了对以 `F13`/相关扫描码上报的 Fn 的兼容探测，但**不能承诺所有设备的 Fn 都可识别**。

因此默认使用右 Ctrl 作为可靠替代，并提供可配置快捷键。设置页有 `Fn / F13` 预设，用于少数会把 Fn 映射为 F13 的键盘；同时钩子会额外探测 `F13` 与相关扫描码，因此即使当前配置是右 Ctrl，支持上报 F13 的设备也可直接用 Fn 触发。这是有意为之的可用性取舍，而不是遗漏。

## 模型下载与离线导入

首次运行只需要两个文件：`model.int8.onnx`（239,233,841 字节）与 `tokens.txt`（315,894 字节），
固定在 sherpa-onnx 的 `sensevoice-zh-en-ja-ko-yue-2024-07-17` revision 上。

- 下载中：悬浮窗按文件显示阶段、百分比、已下载 MB 与速率；连接 30 秒没有新字节就换镜像/重试，`.part` 保留，下次启动续传。
- 卡住时：托盘 → **导入本地模型**；把文件放进 `%LOCALAPPDATA%\VoxKey\models\inbox` 后点一次即可，下次启动也会自动尝试导入。
- 校验：导入时强制校验文件大小与 SHA-256；正常启动只按大小快速判断，避免每次启动都哈希 239 MB。
- 直链、哈希、aria2/curl 命令、常见报错对照：见 [docs/offline-model.md](docs/offline-model.md)。

## 配置与数据

| 内容 | 默认位置 | 覆盖环境变量 |
| --- | --- | --- |
| 设置文件 | `%APPDATA%\VoxKey\config.json` | `VOXKEY_DATA_DIR` |
| 模型缓存 | `%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\` | `VOXKEY_MODEL_DIR` |
| 手动导入目录 | `%LOCALAPPDATA%\VoxKey\models\inbox\` | `VOXKEY_IMPORT_DIR` |
| 错误日志 | `%APPDATA%\VoxKey\voxkey.log` | `VOXKEY_DATA_DIR` |

配置示例（API Key 实际以 `api_key_enc` 密文存储，不会出现明文）：

```json
{
  "language": "zh-CN",
  "hotkey": "right_ctrl",
  "llm_enabled": false,
  "api_base_url": "https://api.openai.com/v1",
  "model": "gpt-5.4-mini",
  "api_key_enc": "<DPAPI 密文 hex；无密钥时省略此字段>"
}
```

- Base URL 可以是 `https://api.openai.com/v1`、`http://localhost:11434`，也可以直接填完整的 `.../chat/completions`。
- 若端点不需要鉴权，API Key 留空即可；VoxKey 不会发送空的 `Authorization` 头。
- API Key 的“彻底清空”：设置页点击“清空”并 Save，配置文件中的 `api_key_enc` 会被移除，绝不会回退到明文。
- 默认模型是 `gpt-5.4-mini`。`gpt-4o-mini` 已被 OpenAI 标记为 Deprecated，旧配置文件里的该模型会在启动时自动迁移到新默认值。
- 请求体默认使用 `max_tokens` + `temperature: 0`（兼容多数 OpenAI 兼容服务端）；若端点返回 400 并提示不支持（例如 GPT-5.x 系列要求 `max_completion_tokens`），VoxKey 会自动改用 `max_completion_tokens`、必要时再去掉 `temperature` 重试一次。

## LLM 纠错约束

系统提示词要求模型只修复明显的同音/近音识别错误，例如“配森”→“Python”、“杰森”→“JSON”；禁止润色、改写、缩写、删减、翻译、补写。原文正确时必须原样返回。

代码侧还有独立于模型的保护：

- 长度比例超出 `0.70..1.45` 时拒绝改写；
- 原文长度 ≥ 10 时，字符编辑距离超过约 25% 直接回退原文；
- 新增换行、空响应、代码围栏等异常会回退原文；
- 任何网络/API 错误都不会阻止原文插入，只会跳过纠错。

LLM 请求是可选联网功能，音频本身始终留在本机。

## 剪贴板兼容性说明

插入流程：

```
读取原剪贴板（Unicode 文本、DIB、DROPFILES、LOCALE、注册的 HGLOBAL 格式等）
→ 写入待插入 Unicode 文本
→ 等待剪贴板更新
→ SendInput Ctrl+V
→ 等待 350 ms
→ 恢复原剪贴板
```

- 使用 `Ctrl+V` 而不是模拟逐字输入，避免破坏中文输入法状态。
- 悬浮窗带 `WS_EX_NOACTIVATE` + `WS_EX_TOOLWINDOW` + `WS_EX_TOPMOST`，不会抢走目标输入框焦点。
- OS 窗口常驻可见，但胶囊内容隐藏时窗口完全透明。这是为了避开 winit 隐藏窗口在 Windows 上不再响应跨线程重绘请求的问题；否则应用空闲约 10 秒后第一次按热键可能没有反应。
- 鼠标穿透由窗口过程在 `WM_NCHITTEST` 中返回 `HTTRANSPARENT` 实现（`platform::overlay_window_proc`）。**不要**改回 eframe 的 `with_mouse_passthrough(true)`：winit 会为此设置 `WS_EX_LAYERED` 却不调用 `SetLayeredWindowAttributes`/`UpdateLayeredWindow`，结果是窗口仍有正确尺寸与内容（`PrintWindow` 能看到胶囊），但桌面合成器完全不绘制它——悬浮窗在屏幕上彻底不可见，波浪动画与实时预览文字也就永远看不到。
- **限制**：纯 Win32 剪贴板 API 无法可靠复制所有 GDI 句柄型格式，例如 `CF_BITMAP`；这些格式在恢复时可能丢失。常见的文本、图片 DIB、文件列表格式会被保留。
- Windows 剪贴板历史会记录本次注入的文本，这是系统行为。
- 若读取原剪贴板失败，VoxKey 会取消插入，以免破坏用户现有剪贴板。

## 构建（仅 GitHub Actions）

本仓库禁止本地编译；所有构建在 GitHub Actions 的 `windows-2022` Runner 上执行。

```text
Actions → Build VoxKey → Run workflow
```

也支持推送 `v*` tag 自动构建并创建 GitHub Release：

```bash
git tag v0.1.0
git push origin v0.1.0
```

工作流见 [`.github/workflows/build.yml`](.github/workflows/build.yml)：`cargo test --release` → `cargo build --release` → 检查 DLL 依赖 → 校验嵌入的版本资源 → 单文件 artifact → Tag 时发布 Release。`build.rs` 会用 MSVC 的 `rc.exe` 编译 `assets/voxkey.rc`（图标、`VERSIONINFO`、`asInvoker` 清单）并链接进 exe；找不到 `rc.exe` 时构建直接失败，避免发布没有任何元数据的“裸”二进制（确需跳过可设 `VOXKEY_SKIP_RC=1`，Release 不允许）。通过 `.cargo/config.toml` 与 Release profile 尽可能静态链接，最终 artifact 只有：

```text
target/release/VoxKey.exe
```

## 杀软误报、签名与校验

**当前所有 Release 都没有 Authenticode 签名**，所以下载和运行时的安全提示是预期现象，不代表文件被投毒。

### 会看到什么

| 提示 | 来源 | 原因 |
| --- | --- | --- |
| 浏览器“此文件可能有害 / 不常下载” | Chrome、Edge 的 Safe Browsing | 新文件哈希、零下载量、无发布者 |
| “Windows 已保护你的电脑”（未知发布者） | SmartScreen | 浏览器给 exe 打了 Mark-of-the-Web，而程序没有签名 |
| Defender 报出具体名字（如 `Trojan:Win32/Wacatac!ml` 这类机器学习命名） | Defender 启发式 / ML | 未签名 + 全局键盘钩子 + 剪贴板注入 + 自更新替换自身，行为特征与键盘记录器、下载器重叠 |

这三类判断都基于**信誉与行为**，不是特征码比对。VoxKey 的相关行为如实说明：

- `WH_KEYBOARD_LL` 全局键盘钩子只用于识别热键，不记录、不落盘、不上传任何按键内容（`src/platform.rs`）；
- 剪贴板只在每轮插入前后读取/写入/恢复各一次（`src/clipboard.rs`）；
- 应用内更新会下载 exe，校验大小、PE 头、SHA-256 与版本号，然后替换正在运行的自身（`src/update.rs`）——标准自更新流程，但 ML 引擎很容易把它和 dropper 混为一谈；
- 首次运行会从 Hugging Face 下载约 229 MB 模型到 `%LOCALAPPDATA%`；启用 LLM 纠错时只发送识别后的**文本**。

音频始终在本机识别，不收集遥测。

### 下载后先校验

只从 [Releases](https://github.com/katriyda/VoxKey/releases) 下载，并核对 Release 说明里给出的大小与 SHA-256：

```powershell
Get-FileHash .\VoxKey.exe -Algorithm SHA256
(Get-Item .\VoxKey.exe).Length
```

一致说明文件与 CI 产物逐字节相同；不一致就不要运行，删掉重新下载。

### 被拦住了怎么办

1. 去掉“来自网络”标记：`Unblock-File .\VoxKey.exe`；
2. SmartScreen 弹窗点“更多信息 → 仍要运行”；
3. Defender 已隔离：Windows 安全中心 → 保护历史记录 → 允许该项目；
4. 公司/学校设备可能由管理员策略统一拦截，需要 IT 把哈希加白，普通用户改不了；
5. 仍被误报可向微软提交（<https://www.microsoft.com/en-us/wdsi/filesubmission>，选“软件开发者 / 误报”并附哈希），这是修正特征库与信誉库的官方通道。

**只在确认哈希一致后放行**，不要为此关闭整机实时防护。

### 我们做了什么、还差什么

- 从下一个构建开始，`build.rs` 会用 MSVC 的 `rc.exe` 把图标、`VERSIONINFO`（产品名、描述、版本、版权）和 `asInvoker` 清单编进 exe，CI 还会断言这些元数据确实进了产物。这能让“属性 → 详细信息”不再是一片空白、降低 ML 打分，但**不会**让“未知发布者”消失。
- 根治手段是 **Authenticode 代码签名 + 时间戳**（OV/EV 证书或 Azure Trusted Signing）。签名后发布者显示为组织名，SmartScreen 不再按“未知发布者”拦截；OV 证书还需要下载量积累信誉，EV 起步信誉更好。接入签名后，应用内更新“下载 → 替换”的整条链路也会一起变干净。
- 在此之前，每个 Release 说明都会附文件大小与 SHA-256，供自行校验。

## 技术栈

- Rust 2024（MSRV 1.95，CI 与 `mise.toml` 固定 1.99.1）
- `windows` 0.62（Win32 钩子、托盘、剪贴板、SendInput、DPAPI）
- `eframe` / `egui` 0.37（悬浮窗与设置）
- `cpal` 0.18（麦克风）
- `sherpa-onnx` 2.0（本地 ASR）
- `tokio` + `reqwest` 0.13（SChannel/native-tls，模型下载与 LLM 请求）
- `serde` / `serde_json`（配置与 API JSON）

## 架构

```text
src/main.rs        eframe 启动、窗口参数
src/app.rs         状态机、悬浮窗显示/隐藏/定位、托盘事件、设置窗口
src/engine.rs      录音线程：模型加载、录音、预览识别、最终识别、LLM、插入
src/audio.rs       cpal 采集与 16 kHz 线性重采样
src/model.rs        SenseVoice 首次下载（HF / hf-mirror，断点续传 + 停滞超时 + SHA-256 校验）与离线导入
src/sha256.rs       模型文件的纯 Rust SHA-256 校验（不引入额外 crypto 依赖）
src/platform.rs    WH_KEYBOARD_LL、托盘菜单、隐藏窗口、NoActivate/工作区
src/clipboard.rs   剪贴板快照/恢复、SendInput Ctrl+V
src/llm.rs         OpenAI-compatible 纠错、保守守卫、连接测试
src/config.rs      配置读写（JSON）
src/secrets.rs     Windows DPAPI 密钥保护
src/overlay.rs     胶囊悬浮窗绘制与动画
src/settings.rs    Settings UI
src/zh.rs          简体→繁体单字转换
src/hotkey.rs      快捷键解析
src/update.rs      应用内更新：GitHub Release 检查、下载校验、替换重启
build.rs           Win32 资源：图标、VERSIONINFO、asInvoker 清单（调用 rc.exe）
```

## 验证状态（2026-11-10）

最终 Release 产物由 GitHub Actions 的 `windows-2022` Runner 构建，提交 `cfec8ec`，运行
[Build VoxKey #42804184486](https://github.com/katriyda/VoxKey/actions/runs/42804184486) 全部通过：

| 检查 | 结果 |
| --- | --- |
| `cargo test --release --locked` | ✅ 17 个单元测试全部通过 |
| `cargo build --release --locked` | ✅ |
| `dumpbin /dependents` 检查 | ✅ 没有 onnxruntime / sherpa / VCRUNTIME / MSVCP 依赖 |
| `VoxKey.exe --version` 冒烟测试 | ✅ |
| 单文件 artifact 上传 | ✅ `VoxKey.exe`，约 40 MB |

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
- 悬浮窗在真实桌面上可见：录音过程中桌面截图能直接看到胶囊、实时预览文字与波形（修复 `WS_EX_LAYERED` 导致的不可见问题后，见下方“悬浮窗可见性修复”）；
- 静音/环境噪声不再插入“我。”或“。”这类幻觉结果，而是提示“没有检测到语音”；
- 错误写入 `%APPDATA%\VoxKey\voxkey.log`。

**仍未实测**：物理 Fn 键（各机型固件差异）、真实人声而非扬声器回放、托盘图标与菜单点击、Settings 窗口交互、真实 LLM 端点、干净 Windows 10 机器上的首次运行。当前会话是低完整性进程，`Shell_NotifyIconW` 返回 Win32 5（拒绝访问），且无法向普通完整性窗口注入测试按键；这些限制来自测试沙箱，不是应用逻辑。普通非提权用户会话应能正常使用托盘；若托盘不出现，请确认 Explorer 正在运行，并避免以管理员身份启动。

## 悬浮窗可见性修复（2026-11-11）

用户反馈“没有看到动画、文字也没有实时写上去”。排查结果是整条链路都在正常工作——录音、1.3 秒一次的预览识别、`Transcript` 事件、每帧动画状态、`PrintWindow` 内容全部正确——但**悬浮窗从未被桌面合成器绘制出来**，所以屏幕上一片空白。

根因：`main.rs` 里的 `with_mouse_passthrough(true)` 会让 winit 给窗口加上 `WS_EX_LAYERED`（`window_state.rs` 中 `IGNORE_CURSOR_EVENT` 分支），却从不设置 layered 属性。一个没有属性的 layered 窗口不会被合成，窗口存在、内容在渲染、`IsWindowVisible` 为真，但屏幕上什么都看不到。

修复：去掉 `with_mouse_passthrough`，改为在窗口过程里对 `WM_NCHITTEST` 返回 `HTTRANSPARENT`，既保留真实的逐像素透明（DWM blur-behind），又保持鼠标穿透。

## 应用内更新（2026-11-11）

不再需要手动去 GitHub 下载 exe 再替换文件。设置窗口底部新增“更新”区块，托盘右键菜单也提供“检查更新…”：

1. **检查更新**：请求 GitHub Releases API，取最新非预发布 Release，把 tag（如 `v0.1.1`）与编译进程序的 `CARGO_PKG_VERSION` 按语义化数字比较。
2. **下载并重启更新**：下载 Release 中的 `VoxKey.exe` 到 `%APPDATA%\VoxKey\updates\`，显示进度。
3. **安装前校验**：文件大小必须与 Release 资产元数据一致；必须是 `MZ` + `PE` 且 machine 为 `x86-64`；若 GitHub 资产带有 `digest`（`sha256:…`），用 Windows CNG 重新计算并比对 SHA-256；最后用 `--version` 自检下载到的程序，输出必须正好是该 Release 的版本号。任一校验失败都会删除文件并中止，不会替换正在运行的程序。
4. **替换并重启**：先把新版复制到 `VoxKey.exe.new`，把正在运行的 `VoxKey.exe` 重命名为 `VoxKey.exe.old`（Windows 允许重命名运行中的可执行文件），再把 `.new` 改名回 `VoxKey.exe` 并启动它，旧进程随后退出。失败会自动回滚；下次启动会清理 `.old` / `.new` 与残留的 `.part` 文件。

实现位于 `src/update.rs`，网络请求在独立线程 + current-thread Tokio 运行时中执行，不阻塞 UI。设计与取舍：

- 更新源固定为 `https://github.com/katriyda/VoxKey`，只有 HTTPS，没有第三方镜像或自签名证书。
- 未接入 Authenticode 签名校验（当前 Release 本身未签名）。完整性依赖 TLS、GitHub 资产大小、PE 校验和“下载到的程序自己报告的版本号”，这与当前无签名发布方式一致；将来若做代码签名，可在这里加入签名校验。
- 安装目录必须对当前用户可写。VoxKey 是绿色单文件程序，放在 `Program Files` 或以管理员身份运行时替换会失败，界面会给出具体错误而不是静默退出。
- 发布流程要求 **git tag 与 `Cargo.toml` 的 `version` 一致**（例如 `version = "0.1.1"` 对应 `v0.1.1`）。CI 会强制校验，否则更新逻辑会把同一个版本一直当成“有新版”。

## 已知限制（如实说明）

- **Fn 键**：见上文。Windows 全局钩子无法可靠监听纯硬件 Fn，默认右 Ctrl。
- **“实时”转写**：SenseVoice 是离线模型。VoxKey 通过录音期间周期性重跑识别来更新右侧预览文字，并非常规意义上的流式 ASR；长句预览会有约 1 秒级延迟，最终文字以松键后的结果为准。
- **繁体中文**：使用 OpenCC 单字映射进行简体→繁体转换，不做完整的地区词/短语本地化；个别一简对多繁的字词可能不完美。
- **剪贴板恢复**：GDI 句柄型格式不支持完整恢复；见上文。
- **自动下载网络**：模型优先从 Hugging Face 下载，失败时回退 hf-mirror；支持断点续传与 30 秒停滞检测，两个镜像都不可达时会给出错误而不是永远停在进度条上。网络受限时可手动下载并导入（托盘 → 导入本地模型），见 [docs/offline-model.md](docs/offline-model.md)。
- **运行时验证**：构建与单元测试在 GitHub Actions（Windows）上执行；真实麦克风、托盘、焦点保持、中文输入法兼容性和各机型的 Fn 行为需要在目标 Windows 设备上实测。仓库会明确记录已验证和未验证项。
- **快捷键冲突**：全局钩子只能观察，不能在所有场景下阻止快捷键被当前应用同时接收；默认右 Ctrl 通常无冲突，若自定义为常用组合请自行注意。
- **代码签名**：当前 Release 未做 Authenticode 签名，SmartScreen 可能提示“未知发布者”，部分杀软会给出基于行为的误报；原因、哈希校验与放行步骤见上文「杀软误报、签名与校验」。
- **托盘与提权**：Windows 的 UIPI 机制会阻止提权进程向普通权限的 Explorer 添加托盘图标。请以普通用户身份运行 VoxKey。

## 第三方资源与许可

- 代码：MIT，见 [LICENSE](LICENSE)。
- Noto Sans CJK SC 字体：SIL Open Font License 1.1，见 [assets/OFL.txt](assets/OFL.txt)。
- OpenCC 简体→繁体映射数据：Apache-2.0，见 [NOTICE](NOTICE) 与 `assets/zh_hans_to_hant.tsv` 文件头。
- SenseVoice 模型与 sherpa-onnx：模型由 k2-fsa/sherpa-onnx 发布，遵循其上游许可证；详见 [NOTICE](NOTICE)。
- 图标由本项目生成。

## 隐私

- 麦克风音频只在本进程内识别。
- 仅当用户在托盘/设置中启用 LLM 纠错时，识别后的**文本**才会发送到配置的 OpenAI-compatible 接口。
- 不收集遥测。

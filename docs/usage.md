# 使用说明

## 安装与运行

1. 从 [Releases](https://github.com/katriyda/VoxKey/releases) 下载 `VoxKey.exe`，双击运行。程序无主窗口，常驻系统托盘。
2. 首次运行会下载 SenseVoice int8 模型（约 229 MB），屏幕底部悬浮窗显示进度与速率。
   下载支持断点续传：中断后保留 `.part`，下次启动从已下载的字节继续；连接超过 30 秒没有新字节会换镜像重试（HF → hf-mirror，最多 3 轮）。
   模型缓存在 `%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\`。
   网络受限、卡在某个百分比不动时，可手动下载两个文件并导入，完整步骤见 [offline-model.md](offline-model.md)。
3. 默认按住**右 Ctrl**说话，松开后自动识别并插入。也可在托盘 `设置…` 中修改为：
   - 右 Ctrl（默认）
   - `Ctrl + Shift + 空格`
   - `Fn / F13`
   - 任意受支持的组合，例如 `ctrl+alt+space`、`f13`、`right_alt`。

## 关于 Fn 键

多数笔记本的 Fn 键在键盘固件层就被消费，根本不会产生 Windows 键盘消息，`WH_KEYBOARD_LL` 也无法拦截。VoxKey 实现了对以 `F13`/相关扫描码上报的 Fn 的兼容探测，但**不能承诺所有设备的 Fn 都可识别**。

因此默认使用右 Ctrl 作为可靠替代，并提供可配置快捷键。设置页有 `Fn / F13` 预设，用于少数会把 Fn 映射为 F13 的键盘；同时钩子会额外探测 `F13` 与相关扫描码，因此即使当前配置是右 Ctrl，支持上报 F13 的设备也可直接用 Fn 触发。这是有意为之的可用性取舍，而不是遗漏。

## 应用内更新

设置窗口底部有“更新”区块，托盘右键菜单也提供“检查更新…”：

1. **检查更新**：请求 GitHub Releases API，取最新非预发布 Release，把 tag（如 `v0.1.1`）与编译进程序的 `CARGO_PKG_VERSION` 按语义化数字比较。
2. **下载并重启更新**：下载 Release 中的 `VoxKey.exe` 到 `%APPDATA%\VoxKey\updates\`，显示进度。
3. **安装前校验**：文件大小必须与 Release 资产元数据一致；必须是 `MZ` + `PE` 且 machine 为 `x86-64`；若 GitHub 资产带有 `digest`（`sha256:…`），用 Windows CNG 重新计算并比对 SHA-256；最后用 `--version` 自检下载到的程序，输出必须正好是该 Release 的版本号。任一校验失败都会删除文件并中止，不会替换正在运行的程序。
4. **替换并重启**：先把新版复制到 `VoxKey.exe.new`，把正在运行的 `VoxKey.exe` 重命名为 `VoxKey.exe.old`（Windows 允许重命名运行中的可执行文件），再把 `.new` 改名回 `VoxKey.exe` 并启动它，旧进程随后退出。失败会自动回滚；下次启动会清理 `.old` / `.new` 与残留的 `.part` 文件。

实现位于 `src/update.rs`，网络请求在独立线程 + current-thread Tokio 运行时中执行，不阻塞 UI。设计与取舍：

- 更新源固定为 `https://github.com/katriyda/VoxKey`，只有 HTTPS，没有第三方镜像或自签名证书。
- 未接入 Authenticode 签名校验（当前 Release 本身未签名）。完整性依赖 TLS、GitHub 资产大小、PE 校验和“下载到的程序自己报告的版本号”，这与当前无签名发布方式一致；将来若做代码签名，可在这里加入签名校验。
- 安装目录必须对当前用户可写。VoxKey 是绿色单文件程序，放在 `Program Files` 或以管理员身份运行时替换会失败，界面会给出具体错误而不是静默退出。
- 发布流程要求 **git tag 与 `Cargo.toml` 的 `version` 一致**（例如 `version = "0.1.1"` 对应 `v0.1.1`）。CI 会强制校验，否则更新逻辑会把同一个版本一直当成“有新版”。

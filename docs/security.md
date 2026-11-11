# 杀软误报、签名与校验

**当前所有 Release 都没有 Authenticode 签名**，所以下载和运行时的安全提示是预期现象，不代表文件被投毒。

## 会看到什么

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

## 下载后先校验

只从 [Releases](https://github.com/katriyda/VoxKey/releases) 下载，并核对 Release 说明里给出的大小与 SHA-256：

```powershell
Get-FileHash .\VoxKey.exe -Algorithm SHA256
(Get-Item .\VoxKey.exe).Length
```

一致说明文件与 CI 产物逐字节相同；不一致就不要运行，删掉重新下载。

## 被拦住了怎么办

1. 去掉“来自网络”标记：`Unblock-File .\VoxKey.exe`；
2. SmartScreen 弹窗点“更多信息 → 仍要运行”；
3. Defender 已隔离：Windows 安全中心 → 保护历史记录 → 允许该项目；
4. 公司/学校设备可能由管理员策略统一拦截，需要 IT 把哈希加白，普通用户改不了；
5. 仍被误报可向微软提交（<https://www.microsoft.com/en-us/wdsi/filesubmission>，选“软件开发者 / 误报”并附哈希），这是修正特征库与信誉库的官方通道。

**只在确认哈希一致后放行**，不要为此关闭整机实时防护。

## 我们做了什么、还差什么

- 从下一个构建开始，`build.rs` 会用 MSVC 的 `rc.exe` 把图标、`VERSIONINFO`（产品名、描述、版本、版权）和 `asInvoker` 清单编进 exe，CI 还会断言这些元数据确实进了产物。这能让“属性 → 详细信息”不再是一片空白、降低 ML 打分，但**不会**让“未知发布者”消失。
- 根治手段是 **Authenticode 代码签名 + 时间戳**（OV/EV 证书或 Azure Trusted Signing）。签名后发布者显示为组织名，SmartScreen 不再按“未知发布者”拦截；OV 证书还需要下载量积累信誉，EV 起步信誉更好。接入签名后，应用内更新“下载 → 替换”的整条链路也会一起变干净。
- 在此之前，每个 Release 说明都会附文件大小与 SHA-256，供自行校验。

## 隐私

- 麦克风音频只在本进程内识别。
- 仅当用户在托盘/设置中启用 LLM 纠错时，识别后的**文本**才会发送到配置的 OpenAI-compatible 接口。
- 不收集遥测。

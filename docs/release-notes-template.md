# VoxKey 发布说明模板

CI 用 `gh release create --generate-notes` 生成变更列表；发布后用本模板补齐“校验”“安装”“杀软提示”三段，覆盖 Release 正文。

`VoxKey <tag>` 建议写成 `VoxKey v0.2.1` 这样的形式，与 `Cargo.toml` 的 `version` 保持一致（CI 会强制校验 tag 与版本号相同）。

---

## 本版主要变化

### 新增
-

### 修复
-

### 其它
-

## 校验

- 资产：`VoxKey.exe`
- 大小：`<bytes>` 字节
- SHA-256：`<hex>`
- 构建：GitHub Actions（`windows-2022`），运行 [#<run-id>](https://github.com/katriyda/VoxKey/actions/runs/<run-id>)

下载后自行核对：

```powershell
Get-FileHash .\VoxKey.exe -Algorithm SHA256
(Get-Item .\VoxKey.exe).Length
```

## 安装与升级

- 单文件绿色程序，直接替换旧的 `VoxKey.exe` 即可，无需安装。
- 0.2.0 及以后版本可在托盘 → “检查更新…”或设置窗口的“更新”区块内自动升级。
- 应用内更新要求程序所在目录对当前用户可写；放在 `Program Files` 或以管理员身份运行时会提示失败，不会半安装。

## 杀软误报（本版重要）

本版**未做 Authenticode 签名**，首次下载可能遇到：

- 浏览器提示“此文件可能有害 / 不常下载”（Chrome、Edge Safe Browsing）；
- SmartScreen 提示“Windows 已保护你的电脑”“未知发布者”；
- Defender 报出 `Trojan:Win32/Wacatac!ml` 一类机器学习命名。VoxKey 使用全局键盘钩子、剪贴板注入和自更新替换自身，这些行为与键盘记录器、下载器重叠，未签名时容易被启发式规则命中。

处理方式：先确认上面的 SHA-256 与本机文件一致，然后 `Unblock-File .\VoxKey.exe`、SmartScreen 点“更多信息 → 仍要运行”；若已被隔离，在 Windows 安全中心“保护历史记录”里允许该项目。确认哈希一致仍被误报，可向 <https://www.microsoft.com/en-us/wdsi/filesubmission> 提交。

原因、校验与上报的完整说明见仓库 README 的「杀软误报、签名与校验」一节。

## 已知问题

-

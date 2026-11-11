# 构建与发布

本仓库禁止本地编译；所有构建在 GitHub Actions 的 `windows-2022` Runner 上执行。

```text
Actions → Build VoxKey → Run workflow
```

也支持推送 `v*` tag 自动构建并创建 GitHub Release：

```bash
git tag v0.1.0
git push origin v0.1.0
```

工作流见 [`.github/workflows/build.yml`](../.github/workflows/build.yml)：`cargo test --release --workspace` → 构建 `voxkey-asr` worker → 把 worker 的 SHA-256 写进环境变量（UI 用 `option_env!` 内嵌该值）→ 构建 `voxkey` UI → 检查两个 exe 的 DLL 依赖 → 断言 UI 体积上限 → 校验嵌入的版本资源 → 上传两个 artifact → Tag 时发布 Release。`build.rs` 会用 MSVC 的 `rc.exe` 编译 `assets/voxkey.rc`（图标、`VERSIONINFO`、`asInvoker` 清单）并链接进 exe；找不到 `rc.exe` 时构建直接失败，避免发布没有任何元数据的“裸”二进制（确需跳过可设 `VOXKEY_SKIP_RC=1`，Release 不允许）。通过 `.cargo/config.toml` 与 Release profile 尽可能静态链接，最终 artifact 是两个文件：

```text
target/release/VoxKey.exe      # UI 本体（约 5.3 MB，不含 ASR 引擎）
target/release/VoxKey-asr.exe  # SenseVoice worker（约 19 MB，首次运行自动下载）
```

`Cargo.toml` 的 `[features] embed-cjk-font` 可把 16 MB 的 Noto CJK 字体内嵌回 UI 本体，仅用于必须在任何机器上渲染一致的构建；默认关闭，运行时读取系统字体。

发布说明模板见 [release-notes-template.md](release-notes-template.md)。Tag 必须与 `Cargo.toml` 的 `version` 一致，CI 会强制校验。

## 工具链

运行时与构建工具版本由 `mise.toml` 固定（Rust 1.99.1，MSRV 1.95）。本地只允许做 `cargo check` 一类只读验证；任何可发布产物必须由 CI 生成。

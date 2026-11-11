# 配置、纠错与剪贴板

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
- **限制**：纯 Win32 剪贴板 API 无法可靠复制所有 GDI 句柄型格式，例如 `CF_BITMAP`；这些格式在恢复时可能丢失。常见的文本、图片 DIB、文件列表格式会被保留。
- Windows 剪贴板历史会记录本次注入的文本，这是系统行为。
- 若读取原剪贴板失败，VoxKey 会取消插入，以免破坏用户现有剪贴板。

悬浮窗窗口层面的实现约束（例如为何不能使用 `with_mouse_passthrough`）见 [architecture.md](architecture.md)。

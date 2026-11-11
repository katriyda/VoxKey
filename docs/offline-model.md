# 离线模型：手动下载与导入

VoxKey 首次运行会从 Hugging Face 下载 SenseVoice int8 模型（约 240 MB）。如果网络受限、下载一直卡住或公司网络不允许大文件下载，可以**手动下载两个文件并导入**，全程不需要能访问 Hugging Face 的程序内下载。

> 相关代码：`src/model.rs`（下载、校验、导入），`src/sha256.rs`（SHA-256）。

## 0. 先确认你遇到的是哪种情况

| 现象 | 含义 |
| --- | --- |
| 悬浮窗进度停在某个百分比不动，速率显示 0 MB/s | 连接停滞（自动重试最多 3 轮 × 2 个镜像；跨次启动会从 `.part` 续传） |
| 进度到 99% 左右不动 | 正在下载第二个文件 `tokens.txt`（模型本身已下完） |
| 悬浮窗提示“本地语音模型未就绪” | 所有下载尝试都失败，或文件校验未通过 |
| 提示“大小不符 / SHA-256 不一致” | 下载到的不是 int8 版本、文件没下完，或拿错了 revision |

详细的失败原因都会写进 `%APPDATA%\VoxKey\voxkey.log`。

## 1. 两个文件（固定 revision）

| 文件 | 大小（字节） | SHA-256 |
| --- | --- | --- |
| `model.int8.onnx` | 239,233,841（约 239.2 MB） | `c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51` |
| `tokens.txt` | 315,894（约 315.9 KB） | `f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc` |

上游仓库：`csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`
固定 revision：`2365baeacb507f821a0c8120fcee3d484dba7a07`

直链（Hugging Face）：

```text
https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx
https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt
```

镜像（内容相同，国内通常更快）：

```text
https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx
https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt
```

程序里也可以直接复制这四条链接：**设置 → 本地模型 → 复制下载链接**。

> 不要下载 `model.onnx`（fp32，约 937 MB），VoxKey 只接受 int8 版本。

## 2. 下载（任选其一）

浏览器直接点链接即可；文件较大时建议用支持续传的下载器：

```bash
# aria2，-c 断点续传
aria2c -c -x 8 -s 8 -o model.int8.onnx "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx"
aria2c -c -x 8 -s 8 -o tokens.txt "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt"
```

```bash
# curl，-C - 断点续传
curl -L -C - -o model.int8.onnx "https://hf-mirror.com/..."
curl -L -C - -o tokens.txt "https://hf-mirror.com/..."
```

## 3. 校验（强烈建议，只需一条命令）

PowerShell：

```powershell
Get-FileHash -Algorithm SHA256 .\model.int8.onnx
Get-FileHash -Algorithm SHA256 .\tokens.txt
```

两个哈希必须分别等于第 1 节的 `c71f0ce0…` 和 `f449eb28…`，大小也要完全一致（239,233,841 / 315,894 字节）。程序导入时会再做一次 SHA-256 校验，哈希不对会直接拒绝并说明原因，不会把坏文件装进缓存。

## 4. 放进导入目录

默认导入目录：

```text
%LOCALAPPDATA%\VoxKey\models\inbox
```

- 直接把两个文件放进去即可，文件名最好保持原样。
- 也支持放**整个解压后的文件夹**（会向下扫描 3 层），所以从 zip 解压出来的带目录结构也能识别。
- 若下载到的是 `.zip`，请先解压再放；程序不会解压压缩包。

## 5. 导入

两种方式，任选其一：

1. **托盘**：右键托盘图标 → **导入本地模型…**。若目录还是空的，VoxKey 会打开该目录并提示你放文件，放好后再点一次即可。
2. **设置窗口**：托盘 → 设置… → 本地模型 → **导入本地模型**。
3. **重启**：下次启动 VoxKey 时会自动尝试从导入目录安装，不需要点按钮。

导入成功后会看到“已导入 model.int8.onnx 与 tokens.txt”，引擎自动重新加载模型，无需重启进程。进度会显示“导入 / 校验”阶段（导入只做本地复制与校验，不联网）。

## 6. 更直接的办法：直接放进缓存目录

如果不想用导入目录，也可以直接把两个文件放到缓存目录（**文件名必须完全一致**）：

```text
%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\model.int8.onnx
%LOCALAPPDATA%\VoxKey\models\sensevoice-int8\tokens.txt
```

然后托盘 → **重新加载模型**（或重启 VoxKey）。启动时只按**文件大小**判断缓存是否可用；如果大小对但内容坏了，用托盘“导入本地模型”让程序做哈希校验，或删掉文件重新下载。

## 7. 环境变量（离线/企业部署）

| 变量 | 作用 |
| --- | --- |
| `VOXKEY_MODEL_DIR` | 指定模型缓存目录（直接放两个文件在这里即可） |
| `VOXKEY_IMPORT_DIR` | 指定“导入目录”，覆盖默认的 `…\models\inbox` |

例如内网批量部署：把模型放到共享目录，设置 `VOXKEY_IMPORT_DIR=Z:\voxkey-model`，首次启动自动导入。

## 8. 常见报错对照

| 提示 | 原因与处理 |
| --- | --- |
| `大小不符：期望 239233841 字节…实际 937617178` | 下成了 fp32 的 `model.onnx`，换 int8 版本 |
| `SHA-256 与上游不一致` | 没下完、被代理/杀软改写、或 revision 不对；重新下载并核对哈希 |
| `在 … 下没有找到 model*.onnx 或 tokens.txt` | 放的是 zip 未解压，或放错了目录 |
| `缺少 tokens.txt（315894 字节）` | 只放了模型，`tokens.txt` 也要放 |
| `连接停滞超过 30 秒` | 与 HF/CDN 的连接被中断；程序会保留 `.part` 并在下次启动续传，也可直接改用手动导入 |
| 设置窗口显示 `model.int8.onnx 不完整（x / 239.2 MB）` | 存在未完成的 `.part` 或大小不对，重新加载会续传/重下 |

## 9. 安全与许可

- 导入的模型只作为数据被 onnxruntime 读取，VoxKey 不会执行模型文件中的任何内容。
- 模型来自 k2-fsa/sherpa-onnx 上游发布，许可与再分发条款见仓库 `NOTICE`；自行分发前请阅读上游 LICENSE。
- 手动下载不经过任何第三方中转，哈希校验保证装进缓存的就是官方文件。

# 已知限制

如实说明当前实现的边界，避免把“设计取舍”误解为 bug。

- **Fn 键**：Windows 全局钩子无法可靠监听纯硬件 Fn，默认右 Ctrl。详见 [usage.md](usage.md#关于-fn-键)。
- **“实时”转写**：SenseVoice 是离线模型。VoxKey 通过录音期间周期性重跑识别来更新右侧预览文字，并非常规意义上的流式 ASR；长句预览会有约 1 秒级延迟，最终文字以松键后的结果为准。
- **繁体中文**：使用 OpenCC 单字映射进行简体→繁体转换，不做完整的地区词/短语本地化；个别一简对多繁的字词可能不完美。
- **剪贴板恢复**：GDI 句柄型格式不支持完整恢复；详见 [configuration.md](configuration.md#剪贴板兼容性说明)。
- **自动下载网络**：模型优先从 Hugging Face 下载，失败时回退 hf-mirror；支持断点续传与 30 秒停滞检测，两个镜像都不可达时会给出错误而不是永远停在进度条上。网络受限时可手动下载并导入（托盘 → 导入本地模型），见 [offline-model.md](offline-model.md)。
- **运行时验证**：构建与单元测试在 GitHub Actions（Windows）上执行；真实麦克风、托盘、焦点保持、中文输入法兼容性和各机型的 Fn 行为需要在目标 Windows 设备上实测。已验证/未验证项见 [verification.md](verification.md)。
- **快捷键冲突**：全局钩子只能观察，不能在所有场景下阻止快捷键被当前应用同时接收；默认右 Ctrl 通常无冲突，若自定义为常用组合请自行注意。
- **代码签名**：当前 Release 未做 Authenticode 签名，SmartScreen 可能提示“未知发布者”，部分杀软会给出基于行为的误报；原因、哈希校验与放行步骤见 [security.md](security.md)。
- **托盘与提权**：Windows 的 UIPI 机制会阻止提权进程向普通权限的 Explorer 添加托盘图标。请以普通用户身份运行 VoxKey。

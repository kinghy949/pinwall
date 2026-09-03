# 原型验证

一次性验证代码，**不是产品代码**。目的是在写正式实现之前，把 [MVP 风险评估](../docs/mvp-risks.md) 里最大的不确定性打掉。

| 原型 | 验证目标 | 状态 |
|---|---|---|
| [`pin-window`](pin-window/) | winit NSWindow 能否覆盖全屏 | **已完成 —— 结论为否** |
| [`pin-panel`](pin-panel/) | NSPanel + NonactivatingPanel 能否覆盖全屏 | **已完成 —— 通过** |

## pin-window

```bash
cd prototypes/pin-window && cargo run
```

交互键位：

| 键 | 作用 |
|---|---|
| `l` | 切换窗口层级 Normal → Floating(3) → Status(25) → PopUpMenu(101) → ScreenSaver(1000) |
| `s` | 切换 collectionBehavior（CanJoinAllSpaces + FullScreenAuxiliary） |
| `c` | 切换鼠标穿透 |
| `[` `]` | 调整透明度 |
| `p` | 打印诊断（位置 / DPI / 所有显示器） |
| `Esc` | 退出 |

### 待验证项

- **A. 全屏应用之上能否浮住**（macOS）：把任意 App 切到全屏，观察本窗口是否仍可见。
  需逐个层级试，找出最低的可用层级 —— 层级设太高会压住系统 UI。
- **B. 跨屏 DPI 变化**：拖到另一块不同缩放的屏，看终端里的 `[DPI 变化]` 日志与细线清晰度。
  **需要至少两块不同缩放的显示器**。

### 已有结论

见 [`docs/mvp-risks.md`](../docs/mvp-risks.md) 的「原型 1 实测结论」一节。


## pin-panel

绕开 winit，直接用 objc2 创建 `NSPanel`。**这是最终采纳的方案。**

```bash
cd prototypes/pin-panel && cargo build --release
# 需打成 .app bundle 并设 LSUIElement=1 后运行
```

实测：连续 197 次采样 `isOnActiveSpace` 全程为 `true`，覆盖他人全屏应用期间未中断。

与 `pin-window` 的唯一差别是窗口类型（NSPanel vs NSWindow），
level / collectionBehavior / bundle 配置完全一致 —— 这构成单变量对照。

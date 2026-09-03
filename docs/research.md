# 竞品调研：主流截图工具 Top 5

> 调研时间：2026-09-02
> 目的：为 Snapzen 的功能取舍提供依据

## 概览

| # | 工具 | 平台 | 价格 | 开源 | 一句话 |
|---|---|---|---|---|---|
| 1 | ShareX | Windows | 免费 | 是 | 功能最强，自定义工作流 / 自动化 / OCR / 滚动截图 |
| 2 | Snagit | Windows, macOS | 约 $39/年 | 否 | 商业标杆，滚动截图 + 录屏 + 素材库 |
| 3 | CleanShot X | macOS | 买断制 | 否 | Mac 上体验最精致，剪贴板优先 + 悬浮托盘 |
| 4 | Lightshot | Windows, macOS | 免费 | 否 | 极简极快，按键即框选标注分享 |
| 5 | Greenshot | Windows（macOS 版收费） | 免费 | 是 | 轻量老牌，多捕获模式与导出目标 |

其他值得关注：**Snipaste（三平台，贴图功能是其杀手锏，见下方补充调研）**、Shottr（macOS 免费原生，社区口碑好）、Awesome Screenshot（浏览器整页截图）、Windows Snipping Tool（系统自带）、Gyazo（截完秒出分享链接）、Monosnap、Droplr。

## 逐个拆解

### 1. ShareX — 能力天花板

**强在哪**：捕获后工作流可完全自定义（截图 → 加水印 → 上传 → 复制链接，全自动串起来）；上传目标支持数十种服务；内置 OCR、滚动截图、录屏、取色器等一大堆工具。开源免费无广告。

**问题**：仅 Windows。配置界面信息密度极高，新用户几乎必然劝退。功能堆叠导致定位模糊 —— 它更像一个工具箱而不是一款截图工具。

**结论**：工作流引擎是它最独特的价值，值得借鉴；但必须重新设计交互，把复杂度藏起来。

### 2. Snagit — 商业标杆

**强在哪**：1990 年至今持续迭代，成熟稳定。滚动长截图做得最好；素材库便于复用历史截图；面向团队文档场景打磨充分。跨 Windows 与 macOS。

**问题**：订阅收费。体量偏重，启动与常驻开销明显。功能偏向文档制作，日常快速截图反而显得笨重。

**结论**：滚动截图和截图库是要抄的作业，但要做得更轻。

### 3. CleanShot X — 体验标杆

**强在哪**：交互设计是几款里最好的。截图自动进剪贴板；悬浮托盘暂存最近截图，避免桌面被文件糊满；标注、模糊、打码手感顺滑；隐藏桌面图标模式、定时截图、CleanShot Cloud 一键分享链接。

**问题**：仅 macOS，且付费。云服务不可自建。

**结论**：Snapzen 的交互基线应该对标 CleanShot X。剪贴板优先 + 悬浮托盘这两点是体验分水岭。

### 4. Lightshot — 极简速度

**强在哪**：路径极短，PrintScreen 之后直接框选、标注、分享，零学习成本。轻量。

**问题**：功能到此为止，没有工作流、没有滚动截图、没有历史管理。维护活跃度低。分享服务隐私性存疑（公开图床曾被批量爬取）。

**结论**："最短路径" 的思路必须保留 —— 无论 Snapzen 后面堆多少功能，默认路径都要保持这么快。

### 5. Greenshot — 轻量老牌

**强在哪**：开源免费，常驻开销小，捕获模式与导出目标灵活，Windows 上的稳妥默认选择。

**问题**：UI 陈旧。macOS 版单独收费，与开源免费的形象割裂。标注能力偏基础。

**结论**：轻量是优点，但"轻"不该以过时的交互为代价。

## 补充调研：Snipaste / Raycast / Paste

> 2026-09-02 追加。第一轮调研遗漏了 Snipaste，而它是对本项目最重要的参考对象。

### Snipaste —— 最重要的参考对象

**核心信息**（核实自官网 snipaste.com，注意 `snipastepro.com.cn`、`snipaste.ijinshan.com` 等均为仿冒站，信息不可信）：

- 平台：Windows（x64 / x86 / ARM64）、macOS（Universal）、**Linux（AppImage x86_64）** —— 三平台俱全
- 当前版本 2.11.x，2.10.x 系列有密集的迭代记录，维护活跃
- 技术栈：**Qt**（其 GitHub 组织下有 `Snipaste/qt`、`Snipaste/qt-patches` 两个仓库佐证），并非某些站点所称的 Electron
- 授权：个人使用免费；**自 2.0 起商业使用需付费许可**（$8.99 / 1 设备，$19.99 / 3 设备）
- 闭源，GitHub 上仅开放 `feedback`（★3.6k）与 `translations` 仓库

**杀手锏 —— 贴图（Pin to screen）**：截图后按 F3，把剪贴板内容（图片、文本、颜色值、HTML）变成一个**置顶浮动窗口"贴"在屏幕上**。对照参考、临时比对、盯着设计稿写代码这类场景，体验碾压"截图存文件再打开"的传统路径。

**这是前五名全都没有的能力。** ShareX、Snagit、CleanShot X、Lightshot、Greenshot 无一提供，而它恰恰是 Snipaste 用户黏性最高的功能。

**Snipaste 尚未覆盖的**：捕获后工作流与自定义上传目标、滚动长截图、OCR、录屏 / GIF、可搜索的历史库。Linux 端以 AppImage 分发，Wayland 下的表现需实测。

### Raycast —— 不是截图工具，但架构值得抄

macOS 起家的键盘优先启动器，现已支持 Windows 10/11。免费版 + Pro（$8/月起）。内置剪贴板历史（文本 / 图片 / 颜色 / 链接，可 pin），免费版保留 3 个月，更长需 Pro。

**值得借鉴的是它的扩展生态模型**：官方只做核心，上传目标、集成、小工具由社区以扩展形式提供，扩展用 TS/Swift/Python/Bash 编写。这直接给出了"如何拥有 ShareX 那样几十种上传目标，却不必自己全写一遍"的答案。

### Paste —— 剪贴板历史的 UX 标杆

macOS / iOS 剪贴板管理器，$3.99/月。把每一条复制记录做成**可视化卡片时间线**，支持 pinboard 分类、全局搜索、iCloud 跨设备同步。

**值得借鉴的是历史库的呈现方式**：Snapzen 的截图历史如果做成卡片时间线而非文件列表，检索效率会高一个量级。

## 对 Snapzen 的启示（已根据补充调研修订）

1. ~~跨平台一致性是最大的空白~~ —— **此结论有误，已作废**。Snipaste 已经覆盖 Windows / macOS / Linux 三平台。平台覆盖不是差异化点。
2. **贴图必须是一等公民**：这是前五名集体缺失、而 Snipaste 用户最离不开的能力。Snapzen 若没有贴图，对 Snipaste 用户就没有迁移理由。
3. **真正的空白是"贴图 × 工作流"的交集**：Snipaste 有贴图但没有自动化上传与滚动截图；ShareX 有工作流但没有贴图且只有 Windows。没有产品同时提供两者。
4. **商用免费是最实在的差异化**：Snipaste 个人免费但商用要买许可。一个 MIT 协议、公司可直接部署的替代品，价值明确。
5. **靠扩展生态覆盖长尾**：学 Raycast，核心只做捕获 / 标注 / 贴图 / 历史，上传目标与集成交给插件 API。
6. **能力与易用不必二选一**：ShareX 证明了能力上限，CleanShot X 证明了体验上限，没人同时做到。
7. **默认路径必须极快**：向 Lightshot 与 Snipaste 看齐，高级功能不能拖慢基础操作。
8. **本地优先，云可自建**：避开 Lightshot 的隐私争议和 CleanShot 的云绑定。

## 资料来源

- [Supademo — 10 Best Screenshot Tools (2026)](https://supademo.com/blog/screenshot-tools)
- [Efficient App — Best Screenshot Tools (2026): Ranked & Reviewed](https://efficient.app/best/screenshot)
- [Scribe — 11 Best Free Screenshot Software Tools Tested (2026)](https://scribe.com/library/free-screenshot-software)
- [DynoMapper — 50 Best Screen Capture Tools (2026 Edition)](https://dynomapper.com/blog/top-50-screen-capture-tools-for-taking-screenshots/)
- [ShareX — Wikipedia](https://en.wikipedia.org/wiki/ShareX)
- [Snagit — Wikipedia](https://en.wikipedia.org/wiki/Snagit)

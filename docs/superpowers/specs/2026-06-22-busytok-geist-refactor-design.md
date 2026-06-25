# Busytok 视觉重构设计 — 借 Geist 的克制，保留桌面工具气质

- **状态**：Approved（9 节逐节过审）
- **日期**：2026-06-22
- **参考**：[Vercel Geist Light](https://vercel.com/design.md) · [Vercel Geist Dark](https://vercel.com/design.dark.md)
- **范围**：`apps/gui` 前端视觉系统（token + material + 组件 + 主题 + 文档）。不涉及 Rust/service/CLI。
- **一句话**：**不是把 Busytok 做成 Vercel，而是做成一种"更克制、更可信、更桌面化的 Geist 风格审计工具"。借 Geist 的克制和秩序，保留 Busytok 的桌面工具气质。**

---

## 0. 已决的默认项（非问题，仅声明）

| 项 | 决定 | 理由 |
|---|---|---|
| accent hue | **保留 indigo** `#4F46E5` 家族 | 保留品牌，不做 Vercel 蓝 |
| 字体 | **保留 SF Pro / SF Mono**（系统字） | macOS-only 桌面工具，原生字最正确，不引入 Geist Sans |
| success 语义 | **绿色 = success** | 审计/运维约定俗成 |
| live vs success | **live 属 data/telemetry family；success 属 system health family**，两者保持区分 | 后台常驻 + 实时数据产品，语义不可互抢 |

---

## 1. 四条治理原则

1. **默认中性，状态稀缺** — 健康即安静；只有异常才上语义色。
2. **结构优先，颜色其次** — 层级先靠中性面阶梯 + 边框 + 间距；颜色是最后手段。
3. **面板是工具容器，不是视觉主角** — 卡片承载数字，不抢数字。
4. **实时信息要清楚，但不能吵** — 实时曲线可稍亮，但不与面板一起发光。

### 各节总括（review 准绳）

- ① 契约：**默认界面 = 不透明中性内容面 + 极轻 chrome；语义色只标记状态，不接管结构。**
- ② Titlebar：**Titlebar 负责系统健康感知，不负责系统细节展示。**
- ③ Sidebar：**Sidebar 读起来应像目录，不像一列可点击卡片。**
- ④ Metric：**Metric card 负责呈现读数，不负责表演状态。**
- ⑤ Overview：**Overview 的层级来自页面节奏和面板安静度，不来自堆叠的材质效果。**
- ⑥ Charts：**图表的任务是帮助读数，不是制造氛围。**
- ⑦ Palette：**Prompt Palette 的视觉目标是命令精度，不是内容陈列。**
- ⑧ Dark：**Dark theme 不是更炫的版本，而是更克制的读数环境。**

---

## 2. 执行骨架（方案 A — Token 先行，再逐层下传）

```
Phase 1  tokens + material contract + usage rules（只立契约，不动组件结构）
Phase 2  组件消费新 token，顺序：
         Titlebar → Sidebar → Metric cards → Overview → Charts → Prompt Palette → Dark 降温
Phase 3  文档与清理（删过期语言 / 废 token / 历史补丁 / 假抽象）
```

- 项目未发布，无历史包袱，可彻底重构；token 名可自由改名（机械迁移）。
- Phase 1 不改组件结构，只把"什么颜色/阴影/圆角/透明度是合法的"定死。

---

## 3. Material Contract（材质契约）— 本设计的核

> **材质只留在壳上，信息只活在实体面里。**

| 角色 | light | dark |
|---|---|---|
| canvas（app 背景） | 不透明 `#F4F5F7` | 不透明 `#0D1117` |
| **content surface**（卡片/图表/详情/对话框体） | **不透明** `#FFFFFF` | **不透明** `#171C24` |
| subtle surface（内嵌/次要分隔） | 不透明 `#F7F8FA` | 不透明 `#202732` |
| **chrome**（titlebar / sidebar） | 极轻 vibrancy `rgba(255,255,255,.94)` + `blur 8px` | 近不透明 `rgba(22,27,34,.96)` + `blur 0–4px`（supporting-only，实机仍飘则降 0） |
| blur 用于内容区 | **禁止** | **禁止** |

- 现状 3 档半透明面阶梯（`surface .85 / strong .96 / elevated .92`）**坍缩成 2 档不透明面** + 1 档 chrome。
- 对话框/抽屉/popover 一律不透明面 + 阴影，不再靠 blur 制造层级。
- dark content surface 用 `#171C24`（比旧 `#161B22` 亮一档），与 chrome 拉开半档，避免层级"黏"。

### 语义色异常层级（写死）

- `info / success` → dot / chip / small inline label
- `warning / degraded` → chip + 1px semantic border 或 left rail
- `danger / blocking` → 才允许更强的 semantic container（semantic border）

---

## 4. Token Contract（Phase 1 落地）

### 4.1 Light

| token | 旧值 | 新值 |
|---|---|---|
| `--color-canvas` | `#EEF1F4` | `#F4F5F7` |
| `--color-surface` | `rgba(255,255,255,.85)` | `#FFFFFF`（不透明） |
| `--color-surface-strong` / `-elevated` | 半透明两档 | **删除**（并入上面 2 档 + popover 阴影） |
| `--color-surface-subtle`（原 `canvas-subtle`） | `#F5F7FA` | `#F7F8FA`（不透明） |
| `--color-chrome`（原 `sidebar`） | `rgba(238,241,244,.88)` | `rgba(255,255,255,.94)` |
| `--color-border-subtle`（原 `border-soft`） | `rgba(15,23,42,.06)` | `rgba(15,23,42,.07)` |
| `--color-border` | `rgba(15,23,42,.12)` | 不变 |
| `--color-border-strong` | `rgba(15,23,42,.20)` | 不变 |
| `--color-hover`（**新增**） | — | `rgba(15,23,42,.04)` |
| `--color-hover-strong`（**新增**，selected/pressed） | — | `rgba(15,23,42,.07)` |
| `--color-text` | `#111827` | `#1A1D23`（去蓝） |
| `--color-text-muted` | `#6b7280` | 不变 |
| `--color-text-faint` | `#9ca3af` | 不变 |
| `--color-accent-*`（indigo 家族） | `#4F46E5` 等 | **不变** |
| `--color-status-success / warning / danger` | `#6dba78 / #c29a55 / #d56a6a` | 不变；`-soft` 维持 `.14–.16`，**仅进 chip/pill/dot/1px 边** |
| `--material-glass-blur`（chrome） | `18px` | `8px` |
| `--material-glass-blur-strong`（sidebar） | `24px` | `8px` |
| `--material-glass-blur-subtle`（内容/遮罩） | `6px` | `0px` |
| `--material-shadow-card` | `0 10px 24px/.05` | `0 2px 2px rgba(15,23,42,.04)` |
| `--material-shadow-elevated`（**仅 floating 层**） | `0 18px 44px/.12` | `0 1px 1px rgba(0,0,0,.02), 0 4px 8px -4px rgba(0,0,0,.04), 0 16px 24px -8px rgba(0,0,0,.06)` |
| `--color-heatmap-empty` | `#ebedf2` | `#EDF0F3`（对齐 neutral 基底） |

### 4.2 Dark

| token | 旧值 | 新值 |
|---|---|---|
| `--color-canvas` | `#0d1117` | `#0D1117` |
| `--color-surface`（content） | `rgba(22,27,34,.92)` | **`#171C24`（不透明）** |
| `--color-surface-subtle` | `#11161d` | `#202732` |
| `--color-chrome`（原 `sidebar`） | `rgba(13,17,23,.92)` | `rgba(22,27,34,.96)` |
| `--color-border-subtle` | `rgba(255,255,255,.06)` | 不变 |
| `--color-border` | `rgba(255,255,255,.10)` | 不变 |
| `--color-hover`（**新增**） | — | `rgba(255,255,255,.05)` |
| `--color-hover-strong`（**新增**，selected/pressed） | — | `rgba(255,255,255,.08)` |
| `--color-text` | `#e6edf3` | 不变（primary） |
| `--color-text-muted` / `-faint` | `#8b949e` / `#6e7681` | 不变 |
| `--color-accent-400`（**dark 文字/选中用**） | `#818cf8` | 不变；**dark 选中/文字禁用 500/600** |
| `--color-status-*-soft` | `.16–.18` | 不变，**面积比 light 更小** |
| `--material-glass-blur` / `-strong` / `-subtle` | `0` | chrome `0–4`（supporting-only）/ 其余 `0` |
| `--material-shadow-card` | `0 10px 24px/.36` | `0 1px 2px rgba(0,0,0,.16)` |
| `--material-shadow-elevated` | `0 18px 44px/.48` | Geist dark popover 阶（同 light 数值，rgba 黑） |
| `--color-heatmap-empty` | `#232a35` | `#202732`（对齐 subtle） |

### 4.3 Radius — 角色映射（不是裸 token）

| 角色 | 值 |
|---|---|
| control / chip / input / segmented / keycap / sidebar-item | `6px` |
| card / panel / metric / popover / menu | `12px` |
| dialog / drawer / palette-shell / page-surface | `16px` |
| status-pill（表内）/ avatar / toggle | `999px` |
| **例外**：heatmap cell | `3px`（微格，匹配日历读法） |
| **禁止**在常规界面出现 | `18 / 20 / 22 / 24 / 32` |

### 4.4 Data 色降温

- 高频三色：`--color-data-primary`（hero indigo）/ `--color-data-live-primary`（实时更亮 indigo）/ `--color-data-neutral`（中性灰，次系列 + ranking bar）。
- `--color-data-secondary`（teal）/ `--color-data-tertiary`（violet）：**保留定义但降为低频**，仅 3+ 系列图表真需要时启用，优先用 indigo 明度阶。
- **永不自动彩色**（无"model 色"）。

---

## 5. Usage Rules（Phase 1 附录 → 后续升 Review Checklist）

1. 内容区禁止 `backdrop-filter`（仅 `.desktop-titlebar` / `.desktop-sidebar` / modal 遮罩例外）。
2. 语义色 `-soft` 不染整块卡片/面板，只进 chip/pill/dot/1px 边。
3. resting 面 = 不透明 surface + 1px `border-subtle` +（可选）极小 card shadow；**floating 层**（popover/dialog/drawer/menu/tooltip）才用 `shadow-elevated`。resting card **border first, shadow optional（可为 0）**。
4. 层级 = surface 2 档 + border 强度 + spacing；不靠 blur/大阴影。
5. 单个 view 圆角只用一族（card=12、control=6），禁混 6/18/24/32。
6. accent 只用于 focus ring / 当前选中 / 唯一主行动；不做大面积装饰。
7. 数字是 metric card 主视觉；卡片不抢数字。
8. 内容/面板 surface 一律不透明（dark 禁 translucent content）。
9. data 色默认只用 indigo + live + neutral；teal/violet 仅 3+ 系列图表时降频启用，优先 indigo 明度阶。
10. dark 阴影用黑、更短（Geist dark 阶）；resting 面 border-first 可 0 shadow。
11. accent 文字/选中：light 用 `accent-600`，dark 用**亮档 `accent-400`**；mid 500/600 不作 dark 文字色。
12. dark segmented/toggle/selected control 禁大面积高饱和块；只允许亮档 accent 文本 + 细边 + 极低 alpha 托底。
13. dark 边框 = 结构提示，非描边装饰；同一视图勿让多 panel 同时显著描边。
14. live（data/telemetry）与 success（system health）在 dark 保持区分，不互抢语义。
15. status-soft 在 dark 只进 dot/pill/border，面积比 light 更小。

---

## 6. 组件契约

### 6.1 Titlebar（#1 优先项）

**健康默认态**：唯一常驻 = 1 个 calm chip。

- 文案默认 `Live capture active`（窄宽 fallback `Capture active`）；**不写** `Service ready / queue 0 / lag 0ms` 机械遥测串。
- chip 形态：**6px 矩形**（非 pill），高 26px；底 `--color-surface-subtle`（**不绿底**）；1px `--color-border-subtle`；左侧 6px **success 柔和档**绿点（heartbeat，非荧光绿、非 CTA）；文字 12.5px / weight 500 / `--color-text-muted`。
- queue 深度 / 聚合 lag / 连接状态 **全部移出标题栏**，进 popover。
- 右组：页面 toolbar（刷新 + range segmented）常驻；`justify-content: space-between`。
- **不在标题栏放页面标题**（页面 H1 由内容区承载）。
- 标题栏高 50px；左 ~72px traffic-light 留白（drag 区）；底 `--color-chrome` + 底边 1px `--color-border-subtle`；`data-tauri-drag-region` 保留。

**异常升级态**（单一入口，原地升级 neutral→warning→danger）：

- degraded / reconnecting / backlog / lag-high → chip 升 warning：底 `status-warning-soft` + 1px 琥珀边 + 琥珀点（可极轻 pulse）+ 具体状态文案。
- **仅"需立即用户处理"**（service down / 权限缺失 / 须即决策）才允许旁边 +1 danger 入口（带 action）。`budget exceeded` 等可感知非阻断问题仍走单 warning。规则：**正常=1 / 异常=1 升级 / 仅紧急=1+1**，永不回到一排胶囊。

**popover**（点 chip 展开，~280px）三段：

```
SERVICE / ● Ready · Normal · unix-socket · pid
LIVE    / Connection · Queue depth · Aggregate lag · Last event · Last sync
ACTIONS / read-only detail + 已有导航动作（View Activity / Open Settings）
```

- 不透明 `--color-surface` + `shadow-elevated` + 1px `border` + r12；label uppercase 11px faint，value 13px text `tabular-nums`；分段线 1px `border-subtle`。

**状态数据边界（写死）**：Titlebar Phase 2 继续以 `shell.status`（`useShellStatus()` → `ReadinessStateDto` / `StatusChipDto`）为**唯一状态来源**，复用现有 `StatusChip` popover 体系；前端只做 adapter / view-model 收敛（readiness + queue depth + aggregate lag + connection 收进单 chip + popover），**不新增并行健康状态机、不重写 DTO**。popover **read-only**：detail + 已有 `open_activity` / `open_settings` 导航动作；**本次不新增** `Restart service` / `Diagnostics` 等壳内运维动作——若需要，单列为后续 backend/protocol follow-up，不混入本次纯前端重构。

### 6.2 Sidebar

- 顶部**不加** branding；padding 顶 `18→12`；纯目录从第一组起。
- 分组：`MONITORING`（Overview, Usage）/ `TOOLS`（Prompt Palette）/ `SYSTEM`（Settings）。label = uppercase 11px `--color-text-faint`（IA，非装饰），三组样式一致；孤立项下不挂 label。
- item：高 `36→32px`，padding `0 12px`，r6，图标 16px / stroke 1.75。
  - **rest**：文字+图标 `--color-nav-text`（用于 primary navigation rest state，区别于 helper/secondary copy `--color-text-muted`），透明底。
  - **hover**：底 `--color-hover`，无 border/阴影。
  - **active**：**删** 整条 accent-tint 块；= accent 文字+图标（light `accent-600` / dark `accent-400`）weight 500 + **2px 左 inset accent 竖条** + **极弱 neutral 托举**（`--color-hover` 级，**绝禁 accent 染底**）。
  - **focus-visible**：2px `--color-focus-ring` inset outline。
- 容器：底 `--color-chrome` + 右边 1px `--color-border-subtle`。

### 6.3 Metric cards

- **默认（含 success）**：无 wash / 无 top-accent / 无 dot / 无阴影，仅 1px `--color-border-subtle`；不透明 `--color-surface`；r12；padding 16/18。数字 28px ~600 `--color-text` `tabular-nums`（**永远中性**）。**删除 `--success` 视觉变体**（success=neutral）。
- **helper**：默认 `--color-text-muted`；仅极短状态词/dot 带语义色，**不染整行**。
- **异常**：**永不整卡染底**。
  - warning：2px **顶 flag**（amber，满宽/贴顶）+ label 旁 6px amber dot。
  - danger：2px 顶红 flag + **1px 边变红**（semantic container 档）。
  - 数字与背景**永不变色**。
- **比例写死**：top-level label `11px` / value `28px` / helper `12px`；secondary（breakdown/detail）label `11px` / value `20px` / helper `11–12px`。嵌套 metric 永远比顶层更安静一档。
- 网格 3 列，gap `14→12`。

### 6.4 Overview 三层（page shell / section panels / in-panel emphasis）

- **page shell**：`.overview-console` 单一 `--space-section-gap: 24px`（顶层块间）；块内网格更紧（metrics 12 / charts-row 16 / rankings 16）。内容 `max-width: 1600px` 居中。外边距 `24px` 横向。
- **section panels**（全删 panel 上的 `shadow-elevated`；圆角全 12）：
  - **Tier A primary**（Usage Trend / Real-time Throughput / Heatmap）= `--color-surface` + **`--color-border`**（强）+ 无阴影。
  - **Tier B summary**（metric row）= `--color-surface` + `--color-border-subtle` + 无阴影。
  - **Tier C supporting**（rankings / recent activity）= `--color-surface-subtle` + `--color-border-subtle` + 无阴影。
- **in-panel emphasis**：title（16/600/text，左上，下 1px `border-subtle` 分隔 header/body）→ data（最强对比/最大面积）→ aux（total/legend/summary，更小 muted，仅 header trailing 或 footer，**禁成第二视觉中心**）。
- **状态住进 frame**（不撑坏结构）：
  - panel-local loading：保留 frame，body 优先**骨架**（图表→低对比骨架曲线 / 表格→骨架行 / 统计→占位数字框），退路单行 `Loading…`。
  - panel-local error：保留 frame，body 一行错误 + 内联 retry（tertiary）。文案 Geist 口吻：`Could not load usage data.` + `Retry`。
  - empty：保留 frame，空态提示 + 首动作。
  - degraded（页级非阻断）：顶部**薄 ribbon**（琥珀点 + 一句 + 可选 action），非居中 `PageState` 大卡。
  - catastrophic（summary 完全不可用）：唯一替换全页 `PageState`（按新契约重样式，无重阴影）。

### 6.5 Charts（读数化）

- **线**：趋势 `--color-data-primary` 1.75px；实时 `--color-data-live-primary` 2px + 右端 4px 当前值 dot（**末端定位器，非常驻装饰，无 halo/glow/pulse**）。**stroke 强制显式消费 chart token，禁回退纯黑/近黑**（dark 安全）。
- **填充**：极轻，顶 ≤8% → 底 0% 渐变。
- **grid**：默认 3–4 条水平细线 `--color-border-subtle`，**垂直 grid 禁用**。
- **axis**：轴线去或 `border-subtle`；刻度 11px `--color-text-faint`。
- **基准/目标线**：1px dashed `--color-border`（中性）或 amber（仅阈值）。
- **tooltip**：不透明 `--color-surface` + `shadow-elevated` + 1px `border` + r6；label 12/600 + value 11 muted `tabular-nums`。
- **多系列**：主 indigo / 次中性灰 / 第三 teal/violet（优先 indigo 明度阶）。
- **热力图**：empty = 中性基底（light `#EDF0F3` / dark `#202732`）；L1–L4 indigo 离散阶（light 加深 / dark 提亮）；cell 13px **r3 例外**；**legend 固定 5 格**（empty+L1–L4），稀疏模式不缩档。
- **rankings**：bar 默认**中性灰**（`--color-data-neutral` / gray-alpha ~.06）；**仅 #1 / hover / selected** 升 indigo（~.12），**禁多行常驻 accent**；value `tabular-nums` `--color-text`；容器 Tier C（`surface-subtle` + `border-subtle` + 无阴影 + r12）。

### 6.6 Prompt Palette（命令面板，非内容浏览器）

- **主次**：search（18/500，去 hero 感，placeholder 与正文都稳）→ list rows（title 15/600 中性）→ actions（overlay 内**无行内按钮**，收进 `⌘K` 菜单；page 列表行内按钮维持 `recessed`）。
- **shell**：surface **r32→16**；不透明 `--color-surface` + 1px `border` + `shadow-elevated`；backdrop **删 radial 顶光晕**，留 `--material-overlay-scrim`（可选 6px blur 聚焦）。window 形态同 r16、无 shadow。
- **row**：min-h 44，r**14→6**；hover `--color-hover`；**selected = 中性 lift（`--color-hover-strong`）+ 2px 左 accent 竖条 + title 维持中性高对比**（**主识别靠背景层级+位置，不靠颜色**；accent 仅 left rail / 极小 cue；与 sidebar/titlebar 同语法）。
- **accessory 降噪**：默认只露必要 metadata，hover 才展开更多 affordance，selected 不点亮全部 icon。`__pin` 去 success 绿 pill → 中性 ◇ glyph / `PIN` mini-label（`--color-text-faint`）；`__tags` text-muted 12px，最多 2 + 溢出。pin/tags/recent **永不语义色**。
- **keycap / close**：移除 `box-shadow-card`，保留 2px 底 border 实体键触感，r6。footer hint = **命令参考非工具栏**，文本弱 list 两档，keycap 仅可学性、不成按钮群视觉中心。
- **⌘K menu**：r**16→12**，不透明 + border + elevated shadow；item r6，hover `--color-hover`。
- **共享语法范围（4 载体，写死）**：`PromptPaletteOverlay` + `PromptPaletteOverlayController` + `PromptPaletteWindowApp`（`presentation="window"`）+ `PromptPalettePage` **四处共享同一 row / selected / hover / accessory 语法**，仅密度与 actions 组织不同。overlay 与 window 经 `PromptPaletteOverlayController` 的 `presentation` prop 切换，**不得分叉出第二套样式**。

---

## 7. Dark 降温（独立系统，非 light 反色）

- **text 三档明确**：primary `#e6edf3` / muted `#8b949e` / faint `#6e7681`。
- **易"发光/发灰/发脏"6 处排查**：translucent content surface（→不透明）/ 多 data tint 同屏（→收敛）/ fill >15%（→≤8%）/ shadow-elevated .48（→Geist dark 阶）/ status-soft 大面积（→仅 dot/pill/border）/ accent 大面积（→仅 rail/dot/focus/单主行动）。
- **accent 存在感不刺眼**：文字/选中亮档 `accent-400`；rail/focus/dot `accent-500`/400；active 极弱托举 = accent 极低 alpha（~.10），非 `accent-50` 深紫块。靠"更亮 hue + 极小面积"，不靠饱和度/面积。
- **独立校准**：chart line `#8d9bff`/`#a7b8ff`，fill ≤8%，grid `rgba(255,255,255,.06)`，axis faint；heatmap empty `#202732` + 提亮 indigo 阶；chip/pill soft 仅小面积；chrome `rgba(22,27,34,.96)` + blur 0–4。

---

## 8. 文档治理（Phase 3）

### 8.1 新单一真源

- **新建 `DESIGN-SYSTEM.md`**（canonical visual contract）：治理原则 + material/token 契约 + usage rules + 各组件契约 + Review Checklist + Do/Don't。
- **`tokens.css` = 可执行契约**（`tokens.test.ts` 守护）；文档与 token 共同演进。
- **`DESIGN.md`**：明确**非规范身份**——`DESIGN.md = narrative overview`（架构叙述），`DESIGN-SYSTEM.md = canonical visual contract`。其 "Visual design" 段改为指针，删 Sentri/violet-lime/Rubik/Monaco 等废弃描述。

### 8.2 旧语言全删

- **删除 `THEME.md`**（551 行，100% 废弃 Sentri 系统，无 salvage 价值）。
- **清扫 CSS 内联 `/* per spec */` 旧注释**（`surfaces/components/pages.css`），更新/删除使其匹配新契约。
- **清死 token**：坍缩后的 `--color-surface-strong/-elevated` 半透明档、归零后未用的 `--material-glass-blur-subtle` 等；消灭"假抽象"。

### 8.3 Review Checklist + 自动守护

- 15 条 usage rules + 组件规则 → `DESIGN-SYSTEM.md` `## Review Checklist`（勾选格式），作 PR review gate。
- 能自动化的**优先扩展现有** `tokens.test.ts` 与 `scripts/check-busytok-gui-surfaces.sh`（bash `rg` guard + capability 断言）；只有当现有守护无法表达某条规则时，才补最小的新检查工具，**不另引新 lint 工具链**：
  - `backdrop-filter` 仅在 chrome 选择器；
  - **裸 hex 默认禁，例外白名单**（品牌 logo 资源 / 第三方图表库必须内联的 fallback）；
  - resting panel 选择器不挂 `shadow-elevated`；
  - `metric-card--success` 视觉变体不存在；
  - dark accent 文字消费 `accent-400` 而非 500/600。

### 8.4 Do / Don't（写入 DESIGN-SYSTEM.md，每关键主题一组）

```
Do:    neutral surface + subtle border
Don't: accent-tinted full card

Do:    one calm chip in titlebar (healthy)
Don't: a row of telemetry capsules

Do:    border-first resting panel, shadow optional
Don't: shadow-elevated on a resting card

Do:    single indigo line + ≤8% fill
Don't: multi-color glow chart

Do:    selected row = neutral lift + left rail
Don't: selected row = accent-tinted block + accent title

Do:    dark accent text in accent-400
Don't: dark accent text in accent-500/600
```

### 8.5 Sync list（共同演进）

| 文件 | 角色 | 同步对象 |
|---|---|---|
| `tokens.css` | 可执行契约 | ↔ `DESIGN-SYSTEM.md` token 表 |
| `tokens.test.ts` | 契约守护 | 断言 token 存在 + 关键值 + usage rule 守护 |
| `surfaces/components/pages.css` | 消费层 | 仅消费 token，禁裸 hex（白名单外） |
| `AppShell` / `Sidebar` / `OverviewPage`（+ overview panels）/ `PromptPaletteOverlay` + `PromptPaletteOverlayController` + `PromptPaletteWindowApp` + `PromptPalettePage` | 组件实现 | ↔ 各组件契约段 |
| `themeRuntime.ts` | 主题切换 | 正确接线新 token |
| 自动守护测试 | 回归闸门 | 保持 green |

---

## 9. Non-goals（非目标）

- 不重构 `OverviewPage` 的数据流/组合与各 panel 的独立加载架构。
- 不更换图表库。
- 不新增功能：loading 骨架是最小必要新增；**Titlebar popover read-only**（detail + 已有 `open_activity` / `open_settings` 导航动作），本次不新增 `Restart` / `Diagnostics` 等壳内运维动作（需则后续 backend/protocol follow-up）。
- 不触碰 Rust / service / CLI / 打包。
- 不引入 Geist Sans 字体（保留 SF Pro）。

---

## 10. 风险与回滚

- **风险**：去 translucency 后，chrome 与内容面对比是否足够 → 已用 dark `#171C24` 与 chrome 拉开半档；实机验证，必要时微调 canvas。
- **风险**：图表库默认 stroke 回退近黑 → 已立规则"强制显式消费 chart token"，并在 Phase 2 验证。
- **回滚**：Phase 1 仅改 `tokens.css` + 新增文档，不动组件结构；若整体观感回退，可单点 revert token 值而不影响组件代码。Phase 2 按组件顺序推进，每组件独立可 review/可回滚。

# GUI 控件系统收敛设计

- **状态**：Draft
- **日期**：2026-06-24
- **参考**：[Vercel Design](https://vercel.com/design) · [Geist Theme Switcher](https://vercel.com/geist/theme-switcher)
- **范围**：`apps/gui` 组件体系、样式分层、设置页与常见交互控件契约。**不涉及** Rust/service/CLI。
- **一句话**：**先收敛控件契约，再迁移页面。不是“修一处尺寸”，而是消灭平行控件体系。**

---

## 1. 目标

将当前 GUI 中“视觉上像同类、代码里却是多套实现”的控件系统收敛为一套偏 Geist / Vercel 风格的、克制且可复用的 canonical controls。

本项目采取 **收敛优先** 策略：

1. 先统一组件语义、尺寸等级、状态表达、样式归属。
2. 再让页面逐步迁移到统一控件。
3. 不先做大面积视觉翻修，不做“边改页面边发明新控件”。

---

## 2. 问题定义

这次问题不是单页 CSS 漂移，而是组件体系已经分叉。

### 2.1 当前症状

- Settings 页面同类 control 尺寸、对齐、密度不一致。
- Prompt Palette 底部存在写死的提示 chrome，不可裁剪。
- 只读值、可操作控件、状态值、值+按钮复合区，视觉上都被塞进同一“右侧 control 槽”，但没有统一规则。

### 2.2 已确认的根因

1. **同语义存在多套实现**
   - `Theme` 使用共享 [`SegmentedControl`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/desktop/SegmentedControl.tsx:13)
   - `Week starts on` 使用页面私有 `segmented-group` / `segmented-label`
   - 多个布尔设置项使用页面私有 `toggle-label` / `toggle` / `toggle-track`
   - `Default action` 使用共享 [`AppSelect`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/Select.tsx:12)
   - `Reporting timezone` / 诊断值使用裸 `diag-value`
   - 失败态+按钮使用 `manual-root-controls`

2. **`SettingsRow` 只是布局壳子，不是设置项契约**
   - [`SettingsRow.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/desktop/SettingsRow.tsx:3) 只提供 label / description / control 插槽
   - 它不定义 control 的合法形态、宽度、尺寸、对齐、状态位置

3. **基础样式被页面层重写**
   - `.settings-row__control` 在 [components.css](/Users/wsd/Data/Busytok/busytok/apps/gui/src/styles/components.css:430) 与 [pages.css](/Users/wsd/Data/Busytok/busytok/apps/gui/src/styles/pages.css:1336) 同时存在
   - 说明组件边界和样式分层已经失效

4. **Prompt Palette surface 不可裁剪**
   - [`PromptPaletteOverlay.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx:350) 直接渲染底部 hints footer
   - footer 是组件内建 chrome，不是可选 surface slot

---

## 3. 设计原则

### 3.1 Canonical control

同一语义只允许一个共享控件实现。

- 主题选择器只能有一套
- segmented choice 只能有一套
- select 只能有一套
- 只读 value 只能有一套
- 状态值与复合控制只能有一套

### 3.2 Size is a system decision

尺寸不是页面自己决定，而是系统预设。

首期只保留两档：

- `default`：设置页、标准表单、主要配置界面
- `dense`：工具条、筛选器、Prompt Palette、紧凑交互区

### 3.3 Structure before styling

先明确：

- 这是不是 control
- 它是否可交互
- 它是否只读
- 它是否是 value + action 的复合块

再决定样式。

### 3.4 Page cannot redefine component contracts

页面样式只能做布局编排，不能重写基础控件的高度、padding、对齐规则、圆角和状态位置。

### 3.5 Geist / Vercel 风格的克制

参考 Geist 的关键点：

- 一个语义一个 canonical control
- 设置页使用默认尺寸，dense 只用于紧凑 chrome
- 控件视觉语言一致，不为单页定制独特外观
- 只读值和可交互控件清晰区分

---

## 4. 目标架构

建议在 `apps/gui/src/components` 内形成三层：

### 4.1 Foundation

提供 token 与尺寸语义，不承载页面业务。

Foundation 层**建立在现有** [`DESIGN-SYSTEM.md`](/Users/wsd/Data/Busytok/busytok/DESIGN-SYSTEM.md)、[`tokens.css`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/styles/tokens.css)、`surfaces.css` 之上，不重新发明通用设计 token。

它新增的是**控件级语义常量**，供 canonical controls 统一消费，例如：

- spacing
- radius
- control heights
- icon sizes
- label sizes
- interaction colors

### 4.2 Canonical controls

共享控件层，作为所有页面唯一来源。

- `SegmentedControl`
- `ToggleSwitch`
- `Select`
- `StatusPill`
- `SettingsValue`
- `SettingsStatus`
- `SettingsActionGroup`
- `Combobox`

### 4.3 Layout contracts

负责页面中的“语义布局容器”。

- `SettingsRow`
- 后续如有需要，可补 `SettingsSectionPanel`

---

## 5. 规范化后的控件模型

### 5.1 `SettingsRow`

`SettingsRow` 升级为“设置项契约”，不再只是左右布局壳子。

这里**不采用封闭式“按枚举渲染所有控件”**的方案；`SettingsRow` 仍保持开放接口，继续接收 `ReactNode`，避免它膨胀成业务编排器。

选择这个方案的原因：

- 现有工程已经有共享控件基础，问题在于没有 canonical control 约束，而不是 `SettingsRow` 缺少渲染分支
- 若改成封闭契约，`SettingsRow` 将不得不“知道”每一种控件和复合块的内部结构，边界会变重
- 对于 `composite` 这类组合形态，开放接口更利于复用共享子组件，而不是把组合逻辑埋进 `SettingsRow`

因此，`SettingsRow` 的职责是：

- 规定 settings item 的左右布局语义
- 规定 control 区的合法布局方向与辅助文案位置
- 限定可传入的控件**必须来自 canonical controls 家族**

它承认以下 control 家族：

- `choice`
- `toggle`
- `select`
- `value`
- `status`
- `action`
- `composite`

其中：

- `value`：纯只读值，例如 timezone
- `status`：只读但带健康/异常语义
- `action`：单操作或单按钮
- `composite`：由共享子组件拼装的“值 + 次级操作”，例如 `SettingsStatus + secondary button`

对于布局方向冲突，canonical `SettingsRow` 明确提供两种 control 区布局语义：

- `horizontal`：单控件或单行复合块
- `vertical`：control 下方承载 helper / error / 次级动作

error / helper 不再依赖页面私有覆写去把 `.settings-row__control` 强行改成 column。

### 5.2 `SegmentedControl`

作为 segmented choice 的唯一实现。

必须支持：

- `size: default | dense`
- `stretch` 或等价宽度策略
- 统一 focus / hover / active / disabled

页面私有 `segmented-group` / `segmented-label` 删除。

### 5.3 `ToggleSwitch`

作为布尔开关的唯一实现。

必须支持：

- `size: default | dense`
- checked / unchecked / disabled / focus-visible
- label 与 description 的标准对齐
- 和 segmented / select 相同的 control height 体系

页面私有 `toggle-label` / `toggle` / `toggle-track` 删除。

### 5.4 `Select`

作为 select 的唯一实现。

必须支持：

- `size: default | dense`
- label 呈现模式
- 统一 trigger height / padding / icon size

### 5.5 `Combobox`

`Combobox` 是 `Select` 体系的**视觉近亲**，不是 `Select` 的实现分支。

这条规则专门覆盖 [`TagFilterCombobox.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/TagFilterCombobox.tsx:1) 这类组件：

- 它在交互语义上是 combobox，不是 select
- 它可以共享 select 的 dropdown / item / focus 视觉语言
- 它不能被强制替换成 `AppSelect`

因此 canonical 规则是：

- `Select` 与 `Combobox` 分属两个共享组件
- 二者共享一套 dropdown surface 与 item density 视觉契约
- 页面不得再写第三种下拉/选择器外观

### 5.6 `SettingsValue`

新建只读值组件，替代裸 `diag-value`。

职责：

- 统一只读文本视觉
- 支持 tabular numbers
- 支持 tone：`default | muted | warning | danger`
- 明确它不是 button / input / select

tone 语义写死如下：

- `default`：主文本强度，用于标准只读结果
- `muted`：次级只读信息，用于补充上下文
- `warning`：警示但非阻断，文字级提示，不做胶囊
- `danger`：失败或需要关注的只读值，文字级提示，不做整块容器

首期 `SettingsValue` 只负责**文本型值**，不承担 pill/badge 责任。

### 5.7 `SettingsStatus`

`SettingsStatus` 是 settings 场景中的状态展示组件，和现有 [`StatusPill`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/desktop/StatusPill.tsx:1) **不是同一个组件**。

边界如下：

- `StatusPill`：表格 / 列表中的紧凑状态 pill
- `SettingsStatus`：设置页 control 区中的状态值组件，可与 `SettingsActionGroup` 组合

`SettingsStatus` 的 tone 枚举为：

- `ok`
- `warning`
- `danger`
- `muted`

视觉规则：

- 默认优先使用文本 + 轻量状态点
- 仅在明确需要 capsule 时才复用 `StatusPill` 视觉语法
- 不把设置页状态一律渲染成列表 pill

### 5.8 `SettingsActionGroup`

新建复合控制容器，替代 `manual-root-controls`。

职责：

- 统一“值 + 按钮”“状态 + 按钮”“只读值 + link/button”布局
- 控制纵向或横向排列规则
- 保证 error / helper / action 的距离一致

它是 `composite` 形态的标准实现，不由 `SettingsRow` 自己拼业务内容。

---

## 6. 尺寸与视觉契约

### 6.1 首期尺寸等级

所有 canonical controls 只允许：

- `default`
- `dense`

禁止页面再出现第三种“自定义控件高度”。

### 6.2 同类控件的统一项

同一 size 档位下，以下属性必须统一：

- height
- horizontal padding
- border radius
- label size
- icon size
- gap
- focus ring strength

### 6.3 只读值和交互控件的区分

只读值不应伪装成交互控件：

- 不做 button 边框
- 不占用 select trigger 外形
- 不与 segmented / select 争夺同样视觉权重

### 6.4 状态表达的优先级

- 结构优先于颜色
- 颜色只用于状态点缀，不单独制造“另一套控件”
- 异常修复动作通过复合控制承载，而不是页面临时拼装

---

## 7. 首批收敛范围

### 7.1 基础件

- [`SettingsRow`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/desktop/SettingsRow.tsx:3)
- [`SegmentedControl`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/desktop/SegmentedControl.tsx:13)
- `ToggleSwitch`
- [`AppSelect`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/Select.tsx:12)
- `Combobox`
- `diag-value` 的组件化替代
- `manual-root-controls` 的组件化替代

### 7.2 首批迁移页面

- [`SettingsPage.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/pages/SettingsPage.tsx:480)
- [`PromptPalettePage.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/pages/PromptPalettePage.tsx:301) 顶部筛选区
- `Overview` / `Breakdown` / `Activity` 中的 shared segmented filters

### 7.3 Prompt Palette 本体

Prompt Palette 本期只做局部清理，不做整套 redesign。

其中一项明确要求是：

- 去掉 [`PromptPaletteOverlay.tsx`](/Users/wsd/Data/Busytok/busytok/apps/gui/src/components/prompt-palette/PromptPaletteOverlay.tsx:350) 的底部 hints footer

理由：

- 它属于内建 chrome，不是必须交互
- 它在 window 模式下占据较大视觉高度
- 这块知识可通过快捷键、菜单或文档承载，不应常驻 surface

---

## 8. 删除与禁止项

### 8.1 必删

- `segmented-group` / `segmented-label`：删除对应 **JSX 结构 + CSS class**
- `toggle-label` / `toggle` / `toggle-track`：删除对应 **JSX 结构 + CSS class**
- 裸 `diag-value`：删除直接在页面中使用的 **JSX + CSS class**
- 裸 `manual-root-controls`：删除直接在页面中使用的 **JSX + CSS class**
- Prompt Palette hints footer：删除 `PromptPaletteOverlay.tsx` 对应 **JSX** 与 `.prompt-overlay__hints` / `.prompt-overlay__hint` / `.prompt-overlay__hint-label` 对应 **CSS**

### 8.2 禁止继续新增

- 页面私有 segmented 实现
- 页面私有 toggle 实现
- 页面私有 select 实现
- 页面私有 combobox 外观实现
- 页面重写 canonical control 的高度或 padding
- 只读值伪装成可交互控件

---

## 9. 样式分层要求

建议收敛为：

- `components.css`：canonical controls only
- `pages.css`：page composition only

强约束：

- 基础控件的形态样式不允许留在 `pages.css`
- `pages.css` 不得重写 `SettingsRow` / `SegmentedControl` / `Select` 的核心契约

当前 `.settings-row__control` 的问题不是简单覆盖，而是**布局方向冲突**：

- `components.css` 版本偏向单控件横向对齐
- `pages.css` 版本偏向 control + helper 的纵向堆叠

canonical 迁移路径是：

- 组件层保留 `SettingsRow` 的标准 control 区契约
- 通过 `horizontal | vertical` 变体解决布局方向，而不是让 `pages.css` 覆写基础类
- error / helper / secondary action 的堆叠规则回收到组件层

---

## 10. 测试策略

### 10.1 组件测试

为下列共享件建立明确测试：

- `SegmentedControl`
- `ToggleSwitch`
- `Select`
- `Combobox`
- `SettingsRow`
- `SettingsValue`
- `SettingsStatus`
- `SettingsActionGroup`

覆盖：

- `default`
- `dense`
- disabled
- keyboard focus
- segmented group 内 keyboard navigation（含 arrow keys）
- long label
- error/supporting text

### 10.2 页面契约测试

Settings 页面新增回归断言：

- 不再出现 `segmented-group`
- 不再出现 `toggle-label` / `toggle` / `toggle-track`
- 不再出现 `diag-value`
- 不再出现 `manual-root-controls`
- `Theme` 与 `Week starts on` 均走 shared segmented control

Prompt Palette 新增回归断言：

- 不再渲染 `.prompt-overlay__hints`

### 10.3 覆盖率要求

首批变更文件保持高覆盖：

- canonical controls：目标 > 90% line coverage
- Settings 页面契约回归必须覆盖

---

## 11. 迁移策略

采用“契约先行，页面后迁”的两阶段方式。

### Phase 1

建立 canonical controls 与 layout contracts：

- 补齐共享组件
- 引入 size / variant 语义
- 清理样式分层
- 建立“页面不得覆盖 canonical control 核心尺寸”的 review / lint 约束

### Phase 2

迁移页面并删除旧实现：

- Settings 页先迁完
- 然后迁常用 filter / toolbar controls
- 删除旧类名和页面私有实现

项目未发布，因此本设计明确采用 **彻底替换** 策略，不做长期双轨兼容。

---

## 12. 验收标准

1. `SettingsPage.tsx` 不再直接使用 `segmented-group`、`segmented-label`、`toggle-label`、`toggle`、`toggle-track`、`diag-value`、`manual-root-controls`
2. `.settings-row__control` 不再被 `pages.css` 重写
3. segmented choice 在项目内只剩一套共享实现
4. settings toggle 在项目内只剩一套共享实现
5. settings select 在项目内只剩一套共享实现
6. combobox 在交互上独立、在视觉上遵守 shared dropdown contract
7. 只读值拥有明确共享组件，不再由裸 `span` 充当
8. Prompt Palette 不再渲染底部 hints footer
9. canonical controls 的测试覆盖率高于 90%

---

## 13. 结论

这不是“修一处 settings 尺寸”的任务，而是一次 GUI 控件契约收敛。

真正要修的是：

- 组件边界
- 样式归属
- 同语义唯一实现
- 尺寸与状态的系统规则

只要这四点不收敛，今天修 `Theme` 和 `Week starts on`，明天别的页面还会继续长出新的平行控件。

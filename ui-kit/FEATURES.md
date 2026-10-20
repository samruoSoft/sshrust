# ui-kit 组件功能记录（含验证清单）

> 用途：组件库**已实现功能与约定**的事实清单。每轮组件改动后同步更新本文件；
> 「⏳ 待验证」项按清单逐条人工验证（遵循 `~/.zcode/AGENTS.md` 的 UI 验证约定：不做自动化合成事件验证）。
> 状态标记：✅ 已人工验证 / 🔄 使用中（app 日常使用，未见异常）/ ⏳ 待验证。

## 总则（组件库红线）

1. **标准化原则**：基础组件要么不做，做了就在单个组件内把通用能力做全——边框、placeholder、
   首/尾图标、输入类型与显示控制、组件事件（change/enter/focus/blur…）、尺寸模板（Sm/Md/…）
   一律进组件本体，不散落在业务代码。
2. **滚动复用**：所有需要滚动的组件/页面一律复用 `pro::scroll_area`（真实滚动：内容真实渲染；
   虚拟滚动：调用方自管视口），不另写滚动/滚动条逻辑。范例：TextArea 虚拟模式接入。
3. **主题注入**：组件不自持颜色，统一 `.theme(Theme)` 注入（app 侧 `Palette::kit()` 转换）。
   **表面透明度**：`Theme.surface_alpha`（0-255，默认 255=不透明）——宿主做窗口透明时注入；
   组件的 bg/panel/card 表面一律经 `theme.surface(v)` 着色（透明时转 rgba，不透明时与
   `rgb(v)` 等价）；文本/描边/强调色保持不透明。**例外——浮层不吃透明**：dropdown 菜单、
   modal 卡片、树拖拽 ghost 这类悬浮层必须用不透明 `rgb(card)`（窗口模糊是整窗级
   NSVisualEffectView，浮层自身无模糊，透底=直接透视底下文字，不可读）。
   **暗色系组件边框色 = 分割线基线色**：app 侧 `PalFx::kit()` 把 border 槽位换成未提亮的
   基线 border（= `divider()`）——毛玻璃档组件描边与结构分割线同色隐约，不吃文字提亮；
   亮色系无提亮、值等价不变。
4. **分层**：base = 无状态/简单组件（RenderOnce 包装）；pro = 有状态复合组件
   （自渲染 Entity：宿主推数据 `set_*` + `cx.subscribe` 订阅事件）。

---

## base — text_input（TextField / InputField）

功能与属性：
- 单行输入；placeholder（构造参数 + `set_placeholder` 运行时更新）；`InputType { Text, Password, Number }`（Number 输入过滤非数字）
- 文本选择（拖拽选、全选 ⌘A、Shift 扩展）、光标渲染、水平 cursor-follow（光标移出视口自动滚）
- 事件：`TextFieldEvent { Change, PressEnter, Focus, Blur }`
- 包装器 `InputField`：边框、尺寸模板（InputSize::Sm/Md）、首/尾图标（svg 或任意元素）、
  `mono` 等宽、聚焦高亮描边

状态：✅ app 表单全部字段使用中（含 Password 显隐、Number 端口；Number 粘贴过滤、
Focus/Blur 事件时机 2026-10-16 验证通过）。

## base — text_area（TextArea / TextAreaField）

功能与属性：
- 多行语义（Enter = 换行）；`rows` 初始行数；placeholder（构造参数 + `set_placeholder` 运行时更新）
- **自带滚动**：内部经 `scroll_area` 虚拟模式接入（滚轮 → on_scroll_delta、滚动条拖拽 → on_goto、
  自管 scroll_y/scroll_x 渲染偏移）；Y 轴常驻，X 轴仅 `set_wrap(false)` 时出现
- 默认自动换行（词边界贪心折行；超宽"单词"按字素硬折）；`set_wrap(false)` → 不折行、横向滚动
- 光标跟随视口（仅光标 offset 变化时跟随，**不与用户拖动滚动条打架**）
- 选择（跨行）、IME marked range（组合下划线）；内容驱动高度（包装器不自算行高）

状态：✅ 自动换行 / X 轴显示与拖拽 / 滚动条贴合视口（第二十六轮修复后确认）；
IME 中文组合输入、base64 长串硬折行 2026-10-16 验证通过。

## base — button / form / modal / dropdown

- button：`ButtonKind × ButtonSize` 组合、`icon_button`；hover/按下态。
- form：`labeled_row` / `labeled_block` / `segmented`（分段选择器）。
- modal：`ModalHandle`（open/close）、overlay、Esc 关闭（`bind_keys`，key_context `ui_kit_modal`）、
  打开时聚焦（焦点句柄）；**卡片本体 on_mouse_down stop_propagation**（gpui 鼠标事件
  分发给帧内全部监听器，否则兄弟层遮罩会先于 click 配对完成关掉模态——"点按钮没反应"）。
- dropdown：`MenuEntry<A>`（**行首 icon**（svg 路径，随行文字色 hover 翻转）/label/trailing 元素、
  `item`/`danger_item`/`disabled_item` 无图标构造器与 `icon_item`/`icon_danger_item`/
  `icon_disabled_item` 带图标构造器）、纯函数高度计算 `menu_height`、
  贴边翻转锚点 `clamp_origin`、overlay 点击外部关闭（on_dismiss）、`on_select` 回调；
  菜单项带 `debug_selector("menu-item-{i}")` 供进程内测试定位。

状态：✅ app 右键菜单（dropdown，含贴近视口底边/右缘翻转）、确认删除弹窗（modal）、
按钮/表单布局使用中。

## base — tooltip

- 悬浮提示的**内容视图**（`Tooltip` + 便捷入口 `tip(text, theme, cx) -> AnyView`），
  挂到 gpui 元素 `.tooltip(move |_, cx| tip(...))` 上（仅 Stateful 元素支持）。
- gpui 0.2.2 框架负责延迟显示（悬停约 0.5s）、鼠标锚点定位、越窗缘自动翻转、
  按下/滚动消失——组件不提供方位/延迟参数（框架无此自由度）。
- 组件本体能力：text（每次弹出时由 builder 现取，**语言/主题切换后新弹即时生效**）、
  可选行首 icon（svg 路径）、text_size（默认 11px，图标随字号 +1）、max_w（默认 280px）、
  theme 注入。
- **浮层不吃透明**：卡片用不透明 `rgb(card)`（同 dropdown/modal 红线）。

状态：✅ app 右缘工具栏按钮使用中（多语言 tooltip；延迟手感、窗缘翻转 2026-10-16 验证通过）。

## base — panel（Panel / PanelSize）

- 三段式卡片容器（RenderOnce 无状态）：标题栏 + body + 底部工具栏，**三段按需开启**——
  给出 icon/title/tools 任一才渲染标题栏；给出 footer 才渲染底栏；body 可缺省
- 标题栏插槽：icon（svg 资产路径，宿主 AssetSource 解析，弱化色）→ title（溢出尾部省略）
  → 弹性空隙 → tools（右侧工具按钮区，任意元素——按钮自带监听、值徽标亦可放此槽）；
  **图标→标题间距 = 图标自带 mr_1（4px 默认间隔）**，标题左侧无额外 gap；
  solo（无 body 且无 footer）时省底边框避免与卡片外框叠双线
- 尺寸模板 `PanelSize`：Sm（标题栏 26 / 底栏 30 / 内边距 10 / 图标 12 / 标题 11px，
  信息密集面板）、Md（32 / 38 / 14 / 14 / 12.5px，默认）；`content_inset()` 暴露
  body 水平内耗（边框 1×2 + 内边距×2）供宿主算内容宽（单测锁定口径）
- `flush_body()` body 满铺（不加内边距，图表/列表全出血）；`bordered`（默认开）；
  **背景默认无色**（不铺底，透出宿主背景，仅边框分隔）——`.bg(色槽)` 给定才铺，
  经 `theme.surface()` 着色（随宿主透明度/模糊档同步：毛玻璃档半透明透入、不透明档实色）；
  标题栏底边框/底栏顶边框与卡片外框同用 theme.border（app 暗色系即分割线基线色）；
  body 包弹性容器（面板定高时占满余量，自然高度时随内容）

状态：✅ app 系统信息面板五分区使用中（Sm：icon+标题+%徽标 tools+body，无底色
`.bg()` 不给；边框=主题 border（app 暗色系即分割线基线色）；**四周 4px 留白**——
宿主容器 pl/py 4px、右侧 0（内容宽已扣 14px 滚动条区，thumb 左缘 −10 → 卡片右缘距
thumb 恰 4px 与左对称）、相邻面板间距 8px=4+4；留白视觉 2026-10-16 验证通过；
footer 插槽与 Md 模板暂无宿主用例）。

## pro — scroll_area

- 真实滚动模式（ScrollHandle，子内容真实渲染整棵）
- 虚拟滚动：`virtual_y(offset, max, page, track_h, on_goto, on_scroll_delta)` +
  `virtual_x(...)` builder；调用方自管视口偏移
- 多实例共存（全局 owner id 分配）；滚动条 hover 热区、拖拽；
  debug_selector（scroll-thumb-y/x）供进程内测试定位

状态：✅ TextArea 双轴（含回归测试 `tests/text_area_scroll_test.rs`）；🔄 树内嵌真实模式（侧栏）。

---

## pro — dialog（DialogWindow，模态风格对话框窗口）

- `DialogSpec { title, width/height, min_*, resizable(默认 true), minimizable(默认 false),
  zoomable(默认 false), stay_on_top(默认 true), surface_alpha(默认 None=不透明),
  surface_blur(默认 true) }`——`surface_alpha = Some(<1.0)` 时开窗按
  `spec.window_background()` 设 Blurred/Transparent（宿主全局窗口透明联动；
  快照语义，与主题快照一致，开窗后变化由内容视图自行对账）
- 置顶（"模式"效果，**app 内置顶**）：按激活态动态调级——app 激活时 floating(3)
  浮于本 app 全部窗口之上，失活时回落 normal(0)、可被其他 app 的窗口遮挡
  （macOS 窗口 level 跨 app 全局，静态 Floating 会盖住别的 app）；经 objc 通知实现
  （DidBecome/ResignActive 全局观察者运行时类 + 每窗 willClose 注销防悬垂指针，
  创建时经 keyWindow 标题校验防误伤；不阻塞其他窗口输入——gpui 无 app-modal）
- 按钮可选：默认仅关闭；`is_minimizable`/`is_resizable` 走 WindowOptions；
  隐藏最大化按钮由 objc `standardWindowButton(zoom)` setHidden 补齐
- 可 resize + 最小尺寸；内容自适应由内容视图负责（宿主全幅交给内容 Entity）
- 事件回调：`open_dialog_listening::<T, E>(content, spec, cx, on_event)`——内容实体
  发 E 即回调 `(事件, DialogCloser, App)`，回调内 `closer.close(cx)` 关窗；
  无事件用 `open_dialog`（E=NoDialogEvent）
- **窗体主题可指定**：`open_dialog_themed(content, spec, theme, cx)`（无事件版）；
  `open_dialog` = 深色默认、`open_dialog_ex`/`listening` 的 theme 参数同义——
  弹出窗配色跟随宿主 app 主题时传 `current(cx).kit()` 快照（窗体不随主题切换实时刷新，内容视图自行读全局则可实时）
- **底部工具栏插槽（footer）**：`open_dialog_ex(content, spec, Some(DialogFooter), theme, on_event, cx)`
  ——`DialogFooter::new(闭包)` 在**引用对话框时**给定内容与回调，闭包每帧以
  (内容实体, closer, App) 重建元素（文案/禁用态实时）；按钮用 `button_for`
  （base/button 实体版，无需宿主渲染上下文，回调形参与 button 一致）；
  内容区钉 [0, 视口高-footer_h]、footer 栏钉底（显式像素布局），高度在
  `DialogSpec.footer_height`（默认 48）
- `DialogHandle { is_open / activate / close / content }`；内容是任意 `Entity<T: Render>`
- 首个用例：app 新增/编辑服务器表单（app/src/form_service_edit.rs 作为内容）；
第二个用例：图标选择器（footer 确定/取消 + 事件回调写回，callback 路径实测走通）
- 终端弹出窗口**有意不用** DialogWindow（工具窗口语义，可最小化/最大化、不置顶）

状态：✅ app 表单/图标选择器使用中（置顶为 app 内、仅关闭按钮、拖角 resize 自适应、
最小尺寸约束、连接成功自动关窗、Esc/⌘W/红绿灯关闭、listening 回调路径——
2026-10-16 全部验证通过）。

## pro — tree

数据模型：`TreeNodeData { id, kind(Header/Group/Leaf), label, icon, icon_open, suffix,
suffix_ellipsis, subtitle, badge, dot, colors: NodeColors{icon,text,bg}, children,
expanded(首次播种), droppable }`

事件（对外全部内聚为事件）：`TreeEvent { Select, Activate(双击), Toggle, ContextMenu{id:Option,position},
Rename(回车提交；Esc/失焦弃), Move(拖拽) }`

内聚功能：展开状态内部管理（`set_expanded` / `expanded_ids` / `expand(id)` 主动展开单个节点
——宿主控制动态出现的分组用，其余展开态不动；读写同步；`expand_all` / `collapse_all`
批量操作——后者只保留最外层分组展开）、内联重命名
（`begin_rename`，Esc 由 TreeEscape action 处理；**编辑中的行忽略行点击**——否则点进
输入框会触发选中/折叠，分组行的编辑框被折叠掉即"无法重命名"）、拖拽 ghost +
droppable 分组高亮、内嵌 scroll_area（真实模式）；行带 `debug_selector("tree-node-{id}")`，
箭头盒/标题盒另带 `tree-arrow-{id}` / `tree-label-{id}`（对齐回归测试用）。

**简洁/详细双模式**（`set_detail(bool)` / `is_detail()`）：默认简洁 = 单行（标题 +
行尾 suffix）；详细 = 带 `subtitle` 的节点渲染两行行（行高 ×2−8，行 1 = 标题[弹性省略]
+ 状态点 + `badge` 徽章[带边框小框，9.5px]，行 2 = subtitle[10px 弱化、尾部省略]），
图标在行外垂直居中跨两行；编辑态与无 subtitle 节点保持单行。suffix/subtitle 可并存
（简洁显示前者、详细显示后者）。行高逐行设置（编辑行 +4、两行行更高），变高行安全。

视觉约定（第二十八/二十九轮定稿；第五十轮补箭头槽位语义）：
- 行高 26、缩进 14/级；行内元素（箭头/图标/标题/状态点/后缀）统一 **16px 等高盒**，
  文字加 `line_height(16)` 使字形在盒内垂直居中（修"文字偏上"）
- **展开箭头画在缩进区内（gutter），不占行内容空间**：槽宽 12px + 前后各 4px 间距，
  内容起点 = 首列基准 20px + 层级 × 步长——同层叶/分组（有无箭头）图标与标题
  严格左对齐，逐级右移一个步长（修目录面板 `.user.ini` 与 `.well-known` 错位、
  步长 12 < 箭头 16 时文件图标比父级还靠左的 bug；回归测试
  `tests/tree_align_test.rs`）
- 图标 → 标题间距 ≈ 8px（body gap_1 + 图标盒 mr_1）
- 后缀（如 IP:端口）10px、右对齐、占行内弹性余量；溢出按 `suffix_ellipsis`：
  - `Tail`（默认）：常规尾部省略 `text_ellipsis`
  - `Middle`：**同一 label 的中间省略**——两头保留、中间 `…`（头部约占保留量 2/3），
    随宽度收缩两头同步变短，极窄只剩 `…`；自绘 `MiddleLabel` 元素按实测宽度截断

状态：✅ app 侧栏使用中（选中/展开/双击连接/右键菜单/新增/删除/重命名入口/拖拽迁移自旧树）。
验证清单（2026-10-16 全部通过）：

| # | 验证点 | 操作步骤 | 预期结果 |
|---|--------|---------|---------|
| 1 | 行内垂直居中 | 观察各类型行（服务器/命令/分组/小节标题） | 箭头、图标、标题、IP 小字在同一水平中线，文字不偏上 |
| 2 | 图标-标题间距 | 观察任意行 | 图标与标题约 8px 间隔，不紧贴 |
| 3 | Middle 中间省略 | 拖窄侧栏看服务器行 IP | 两头保留中间 `…`（形如 `121.40.…2222`），随宽度同步收缩；极窄只剩 `…`；拖宽恢复完整 |
| 4 | Tail 默认省略 | 构造长标题节点（别名超长） | 标题尾部省略 `…`，不溢出不换行 |
| 5 | 状态点 dot | 连接一台真实服务器后观察 | 在线绿点（pal.ok）/离线灰点（pal.dim） |
| 6 | NodeColors 三槽覆写 | 在 app 或 demo 给节点单独设 icon/text/bg 色 | 三槽各自生效（常态）；选中态仍用主题选中样式 |
| 7 | 内联重命名 | 右键/新增后直接输入 | 回车提交 Rename；Esc/失焦放弃；编辑行高 +4、输入框选中全部 |
| 8 | 拖拽 Move | 拖叶节点到另一分组/根 | 目标分组高亮，落下后发 Move 事件（app 侧移动节点） |
| 9 | 展开持久化 | 展开部分分组后重启 app | 展开状态恢复 |
| 10 | 主题切换 | 切换明暗主题 | 树配色随 set_theme 更新（含选中态） |
| 11 | Header 行隔离 | 点击/右键小节标题行 | 不发 Select/ContextMenu(id)，不选中 |
| 12 | 内嵌滚动 | 节点多到溢出 | 滚动条出现、滚轮+拖拽可用，拖拽树节点时不受干扰 |

---

## platform — macOS 平台样式补丁

- `set_blur_tint(dark: bool)`：接管 gpui 窗口模糊（`WindowBackgroundAppearance::Blurred`
  底层 NSVisualEffectView，material=Selection、tint 跟随**系统**外观）的 appearance——
  深色主题 → VibrantDark、亮色 → VibrantLight（毛玻璃明暗倾向）。
  全局枚举 NSApp 全部窗口的模糊视图统一设置（幂等可重入）；**gpui 开窗/重开模糊会重建
  视图，重建后必须再调一次**（app 侧在各窗口 render 对账与开窗点接线）。
  **只设 appearance、绝不动 material**：gpui 默认 Selection 材质才有背后采样模糊，
  实验证实换成 HudWindow 后模糊整个消失（HUD 材质不采样背后内容，且其层背景被
  gpui 的 updateLayer 剥掉）。深色可读性由宿主表面色高 alpha+压暗保证，不依赖模糊层着色。
  无 NSApplication 环境（单测）安全返回；非 macOS 为 no-op。

## 预留 / 未实现（不在当前承诺内）

- 拖拽排序的顺序语义（Move 目前排到目标末尾；`sort` 字段预留）
- tree 虚拟滚动模式（当前真实滚动；超大节点数时再议）

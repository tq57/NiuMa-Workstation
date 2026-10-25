# NiuMa-Workstation · 项目代码说明

> 版本 0.1.0 · 仅 Windows · 桌面端（Rust + GPUI + SQLite）

## 1. 这是什么东西

「NiuMa-Workstation」是一个本地优先的个人办公桌面应用，包含四个模块和一个跨模块的置顶区：

| 模块 | 解决的问题 |
| --- | --- |
| 今日待办 | 今天到底要做什么：逾期 / 今天 / 未来 / 已完成 |
| 灵感速记 | 随手记下想法，打标签、置顶、一键转成待办 |
| 项目跟进 | 项目目标与需求、当前在做什么、下一步、卡在哪里 |
| 每周复盘 | 完成数、完成率、顺延次数、卡住项目、日分布、优先级分布 |
| 置顶区「今天要处理的」 | 逾期任务 + 卡住的项目，跨模块汇总到今日页顶部 |

所有数据存在本机 SQLite，不联网。发布形态有两种：安装版（Inno Setup）和绿色版（zip 解压即用，数据随程序走）。

## 2. 技术栈

| 项 | 选择 |
| --- | --- |
| 语言 | Rust stable，edition 2024，MSRV 1.92 |
| GUI | GPUI（`gpui-kit` 0.7.x，组件来自 gpui-component） |
| 存储 | SQLite（`rusqlite`，bundled，无需外部 DLL） |
| 日期 | `chrono`（`NaiveDate` / `NaiveDateTime`） |
| 配置 | TOML（`serde` + `toml`） |
| 打包 | Cargo workspace + Inno Setup 6 + PowerShell |

## 3. 目录结构

```
NiuMa-Workstation/
├─ Cargo.toml                     # workspace：core + app
├─ crates/
│  ├─ core/                       # niu_ma_workstation_core：纯领域层（只依赖 chrono）
│  │  └─ src/domain/
│  │     ├─ types.rs              # Todo / Project / Note / WeeklyReview / DataSnapshot
│  │     ├─ todo.rs               # 分区、排序、逾期天数、跨天顺延
│  │     ├─ project.rs            # 进度、状态联动、关联任务
│  │     ├─ stats.rs              # 周复盘统计
│  │     └─ date.rs               # 周一为周起点
│  └─ app/                        # 二进制 NiuMa-Workstation
│     ├─ src/main.rs              # 启动：配置 → DB → 种子 → 顺延 → 窗口
│     ├─ src/state.rs             # Workspace 状态、全部动作、根视图/标题栏/侧栏
│     ├─ src/views/               # 四个页面（today / notes / projects / review）
│     ├─ src/storage/             # SQLite：schema / repo / backup
│     ├─ src/theme.rs             # 双主题色板
│     ├─ src/config.rs            # config.toml 读写（当前只有主题）
│     ├─ src/seed.rs              # 首次启动的演示数据 + 回归断言
│     ├─ build.rs                 # 把 icon.ico 嵌进 exe
│     └─ assets/{icon.ico,icon.rc}
├─ docs/                          # 设计稿、技术方案、视觉 demo
├─ packaging/NiuMa-Workstation.iss      # 安装版脚本
└─ scripts/package.ps1            # 一键出安装版 + 绿色版
```

## 4. 分层与数据流

拆成两个 crate 的目的：领域层测试不需要编译 GPUI / SQLite 全量栈。

- **core**：纯函数 + 数据结构，不碰 IO、不碰 UI，可以秒级跑测试。
- **app**：所有 GPUI 渲染、SQLite 落库、配置文件读写。

数据流是单向的：

```
鼠标/键盘事件
   ↓
Workspace 动作（state.rs）
   ↓
修改内存快照 DataSnapshot
   ↓
Storage::save（单事务整库重写）
   ↓
SQLite 文件
   ↓
cx.notify() → Render 重绘
```

界面永远读 `Workspace.data` 这个内存快照，不在渲染过程中查库；每次动作改完就落盘，保证「界面看到的 = 磁盘里的」。

## 5. 领域层（crates/core）

### 5.1 数据模型

| 类型 | 关键字段 | 说明 |
| --- | --- | --- |
| `Todo` | `title` `priority` `date` `original_date` `project_id` `status` `done_at` `postpone_count` `postpone_log` | `date` 是计划日，可被顺延改写；`original_date` 永远保持第一次设置的日期 |
| `Project` | `name` `goal` `current` `next` `blocker` `status` `blocker_log` | 名称、目标需求与三项进展在新增/编辑弹框中统一填写；`current / next / blocker` 仍可在卡片上行内编辑 |
| `Note` | `content` `tags` `pinned` `created_at` `updated_at` | 灵感的标签从 `#tag` 里解析 |
| `WeeklyReview` | `week_start` `good` `watch` `focus` | 按周一日期聚合的三段文本 |
| `DataSnapshot` | `projects` `todos` `notes` `reviews` `seeded_at` `last_run_date` | 整库内存镜像，也是一次保存的最小单位 |

### 5.2 必须保持的领域口径

这些是产品设计的硬规则，也是种子数据的回归基准：

1. **五分区 `bucket(todo, today)`**：`Done` → `Someday` → `Future` → `Overdue` → `Today`，按顺序命中即返回。
   - 已完成的任务归 `Done`，没有日期的归 `Someday`，日期晚于今天归 `Future`。
   - **逾期判定看 `original_date` 而不是 `date`**，所以顺延过的任务只要原计划日已过，仍然算逾期。
2. **跨天自动顺延（Y2T）**：启动时若 `date < today`，把 `date` 改成今天、`postpone_count + 1`、写一条顺延流水；`original_date` 不变。隔 N 天没开应用也只补 1 次。
3. **项目状态由卡点驱动**：`blocker` 为空 → 进行中，非空 → 卡住了；`Done / Paused` 是终态，不会被自动覆盖。
4. **周复盘分母 = 本周到期 ∪ 本周完成**，完成率 = 本周完成 / 分母。
5. **周起点固定为周一**（`start_of_week`）。

### 5.3 文件职责

| 文件 | 主要函数 |
| --- | --- |
| `todo.rs` | `bucket` `sort_for_today` `group_future` `overdue_days` `is_repeated` `run_auto_rollover` |
| `project.rs` | `progress`（完成数/总数/逾期数/完成率）、`linked_todos`、`sync_status` |
| `stats.rs` | `week(snapshot, week_start) -> WeekStats`：本周完成、分母、完成率、逾期、卡住项目、顺延事件、日分布、优先级分布 |
| `date.rs` | `start_of_week`、周内日期计算 |
| `types.rs` | 所有领域结构体与枚举，不依赖任何 IO |

## 6. 存储层（crates/app/src/storage）

### 6.1 数据放在哪

| 形态 | 数据目录 |
| --- | --- |
| 安装版 / 开发运行 | `%APPDATA%\NiuMa-Workstation\` |
| 绿色版 | `exe 同目录\data\`（exe 旁有 `portable.flag` 时启用） |

数据目录下是 `NiuMa-Workstation.db` 和 `config.toml`；运行后还会生成 `backups/`（最近的数据库备份）。`portable.flag` 由打包脚本生成，安装版不会带这个文件。

首次启动时若新数据库不存在，会从旧版 `%APPDATA%\office_desk\` 迁移数据库、配置与备份（旧备份文件名前缀改为 `NiuMa-Workstation-`），迁移后旧目录保留不删。

### 6.2 表结构

- `meta`：`schema_version` 等键值
- `todos`：任务主体
- `postpone_log`：每次顺延的流水（原日期、新日期、时间）
- `projects`：项目主体
- `blocker_log`：卡点变化流水
- `notes`：灵感
- `weekly_reviews`：按 `week_start` 唯一的三段复盘

迁移只追加：`schema.rs` 里按版本号顺序执行，新版本只加新脚本，不改老脚本。

### 6.3 读写策略

- `Storage::load()`：把整库读成 `DataSnapshot`。
- `Storage::save(&snapshot)`：**单事务整库重写**（先清空再插入）。数据量是个人级别，简单可靠优先。
- `Storage::integrity_ok()`：打开时做一次完整性检查。
- `Storage::backup_to(path)`：用 `VACUUM INTO` 做一致性快照。
- `Storage::backup_now(stamp)`：备份到 `<数据目录>/backups/NiuMa-Workstation-<stamp>.db`，自动只保留最近 30 份。
- `Storage::auto_backup_daily(day)`：每天首次启动自动备份一次，当天已有备份则跳过。
- 内存存储 `open_memory()` 供单元测试使用。

## 7. 应用层

### 7.1 启动流程（main.rs）

1. 首次启动时把旧版 `%APPDATA%\office_desk\` 数据迁移到新目录（失败不阻断启动）。
2. 读取 `config.toml`（缺省浅色主题）。
3. 打开 SQLite，必要时建表。
4. 读 `DataSnapshot`，执行每日自动备份（`backups/` 下保留最近 30 份）。
5. 若是空库且从未种过（`seeded_at` 为空），灌入演示数据并标记。
6. 执行跨天自动顺延，并记录 `last_run_date`。
7. 打开 1120×600 的 GPUI 窗口（自绘标题栏，系统标题栏隐藏）。
8. 挂载 `Workspace` 作为根视图。

### 7.2 Workspace（state.rs）

一个 `Workspace` 就是整个应用的状态机：

| 字段 | 作用 |
| --- | --- |
| `storage` / `data` | 存储句柄与内存快照 |
| `page` | 当前页面（Today / Notes / Projects / Review） |
| `mode` / `palette` | 主题模式与当前色板 |
| `today` | 启动当天的日期 |
| `quick_input` / `note_input` / `note_edit_input` / `project_input` / `review_inputs` | GPUI 输入框实体（灵感行内编辑与新增项目各有一个专属输入框） |
| `project_modal` | `None` / `Create` / `Edit(id)`，控制新增与编辑项目弹框 |
| `project_modal_name / goal / current / next / blocker` | 弹框内五个编辑字段，`goal` 为多行输入 |
| `default_project` | 快速添加 / 灵感转待办时默认归入的项目（None = 不归入，点击 chip 循环切换） |
| `project_inputs` | 每个项目三个行内输入框（按项目 id 索引） |
| `editing_note` | 当前正在行内编辑的灵感 id |
| `project_filter` | 项目页状态筛选（全部 / 进行中 / 卡住 / 已完成） |
| `pinned_expanded` | 置顶区是否展开（默认只显示前 3 条，其余折叠为「还有 N 项」） |
| `project_focus` | 今日页只看某个项目的待办（从置顶区「看待办」进入） |
| `done_range` | 已完成区时间筛选（今天 / 7 天内 / 更早） |
| `confirm_delete` | 待办 / 项目 / 灵感删除的二次确认目标 id |
| `confirm_postpone_all` | 「全部顺延到明天」的二次确认状态 |
| `import_confirm` | 「导入」覆盖当前数据的确认态 |
| `undo` / `undo_seq` | 5 秒内可撤销的最近一次完成 / 顺延动作及其快照 |
| `tag_filter` / `show_done` / `week_offset` | 各页面的筛选与翻周状态 |
| `notice` | 右下角 toast 文案 |
| `clear_confirm` / `clear_input` | 「清理测试数据」的确认态与「输入清空」输入框 |

动作按模块分组：

| 模块 | 动作 |
| --- | --- |
| 待办 | 快速添加（可选归入项目）、勾选完成/恢复、顺延到明天、优先级轮换、行内切换所属项目、删除（二次确认）、逾期批量顺延（二次确认） |
| 灵感 | 新增（解析 `#标签`）、行内编辑、置顶/取消置顶、删除（二次确认）、转成待办（归入所选项目）、按标签筛选 |
| 项目 | 新增/编辑弹框（名称、目标需求、当前、下一步、卡点）、行内修改 `current / next / blocker`、按状态筛选、标记完成/恢复、暂停/恢复推进、删除（二次确认，关联待办解绑）；状态由卡点自动推导 |
| 复盘 | 三段文本保存、上一周/下一周切换 |
| 全局 | 切换主题（写 config.toml）、手动备份、导出 JSON、导入 JSON（覆盖前二次确认）、清理测试数据（输入「清空」后执行）、撤销最近一次完成/顺延（5 秒内） |

所有动作统一走 `commit()`：保存成功就重绘；保存失败则把错误写进 `notice`，不静默吞掉。

ID 生成用「微秒时间戳 + 进程内自增计数」，前缀区分 `t`（待办）/ `p`（项目）/ `n`（灵感）。

### 7.3 四个视图

- `views/today.rs`：置顶区（逾期任务 + 卡住项目，超过 3 条折叠为「还有 N 项」，卡住项目带「看待办 / 去处理」）→ 快速添加（含「归入 / 不归项目」选择 chip）→ 逾期 / 今天 / 未来（按日折叠）/ 已完成（本周完成数，展开后按今天 / 7 天内 / 更早筛选）；每条待办右侧有项目 chip（无项目时显示「＋项目」），点击循环切换所属项目；批量顺延与单条删除均需二次确认。
- `views/notes.rs`：输入框 → 「转任务时归入」项目选择 → 标签筛选条 → 置顶区 → 全部灵感；点正文进入行内编辑（回车 / 失焦保存），每条可置顶、删除（二次确认）、转为今天的待办（归入所选项目）。
- `views/projects.rs`：顶部新增项目输入 → 状态筛选 chips → 两列项目卡（卡宽 400）。卡片按「卡住 → 有逾期 → 进行中 → 已完成」排序，已完成卡降透明度；卡内直接编辑三个字段，状态胶囊由 `blocker` 推导，底部是关联任务进度与删除入口。
- `views/review.rs`：四个 KPI + 三张图（日分布柱状、优先级、精力分布）+ 卡点汇总列表（本周新增/未解除的卡点，显示项目名、卡点原文与持续天数）+ 三段复盘输入 + 周切换。

切换页面时自动聚焦该页录入框（今日 → 快速添加、灵感 → 灵感输入、项目 → 新增项目、复盘 → 第一段复盘）。

四页都启用 `overflow_y_scroll`，内容超出窗口时滚动，不撑破布局。

### 7.4 标题栏与主题

标题栏是**应用自绘**的 34px 条：

- 左侧 232px 与侧栏同色，显示「NiuMa-Workstation / 工作台」。
- 中段显示当前页面名，切换页面时同步更新。
- 右侧三个窗口控件（最小化 / 最大化-还原 / 关闭），用内联 SVG 绘制。
- 拖拽、双击最大化、最小化、关闭交给 Windows 原生处理（`window_control_area`），不自己模拟。

主题是两套完整色板（`theme.rs`）：浅色 `#f6f8f9` 底 + `#0f766e` 主色，深色 `#101614` 底 + `#2dd4bf` 主色。切换后立即生效，并写入 `config.toml`，重启保持。

所有可点击元素（导航、按钮、标签、优先级徽标、勾选框、删除）都设置了手形光标。

### 7.5 种子数据与清理

首次启动灌入的演示数据用于展示和回归：

- 3 个项目（Q3 增长方案 3/5、首页改版 3/4、个人博客迁移 2/3）
- 18 条待办、6 条灵感
- 本周完成 11、完成率 84.6%（11/13）、逾期 2、卡住 1、顺延事件 4（2 条逾期 × 各 2 次）
- 一周日分布 `0 · 2 · 2 · 1 · 2 · 2 · 2`
- 优先级分布 P0 4 / P1 3 / P2 4 / P3 0

侧栏底部的「清理测试数据」用于清空这些演示内容开始正式使用：点击后需在输入框里输入「清空」再点确认，避免误触。清空会重置待办、项目、灵感、复盘与各页筛选状态，并保留 `seeded_at`，因此重启不会再灌回种子数据。

侧栏底部的「备份 · 导出 · 导入」：备份写入 `<数据目录>/backups/`（自动保留 30 份，每天首次启动也会自动备份一次）；导出/导入为全量 JSON（数据快照），导入前需再点一次「确认导入」，成功后立即覆盖当前数据。

## 8. 构建、测试与打包

```powershell
# 开发运行（带控制台，便于看日志）
cargo run -p niu_ma_workstation

# 测试：领域层 + 应用层
cargo test -p niu_ma_workstation_core -p niu_ma_workstation

# 一次产出安装版 + 绿色版（release 构建）
powershell -ExecutionPolicy Bypass -File scripts\package.ps1
```

打包脚本做三件事：

1. 用 `RUSTFLAGS="-C target-feature=+crt-static"` 构建 release，静态链接 CRT，干净机器不依赖 VC++ 运行库；release 版带 `windows_subsystem = "windows"`，不弹控制台。
2. 生成绿色版目录 + zip：`exe` + `portable.flag`。
3. 调用 Inno Setup 生成安装版（当前用户安装、开始菜单、可选桌面图标、卸载器）。

产物在 `dist\`：

- `NiuMa-Workstation-<版本>-setup.exe`
- `NiuMa-Workstation-<版本>-portable.zip`

## 9. 测试与回归基线

- `cargo test -p niu_ma_workstation_core`：11 个领域测试（分区、排序、项目排序、顺延、进度、状态联动、周统计、周边界）。
- `cargo test -p niu_ma_workstation`：21 个应用测试（存储往返、v1→v2 迁移、备份保留 30 份、每日备份去重、旧数据目录迁移、种子回归、清理快照、新项目默认值、已完成区间筛选、项目关联循环、JSON 往返）。
- 种子数据的断言值就是产品口径：本周完成 11 / 完成率 84.6% / 逾期 2 / 卡住 1 / 顺延 4，改动领域逻辑时必须保持这些数字。

## 10. 约定与已知限制

- **仅 Windows**：依赖 MSVC 工具链与 Win32 窗口 API。
- **GPUI 仍是 pre-alpha**：升级 `gpui-kit` 时要选与 `gpui` 版本匹配的版本，图标、光标等 API 可能变动。
- 只支持单窗口、单实例数据目录；不做云同步。
- 可配置的只有主题；优先级命名、周起始日、快捷键在 v0.1 固定。
- 删除一律二次确认（按钮原地变成「确认删除」），没有跨动作的全局 undo；撤销只在完成/顺延后 5 秒内有效。
- 自动备份只在每天首次启动时触发，保留最近 30 份；手动备份按秒命名，同一秒内重复点击不会覆盖（`VACUUM INTO` 目标已存在会失败并提示）。

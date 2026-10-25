# NiuMa-Workstation

本地优先的个人办公桌面应用：**今日待办 / 灵感速记 / 项目跟进 / 每周复盘** 四个模块 + 一个跨模块置顶区「今天要处理的」。

原生 Windows 桌面程序，Rust + GPUI + SQLite，数据全部存在本机，不联网、不登录。

## 功能

| 模块 | 做什么 |
| --- | --- |
| 今日待办 | 逾期 / 今天 / 未来 / 已完成分区；快速添加、勾选完成、顺延到明天、优先级轮换、归入项目；逾期可批量顺延 |
| 灵感速记 | 随手记录，`#标签` 自动解析、打标签筛选、置顶、行内编辑、一键转成待办 |
| 项目跟进 | 新增 / 编辑弹框统一填写「名称 / 目标需求 / 当前 / 下一步 / 卡点」，卡片上仍可行内改进展；状态由卡点自动推导，按卡住 / 逾期 / 进行中 / 已完成排序 |
| 每周复盘 | 完成数、完成率、逾期数、顺延次数、日分布、优先级分布、卡点汇总 + 三段手写复盘，支持翻周 |
| 置顶区 | 逾期任务 + 卡住项目跨模块汇总到今日页顶部，超过 3 条折叠，可直接跳到项目处理 |

其他：浅色 / 深色主题（写入 `config.toml`，重启保持）、5 秒内撤销完成 / 顺延、所有删除二次确认、每天首次启动自动备份（保留 30 份）、全量 JSON 导出 / 导入、一键清理演示数据。

## 技术栈

- **语言**：Rust stable（edition 2024，MSRV 1.92）
- **GUI**：[GPUI](https://github.com/zed-industries/zed) + `gpui-kit` 0.7.x 组件库，自绘标题栏
- **存储**：SQLite（`rusqlite` bundled，二进制内嵌，无需外部 DLL）
- **日期**：`chrono`；**配置**：TOML
- **打包**：Cargo workspace + Inno Setup 6 + PowerShell
- **平台**：仅 Windows（MSVC 工具链）

## 快速开始

```powershell
# 开发运行（带控制台，便于看日志）
cargo run -p niu_ma_workstation

# 全部测试（领域层 + 应用层）
cargo test -p niu_ma_workstation_core -p niu_ma_workstation

# 一次产出安装版 + 绿色版（release 构建，需 Inno Setup 6）
pwsh -File scripts\package.ps1
```

> 打包脚本是 UTF-8 无 BOM，请用 `pwsh`（PowerShell 7）执行；`powershell.exe` 5.1 会按 ANSI 解码导致解析报错。

产物在 `dist\`：

- `NiuMa-Workstation-<版本>-setup.exe`：安装版（当前用户安装、开始菜单、可选桌面图标、卸载器）
- `NiuMa-Workstation-<版本>-portable.zip`：绿色版（解压即用，数据随程序走）

首次构建需编译 GPUI 全量依赖，耗时较长；之后增量构建通常几十秒。

## 更新

侧栏左下角显示当前版本，点「检查更新」会读取 GitHub Release 的 `latest.json`。启动 6 秒后也会静默检查一次，有新版本时在侧栏提示。

- **安装版**：点「下载更新」→ 自动校验 SHA-256 → 点「安装并重启」；安装前自动备份数据库，装完自动拉起新版本。
- **绿色版**：只提示并打开下载页，手动替换程序文件，数据目录不受影响。

发布新版本：改 `crates\app\Cargo.toml` 的 `version`，执行 `pwsh -File scripts\package.ps1`，把 `dist\` 里的 `NiuMa-Workstation-<版本>-setup.exe`、`-portable.zip` 和 `latest.json` 一起上传到 GitHub Release，并发布为最新的正式版（非草稿 / 非预发布）。

## 数据放在哪

| 形态 | 数据目录 |
| --- | --- |
| 安装版 / `cargo run` | `%APPDATA%\NiuMa-Workstation\` |
| 绿色版 | `exe 所在目录\data\`（exe 旁存在 `portable.flag` 时启用） |

目录内包含 `NiuMa-Workstation.db`（SQLite）、`config.toml`（主题配置）与 `backups\`（自动备份，保留最近 30 份）。没有数据库主机 / 端口 / 账号之类的连接配置，只是一个本地文件。

首次启动会自动把旧版 `%APPDATA%\office_desk\` 下的数据库、配置与备份迁移到新目录，迁移完成后旧目录保留不删。

## 项目结构

```
NiuMa-Workstation/
├─ crates/
│  ├─ core/                    # niu_ma_workstation_core：纯领域层，只依赖 chrono，可秒级跑测试
│  │  └─ src/domain/           #   types / todo / project / stats / date
│  └─ app/                     # 二进制 NiuMa-Workstation：GPUI 渲染 + SQLite 存储
│     ├─ src/main.rs           #   启动：配置 → DB → 种子 → 顺延 → 窗口
│     ├─ src/state.rs          #   Workspace 状态机与全部动作
│     ├─ src/views/            #   today / notes / projects / review 四页
│     ├─ src/storage/          #   schema / repo / backup
│     ├─ src/theme.rs          #   双主题色板
│     ├─ src/config.rs         #   config.toml 读写
│     └─ src/seed.rs           #   演示数据 + 回归断言
├─ docs/                       # 产品设计、技术方案、项目代码说明
├─ packaging/NiuMa-Workstation.iss   # 安装版脚本
└─ scripts/package.ps1         # 一键打包安装版 + 绿色版
```

拆两个 crate 是为了让领域层测试不必编译 GPUI / SQLite 全量栈。数据流单向：**事件 → Workspace 动作 → 改内存快照 `DataSnapshot` → 单事务整库落盘 → 重绘**，界面只读内存快照，不在渲染时查库。

## 领域口径

这些是产品硬规则，也是种子数据的回归基准：

1. **分区顺序**：已完成 → 无日期（Someday）→ 未来 → 逾期 → 今天；逾期判定看 `original_date`，顺延过的任务只要原计划日已过仍算逾期。
2. **跨天自动顺延**：启动时 `date < today` 的任务改到今天、`postpone_count + 1`、写顺延流水，`original_date` 不变；隔 N 天没开也最多补 1 次。
3. **项目状态由卡点驱动**：卡点为空 = 进行中，非空 = 卡住了；已完成 / 已暂停不会被自动覆盖。
4. **周复盘分母 = 本周到期 ∪ 本周完成**，完成率 = 本周完成 / 分母；周起点固定周一。
5. 种子数据基线：本周完成 **11**、完成率 **84.6%**、逾期 **2**、卡住 **1**、顺延事件 **4**，日分布 `0·2·2·1·2·2·2`，优先级 `P0 4 / P1 3 / P2 4 / P3 0`。

## 测试

- `cargo test -p niu_ma_workstation_core`：11 个领域测试（分区、排序、项目排序、顺延、进度、状态联动、周统计、周边界）
- `cargo test -p niu_ma_workstation`：29 个应用测试（存储往返、v1→v2 迁移、备份保留 30 份、每日备份去重、旧数据目录迁移、种子回归、清理快照、JSON 往返、绿色版数据目录切换、更新清单解析与版本比较、SHA-256 校验等）

改动领域逻辑时必须保持种子基线数字不变。

## 已知限制

- 仅 Windows，依赖 MSVC 工具链与 Win32 窗口 API。
- GPUI 仍处于 pre-alpha，升级 `gpui-kit` 时需选用与 `gpui` 匹配的版本。
- 单窗口、单实例数据目录，不做云同步。
- 可配置项只有主题；优先级命名、周起始日、快捷键在 v0.1 固定。
- 删除均为二次确认，只有完成 / 顺延支持 5 秒内撤销。

## 更多文档

- [docs/PROJECT.md](docs/PROJECT.md)：分层、领域层、存储层、应用层的完整代码说明
- [docs/design-spec.html](docs/design-spec.html)：产品设计稿
- [docs/tech-spec-rust.html](docs/tech-spec-rust.html)：Rust 技术方案

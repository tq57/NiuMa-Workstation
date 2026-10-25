//! 工作台状态与动作层：内存快照 + SQLite 落库 + 四视图切换。
//!
//! 所有动作统一走「改快照 → storage.save → cx.notify」，保证界面与磁盘一致。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime};
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::{Icon, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use niu_ma_workstation_core::domain::date::start_of_week;
use niu_ma_workstation_core::domain::project::sync_status;
use niu_ma_workstation_core::domain::todo::{bucket, Bucket};
use niu_ma_workstation_core::domain::types::*;

use crate::config::Config;
use crate::storage::Storage;
use crate::theme::{Palette, ThemeMode};
use crate::update::{self, AvailableUpdate, UpdateStatus};

/// 左侧导航的四个模块
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Page {
    Today,
    Notes,
    Projects,
    Review,
}

impl Page {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Page::Today => "今日待办",
            Page::Notes => "灵感速记",
            Page::Projects => "项目跟进",
            Page::Review => "每周复盘",
        }
    }
}

/// 项目卡上的三个可编辑字段
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Current,
    Next,
    Blocker,
}

pub(crate) struct ProjectInputs {
    pub(crate) current: Entity<InputState>,
    pub(crate) next: Entity<InputState>,
    pub(crate) blocker: Entity<InputState>,
}

/// 项目弹框模式：新增，或编辑已有项目。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ProjectModalMode {
    Create,
    Edit(String),
}

/// 项目页状态筛选
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProjectFilter {
    All,
    Active,
    Blocked,
    Done,
}

impl ProjectFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ProjectFilter::All => "全部",
            ProjectFilter::Active => "进行中",
            ProjectFilter::Blocked => "卡住",
            ProjectFilter::Done => "已完成",
        }
    }

    pub(crate) fn matches(self, status: ProjectStatus) -> bool {
        match self {
            ProjectFilter::All => true,
            ProjectFilter::Active => status == ProjectStatus::Active,
            ProjectFilter::Blocked => status == ProjectStatus::Blocked,
            ProjectFilter::Done => status == ProjectStatus::Done,
        }
    }
}

pub(crate) const PROJECT_FILTERS: [ProjectFilter; 4] = [
    ProjectFilter::All,
    ProjectFilter::Active,
    ProjectFilter::Blocked,
    ProjectFilter::Done,
];

/// 已完成区的时间筛选
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DoneRange {
    Today,
    Week,
    Earlier,
}

impl DoneRange {
    pub(crate) fn label(self) -> &'static str {
        match self {
            DoneRange::Today => "今天",
            DoneRange::Week => "7 天内",
            DoneRange::Earlier => "更早",
        }
    }

    /// 完成日期是否落在该区间内（未来时间不算）
    pub(crate) fn matches(self, done_at: Option<NaiveDateTime>, today: NaiveDate) -> bool {
        let Some(at) = done_at else {
            return false;
        };
        let day = at.date();
        if day > today {
            return false;
        }
        match self {
            DoneRange::Today => day == today,
            DoneRange::Week => day > today - Duration::days(7),
            DoneRange::Earlier => day <= today - Duration::days(7),
        }
    }
}

pub(crate) const DONE_RANGES: [DoneRange; 3] =
    [DoneRange::Today, DoneRange::Week, DoneRange::Earlier];

/// 最近一次可撤销的动作（完成 / 顺延），保留动作前的整库快照。
struct UndoState {
    data: DataSnapshot,
    label: String,
}

pub struct Workspace {
    pub(crate) storage: Storage,
    pub(crate) data: DataSnapshot,
    pub(crate) page: Page,
    pub(crate) mode: ThemeMode,
    pub(crate) palette: Palette,
    pub(crate) today: NaiveDate,
    pub(crate) quick_input: Entity<InputState>,
    pub(crate) quick_priority: u8,
    /// 快速添加 / 灵感转待办时默认归入的项目（None = 不归入）
    pub(crate) default_project: Option<String>,
    pub(crate) note_input: Entity<InputState>,
    /// 灵感行内编辑复用的输入框
    pub(crate) note_edit_input: Entity<InputState>,
    /// 当前正在编辑的灵感 id
    pub(crate) editing_note: Option<String>,
    /// 新增 / 编辑项目弹框；None = 未打开
    pub(crate) project_modal: Option<ProjectModalMode>,
    pub(crate) project_modal_name: Entity<InputState>,
    pub(crate) project_modal_goal: Entity<TextareaState>,
    pub(crate) project_modal_current: Entity<InputState>,
    pub(crate) project_modal_next: Entity<InputState>,
    pub(crate) project_modal_blocker: Entity<InputState>,
    pub(crate) project_input: Entity<InputState>,
    /// 更新弹框是否打开
    pub(crate) update_modal: bool,
    /// 更新流程状态；None = 尚未检查
    pub(crate) update_status: Option<UpdateStatus>,
    /// 项目页状态筛选
    pub(crate) project_filter: ProjectFilter,
    /// 删除类操作的二次确认目标（待办 / 项目 / 灵感共用）
    pub(crate) confirm_delete: Option<String>,
    /// 「全部顺延到明天」的二次确认
    pub(crate) confirm_postpone_all: bool,
    /// 「导入数据」的覆盖确认
    pub(crate) import_confirm: bool,
    /// 置顶区是否展开全部（默认只显示前 3 条）
    pub(crate) pinned_expanded: bool,
    /// 今日页只看某个项目的待办（从置顶区「看待办」进入）
    pub(crate) project_focus: Option<String>,
    /// 已完成区的时间筛选
    pub(crate) done_range: DoneRange,
    /// 5 秒内可撤销的最近一次完成 / 顺延
    undo: Option<UndoState>,
    undo_seq: u64,
    pub(crate) tag_filter: Option<String>,
    pub(crate) show_done: bool,
    pub(crate) week_offset: i32,
    /// 「清理测试数据」是否处于待确认状态（输入「清空」后才可执行）
    pub(crate) clear_confirm: bool,
    /// 清空确认输入框
    pub(crate) clear_input: Entity<InputState>,
    pub(crate) review_inputs: [Entity<InputState>; 3],
    pub(crate) project_inputs: HashMap<String, ProjectInputs>,
    pub(crate) notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        storage: Storage,
        data: DataSnapshot,
        config: Config,
    ) -> Self {
        let today = Local::now().date_naive();
        let week_start = start_of_week(today);
        apply_caret_theme(config.theme, cx);

        let quick_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("记一件事，回车添加（默认 重要不紧急 · 今天）")
        });
        let note_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("随手记一条灵感，可用 #标签 标记")
        });
        let project_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("新项目名称，回车打开详情")
        });
        let note_edit_input = cx.new(|cx| InputState::new(window, cx));
        let project_modal_name = cx.new(|cx| {
            InputState::new(window, cx).placeholder("项目名称（必填）")
        });
        let project_modal_goal = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("项目目标、需求、要改造的内容或背景")
        });
        let project_modal_current = cx.new(|cx| {
            InputState::new(window, cx).placeholder("当前在推进什么")
        });
        let project_modal_next = cx.new(|cx| {
            InputState::new(window, cx).placeholder("下一步是什么")
        });
        let project_modal_blocker = cx.new(|cx| {
            InputState::new(window, cx).placeholder("卡在哪里（留空 = 进行中）")
        });
        let clear_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("输入「清空」确认")
        });

        let review_inputs = [
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("这周做得好的")
                    .default_value(review_value(&data, week_start, 0))
            }),
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("值得留意 / 想改的")
                    .default_value(review_value(&data, week_start, 1))
            }),
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("下周聚焦什么")
                    .default_value(review_value(&data, week_start, 2))
            }),
        ];

        let mut subscriptions: Vec<Subscription> = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &quick_input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_quick(input, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &note_input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_note(input, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &project_input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.open_project_modal(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &note_edit_input,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.save_edit_note(cx);
                }
            },
        ));
        // 弹框内单行字段回车即保存；多行目标输入回车用于换行
        for entity in [
            &project_modal_name,
            &project_modal_current,
            &project_modal_next,
            &project_modal_blocker,
        ] {
            subscriptions.push(cx.subscribe_in(
                entity,
                window,
                |this, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_project_modal(window, cx);
                    }
                },
            ));
        }
        // 清空输入框内容变化时重绘，让确认按钮的可用态即时更新
        subscriptions.push(cx.subscribe_in(
            &clear_input,
            window,
            |_this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        for (index, input) in review_inputs.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.save_review_field(index, window, cx);
                    }
                },
            ));
        }

        let (project_inputs, project_subs) = spawn_project_inputs(window, cx, &data.projects);
        subscriptions.extend(project_subs);

        let workspace = Self {
            storage,
            data,
            page: Page::Today,
            mode: config.theme,
            palette: Palette::of(config.theme),
            today,
            quick_input,
            quick_priority: 1,
            default_project: None,
            note_input,
            note_edit_input,
            editing_note: None,
            project_modal: None,
            project_modal_name,
            project_modal_goal,
            project_modal_current,
            project_modal_next,
            project_modal_blocker,
            project_input,
            update_modal: false,
            update_status: None,
            project_filter: ProjectFilter::All,
            confirm_delete: None,
            confirm_postpone_all: false,
            import_confirm: false,
            pinned_expanded: false,
            project_focus: None,
            done_range: DoneRange::Week,
            undo: None,
            undo_seq: 0,
            tag_filter: None,
            show_done: false,
            week_offset: 0,
            clear_confirm: false,
            clear_input,
            review_inputs,
            project_inputs,
            notice: None,
            _subscriptions: subscriptions,
        };
        // 启动后延迟检查一次，避免和首屏渲染抢资源
        Self::schedule_update_check(cx);
        workspace
    }

    /* ---------------- 通用 ---------------- */

    fn now(&self) -> NaiveDateTime {
        Local::now().naive_local()
    }

    pub(crate) fn shown_week_start(&self) -> NaiveDate {
        start_of_week(self.today) + Duration::days(7 * self.week_offset as i64)
    }

    /// 切到某页时聚焦该页的录入框
    pub(crate) fn focus_page_input(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match page {
            Page::Today => self.quick_input.clone(),
            Page::Notes => self.note_input.clone(),
            Page::Projects => self.project_input.clone(),
            Page::Review => self.review_inputs[0].clone(),
        };
        input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.storage.save(&self.data) {
            self.notice = Some(format!("保存失败：{err}"));
        }
        cx.notify();
    }

    fn flash(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.commit(cx);
    }

    /// 记录动作前的快照，并安排 5 秒后自动清除撤销入口
    fn arm_undo(&mut self, label: impl Into<String>, cx: &mut Context<Self>) {
        self.undo = Some(UndoState {
            data: self.data.clone(),
            label: label.into(),
        });
        self.undo_seq += 1;
        let seq = self.undo_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(StdDuration::from_secs(5))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.undo_seq == seq && this.undo.is_some() {
                    this.undo = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        self.undo_seq += 1;
        self.data = undo.data;
        self.confirm_delete = None;
        self.confirm_postpone_all = false;
        self.notice = Some(format!("已撤销{}", undo.label));
        self.commit(cx);
    }

    pub(crate) fn project_name(&self, id: &str) -> Option<&str> {
        self.data
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.as_str())
    }

    /// 展开 / 收起置顶区超出前 3 条的条目
    pub(crate) fn toggle_pinned(&mut self, cx: &mut Context<Self>) {
        self.pinned_expanded = !self.pinned_expanded;
        cx.notify();
    }

    /// 置顶区「看待办」：回到今日页并只看该项目关联的待办
    pub(crate) fn view_project_todos(&mut self, id: &str, cx: &mut Context<Self>) {
        self.page = Page::Today;
        self.project_focus = Some(id.to_string());
        self.notice = None;
        cx.notify();
    }

    /// 置顶区「去处理」：跳到项目卡并聚焦卡点输入框
    pub(crate) fn open_project_blocker(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Projects;
        self.project_filter = ProjectFilter::All;
        self.confirm_delete = None;
        if let Some(inputs) = self.project_inputs.get(id) {
            inputs
                .blocker
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /* ---------------- 待办动作 ---------------- */

    pub(crate) fn submit_quick(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = input.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let now = self.now();
        self.data.todos.push(Todo {
            id: next_id("t"),
            title,
            priority: self.quick_priority,
            date: Some(self.today),
            original_date: Some(self.today),
            project_id: self.default_project.clone(),
            status: TodoStatus::Todo,
            done_at: None,
            note: String::new(),
            postpone_count: 0,
            postpone_log: Vec::new(),
            created_at: now,
        });
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.flash("已加入今天", cx);
    }

    pub(crate) fn toggle_todo(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        let becoming_done = self
            .data
            .todos
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.status != TodoStatus::Done)
            .unwrap_or(false);
        self.arm_undo(if becoming_done { "完成" } else { "恢复待办" }, cx);
        if let Some(todo) = self.data.todos.iter_mut().find(|t| t.id == id) {
            if todo.status == TodoStatus::Done {
                todo.status = TodoStatus::Todo;
                todo.done_at = None;
            } else {
                todo.status = TodoStatus::Done;
                todo.done_at = Some(now);
            }
        }
        self.notice = Some(if becoming_done {
            "已完成"
        } else {
            "已恢复待办"
        }
        .into());
        self.commit(cx);
    }

    pub(crate) fn postpone_todo(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        let tomorrow = self.today + Duration::days(1);
        let postponable = self
            .data
            .todos
            .iter()
            .any(|t| t.id == id && t.status == TodoStatus::Todo);
        if !postponable {
            self.commit(cx);
            return;
        }
        self.arm_undo("顺延", cx);
        let mut message = None;
        if let Some(todo) = self.data.todos.iter_mut().find(|t| t.id == id) {
            if todo.status == TodoStatus::Todo {
                let from = todo.date;
                todo.postpone_log.push(PostponeEvent {
                    from,
                    to: tomorrow,
                    at: now,
                    auto: false,
                });
                todo.postpone_count += 1;
                todo.date = Some(tomorrow);
                message = Some(format!("已顺延到明天，第 {} 次", todo.postpone_count));
            }
        }
        match message {
            Some(msg) => self.flash(msg, cx),
            None => self.commit(cx),
        }
    }

    pub(crate) fn postpone_all_overdue(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let today = self.today;
        let tomorrow = today + Duration::days(1);
        let overdue_count = self
            .data
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::Todo && bucket(t, today) == Bucket::Overdue)
            .count();
        if overdue_count == 0 {
            return;
        }
        if !self.confirm_postpone_all {
            self.confirm_postpone_all = true;
            self.notice = Some(format!(
                "再点一次确认：把 {overdue_count} 件逾期任务顺延到明天"
            ));
            cx.notify();
            return;
        }
        self.confirm_postpone_all = false;
        self.arm_undo(format!("顺延 {overdue_count} 件"), cx);
        let mut count = 0usize;
        for todo in self.data.todos.iter_mut() {
            if todo.status == TodoStatus::Todo && bucket(todo, today) == Bucket::Overdue {
                let from = todo.date;
                todo.postpone_log.push(PostponeEvent {
                    from,
                    to: tomorrow,
                    at: now,
                    auto: false,
                });
                todo.postpone_count += 1;
                todo.date = Some(tomorrow);
                count += 1;
            }
        }
        if count > 0 {
            self.flash(format!("已把 {count} 件逾期任务顺延到明天"), cx);
        } else {
            self.commit(cx);
        }
    }

    pub(crate) fn delete_todo(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(id) {
            self.confirm_delete = Some(id.to_string());
            self.notice = Some("再点一次确认删除这条任务".into());
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        self.data.todos.retain(|t| t.id != id);
        self.flash("已删除任务", cx);
    }

    pub(crate) fn cycle_priority(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(todo) = self.data.todos.iter_mut().find(|t| t.id == id) {
            todo.priority = (todo.priority + 1) % 4;
        }
        self.commit(cx);
    }

    /// 切换快速添加 / 灵感转待办的默认项目：无 → 项目 1 → 项目 2 → … → 无
    pub(crate) fn cycle_default_project(&mut self, cx: &mut Context<Self>) {
        let next = next_project(self.default_project.as_deref(), &self.data.projects);
        self.default_project = next;
        self.notice = Some(match self.default_project.as_deref().and_then(|id| self.project_name(id)) {
            Some(name) => format!("新任务将归入「{name}」"),
            None => "新任务不归入项目".to_string(),
        });
        cx.notify();
    }

    /// 待办行内切换所属项目：无 → 项目 1 → 项目 2 → … → 无
    pub(crate) fn cycle_todo_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let current = self
            .data
            .todos
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.project_id.clone());
        let next = next_project(current.as_deref(), &self.data.projects);
        if let Some(todo) = self.data.todos.iter_mut().find(|t| t.id == id) {
            todo.project_id = next;
        }
        self.commit(cx);
    }

    /* ---------------- 灵感动作 ---------------- */

    pub(crate) fn submit_note(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        let now = self.now();
        let tags = parse_tags(&content);
        self.data.notes.push(Note {
            id: next_id("n"),
            content,
            tags,
            pinned: false,
            created_at: now,
            updated_at: now,
        });
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.flash("已记录灵感", cx);
    }

    /// 回车 /「＋ 加项目」：打开新增项目弹框，带入快捷输入框里的名称
    pub(crate) fn open_project_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.project_input.read(cx).value().trim().to_string();
        self.project_modal = Some(ProjectModalMode::Create);
        self.fill_project_modal(&name, "", "", "", "", window, cx);
        self.confirm_delete = None;
        cx.notify();
    }

    /// 项目卡「编辑」：打开同一个弹框并预填当前内容
    pub(crate) fn open_project_edit_modal(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.data.projects.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        self.project_modal = Some(ProjectModalMode::Edit(id.to_string()));
        self.fill_project_modal(
            &project.name,
            &project.goal,
            &project.current,
            &project.next,
            &project.blocker,
            window,
            cx,
        );
        self.confirm_delete = None;
        cx.notify();
    }

    fn fill_project_modal(
        &mut self,
        name: &str,
        goal: &str,
        current: &str,
        next: &str,
        blocker: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_modal_name.update(cx, |state, cx| {
            state.set_value(name, window, cx);
            state.focus(window, cx);
        });
        self.project_modal_goal
            .update(cx, |state, cx| state.set_value(goal, window, cx));
        self.project_modal_current
            .update(cx, |state, cx| state.set_value(current, window, cx));
        self.project_modal_next
            .update(cx, |state, cx| state.set_value(next, window, cx));
        self.project_modal_blocker
            .update(cx, |state, cx| state.set_value(blocker, window, cx));
    }

    pub(crate) fn close_project_modal(&mut self, cx: &mut Context<Self>) {
        self.project_modal = None;
        cx.notify();
    }

    /// 弹框保存：新增时创建项目，编辑时更新名称与总体内容
    pub(crate) fn save_project_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.project_modal.clone() else {
            return;
        };
        let name = self
            .project_modal_name
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty() {
            self.notice = Some("项目名称不能为空".into());
            self.project_modal_name
                .update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
            return;
        }
        let goal = self.project_modal_goal.read(cx).value().trim().to_string();
        let current = self
            .project_modal_current
            .read(cx)
            .value()
            .trim()
            .to_string();
        let next = self.project_modal_next.read(cx).value().trim().to_string();
        let blocker = self
            .project_modal_blocker
            .read(cx)
            .value()
            .trim()
            .to_string();
        let now = self.now();

        match mode {
            ProjectModalMode::Create => {
                let mut project = new_project(next_id("p"), name, now);
                apply_project_details(&mut project, goal, current, next, blocker, now);

                let (inputs, subscriptions) =
                    spawn_project_inputs(window, cx, std::slice::from_ref(&project));
                self.project_inputs.extend(inputs);
                self._subscriptions.extend(subscriptions);
                self.data.projects.push(project);
                self.project_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.project_modal = None;
                self.flash("已新增项目", cx);
            }
            ProjectModalMode::Edit(id) => {
                if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == id) {
                    project.name = name;
                    apply_project_details(project, goal, current, next, blocker, now);
                }
                if let Some(inputs) = self.project_inputs.get(&id) {
                    let current = self.project_modal_current.read(cx).value().to_string();
                    let next = self.project_modal_next.read(cx).value().to_string();
                    let blocker = self.project_modal_blocker.read(cx).value().to_string();
                    inputs
                        .current
                        .update(cx, |state, cx| state.set_value(&current, window, cx));
                    inputs
                        .next
                        .update(cx, |state, cx| state.set_value(&next, window, cx));
                    inputs
                        .blocker
                        .update(cx, |state, cx| state.set_value(&blocker, window, cx));
                }
                self.project_modal = None;
                self.flash("已更新项目", cx);
            }
        }
    }

    pub(crate) fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        if let Some(note) = self.data.notes.iter_mut().find(|n| n.id == id) {
            note.pinned = !note.pinned;
            note.updated_at = now;
        }
        self.commit(cx);
    }

    pub(crate) fn delete_note(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(id) {
            self.confirm_delete = Some(id.to_string());
            self.notice = Some("再点一次确认删除这条灵感".into());
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        self.data.notes.retain(|n| n.id != id);
        self.flash("已删除灵感", cx);
    }

    /// 点击灵感文字进入行内编辑
    pub(crate) fn start_edit_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(content) = self
            .data
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.content.clone())
        else {
            return;
        };
        self.editing_note = Some(id.to_string());
        self.note_edit_input.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// 保存行内编辑：重新解析标签、刷新更新时间
    pub(crate) fn save_edit_note(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_note.take() else {
            return;
        };
        let content = self.note_edit_input.read(cx).value().trim().to_string();
        let now = self.now();
        if let Some(note) = self.data.notes.iter_mut().find(|n| n.id == id) {
            if note.content != content {
                note.content = content.clone();
                note.tags = parse_tags(&content);
                note.updated_at = now;
            }
        }
        self.commit(cx);
    }

    pub(crate) fn note_to_todo(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(note) = self.data.notes.iter().find(|n| n.id == id) else {
            return;
        };
        let title = note.content.clone();
        let now = self.now();
        self.data.todos.push(Todo {
            id: next_id("t"),
            title,
            priority: 1,
            date: Some(self.today),
            original_date: Some(self.today),
            project_id: self.default_project.clone(),
            status: TodoStatus::Todo,
            done_at: None,
            note: String::new(),
            postpone_count: 0,
            postpone_log: Vec::new(),
            created_at: now,
        });
        self.flash("已转为今天的任务", cx);
    }

    /* ---------------- 检查更新 ---------------- */

    fn schedule_update_check(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(StdDuration::from_secs(6))
                .await;
            let result = cx
                .background_executor()
                .spawn(async { update::check_latest() })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_update_check(result, false, cx));
        })
        .detach();
    }

    /// 手动检查：打开弹框并立即请求清单。
    pub(crate) fn check_update(&mut self, cx: &mut Context<Self>) {
        self.update_modal = true;
        self.update_status = Some(UpdateStatus::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { update::check_latest() })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_update_check(result, true, cx));
        })
        .detach();
    }

    fn finish_update_check(
        &mut self,
        result: Result<Option<AvailableUpdate>, anyhow::Error>,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Some(update)) => {
                if !manual {
                    self.notice = Some(format!(
                        "发现新版本 v{}，点左下角「检查更新」查看",
                        update.version
                    ));
                }
                self.update_status = Some(UpdateStatus::Available(update));
            }
            Ok(None) => {
                self.update_status = if manual {
                    Some(UpdateStatus::UpToDate)
                } else {
                    None
                };
            }
            Err(err) => {
                self.update_status = if manual {
                    Some(UpdateStatus::Failed(format!("{err:#}")))
                } else {
                    None
                };
            }
        }
        cx.notify();
    }

    pub(crate) fn close_update_modal(&mut self, cx: &mut Context<Self>) {
        self.update_modal = false;
        cx.notify();
    }

    /// 下载安装包，成功后等待用户点「安装并重启」。
    pub(crate) fn download_update(&mut self, cx: &mut Context<Self>) {
        let Some(UpdateStatus::Available(update)) = self.update_status.clone() else {
            return;
        };
        let version = update.version.clone();
        self.update_status = Some(UpdateStatus::Downloading {
            version: version.clone(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { update::download_installer(&update) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.update_status = Some(match result {
                    Ok(installer) => UpdateStatus::Ready {
                        version,
                        installer,
                    },
                    Err(err) => UpdateStatus::Failed(format!("下载失败：{err:#}")),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// 半自动安装：先备份数据库，再启动安装包并退出应用。
    pub(crate) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(UpdateStatus::Ready { version, installer }) = self.update_status.clone() else {
            return;
        };
        if update::is_portable() {
            self.update_status = Some(UpdateStatus::Failed(
                "绿色版不支持自动安装，请手动下载替换".into(),
            ));
            cx.notify();
            return;
        }
        let stamp = format!("{}-pre-update", self.now().format("%Y%m%d-%H%M%S"));
        let _ = self.storage.backup_now(&stamp);
        match update::launch_installer(&installer) {
            Ok(()) => {
                self.update_modal = false;
                self.notice = Some(format!("正在安装 v{version}，应用即将重启"));
                cx.notify();
                cx.quit();
            }
            Err(err) => {
                self.update_status = Some(UpdateStatus::Failed(format!("启动安装包失败：{err:#}")));
                cx.notify();
            }
        }
    }

    pub(crate) fn open_releases_page(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = update::open_releases_page() {
            self.update_status = Some(UpdateStatus::Failed(format!("打开下载页失败：{err:#}")));
            cx.notify();
        }
    }

    /* ---------------- 项目动作 ---------------- */

    fn save_project_field(
        &mut self,
        pid: &str,
        _field: Field,
        cx: &mut Context<Self>,
    ) {
        let Some(inputs) = self.project_inputs.get(pid) else {
            return;
        };
        let current = inputs.current.read(cx).value().to_string();
        let next = inputs.next.read(cx).value().to_string();
        let blocker = inputs.blocker.read(cx).value().to_string();
        let now = self.now();
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == pid) {
            project.current = current;
            project.next = next;
            project.blocker = blocker;
            project.updated_at = now;
            project.status = sync_status(project);
        }
        self.commit(cx);
    }

    pub(crate) fn toggle_project_done(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == id) {
            project.status = if project.status == ProjectStatus::Done {
                if project.blocker.trim().is_empty() {
                    ProjectStatus::Active
                } else {
                    ProjectStatus::Blocked
                }
            } else {
                ProjectStatus::Done
            };
            project.updated_at = now;
        }
        self.commit(cx);
    }

    pub(crate) fn toggle_project_pause(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == id) {
            project.status = if project.status == ProjectStatus::Paused {
                if project.blocker.trim().is_empty() {
                    ProjectStatus::Active
                } else {
                    ProjectStatus::Blocked
                }
            } else {
                ProjectStatus::Paused
            };
            project.updated_at = now;
        }
        self.commit(cx);
    }

    /// 删除项目：第一次点击进入确认态，再点一次删除并解除关联待办。
    pub(crate) fn delete_project(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(id) {
            self.confirm_delete = Some(id.to_string());
            self.notice = Some("再点一次确认删除项目，关联待办会解除关联".into());
            cx.notify();
            return;
        }

        self.confirm_delete = None;
        self.data.projects.retain(|p| p.id != id);
        for todo in &mut self.data.todos {
            if todo.project_id.as_deref() == Some(id) {
                todo.project_id = None;
            }
        }
        self.project_inputs.remove(id);
        if self.project_focus.as_deref() == Some(id) {
            self.project_focus = None;
        }
        if self.default_project.as_deref() == Some(id) {
            self.default_project = None;
        }
        self.notice = Some("已删除项目，关联待办已解除关联".into());
        self.commit(cx);
    }

    /* ---------------- 复盘动作 ---------------- */

    fn save_review_field(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let value = self.review_inputs[index].read(cx).value().to_string();
        let week_start = self.shown_week_start();
        let now = self.now();
        if let Some(review) = self
            .data
            .reviews
            .iter_mut()
            .find(|r| r.week_start == week_start)
        {
            match index {
                0 => review.good = value,
                1 => review.watch = value,
                _ => review.focus = value,
            }
            review.updated_at = Some(now);
        } else {
            let mut review = WeeklyReview {
                week_start,
                good: String::new(),
                watch: String::new(),
                focus: String::new(),
                updated_at: Some(now),
            };
            match index {
                0 => review.good = value,
                1 => review.watch = value,
                _ => review.focus = value,
            }
            self.data.reviews.push(review);
        }
        self.commit(cx);
    }

    pub(crate) fn shift_week(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        self.week_offset += delta;
        let week_start = self.shown_week_start();
        for index in 0..3 {
            let value = review_value(&self.data, week_start, index);
            self.review_inputs[index].update(cx, |state, cx| {
                state.set_value(value, window, cx);
            });
        }
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn reset_week(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.week_offset = 0;
        let week_start = self.shown_week_start();
        for index in 0..3 {
            let value = review_value(&self.data, week_start, index);
            self.review_inputs[index].update(cx, |state, cx| {
                state.set_value(value, window, cx);
            });
        }
        self.notice = None;
        cx.notify();
    }

    /* ---------------- 主题 ---------------- */

    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.mode = self.mode.toggled();
        self.palette = Palette::of(self.mode);
        apply_caret_theme(self.mode, cx);
        let config = Config { theme: self.mode };
        if let Err(err) = config.save() {
            self.notice = Some(format!("主题已切换，但保存配置失败：{err}"));
        } else {
            self.notice = Some(format!("已切换到{}主题", self.mode.label()));
        }
        cx.notify();
    }

    /* ---------------- 清理测试数据 ---------------- */

    pub(crate) fn clear_test_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 破坏性操作：先进入确认态，输入「清空」后才允许执行
        if !self.clear_confirm {
            self.clear_confirm = true;
            self.clear_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.notice = Some("输入「清空」两个字后点击确认".into());
            cx.notify();
            return;
        }

        let typed = self.clear_input.read(cx).value().trim().to_string();
        if typed != "清空" {
            self.notice = Some("请先输入「清空」再确认".into());
            cx.notify();
            return;
        }

        self.clear_confirm = false;
        self.clear_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.data = cleared_snapshot(self.today, self.now());
        self.project_inputs.clear();
        self.project_filter = ProjectFilter::All;
        self.confirm_delete = None;
        self.confirm_postpone_all = false;
        self.import_confirm = false;
        self.editing_note = None;
        self.tag_filter = None;
        self.show_done = false;
        self.week_offset = 0;
        self.project_focus = None;
        self.pinned_expanded = false;
        self.default_project = None;
        self.done_range = DoneRange::Week;
        for input in &self.review_inputs {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.notice = Some("已清空全部待办、项目、灵感与复盘".into());
        self.commit(cx);
    }

    /* ---------------- 数据备份 / 导出 / 导入 ---------------- */

    /// 立即备份一份数据库，保留最近 30 份。
    pub(crate) fn backup_now(&mut self, cx: &mut Context<Self>) {
        let stamp = self.now().format("%Y%m%d-%H%M%S").to_string();
        match self.storage.backup_now(&stamp) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.notice = Some(format!("已备份：backups/{name}"));
            }
            Err(err) => self.notice = Some(format!("备份失败：{err}")),
        }
        cx.notify();
    }

    /// 导出全部数据为 JSON（自选保存位置）。
    pub(crate) fn export_data(&mut self, cx: &mut Context<Self>) {
        let stamp = self.now().format("%Y%m%d-%H%M%S").to_string();
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}-{stamp}.json", crate::storage::APP_NAME))
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(err) => {
                self.notice = Some(format!("导出失败：{err}"));
                cx.notify();
                return;
            }
        };
        match serde_json::to_writer_pretty(file, &self.data) {
            Ok(()) => self.notice = Some("已导出 JSON 备份".into()),
            Err(err) => self.notice = Some(format!("导出失败：{err}")),
        }
        cx.notify();
    }

    /// 导入 JSON：第一次点击进入确认态，再点一次选择文件并覆盖当前数据。
    pub(crate) fn import_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.import_confirm {
            self.import_confirm = true;
            self.notice = Some("再点一次「确认导入」，导入会覆盖当前全部数据".into());
            cx.notify();
            return;
        }
        self.import_confirm = false;

        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            cx.notify();
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                self.notice = Some(format!("读取失败：{err}"));
                cx.notify();
                return;
            }
        };
        let imported: DataSnapshot = match serde_json::from_str(&text) {
            Ok(data) => data,
            Err(err) => {
                self.notice = Some(format!("文件格式不正确：{err}"));
                cx.notify();
                return;
            }
        };

        let counts = format!(
            "{} 条待办 · {} 个项目 · {} 条灵感",
            imported.todos.len(),
            imported.projects.len(),
            imported.notes.len()
        );
        self.data = imported;

        let (inputs, subscriptions) = spawn_project_inputs(window, cx, &self.data.projects);
        self.project_inputs = inputs;
        self._subscriptions.extend(subscriptions);

        self.project_filter = ProjectFilter::All;
        self.confirm_delete = None;
        self.confirm_postpone_all = false;
        self.editing_note = None;
        self.project_modal = None;
        self.tag_filter = None;
        self.show_done = false;
        self.week_offset = 0;
        self.project_focus = None;
        self.pinned_expanded = false;
        self.default_project = None;
        self.done_range = DoneRange::Week;
        self.clear_confirm = false;
        let week_start = start_of_week(self.today);
        for (index, input) in self.review_inputs.iter().enumerate() {
            let value = review_value(&self.data, week_start, index);
            input.update(cx, |state, cx| state.set_value(&value, window, cx));
        }

        self.notice = Some(format!("已导入 {counts}"));
        self.commit(cx);
    }
}

/// 为每个项目创建三个行内输入框，并订阅回车 / 失焦保存。
fn spawn_project_inputs(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    projects: &[Project],
) -> (HashMap<String, ProjectInputs>, Vec<Subscription>) {
    let mut map: HashMap<String, ProjectInputs> = HashMap::new();
    let mut subscriptions: Vec<Subscription> = Vec::new();
    for project in projects {
        let current = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("当前在推进什么")
                .default_value(project.current.clone())
        });
        let next = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("下一步是什么")
                .default_value(project.next.clone())
        });
        let blocker = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("卡在哪里（留空 = 进行中）")
                .default_value(project.blocker.clone())
        });

        let pid = project.id.clone();
        for (field, entity) in [
            (Field::Current, &current),
            (Field::Next, &next),
            (Field::Blocker, &blocker),
        ] {
            let pid = pid.clone();
            subscriptions.push(cx.subscribe_in(
                entity,
                window,
                move |this, _input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.save_project_field(&pid, field, cx);
                    }
                },
            ));
        }

        map.insert(
            project.id.clone(),
            ProjectInputs {
                current,
                next,
                blocker,
            },
        );
    }
    (map, subscriptions)
}

/// 清空后的快照：列表全空，但保留种子/顺延标记，避免重启后又灌回测试数据。
fn cleared_snapshot(today: NaiveDate, now: NaiveDateTime) -> DataSnapshot {
    let mut data = DataSnapshot::new();
    data.seeded_at = Some(now);
    data.last_run_date = Some(today);
    data
}

/// 关联项目的循环顺序：无 → 第一个项目 → 第二个 → … → 无。
/// 当前项目已被删除（不在列表里）时也回到「无」。
pub(crate) fn next_project(current: Option<&str>, projects: &[Project]) -> Option<String> {
    match current {
        None => projects.first().map(|p| p.id.clone()),
        Some(cur) => {
            let index = projects.iter().position(|p| p.id == cur);
            match index {
                Some(i) if i + 1 < projects.len() => Some(projects[i + 1].id.clone()),
                _ => None,
            }
        }
    }
}

/// 从 `#tag` 形式的文本里解析标签（去重、去掉尾部标点）。
fn parse_tags(content: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for word in content.split_whitespace() {
        if let Some(tag) = word.strip_prefix('#') {
            let tag = tag.trim_matches(|c: char| c == ',' || c == '，' || c == '。' || c == '#');
            if !tag.is_empty() && !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
        }
    }
    tags
}

/// 把弹框里的项目总体内容写入项目，并按卡点重新同步状态。
fn apply_project_details(
    project: &mut Project,
    goal: String,
    current: String,
    next: String,
    blocker: String,
    now: NaiveDateTime,
) {
    project.goal = goal;
    project.current = current;
    project.next = next;
    project.blocker = blocker;
    project.updated_at = now;
    project.status = sync_status(project);
}

/// 新项目的初始形态：三个字段为空、进行中、没有卡点流水。
fn new_project(id: String, name: String, now: NaiveDateTime) -> Project {
    Project {
        id,
        name,
        goal: String::new(),
        current: String::new(),
        next: String::new(),
        blocker: String::new(),
        status: ProjectStatus::Active,
        updated_at: now,
        blocker_log: Vec::new(),
    }
}

fn review_value(data: &DataSnapshot, week_start: NaiveDate, index: usize) -> String {
    data.reviews
        .iter()
        .find(|r| r.week_start == week_start)
        .map(|r| match index {
            0 => r.good.clone(),
            1 => r.watch.clone(),
            _ => r.focus.clone(),
        })
        .unwrap_or_default()
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 输入框闪烁光标跟随应用主题：组件库默认光标偏黑，深色主题下几乎不可见。
fn apply_caret_theme(mode: ThemeMode, cx: &mut App) {
    let caret = mode.caret_color();
    Theme::update(cx, |theme| theme.caret = rgb(caret).into());
}

fn next_id(prefix: &str) -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{micros:x}-{n:x}")
}

/* ---------------- 根视图 ---------------- */

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let titlebar = self.render_titlebar(window);
        let sidebar = self.render_sidebar(cx);
        let body = match self.page {
            Page::Today => self.render_today(cx).into_any_element(),
            Page::Notes => self.render_notes(cx).into_any_element(),
            Page::Projects => self.render_projects(cx).into_any_element(),
            Page::Review => self.render_review(cx).into_any_element(),
        };
        let notice = self.notice.clone();
        let undo_available = self.undo.is_some();
        let project_modal = if self.project_modal.is_some() {
            Some(self.render_project_modal(cx))
        } else {
            None
        };
        let update_modal = if self.update_modal {
            Some(self.render_update_modal(cx))
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.ink))
            .child(titlebar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(sidebar)
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(body)
                            .when_some(notice, |el, text| {
                                el.child(
                                    div()
                                        .absolute()
                                        .bottom(px(18.))
                                        .right(px(18.))
                                        .px(px(14.))
                                        .py(px(8.))
                                        .rounded(px(8.))
                                        .bg(rgb(palette.teal_ink))
                                        .text_color(rgb(palette.paper))
                                        .text_size(px(12.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .child(SharedString::from(text))
                                                .when(undo_available, |el| {
                                                    el.child(
                                                        div()
                                                            .cursor_pointer()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .underline()
                                                            .child("撤销")
                                                            .id("undo-toast")
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.undo(cx)
                                                            })),
                                                    )
                                                }),
                                        ),
                                )
                            }),
                    ),
            )
            .when_some(project_modal, |el, modal| el.child(modal))
            .when_some(update_modal, |el, modal| el.child(modal))
    }
}

impl Workspace {
    /// 自绘标题栏：左侧与侧栏同色延续，右侧与内容区同色，窗口控件贴右上角。
    ///
    /// Windows 下 `window_control_area` 让系统负责拖拽 / 双击最大化 / 最小化 / 关闭，
    /// 因此这里不需要绑定点击事件。
    fn render_titlebar(&self, window: &Window) -> Div {
        let palette = self.palette;
        let maximize = if window.is_maximized() {
            RESTORE_SVG
        } else {
            MAXIMIZE_SVG
        };

        div()
            .flex()
            .flex_row()
            .h(px(34.))
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(palette.line))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.))
                    .pl(px(16.))
                    .w(px(232.))
                    .h_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .bg(rgb(palette.paper))
                    .border_r_1()
                    .border_color(rgb(palette.line))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("NiuMa-Workstation"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.ink3))
                            .child("工作台"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_end()
                    .bg(rgb(palette.bg))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .pl(px(24.))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag)
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.page.title()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(self.titlebar_button(WindowControlArea::Min, MINIMIZE_SVG, false))
                    .child(self.titlebar_button(WindowControlArea::Max, maximize, false))
                    .child(self.titlebar_button(WindowControlArea::Close, CLOSE_SVG, true)),
            )
    }

    fn titlebar_button(
        &self,
        area: WindowControlArea,
        icon: &'static [u8],
        is_close: bool,
    ) -> AnyElement {
        let palette = self.palette;
        let (hover_bg, hover_fg) = if is_close {
            (palette.red, 0xffffff)
        } else {
            (palette.hover, palette.ink)
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h_full()
            .flex_shrink_0()
            .text_color(rgb(palette.ink2))
            .hover(move |style| style.bg(rgb(hover_bg)).text_color(rgb(hover_fg)))
            .window_control_area(area)
            .child(Icon::default().data(icon).small())
            .into_any_element()
    }
}

/// 窗口控件图标：内联 SVG（组件库的图标资源未随 crate 发布，路径式图标无法加载）。
const MINIMIZE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="3" y="7.5" width="10" height="1" rx="0.5"/></svg>"#;
const MAXIMIZE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill-rule="evenodd" d="M3.5 3.5h9v9h-9zm1.2 1.2v6.6h6.6V4.7z"/></svg>"#;
const RESTORE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill-rule="evenodd" d="M5 2.5h8.5V11h-1.2V3.7H5zm-2.5 2.5h8.5v8.5H2.5zm1.2 1.2v6.1h6.1V6.2z"/></svg>"#;
const CLOSE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="M4.1 3.1 8 7l3.9-3.9 1 1L9 8l3.9 3.9-1 1L8 9l-3.9 3.9-1-1L7 8 3.1 4.1z"/></svg>"#;

impl Workspace {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let palette = self.palette;
        let pages = [
            Page::Today,
            Page::Notes,
            Page::Projects,
            Page::Review,
        ];
        let mut nav: Vec<AnyElement> = Vec::new();
        for page in pages {
            let active = self.page == page;
            nav.push(
                div()
                    .cursor_pointer()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .text_size(px(13.))
                    .when(active, |el| {
                        el.bg(rgb(palette.teal_soft))
                            .text_color(rgb(palette.teal_ink))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!active, |el| el.text_color(rgb(palette.ink2)))
                    .child(page.title())
                    .id(SharedString::from(format!("nav-{}", page.title())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.page = page;
                        this.notice = None;
                        this.clear_confirm = false;
                        this.confirm_delete = None;
                        this.confirm_postpone_all = false;
                        this.import_confirm = false;
                        this.editing_note = None;
                        this.project_focus = None;
                        this.clear_input
                            .update(cx, |state, cx| state.set_value("", window, cx));
                        this.focus_page_input(page, window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(232.))
            .h_full()
            .bg(rgb(palette.paper))
            .border_r_1()
            .border_color(rgb(palette.line))
            .px(px(16.))
            .pt(px(18.))
            .pb(px(14.))
            .gap(px(6.))
            .children(nav)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(12.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(rgb(palette.ink3))
                    .child(
                        div()
                            .cursor_pointer()
                            .hover(|el| el.text_color(rgb(palette.teal)))
                            .child("备份")
                            .id("backup-now")
                            .on_click(cx.listener(|this, _, _, cx| this.backup_now(cx))),
                    )
                    .child("·")
                    .child(
                        div()
                            .cursor_pointer()
                            .hover(|el| el.text_color(rgb(palette.teal)))
                            .child("导出")
                            .id("export-data")
                            .on_click(cx.listener(|this, _, _, cx| this.export_data(cx))),
                    )
                    .child("·")
                    .child(
                        div()
                            .cursor_pointer()
                            .when(self.import_confirm, |el| {
                                el.text_color(rgb(palette.red))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(|el| el.text_color(rgb(palette.teal)))
                            .child(if self.import_confirm {
                                "确认导入"
                            } else {
                                "导入"
                            })
                            .id("import-data")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_data(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(!self.clear_confirm, |el| {
                        el.child(
                            div()
                                .cursor_pointer()
                                .px(px(12.))
                                .py(px(6.))
                                .rounded(px(8.))
                                .text_size(px(11.))
                                .text_color(rgb(palette.ink3))
                                .hover(|el| {
                                    el.bg(rgb(palette.red_soft))
                                        .text_color(rgb(palette.red))
                                })
                                .child("清理测试数据")
                                .id("clear-data")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_test_data(window, cx)
                                })),
                        )
                    })
                    .when(self.clear_confirm, |el| {
                        let ready = self.clear_input.read(cx).value().trim() == "清空";
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .p(px(8.))
                                .rounded(px(8.))
                                .bg(rgb(palette.red_soft))
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette.red))
                                        .child("输入「清空」后确认，不可恢复"),
                                )
                                .child(
                                    div()
                                        .px(px(8.))
                                        .py(px(3.))
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(rgb(palette.line))
                                        .bg(rgb(palette.paper))
                                        .child(
                                            Input::new(&self.clear_input)
                                                .appearance(false)
                                                .text_size(px(11.))
                                                .w_full(),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .flex_1()
                                                .py(px(3.))
                                                .rounded(px(6.))
                                                .text_size(px(11.))
                                                .text_center()
                                                .bg(rgb(if ready {
                                                    palette.red
                                                } else {
                                                    palette.line2
                                                }))
                                                .text_color(rgb(if ready {
                                                    palette.paper
                                                } else {
                                                    palette.ink3
                                                }))
                                                .child("确认清空")
                                                .id("clear-confirm")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.clear_test_data(window, cx)
                                                })),
                                        )
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .px(px(8.))
                                                .py(px(3.))
                                                .rounded(px(6.))
                                                .text_size(px(11.))
                                                .text_color(rgb(palette.ink3))
                                                .child("取消")
                                                .id("clear-cancel")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.clear_confirm = false;
                                                    this.clear_input.update(cx, |state, cx| {
                                                        state.set_value("", window, cx)
                                                    });
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.))
                    .py(px(4.))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(palette.ink3))
                            .child(format!("v{}", update::CURRENT_VERSION)),
                    )
                    .child({
                        let (label, color) = match &self.update_status {
                            Some(UpdateStatus::Available(update)) => {
                                (format!("发现 v{}", update.version), palette.teal)
                            }
                            Some(UpdateStatus::Checking) => ("检查中…".into(), palette.ink3),
                            _ => ("检查更新".into(), palette.ink3),
                        };
                        div()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .hover(|el| el.text_color(rgb(palette.teal)))
                            .child(label)
                            .id("check-update")
                            .on_click(cx.listener(|this, _, _, cx| this.check_update(cx)))
                    }),
            )
            .child(
                div()
                    .cursor_pointer()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(palette.line))
                    .text_size(px(12.))
                    .text_color(rgb(palette.ink2))
                    .child(SharedString::from(format!(
                        "切换{}主题",
                        self.mode.toggled().label()
                    )))
                    .id("theme-toggle")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use niu_ma_workstation_core::domain::types::{DataSnapshot, ProjectStatus};

    use super::{apply_project_details, cleared_snapshot, new_project, next_project, DoneRange};

    #[test]
    fn cleared_snapshot_is_empty_but_keeps_seed_marker() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let now = today.and_hms_opt(9, 0, 0).unwrap();

        let data = cleared_snapshot(today, now);

        assert!(data.todos.is_empty());
        assert!(data.projects.is_empty());
        assert!(data.notes.is_empty());
        assert!(data.reviews.is_empty());
        // 标记保留：下次启动不会再自动灌种子数据
        assert_eq!(data.seeded_at, Some(now));
        assert_eq!(data.last_run_date, Some(today));
    }

    #[test]
    fn new_project_starts_empty_and_active() {
        let now = NaiveDate::from_ymd_opt(2026, 10, 22)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();

        let project = new_project("p1".into(), "测试项目".into(), now);

        assert_eq!(project.name, "测试项目");
        assert_eq!(project.status, ProjectStatus::Active);
        assert_eq!(project.updated_at, now);
        assert!(project.current.is_empty());
        assert!(project.next.is_empty());
        assert!(project.blocker.is_empty());
        assert!(project.blocker_log.is_empty());
    }

    #[test]
    fn project_details_update_all_fields_and_status() {
        let now = NaiveDate::from_ymd_opt(2026, 10, 22)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let mut project = new_project("p1".into(), "测试项目".into(), now);

        apply_project_details(
            &mut project,
            "重构导出链路".into(),
            "梳理旧接口".into(),
            "补迁移脚本".into(),
            "等数据组确认".into(),
            now,
        );

        assert_eq!(project.goal, "重构导出链路");
        assert_eq!(project.current, "梳理旧接口");
        assert_eq!(project.next, "补迁移脚本");
        assert_eq!(project.blocker, "等数据组确认");
        assert_eq!(project.status, ProjectStatus::Blocked);

        apply_project_details(
            &mut project,
            "重构导出链路".into(),
            "梳理旧接口".into(),
            "补迁移脚本".into(),
            String::new(),
            now,
        );
        assert_eq!(project.status, ProjectStatus::Active);
    }

    #[test]
    fn done_range_filters_by_completed_date() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let at = |day: i64| Some((today - chrono::Duration::days(day)).and_hms_opt(9, 0, 0).unwrap());

        assert!(DoneRange::Today.matches(at(0), today));
        assert!(!DoneRange::Today.matches(at(1), today));

        assert!(DoneRange::Week.matches(at(0), today));
        assert!(DoneRange::Week.matches(at(6), today));
        assert!(!DoneRange::Week.matches(at(7), today));

        assert!(DoneRange::Earlier.matches(at(7), today));
        assert!(DoneRange::Earlier.matches(at(30), today));
        assert!(!DoneRange::Earlier.matches(at(6), today));

        // 无完成时间 / 未来日期都不属于任何区间
        assert!(!DoneRange::Week.matches(None, today));
        assert!(!DoneRange::Week.matches(
            Some((today + chrono::Duration::days(1)).and_hms_opt(9, 0, 0).unwrap()),
            today
        ));
    }

    #[test]
    fn next_project_cycles_none_then_each_then_back() {
        let now = NaiveDate::from_ymd_opt(2026, 10, 22)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let projects = vec![
            new_project("p1".into(), "项目一".into(), now),
            new_project("p2".into(), "项目二".into(), now),
        ];

        assert_eq!(next_project(None, &projects), Some("p1".into()));
        assert_eq!(next_project(Some("p1"), &projects), Some("p2".into()));
        assert_eq!(next_project(Some("p2"), &projects), None);
        // 项目已被删除时回到「不归项目」
        assert_eq!(next_project(Some("gone"), &projects), None);
        assert_eq!(next_project(None, &[]), None);
    }

    #[test]
    fn snapshot_survives_json_roundtrip() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let data = cleared_snapshot(today, today.and_hms_opt(9, 0, 0).unwrap());

        let json = serde_json::to_string_pretty(&data).unwrap();
        let back: DataSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(back, data);
    }
}

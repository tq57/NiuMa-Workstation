//! NiuMa-Workstation 入口：旧数据迁移 → 读取配置 → 打开 SQLite → 首次种子 → 跨天自动顺延 → 主窗口。

// 发布版不弹控制台窗口（调试版保留，便于看日志）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod seed;
mod state;
mod storage;
mod theme;
mod tray;
mod update;
mod views;
mod win32;

use std::rc::Rc;

use chrono::Local;
use gpui_kit::*;
use niu_ma_workstation_core::domain::todo::run_auto_rollover;

use state::Workspace;
use storage::Storage;

fn main() -> anyhow::Result<()> {
    // 首次启动把旧版数据迁到新目录；迁移失败不阻断启动
    let _ = storage::migrate_legacy_data();

    let config = config::Config::load();

    let mut storage = Storage::open_default()?;
    if !storage.integrity_ok() {
        anyhow::bail!("数据库完整性检查未通过，请先备份后重建数据库");
    }

    let mut data = storage.load()?;
    let now = Local::now().naive_local();

    // 每天首次启动自动备份一次，保留最近 30 份；备份失败不阻断启动
    let _ = storage.auto_backup_daily(&now.format("%Y%m%d").to_string());

    // 首次启动写入示例数据；已有任何数据的旧库不会被覆盖
    if data.seeded_at.is_none() {
        data = seed::seed(now.date(), now);
        storage.save(&data)?;
    }

    if run_auto_rollover(&mut data, now.date(), now) {
        storage.save(&data)?;
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);

        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("NiuMa-Workstation".into()),
                // 去掉系统标题栏，改由应用自绘（见 Workspace::render_titlebar），
                // 让窗口顶部与内容区成为同一块连续表面。
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(40.), px(20.)),
                size: size(px(1120.), px(600.)),
            })),
            ..Default::default()
        };

        let data = data.clone();
        let mini_state = Rc::new(views::mini::MiniWindowState::default());
        gpui_kit::open_window(options, cx, move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, storage, data.clone(), config));

            // 点 × 不退出：收起主窗口，保留托盘图标与右下角今日待办小窗
            let workspace_for_close = workspace.clone();
            let mini_state_for_close = mini_state.clone();
            window.on_window_should_close(cx, move |window, cx| {
                if let Some(hwnd) = win32::current_hwnd(window) {
                    tray::set_main_hwnd(hwnd as isize);
                }
                win32::hide_window(window);
                let workspace = workspace_for_close.clone();
                let state = mini_state_for_close.clone();
                window.defer(cx, move |window, cx| {
                    views::mini::show_mini(window, cx, workspace.clone(), &state);
                });
                false
            });

            workspace
        })
        .expect("打开主窗口失败");
    });

    Ok(())
}

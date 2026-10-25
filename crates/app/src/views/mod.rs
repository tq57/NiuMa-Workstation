//! 四个视图共用的轻量组件样式。

pub(crate) mod mini;
pub(crate) mod notes;
pub(crate) mod projects;
pub(crate) mod review;
pub(crate) mod today;
pub(crate) mod update;

use chrono::{Datelike, NaiveDate};
use gpui_kit::*;

use crate::theme::Palette;

/// 标准内容卡
pub(crate) fn card(p: &Palette) -> Div {
    div()
        .bg(rgb(p.paper))
        .rounded(px(14.))
        .border_1()
        .border_color(rgb(p.line))
}

/// 小徽标
pub(crate) fn pill(text: impl Into<SharedString>, fg: u32, bg: u32) -> Div {
    div()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(5.))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// 优先级四档命名（0 最高）
pub(crate) fn priority_label(priority: u8) -> &'static str {
    match priority {
        0 => "紧急重要",
        1 => "重要不紧急",
        2 => "紧急不重要",
        _ => "不急不重要",
    }
}

pub(crate) fn priority_colors(priority: u8, p: &Palette) -> (u32, u32) {
    match priority {
        0 => (p.red, p.red_soft),
        1 => (p.teal, p.teal_soft),
        2 => (p.amber, p.amber_soft),
        _ => (p.ink3, p.line2),
    }
}

pub(crate) fn priority_pill(priority: u8, p: &Palette) -> Div {
    let (fg, bg) = priority_colors(priority, p);
    pill(priority_label(priority), fg, bg)
}

/// 描边小按钮
pub(crate) fn ghost_button(label: impl Into<SharedString>, fg: u32, p: &Palette) -> Div {
    div()
        .cursor_pointer()
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(p.line))
        .bg(rgb(p.paper))
        .text_size(px(11.))
        .text_color(rgb(fg))
        .child(label.into())
}

/// 实心主按钮
pub(crate) fn primary_button(label: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .cursor_pointer()
        .px(px(12.))
        .py(px(5.))
        .rounded(px(7.))
        .bg(rgb(p.teal))
        .text_color(rgb(p.paper))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.into())
}

/// 进度 / 比例条
pub(crate) fn bar(rate: f32, color: u32, track: u32) -> Div {
    div()
        .w_full()
        .h(px(6.))
        .rounded(px(3.))
        .bg(rgb(track))
        .child(
            div()
                .h_full()
                .rounded(px(3.))
                .bg(rgb(color))
                .w(relative(rate.clamp(0.0, 1.0))),
        )
}

/// 灰色提示
pub(crate) fn hint(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(p.ink3))
        .child(text.into())
}

/// 把主题色按透明度叠加到背景上（近似 demo 里的 color-mix）。
pub(crate) fn tint(color: u32, alpha: u8) -> Rgba {
    rgba((color << 8) | alpha as u32)
}

pub(crate) fn weekday_cn(day: NaiveDate) -> &'static str {
    match day.weekday().num_days_from_monday() {
        0 => "周一",
        1 => "周二",
        2 => "周三",
        3 => "周四",
        4 => "周五",
        5 => "周六",
        _ => "周日",
    }
}

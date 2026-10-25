//! 检查更新弹框。

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{card, ghost_button, primary_button};
use crate::state::Workspace;
use crate::update::{self, UpdateStatus};

impl Workspace {
    pub(crate) fn render_update_modal(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let portable = update::is_portable();
        let status = self.update_status.clone();

        let body = match status {
            None | Some(UpdateStatus::Checking) => {
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(p.ink2))
                    .child("正在检查最新版本…")
                    .into_any_element()
            }
            Some(UpdateStatus::UpToDate) => div()
                .text_size(px(12.5))
                .text_color(rgb(p.ink2))
                .child(format!("当前已是最新版本 v{}", update::CURRENT_VERSION))
                .into_any_element(),
            Some(UpdateStatus::Available(update)) => {
                let notes = div()
                    .id("update-notes")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(p.ink2))
                    .child(if update.notes.trim().is_empty() {
                        "本次更新没有附说明。".to_string()
                    } else {
                        update.notes.clone()
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.teal))
                                    .child(format!("发现新版本 v{}", update.version)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(p.ink3))
                                    .child(if update.installer.size > 0 {
                                        format!("{:.1} MB", update.installer.size as f64 / 1024.0 / 1024.0)
                                    } else {
                                        String::new()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .p(px(10.))
                            .rounded(px(8.))
                            .bg(rgb(p.bg))
                            .child(notes),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(p.ink3))
                            .child(if portable {
                                "当前是绿色版：请下载后手动替换程序文件，数据目录不受影响。"
                            } else {
                                "下载完成后由你确认安装；安装前会自动备份数据库，应用退出后自动重启。"
                            }),
                    )
                    .into_any_element()
            }
            Some(UpdateStatus::Downloading { version }) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(p.ink2))
                        .child(format!("正在下载 v{version}…")),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(p.ink3))
                        .child("下载完成后会校验 SHA-256，再等待你确认安装。"),
                )
                .into_any_element(),
            Some(UpdateStatus::Ready { version, .. }) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.teal))
                        .child(format!("v{version} 已下载并通过校验")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(p.ink3))
                        .child("点「安装并重启」后应用会退出，安装完成后自动重新打开。"),
                )
                .into_any_element(),
            Some(UpdateStatus::Failed(message)) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.red))
                        .child("更新失败"),
                )
                .child(
                    div()
                        .id("update-error")
                        .max_h(px(150.))
                        .overflow_y_scroll()
                        .text_size(px(11.5))
                        .line_height(px(17.))
                        .text_color(rgb(p.ink2))
                        .child(message),
                )
                .into_any_element(),
        };

        let mut actions = div().flex().items_center().justify_end().gap(px(8.));
        actions = actions.child(
            ghost_button("关闭", p.ink2, &p)
                .id("um-close")
                .on_click(cx.listener(|this, _, _, cx| this.close_update_modal(cx))),
        );
        match self.update_status.clone() {
            Some(UpdateStatus::Available(_)) if portable => {
                actions = actions.child(
                    primary_button("打开下载页", &p)
                        .id("um-release")
                        .on_click(cx.listener(|this, _, _, cx| this.open_releases_page(cx))),
                );
            }
            Some(UpdateStatus::Available(_)) => {
                actions = actions.child(
                    primary_button("下载更新", &p)
                        .id("um-download")
                        .on_click(cx.listener(|this, _, _, cx| this.download_update(cx))),
                );
            }
            Some(UpdateStatus::Ready { .. }) => {
                actions = actions.child(
                    primary_button("安装并重启", &p)
                        .id("um-install")
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                );
            }
            Some(UpdateStatus::Failed(_)) => {
                actions = actions.child(
                    primary_button("重新检查", &p)
                        .id("um-retry")
                        .on_click(cx.listener(|this, _, _, cx| this.check_update(cx))),
                );
            }
            _ => {}
        }

        div()
            .id("update-modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .child(
                card(&p)
                    .w(px(520.))
                    .p(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("软件更新"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(p.ink3))
                                    .child(format!("当前版本 v{}", update::CURRENT_VERSION)),
                            ),
                    )
                    .child(body)
                    .child(actions),
            )
            .into_any_element()
    }
}

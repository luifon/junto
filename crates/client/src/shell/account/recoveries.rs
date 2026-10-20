//! The recoveries page of the account layer (ADR 0112, ADR 0118, ADR 0781): every recovery
//! of the account, the newest first, also those that this device has acknowledged.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};

use crate::shell::Shell;
use crate::shell::admin::{line, named, page_title, sentence};
use crate::state::account::{AccountPage, RecoveryLine, RecoveryList};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Chip, Notice, StyleExt};

/// One recovery: when, and by whom, and whether it added this device. Each line is cut at
/// the width of the row.
fn recovery_row(row: &RecoveryLine, theme: &Theme) -> AnyElement {
    let title = div()
        .flex()
        .items_center()
        .gap(u(8.0))
        .child(
            div()
                .min_w_0()
                .typeface(theme.types.control_strong, theme)
                .ink(theme.colors.ink)
                .truncate()
                .child(text::account::recovery_line(row.performed_at, row.by_host)),
        )
        .when(row.unseen, |title| {
            title.child(Chip::new(text::account::NOT_SEEN_HERE))
        });
    let content = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(title)
        .when(row.this_device, |content| {
            content.child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .truncate()
                    .child(text::account::ADDED_THIS_DEVICE),
            )
        });
    named(
        format!("recovery-row-{}", row.id),
        line(theme).child(content),
    )
    .into_any_element()
}

impl Shell {
    pub(crate) fn render_recoveries_page(
        &mut self,
        list: &RecoveryList,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let not_read = |reason: &str, cx: &mut Context<Self>| {
            div().flex_none().py(u(8.0)).child(
                Notice::error(format!("{} {reason}", text::account::RECOVERIES_NOT_READ))
                    .selector("recoveries-read-failed")
                    .action(
                        Button::new("recoveries-read-again")
                            .label(text::TRY_AGAIN)
                            .selector("recoveries-read-again")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.reload_recoveries(cx));
                            })),
                    ),
            )
        };
        let body: AnyElement = match list {
            RecoveryList::Reading => sentence(text::account::READING, &theme)
                .py(u(10.0))
                .debug_selector(|| "recoveries-reading".to_owned())
                .into_any_element(),
            RecoveryList::Failed(reason) => not_read(reason, cx).into_any_element(),
            RecoveryList::Listed {
                rows,
                hidden,
                failed,
            } => {
                let mut column = div().flex().flex_col().flex_none();
                if rows.is_empty() {
                    column = column.child(
                        sentence(text::account::NO_RECOVERIES, &theme)
                            .py(u(10.0))
                            .debug_selector(|| "recoveries-none".to_owned()),
                    );
                }
                for row in rows {
                    column = column.child(recovery_row(row, &theme));
                }
                column
                    .when(*hidden > 0, |column| {
                        column.child(
                            sentence(text::account::more_recoveries(*hidden), &theme).mt(u(6.0)),
                        )
                    })
                    .child(
                        crate::ui::notice_slot("recoveries-read-failed-slot")
                            .children(failed.as_deref().map(|reason| not_read(reason, cx))),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .child(page_title(
                text::account::page_name(AccountPage::Recoveries),
                text::account::RECOVERIES_HELP,
                &theme,
            ))
            .child(body)
            .into_any_element()
    }
}

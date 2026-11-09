use super::{Button, ButtonKind};
use crate::{
    assets::Icon,
    text,
    theme::{self, Theme, u},
};
use gpui::{
    Context, IntoElement, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window,
    div, px,
};

#[derive(Clone, Copy, Debug)]
enum Sizing {
    Word,
    Copy,
    Presence,
    Emoji,
    Section,
}

struct Host {
    label: String,
    sizing: Sizing,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let button = Button::new("ghost-caption-audit")
            .kind(ButtonKind::Ghost)
            .label(self.label.clone())
            .selector("ghost-caption-audit");
        let button = match self.sizing {
            Sizing::Word => button,
            Sizing::Copy => button.h(u(22.0)).px(u(6.0)),
            Sizing::Presence => button.h(u(19.0)).px(u(0.0)),
            Sizing::Emoji => button.h(u(22.0)).min_w(u(26.0)).px(u(1.0)),
            Sizing::Section => button.w_full().icon(Icon::Plus),
        };
        div().size_full().flex().items_start().p(u(20.0)).child(
            div()
                .flex()
                .items_start()
                .w(u(if matches!(self.sizing, Sizing::Section) {
                    144.0
                } else {
                    650.0
                }))
                .child(button),
        )
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            cx.run_until_parked();
            return;
        }
    }
    panic!("button geometry did not settle");
}

#[gpui::test]
fn ghost_captions_fit_the_styles_used_by_the_application(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::assets::load_fonts(cx).unwrap();
        theme::install(Theme::stone(), cx);
    });
    let mut cases: Vec<_> = [
        text::SET_KEYS,
        text::CHANGE_KEYS,
        text::REMOVE_KEYS,
        text::RESET_KEYS,
        text::VIEW_ORIGINAL,
        text::DISMISS,
        text::SHOW_EARLIER_LINES,
        "Open file",
        text::followed::USE_DEFAULT,
    ]
    .into_iter()
    .map(|label| (label.to_owned(), Sizing::Word))
    .collect();
    cases.push((text::COPY_TEXT.to_owned(), Sizing::Copy));
    cases.push((text::admin::NEW_CATEGORY.to_owned(), Sizing::Section));
    for presence in [
        junto_client_core::protocol::presence::Presence::Online,
        junto_client_core::protocol::presence::Presence::Idle,
        junto_client_core::protocol::presence::Presence::DoNotDisturb,
        junto_client_core::protocol::presence::Presence::Invisible,
        junto_client_core::protocol::presence::Presence::Offline,
    ] {
        cases.push((text::presence::label(presence).to_owned(), Sizing::Presence));
    }
    for label in ["👍", "👍🏽", "👨‍👩‍👧‍👦", "🇺🇳", "🙂", "♥️"] {
        cases.push((label.to_owned(), Sizing::Emoji));
    }
    cases.extend(crate::zones::names().map(|name| (crate::zones::shown(name), Sizing::Word)));
    let (host, cx) = cx.add_window_view(|_, _| Host {
        label: String::new(),
        sizing: Sizing::Word,
    });
    for dark in [false, true] {
        for scale in [theme::SCALE_MIN, theme::SCALE_DEFAULT, theme::SCALE_MAX] {
            cx.update(|window, cx| {
                theme::install(
                    if dark {
                        Theme::broadcast()
                    } else {
                        Theme::stone()
                    },
                    cx,
                );
                window.set_rem_size(theme::rem_size(scale));
            });
            for (label, sizing) in &cases {
                let _record = crate::ui::record_painted_text();
                host.update(cx, |host, cx| {
                    host.label = label.clone();
                    host.sizing = *sizing;
                    cx.notify();
                });
                cx.update(|window, _| window.refresh());
                settle(cx);
                let button = cx.debug_bounds("ghost-caption-audit").unwrap();
                let painted = crate::ui::take_painted_text();
                let shown = painted
                    .iter()
                    .find(|text| text.text.as_ref() == label.as_str())
                    .unwrap_or_else(|| panic!("{label:?} was not painted"));
                let edge = px(0.5);
                assert!(
                    shown.bounds.left() >= button.left() - edge
                        && shown.bounds.right() <= button.right() + edge
                        && shown.bounds.top() >= button.top() - edge
                        && shown.bounds.bottom() <= button.bottom() + edge,
                    "{label:?} ({sizing:?}, scale{scale}, dark{dark}) at {:?} leaves {button:?}",
                    shown.bounds
                );
            }
        }
    }
}

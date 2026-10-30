//! The pictures of people in the window (ADR 0783): how a drawing gets the avatar of an
//! account, and the picture of the profile preset in the settings.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Role,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::{AccountId, ServerId};

use super::Shell;
use super::channels::name_seed;
use crate::state::people::ServerFaces;
use crate::text::people as txt;
use crate::theme::{Theme, u};
use crate::ui::{Avatar, Button, Notice, StyleExt};

/// The side of the picture of the preset in the settings, in design pixels.
const PRESET_PICTURE_SIZE: f32 = 56.0;

/// The avatar of an account at a size in design pixels: its image when the window holds
/// it, and the generated avatar otherwise.
pub(crate) fn avatar(faces: &ServerFaces, account_id: AccountId, size: f32) -> Avatar {
    Avatar::new(account_id.as_uuid().as_u128(), size)
        .image(faces.image(account_id))
        .presence(faces.presence(account_id))
        .when_some(faces.server_id(), |avatar, server| {
            avatar.profile(crate::ui::OpenProfile {
                server,
                account: account_id,
            })
        })
}

pub(crate) fn person_name(
    faces: &ServerFaces,
    account: AccountId,
    name: impl Into<SharedString>,
    id: &'static str,
) -> AnyElement {
    let name = name.into();
    match faces.server_id() {
        Some(server) => crate::ui::ProfileName::new(
            (id, account.as_uuid().as_u128() as u64),
            name,
            crate::ui::OpenProfile { server, account },
        )
        .role_color(faces.color(account))
        .into_any_element(),
        None => name.into_any_element(),
    }
}

pub(crate) fn person_element(
    faces: &ServerFaces,
    account: AccountId,
    name: &str,
    id: &'static str,
    element: impl IntoElement,
) -> AnyElement {
    match faces.server_id() {
        Some(server) => crate::ui::ProfileAccess::new(
            (id, account.as_uuid().as_u128() as u64),
            crate::ui::OpenProfile { server, account },
            crate::text::profile::open_for(name),
            element,
        )
        .into_any_element(),
        None => element.into_any_element(),
    }
}

pub(crate) fn person_label(
    faces: &ServerFaces,
    account: AccountId,
    label: impl Into<SharedString>,
    id: &'static str,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(u(4.))
        .min_w_0()
        .child(crate::ui::PresenceMark::new(faces.presence(account), 10.))
        .child(person_name(faces, account, label, id))
        .into_any_element()
}

/// The avatar of an account in a list of people, with the readable alternative of its
/// picture (ADR 0702). `pictured` is what the list of the server states: the account set
/// a picture. A profile that the window read states the same. The alternative names the
/// person, and it is present also while the image itself is not held.
pub(crate) fn avatar_of(
    faces: &ServerFaces,
    account_id: AccountId,
    name: &str,
    pictured: bool,
    size: f32,
) -> Avatar {
    let shown = avatar(faces, account_id, size);
    if pictured || faces.face(account_id).is_some() {
        shown.described(
            ("picture", account_id.as_uuid().as_u128() as u64),
            txt::picture_of(name),
        )
    } else {
        shown
    }
}

impl Shell {
    /// The pictures of the people of a server, as the view of this frame carries them.
    pub(crate) fn faces(&self, server_id: ServerId) -> ServerFaces {
        self.view
            .as_ref()
            .map(|view| view.faces.of(server_id))
            .unwrap_or_default()
    }

    /// The pictures of the people of the selected server.
    pub(crate) fn shown_faces(&self) -> ServerFaces {
        self.view
            .as_ref()
            .and_then(|view| {
                Some(
                    view.faces
                        .of(view.selected.as_ref()?.id)
                        .in_workspace(view.workspace().map(|workspace| workspace.id)),
                )
            })
            .unwrap_or_default()
    }

    /// Asks the person for a file, and stores it as the picture of the profile preset.
    pub(crate) fn choose_preset_picture(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.use_preset_picture(file, cx));
            }
        })
        .detach();
    }

    /// Stores the file at this path as the picture of the profile preset, as the file
    /// dialog does.
    pub fn use_preset_picture(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.set_preset_picture(file, cx));
    }

    /// The picture of the profile preset in the settings: the picture, the two actions
    /// with the bounds of a picture, why the last change was not made, and one line for
    /// each server that refused the picture (ADR 0717).
    pub(crate) fn render_preset_picture(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (picture, name) = {
            let state = self.state.read(cx);
            (
                state.preset_picture().clone(),
                state.preset_name().to_owned(),
            )
        };
        let refused = self
            .view
            .as_ref()
            .map(|view| view.preset_refused.clone())
            .unwrap_or_default();
        let note = |sentence: &'static str| {
            div()
                .mt(u(5.0))
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        let mut rows = vec![
            div()
                .mt(u(12.0))
                .flex()
                .items_center()
                .gap(u(12.0))
                .child(
                    div()
                        .id("preset-picture")
                        .role(Role::Image)
                        .aria_label(txt::PROFILE_PICTURE)
                        .debug_selector(|| "preset-picture".to_owned())
                        .flex_none()
                        .child(
                            Avatar::new(name_seed(&name), PRESET_PICTURE_SIZE)
                                .radius(4.0)
                                .image(picture.image.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(u(8.0))
                        .child(
                            Button::new("choose-picture")
                                .label(if picture.saving {
                                    txt::SAVING_PICTURE
                                } else {
                                    txt::CHOOSE_PICTURE
                                })
                                .disabled(picture.saving)
                                .selector("choose-picture")
                                .h(u(32.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.choose_preset_picture(cx);
                                })),
                        )
                        .when(picture.stored, |actions| {
                            actions.child(
                                Button::new("remove-picture")
                                    .label(txt::REMOVE_PICTURE)
                                    .disabled(picture.saving)
                                    .selector("remove-picture")
                                    .h(u(32.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.remove_preset_picture(cx)
                                        });
                                    })),
                            )
                        }),
                )
                .into_any_element(),
            note(txt::PICTURE_HELP).into_any_element(),
        ];
        let mut notices = Vec::new();
        if !picture.stored {
            notices.push(note(txt::NO_PICTURE).into_any_element());
        } else if picture.image.is_none() && !picture.saving {
            notices.push(
                note(txt::PICTURE_NOT_SHOWN)
                    .debug_selector(|| "picture-not-shown".to_owned())
                    .into_any_element(),
            );
        }
        if let Some(failed) = picture.failed {
            notices.push(
                div()
                    .mt(u(8.0))
                    .child(Notice::error(failed).selector("picture-error"))
                    .into_any_element(),
            );
        }
        for (server_id, server) in refused {
            notices.push(
                div()
                    .mt(u(8.0))
                    .debug_selector(move || format!("preset-refused:{server_id}"))
                    .child(Notice::new(SharedString::from(txt::preset_refused(
                        &server,
                    ))))
                    .into_any_element(),
            );
        }
        rows.push(
            crate::ui::notice_slot("picture-notice-slot")
                .at_end()
                .children(notices)
                .into_any_element(),
        );
        rows
    }
}

/// Colours a retained account name within a sentence without colouring its other words.
pub(crate) fn name_in_sentence(
    line: impl Into<SharedString>,
    range: std::ops::Range<usize>,
    color: Option<junto_client_core::protocol::appearance::RoleColor>,
    background: gpui::Rgba,
    bold: bool,
) -> gpui::StyledText {
    let line = line.into();
    let style = gpui::HighlightStyle {
        color: color.map(|color| crate::theme::role_ink(color, background).into()),
        font_weight: bold.then_some(gpui::FontWeight::BOLD),
        ..gpui::HighlightStyle::default()
    };
    if range.start <= range.end
        && range.end <= line.len()
        && line.is_char_boundary(range.start)
        && line.is_char_boundary(range.end)
    {
        gpui::StyledText::new(line).with_highlights([(range, style)])
    } else {
        gpui::StyledText::new(line)
    }
}

pub(crate) fn person_sentence_label(
    faces: &ServerFaces,
    account: AccountId,
    line: impl Into<SharedString>,
    range: std::ops::Range<usize>,
    id: &'static str,
    background: gpui::Rgba,
) -> AnyElement {
    let line = line.into();
    div()
        .flex()
        .items_center()
        .gap(u(4.))
        .min_w_0()
        .child(crate::ui::PresenceMark::new(faces.presence(account), 10.))
        .child(person_element(
            faces,
            account,
            &line,
            id,
            name_in_sentence(line.clone(), range, faces.color(account), background, false),
        ))
        .into_any_element()
}

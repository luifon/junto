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
    Avatar::new(account_id.as_uuid().as_u128(), size).image(faces.image(account_id))
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
            .and_then(|view| Some(view.faces.of(view.selected.as_ref()?.id)))
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
        if !picture.stored {
            rows.push(note(txt::NO_PICTURE).into_any_element());
        } else if picture.image.is_none() && !picture.saving {
            rows.push(
                note(txt::PICTURE_NOT_SHOWN)
                    .debug_selector(|| "picture-not-shown".to_owned())
                    .into_any_element(),
            );
        }
        if let Some(failed) = picture.failed {
            rows.push(
                div()
                    .mt(u(8.0))
                    .child(Notice::error(failed).selector("picture-error"))
                    .into_any_element(),
            );
        }
        for (server_id, server) in refused {
            rows.push(
                div()
                    .mt(u(8.0))
                    .debug_selector(move || format!("preset-refused:{server_id}"))
                    .child(Notice::new(SharedString::from(txt::preset_refused(
                        &server,
                    ))))
                    .into_any_element(),
            );
        }
        rows
    }
}

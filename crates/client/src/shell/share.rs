//! Screen shares in the window (ADR 0738): the dialog in which a person chooses what to
//! share and its sound, and the stage of a call with a share: the bar of shares, the line
//! about the watched share, its picture, and the strip of the people.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    RenderImage, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    StyledText, Window, canvas, div, img, prelude::FluentBuilder,
};
use junto_client_core::protocol::call::ShareAudio;
use junto_media::engine::ShareId;
use junto_media::share::picture::PictureFrame;
use junto_media::share::{
    ApplicationId, CaptureError, PictureSource, ScreenAccess, ShareKind, ShareSources, SoundChoice,
};

use super::call::{occupants, tile_marks};
use super::channels::occupant_name;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{CallPanel, ChannelRow, Microphone, ShareCard, ShareStage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Mosaic, MosaicKind, Notice, PersonTile, ScrollArea, Segmented,
    StyleExt, dialog_footer, focus_ring, focus_ring_inside, icon, tooltip,
};

/// The kinds of sound that a share can have (ADR 0502).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    None,
    System,
    Application,
}

const SOUND_KINDS: [SoundKind; 3] = [SoundKind::None, SoundKind::System, SoundKind::Application];

/// What the system lists for a share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listed {
    Reading,
    Sources(ShareSources),
    Failed(CaptureError),
}

/// The dialog in which a person chooses what to share. Nothing is chosen at its start:
/// the person chooses the picture and the sound of each share (ADR 0502).
pub(crate) struct SharePicker {
    pub access: ScreenAccess,
    pub listed: Listed,
    pub picture: Option<PictureSource>,
    pub sound: Option<SoundKind>,
    /// The application whose sound is shared with a screen.
    pub application: Option<ApplicationId>,
    scroll: ScrollHandle,
    /// Counts the reads of the lists. The answer of an earlier read is not used.
    read: u64,
}

impl SharePicker {
    fn new(access: ScreenAccess, listed: Listed, read: u64) -> Self {
        Self {
            access,
            listed,
            picture: None,
            sound: None,
            application: None,
            scroll: ScrollHandle::new(),
            read,
        }
    }

    /// Takes the picture that the person picked. The sound of one application names an
    /// application through the picture: the application of a window, or the one chosen
    /// with a screen. With another picture that choice is not the choice of the person any
    /// more, and the sound is asked again (ADR 0502).
    pub(crate) fn pick_picture(&mut self, picture: PictureSource) {
        if self.picture != Some(picture) && self.sound == Some(SoundKind::Application) {
            self.sound = None;
            self.application = None;
        }
        self.picture = Some(picture);
    }

    fn sources(&self) -> Option<&ShareSources> {
        match &self.listed {
            Listed::Sources(sources) => Some(sources),
            Listed::Reading | Listed::Failed(_) => None,
        }
    }

    fn sound_kinds(&self) -> Vec<SoundKind> {
        let Some(sources) = self.sources() else {
            return vec![SoundKind::None];
        };
        SOUND_KINDS
            .into_iter()
            .filter(|kind| match kind {
                SoundKind::None => true,
                SoundKind::System => sources.system_sound,
                SoundKind::Application => {
                    !sources.applications.is_empty() || !sources.windows.is_empty()
                }
            })
            .collect()
    }

    /// The share that the choices of the person describe, when they are complete. The
    /// sound of one application is the application of a shared window, and the chosen
    /// application with a shared screen.
    pub(crate) fn chosen(&self) -> Option<(PictureSource, SoundChoice)> {
        let sources = self.sources()?;
        let picture = self.picture?;
        let listed = match picture {
            PictureSource::Portal(kind) => sources.portal.contains(&kind),
            PictureSource::Display(id) => sources.displays.iter().any(|held| held.id == id),
            PictureSource::Window(id) => sources.windows.iter().any(|held| held.id == id),
        };
        if !listed {
            return None;
        }
        let sound = match (self.sound?, picture) {
            (SoundKind::None, _) => SoundChoice::None,
            (SoundKind::System, _) if sources.system_sound => SoundChoice::System,
            (SoundKind::System, _) => return None,
            (SoundKind::Application, PictureSource::Window(id)) => SoundChoice::Application(
                sources
                    .windows
                    .iter()
                    .find(|window| window.id == id)?
                    .application
                    .clone(),
            ),
            (SoundKind::Application, PictureSource::Display(_) | PictureSource::Portal(_)) => {
                let application = self.application.as_ref()?;
                sources
                    .applications
                    .iter()
                    .find(|held| held.id == *application)?;
                SoundChoice::Application(application.clone())
            }
        };
        Some((picture, sound))
    }
}

/// The pictures of the shares as the window draws them.
#[derive(Default)]
pub(crate) struct ShownPictures {
    pub main: Option<(ShareId, Arc<RenderImage>)>,
    pub small: HashMap<ShareId, (Arc<PictureFrame>, Arc<RenderImage>)>,
}

/// A picture of the media engine as an image of the window.
pub(crate) fn render_image(frame: &PictureFrame) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra.to_vec())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl Shell {
    /// Follows the pictures of the call of this device. An image that is replaced is
    /// released: the window keeps each image that it drew until it is told otherwise.
    pub(crate) fn follow_pictures(&mut self, cx: &mut Context<Self>) {
        let calls = self.state.read(cx).calls().clone();
        let mut main = calls.main_picture();
        let follow_main = cx.spawn(async move |this, cx| {
            while main.changed().await.is_ok() {
                let picture = main.borrow_and_update().clone();
                let shown = picture
                    .and_then(|picture| Some((picture.share, render_image(&picture.frame)?)));
                if this
                    .update(cx, |this, cx| this.show_main(shown, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut small = calls.small_pictures();
        let follow_small = cx.spawn(async move |this, cx| {
            while small.changed().await.is_ok() {
                let pictures = small.borrow_and_update().clone();
                if this
                    .update(cx, |this, cx| this.show_small(&pictures, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.picture_tasks = vec![follow_main, follow_small];
    }

    pub(crate) fn show_main(
        &mut self,
        shown: Option<(ShareId, Arc<RenderImage>)>,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, before)) = std::mem::replace(&mut self.pictures.main, shown) {
            cx.drop_image(before, None);
        }
        cx.notify();
    }

    pub(crate) fn show_small(
        &mut self,
        pictures: &HashMap<ShareId, Arc<PictureFrame>>,
        cx: &mut Context<Self>,
    ) {
        let mut released = Vec::new();
        self.pictures.small.retain(|share, (frame, image)| {
            let kept = pictures
                .get(share)
                .is_some_and(|now| Arc::ptr_eq(now, frame));
            if !kept {
                released.push(image.clone());
            }
            kept
        });
        for (share, frame) in pictures {
            if !self.pictures.small.contains_key(share)
                && let Some(image) = render_image(frame)
            {
                self.pictures.small.insert(*share, (frame.clone(), image));
            }
        }
        for image in released {
            cx.drop_image(image, None);
        }
        cx.notify();
    }

    /// The share button of the strip of the call: it opens the dialog, or ends the share
    /// that runs.
    pub(crate) fn toggle_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sharing = self
            .state
            .read(cx)
            .view()
            .call
            .is_some_and(|strip| strip.share != crate::state::view::OwnShare::Off);
        if sharing {
            self.state.update(cx, |state, _| state.stop_share());
        } else {
            self.open_share_picker(window, cx);
        }
    }

    /// Opens the dialog of a share, and reads what the system lists. Without the
    /// permission of the system the dialog tells how the person gives it, and nothing is
    /// read: the system would show its own question (ADR 0736).
    pub(crate) fn open_share_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let calls = self.state.read(cx).calls().clone();
        self.share_reads += 1;
        let read = self.share_reads;
        if !calls.can_share() {
            let listed = Listed::Failed(CaptureError::Unsupported);
            let picker = SharePicker::new(ScreenAccess::NotAllowed, listed, read);
            self.open_layer(Layer::SharePicker(picker), window, cx);
            return;
        }
        let access = calls.screen_access();
        let picker = SharePicker::new(access, Listed::Reading, read);
        self.open_layer(Layer::SharePicker(picker), window, cx);
        if access == ScreenAccess::NotAllowed {
            return;
        }
        let reading = calls.share_sources();
        cx.spawn(async move |this, cx| {
            let listed = reading.await;
            let _ = this.update(cx, |this, cx| {
                if let Layer::SharePicker(picker) = &mut this.layer
                    && picker.read == read
                {
                    picker.listed = match listed {
                        Ok(sources) => Listed::Sources(sources),
                        Err(error) => Listed::Failed(error),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// True while the dialog of a share is open.
    pub fn share_picker_is_open(&self) -> bool {
        matches!(self.layer, Layer::SharePicker(_))
    }

    /// True when the dialog of a share shows what the system lists.
    pub fn share_picker_is_listed(&self) -> bool {
        matches!(&self.layer, Layer::SharePicker(picker) if picker.sources().is_some())
    }

    /// Chooses the picture of the share in its dialog, as a click on its row does.
    pub fn pick_share_picture(&mut self, picture: PictureSource, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker() {
            picker.pick_picture(picture);
            cx.notify();
        }
    }

    /// Chooses the sound of the share in its dialog, as a click on its control does.
    pub fn pick_share_sound(&mut self, sound: SoundKind, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker() {
            picker.sound = Some(sound);
            cx.notify();
        }
    }

    /// Chooses the application whose sound is shared with a screen.
    pub fn pick_share_application(&mut self, application: ApplicationId, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker() {
            picker.application = Some(application);
            cx.notify();
        }
    }

    /// Draws these pictures in place of the pictures of the call. The screen tool uses it
    /// to draw a share without a media server.
    #[cfg(feature = "test-support")]
    pub fn show_pictures_fixture(
        &mut self,
        main: Option<(ShareId, PictureFrame)>,
        small: Vec<(ShareId, PictureFrame)>,
        cx: &mut Context<Self>,
    ) {
        self.picture_tasks.clear();
        let main = main.and_then(|(share, frame)| Some((share, render_image(&frame)?)));
        self.show_main(main, cx);
        let small = small
            .into_iter()
            .map(|(share, frame)| (share, Arc::new(frame)))
            .collect();
        self.show_small(&small, cx);
    }

    fn picker(&mut self) -> Option<&mut SharePicker> {
        match &mut self.layer {
            Layer::SharePicker(picker) => Some(picker),
            _ => None,
        }
    }

    /// Starts the share that the dialog describes, and closes the dialog.
    fn start_chosen_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((picture, sound)) = self.picker().and_then(|picker| picker.chosen()) else {
            return;
        };
        self.state
            .update(cx, |state, _| state.start_share(picture, sound));
        self.close_layer(window, cx);
    }

    pub(crate) fn render_share_picker(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let Layer::SharePicker(picker) = &self.layer else {
            return div().into_any_element();
        };
        let dialog = Dialog::new(text::SHARE_PICKER_TITLE, self.layer_focus())
            .selector("share-picker")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }));
        let cancel = |cx: &mut Context<Self>| {
            Button::new("share-cancel")
                .label(text::CANCEL)
                .selector("share-cancel")
                .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                .into_any_element()
        };
        let note = |sentence: SharedString| {
            div()
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        let label = |title: &'static str| {
            div()
                .mb(u(4.0))
                .typeface(theme.types.control, &theme)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .ink(colors.ink)
                .child(title)
        };

        if let Listed::Failed(CaptureError::Unsupported) = &picker.listed {
            return dialog
                .child(Notice::new(text::SHARE_UNSUPPORTED).selector("share-unsupported"))
                .child(dialog_footer([cancel(cx)]))
                .into_any_element();
        }
        if picker.access == ScreenAccess::NotAllowed {
            return dialog
                .child(
                    div()
                        .id("screen-access")
                        .role(Role::Status)
                        .aria_label(text::SCREEN_ACCESS_TITLE)
                        .debug_selector(|| "screen-access".to_owned())
                        .flex()
                        .gap(u(10.0))
                        .child(div().mt(u(2.0)).child(icon(
                            Icon::Lock,
                            17.0,
                            colors.terracotta_deep,
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .typeface(theme.types.heading, &theme)
                                        .ink(colors.ink)
                                        .child(text::SCREEN_ACCESS_TITLE),
                                )
                                .child(
                                    div()
                                        .mt(u(3.0))
                                        .typeface(theme.types.notice, &theme)
                                        .ink(colors.ink_2)
                                        .child(text::SCREEN_ACCESS_HELP),
                                ),
                        ),
                )
                .child(dialog_footer([
                    cancel(cx),
                    Button::new("check-screen-access")
                        .label(text::CHECK_AGAIN)
                        .icon(Icon::Retry)
                        .selector("check-screen-access")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.layer = Layer::None;
                            this.open_share_picker(window, cx);
                        }))
                        .into_any_element(),
                    Button::new("open-system-settings")
                        .kind(ButtonKind::Primary)
                        .label(text::OPEN_SYSTEM_SETTINGS)
                        .selector("open-system-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.read(cx).calls().ask_for_screen_access();
                        }))
                        .into_any_element(),
                ]))
                .into_any_element();
        }
        let sources = match &picker.listed {
            Listed::Reading => {
                return dialog
                    .child(
                        div()
                            .id("share-reading")
                            .role(Role::Status)
                            .aria_label(text::SHARE_READING)
                            .debug_selector(|| "share-reading".to_owned())
                            .child(note(text::SHARE_READING.into())),
                    )
                    .child(dialog_footer([cancel(cx)]))
                    .into_any_element();
            }
            Listed::Failed(error) => {
                return dialog
                    .child(Notice::error(text::capture_error(error)).selector("share-list-error"))
                    .child(dialog_footer([
                        cancel(cx),
                        Button::new("share-read-again")
                            .label(text::TRY_AGAIN)
                            .icon(Icon::Retry)
                            .selector("share-read-again")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.layer = Layer::None;
                                this.open_share_picker(window, cx);
                            }))
                            .into_any_element(),
                    ]))
                    .into_any_element();
            }
            Listed::Sources(sources) => sources.clone(),
        };
        let (picture, sound, application) =
            (picker.picture, picker.sound, picker.application.clone());
        let scroll = picker.scroll.clone();
        let ready = picker.chosen().is_some();
        let sound_kinds = picker.sound_kinds();

        // One row of a list in which the person chooses one entry.
        let row = |id: (&'static str, usize),
                   selector: String,
                   shown: Icon,
                   title: SharedString,
                   detail: SharedString,
                   on: bool| {
            let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
            div()
                .id(id)
                .role(Role::RadioButton)
                .aria_label(title.clone())
                .aria_selected(on)
                .debug_selector(move || selector.clone())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(9.0))
                .py(u(5.0))
                .px(u(10.0))
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .when(on, |row| row.bg(Hsla::from(colors.limestone_lo)))
                .when(!on, |row| {
                    row.hover(move |style| style.bg(Hsla::from(colors.travertine)))
                })
                .focus_visible(move |style| style.shadow(ring))
                .child(icon(
                    if on { Icon::Check } else { shown },
                    15.0,
                    if on {
                        colors.terracotta_deep
                    } else {
                        colors.ink_2
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .typeface(theme.types.control, &theme)
                                .font_weight(if on {
                                    gpui::FontWeight::BOLD
                                } else {
                                    gpui::FontWeight::SEMIBOLD
                                })
                                .ink(colors.ink)
                                .child(title),
                        )
                        .child(
                            div()
                                .truncate()
                                .typeface(theme.types.small, &theme)
                                .ink(colors.ink_2)
                                .child(detail),
                        ),
                )
        };
        let section = |title: &'static str| {
            div()
                .flex_none()
                .pt(u(6.0))
                .pb(u(2.0))
                .px(u(10.0))
                .typeface(theme.types.small_strong, &theme)
                .ink(colors.bronze)
                .child(title)
        };
        let mut rows: Vec<AnyElement> = vec![section(text::SCREENS).into_any_element()];
        if sources.portal.contains(&ShareKind::Screen) {
            let source = PictureSource::Portal(ShareKind::Screen);
            rows.push(
                row(
                    ("share-portal-screen", 0),
                    "share-portal-screen".into(),
                    Icon::Screen,
                    text::SHARE_DESKTOP_SCREEN.into(),
                    text::SHARE_DESKTOP_HELP.into(),
                    picture == Some(source),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.picker() {
                        picker.pick_picture(source);
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }
        for (index, display) in sources.displays.iter().enumerate() {
            let source = PictureSource::Display(display.id);
            rows.push(
                row(
                    ("share-display", index),
                    format!("share-display:{}", display.id),
                    Icon::Screen,
                    text::screen_name(index + 1).into(),
                    text::pixels((display.width, display.height)).into(),
                    picture == Some(source),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.picker() {
                        picker.pick_picture(source);
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }
        rows.push(section(text::WINDOWS).into_any_element());
        if sources.portal.contains(&ShareKind::Window) {
            let source = PictureSource::Portal(ShareKind::Window);
            rows.push(
                row(
                    ("share-portal-window", 0),
                    "share-portal-window".into(),
                    Icon::Detach,
                    text::SHARE_DESKTOP_WINDOW.into(),
                    text::SHARE_DESKTOP_HELP.into(),
                    picture == Some(source),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.picker() {
                        picker.pick_picture(source);
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }
        if sources.windows.is_empty() && !sources.portal.contains(&ShareKind::Window) {
            rows.push(
                div()
                    .flex_none()
                    .px(u(10.0))
                    .py(u(4.0))
                    .child(note(text::NO_WINDOWS.into()))
                    .into_any_element(),
            );
        }
        for (index, shared) in sources.windows.iter().enumerate() {
            let source = PictureSource::Window(shared.id);
            let title: SharedString = if shared.title.is_empty() {
                shared.application_name.clone().into()
            } else {
                shared.title.clone().into()
            };
            let detail = format!(
                "{} · {}",
                shared.application_name,
                text::pixels((shared.width, shared.height))
            );
            rows.push(
                row(
                    ("share-window", index),
                    format!("share-window:{}", shared.id),
                    Icon::Detach,
                    title,
                    detail.into(),
                    picture == Some(source),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.picker() {
                        picker.pick_picture(source);
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }

        // The sound: the kind, and with a screen the application.
        let window_application = match picture {
            Some(PictureSource::Window(id)) => sources
                .windows
                .iter()
                .find(|window| window.id == id)
                .map(|window| window.application_name.clone()),
            _ => None,
        };
        let kinds: Vec<SharedString> = sound_kinds
            .iter()
            .map(|kind| match kind {
                SoundKind::None => text::SOUND_NONE.into(),
                SoundKind::System => text::SOUND_SYSTEM.into(),
                SoundKind::Application if window_application.is_some() => {
                    text::SOUND_THIS_APPLICATION.into()
                }
                SoundKind::Application => text::SOUND_ONE_APPLICATION.into(),
            })
            .collect();
        let sound_help: SharedString = match (sound, &window_application) {
            (None, _) => text::SOUND_CHOICE_HELP.into(),
            (Some(SoundKind::None), _) => text::SOUND_NONE_HELP.into(),
            (Some(SoundKind::System), _) => text::SOUND_SYSTEM_HELP.into(),
            (Some(SoundKind::Application), Some(name)) => text::sound_of_window(name).into(),
            (Some(SoundKind::Application), None) => text::SOUND_APPLICATION_HELP.into(),
        };
        let applications: Vec<AnyElement> =
            if sound == Some(SoundKind::Application) && window_application.is_none() {
                let mut listed: Vec<AnyElement> = sources
                    .applications
                    .iter()
                    .enumerate()
                    .map(|(index, shared)| {
                        let id = shared.id.clone();
                        row(
                            ("share-application", index),
                            if shared.id.process_id == 0 {
                                format!("share-application-index:{index}")
                            } else {
                                format!("share-application:{}", shared.id.process_id)
                            },
                            Icon::Speaker,
                            shared.name.clone().into(),
                            if shared.id.process_id == 0 {
                                "".into()
                            } else {
                                shared.id.bundle_id.clone().into()
                            },
                            application.as_ref() == Some(&shared.id),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(picker) = this.picker() {
                                picker.application = Some(id.clone());
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                    })
                    .collect();
                if listed.is_empty() {
                    listed.push(note(text::NO_APPLICATIONS.into()).into_any_element());
                }
                listed
            } else {
                Vec::new()
            };
        let list_frame = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(u(theme.metrics.radius))
                .border_1()
                .border_color(Hsla::from(colors.rule))
                .bg(Hsla::from(colors.travertine_hi))
                .children(children)
        };
        dialog
            .child(label(text::WHAT_TO_SHARE))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .h(u(220.0))
                    .overflow_hidden()
                    .rounded(u(theme.metrics.radius))
                    .border_1()
                    .border_color(Hsla::from(colors.rule))
                    .bg(Hsla::from(colors.travertine_hi))
                    .child(
                        ScrollArea::new("share-sources", &scroll, text::WHAT_TO_SHARE)
                            .selector("share-sources")
                            .children(rows)
                            .child(div().flex_none().h(u(4.0))),
                    ),
            )
            .child(div().mt(u(14.0)).child(label(text::SHARE_SOUND)))
            .child(
                div().flex().child(
                    Segmented::new("share-sound", text::SHARE_SOUND, kinds)
                        .selected(
                            sound.and_then(|sound| {
                                sound_kinds.iter().position(|kind| *kind == sound)
                            }),
                        )
                        .selector("share-sound")
                        .on_select({
                            let select = cx.listener(|this, index: &usize, _, cx| {
                                if let Some(picker) = this.picker()
                                    && let Some(kind) = picker.sound_kinds().get(*index).copied()
                                {
                                    picker.sound = Some(kind);
                                    cx.notify();
                                }
                            });
                            move |index, window, cx| select(&index, window, cx)
                        }),
                ),
            )
            .child(
                div()
                    .id("share-sound-help")
                    .debug_selector(|| "share-sound-help".to_owned())
                    .mt(u(5.0))
                    .child(note(sound_help)),
            )
            .when(!applications.is_empty(), |dialog| {
                dialog.child(div().mt(u(8.0)).child(list_frame(applications)))
            })
            .child(dialog_footer([
                cancel(cx),
                Button::new("start-share")
                    .kind(ButtonKind::Primary)
                    .icon(Icon::Screen)
                    .label(text::SHARE)
                    .disabled(!ready)
                    .selector("start-share")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start_chosen_share(window, cx);
                    }))
                    .into_any_element(),
            ]))
            .into_any_element()
    }

    /// The stage of a call with a share: the bar of the shares when there are several, the
    /// line about the watched one, its picture, and the strip of the people.
    pub(crate) fn render_share_stage(
        &mut self,
        channel: &ChannelRow,
        panel: &CallPanel,
        stage: &ShareStage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let watched = stage.watched();
        let several = stage.shares.len() > 1;
        let title_of = |share: &ShareCard| text::share_title(share, share.id == ShareId::Own);

        let bar = several.then(|| {
            let cards = stage.shares.iter().enumerate().map(|(index, share)| {
                let id = share.id;
                let title: SharedString = title_of(share).into();
                let small = self
                    .pictures
                    .small
                    .get(&share.id)
                    .map(|(_, image)| image.clone());
                let ring = focus_ring(colors.ochre, theme.metrics.focus_ring);
                div()
                    .id(("share-card", index))
                    .role(Role::RadioButton)
                    .aria_label(title.clone())
                    .aria_selected(share.watched)
                    .debug_selector(move || match id {
                        ShareId::Own => "share-card:own".to_owned(),
                        ShareId::Of(account_id) => format!("share-card:{account_id}"),
                    })
                    .tooltip(tooltip(format!("{} {title}", text::WATCH)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .h(u(62.0))
                    .rounded(u(theme.metrics.radius))
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |style| style.shadow(ring))
                    .map(|card| {
                        if share.watched {
                            card.pl(u(5.0))
                                .pr(u(9.0))
                                .border_2()
                                .border_color(Hsla::from(colors.ochre))
                                .bg(Hsla::from(colors.basalt_rule))
                        } else {
                            card.pl(u(6.0))
                                .pr(u(10.0))
                                .border_1()
                                .border_color(Hsla::from(colors.basalt_rule))
                                .bg(Hsla::from(colors.basalt_3))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, _| state.watch_share(id));
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(u(88.0))
                            .h(u(55.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(Hsla::from(colors.basalt_deep))
                            .bg(Hsla::from(colors.stage))
                            .when_some(small, |frame, image| {
                                frame.child(img(image).size_full().object_fit(ObjectFit::Contain))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(u(6.0))
                                    .typeface(theme.types.name, theme)
                                    .text_size(u(14.0))
                                    .line_height(u(19.0))
                                    .ink(colors.bone)
                                    .child(
                                        Mosaic::new(
                                            MosaicKind::Large,
                                            share.account_id.as_uuid().as_u128(),
                                            16.0,
                                        )
                                        .radius(2.0),
                                    )
                                    .child(title),
                            )
                            .child(
                                div()
                                    .typeface(theme.types.small, theme)
                                    .line_height(u(17.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .ink(colors.bone_2)
                                    .child(text::share_kind(share.kind)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(u(5.0))
                            .ml(u(8.0))
                            .h(u(24.0))
                            .px(u(8.0))
                            .rounded(u(theme.metrics.radius))
                            .border_1()
                            .typeface(theme.types.control_strong, theme)
                            .text_size(u(13.0))
                            .map(|tag| {
                                if share.watched {
                                    tag.border_color(Hsla::from(colors.ochre))
                                        .bg(Hsla::from(colors.ochre))
                                        .ink(colors.stage)
                                        .child(icon(Icon::Check, 13.0, colors.stage))
                                        .child(text::WATCHING)
                                } else {
                                    tag.border_color(Hsla::from(colors.basalt_tag_rule))
                                        .ink(colors.bone)
                                        .child(text::WATCH)
                                }
                            }),
                    )
            });
            div()
                .id("shares-bar")
                .role(Role::RadioGroup)
                .aria_label(text::shares_count(stage.shares.len()))
                .debug_selector(|| "shares-bar".to_owned())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(8.0))
                .h(u(78.0))
                .px(u(12.0))
                .overflow_x_scroll()
                .bg(Hsla::from(colors.basalt_2))
                .border_b_1()
                .border_color(Hsla::from(colors.basalt_rule))
                .child(
                    div()
                        .flex_none()
                        .mr(u(4.0))
                        .typeface(theme.types.overline, theme)
                        .ink(colors.bone_label)
                        .child(text::shares_count(stage.shares.len())),
                )
                .children(cards)
        });

        let header = watched.map(|share| {
            let own = share.id == ShareId::Own;
            let name: SharedString = share
                .name
                .clone()
                .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
                .into();
            // The sentence about the share, with its strong part.
            let (line, strong) = if several {
                let title = title_of(share);
                let line = format!("You watch {title}");
                let range = line.len() - title.len()..line.len();
                (line, range)
            } else if own {
                ("You are sharing".to_owned(), 0..3)
            } else {
                (format!("{name} is sharing"), 0..name.len())
            };
            let muted = !own && stage.volume == 0 && share.sound != Some(ShareAudio::None);
            let sound: Option<(SharedString, bool)> = if share.sound_held {
                Some((text::SHARE_SOUND_HELD.into(), false))
            } else if muted {
                Some((text::SHARE_SOUND_OFF_FOR_YOU.into(), false))
            } else if several && !own {
                text::watched_sound(share.sound).map(|heard| {
                    (
                        format!("{heard} {}", text::SOUND_FOLLOWS).into(),
                        share.sound != Some(ShareAudio::None),
                    )
                })
            } else {
                text::share_sound(share.sound)
                    .map(|sound| (sound.into(), share.sound != Some(ShareAudio::None)))
            };
            let measure = text::share_measure(
                if several { None } else { share.kind },
                share.size,
                share.frame_rate,
            );
            div()
                .id("share-header")
                .role(Role::Status)
                .aria_label(line.clone())
                .debug_selector(|| "share-header".to_owned())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(8.0))
                .h(u(34.0))
                .px(u(12.0))
                .overflow_hidden()
                .bg(Hsla::from(colors.basalt_2))
                .border_b_1()
                .border_color(Hsla::from(colors.basalt_rule))
                .typeface(theme.types.control, theme)
                .text_size(u(13.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .ink(colors.bone)
                .whitespace_nowrap()
                .child(
                    Mosaic::new(
                        MosaicKind::Large,
                        share.account_id.as_uuid().as_u128(),
                        18.0,
                    )
                    .radius(2.0),
                )
                .child(
                    div()
                        .flex_none()
                        .child(StyledText::new(line).with_highlights([(
                            strong,
                            gpui::HighlightStyle {
                                font_weight: Some(gpui::FontWeight::BOLD),
                                ..gpui::HighlightStyle::default()
                            },
                        )])),
                )
                .when_some(sound, |header, (sentence, heard)| {
                    let ink = if heard {
                        colors.olive_bright
                    } else {
                        colors.bone_2
                    };
                    header.child(
                        div()
                            .id("share-sound")
                            .debug_selector(|| "share-sound".to_owned())
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(u(5.0))
                            .ink(ink)
                            .child(icon(Icon::Speaker, 14.0, ink))
                            .child(div().min_w_0().truncate().child(sentence)),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .typeface(theme.types.small, theme)
                        .ink(colors.bone_2)
                        .child(measure),
                )
        });

        let shown = watched.and_then(|share| {
            self.pictures
                .main
                .as_ref()
                .filter(|(of, _)| *of == share.id)
                .map(|(_, image)| image.clone())
        });
        let calls = self.state.read(cx).calls().clone();
        let viewport = div()
            .id("share-viewport")
            .debug_selector(|| "share-viewport".to_owned())
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .px(u(12.0))
            .py(u(12.0))
            .child(
                // The size at which the picture is drawn, in pixels of the display: the
                // media server is asked for the layer of that size (ADR 0737).
                canvas(
                    move |bounds, window, _| {
                        let scale = window.scale_factor();
                        let width = (f32::from(bounds.size.width) * scale).round() as u32;
                        let height = (f32::from(bounds.size.height) * scale).round() as u32;
                        if width > 0 && height > 0 {
                            calls.set_view_size(width, height);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset(u(12.0)),
            )
            .map(|viewport| match shown {
                Some(image) => viewport.child(
                    div()
                        .id("share-picture")
                        .role(Role::Image)
                        .aria_label(watched.map(title_of).unwrap_or_default())
                        .debug_selector(|| "share-picture".to_owned())
                        .size_full()
                        .child(img(image).size_full().object_fit(ObjectFit::Contain)),
                ),
                None => viewport.child(
                    div()
                        .id("share-waiting")
                        .role(Role::Status)
                        .aria_label(text::PICTURE_ARRIVES)
                        .debug_selector(|| "share-waiting".to_owned())
                        .typeface(theme.types.notice, theme)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .ink(colors.bone_2)
                        .child(text::PICTURE_ARRIVES),
                ),
            });

        // The strip of the people. This version has no camera, so every person is without
        // video: with the setting on, the strip tells how many it does not show.
        let people = occupants(channel);
        let hidden = panel.hide_without_video;
        let tiles: Vec<AnyElement> = if hidden {
            Vec::new()
        } else {
            people
                .iter()
                .enumerate()
                .map(|(index, occupant)| {
                    let (marks, label) = tile_marks(occupant);
                    let name: SharedString = if occupant.is_self {
                        format!("{} ({})", occupant_name(occupant), text::YOU).into()
                    } else {
                        occupant_name(occupant)
                    };
                    PersonTile::new(
                        ("strip-tile", index),
                        name,
                        occupant.account_id.as_uuid().as_u128(),
                        label,
                    )
                    .in_strip()
                    .speaking(matches!(
                        occupant.microphone,
                        Microphone::Open { speaking: true }
                    ))
                    .marks(marks)
                    .selector(format!("call-tile:{}", occupant.account_id))
                    .into_any_element()
                })
                .collect()
        };
        let hidden_note = (hidden && !people.is_empty()).then(|| {
            let (strong, rest) = text::people_hidden(people.len());
            let line = format!("{strong}{rest}");
            let avatars = people.iter().take(3).map(|occupant| {
                Mosaic::new(
                    MosaicKind::Large,
                    occupant.account_id.as_uuid().as_u128(),
                    20.0,
                )
                .radius(2.0)
            });
            div()
                .id("people-hidden")
                .role(Role::Status)
                .aria_label(line.clone())
                .debug_selector(|| "people-hidden".to_owned())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(9.0))
                .ml_auto()
                .mb(u(22.0))
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .line_height(u(18.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .ink(colors.bone_2)
                .child(div().flex().gap(u(3.0)).children(avatars))
                .child(
                    div()
                        .child(StyledText::new(line).with_highlights([(
                            0..strong.len(),
                            gpui::HighlightStyle {
                                font_weight: Some(gpui::FontWeight::BOLD),
                                color: Some(Hsla::from(colors.bone)),
                                ..gpui::HighlightStyle::default()
                            },
                        )]))
                        .child(text::LISTED_IN_MEMBERS),
                )
        });
        let strip = div()
            .id("call-people")
            .role(Role::List)
            .aria_label(text::IN_THE_CALL)
            .debug_selector(|| "call-people".to_owned())
            .flex()
            .flex_none()
            .items_start()
            .gap(u(8.0))
            .h(u(106.0))
            .pt(u(8.0))
            .px(u(12.0))
            .overflow_x_scroll()
            .border_t_1()
            .border_color(Hsla::from(colors.basalt_rule))
            .bg(Hsla::from(colors.basalt_deep))
            .when(hidden, |strip| strip.items_center())
            .children(tiles)
            .children(hidden_note);

        div()
            .id("call-stage")
            .debug_selector(|| "call-stage".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(Hsla::from(colors.stage))
            .children(bar)
            .children(header)
            .child(viewport)
            .child(strip)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use junto_media::share::{ApplicationSource, DisplaySource, WindowSource};

    use super::*;

    fn player() -> ApplicationId {
        ApplicationId {
            process_id: 7,
            bundle_id: "example.player".to_owned(),
        }
    }

    fn picker() -> SharePicker {
        let sources = ShareSources {
            portal: Vec::new(),
            system_sound: true,
            displays: vec![DisplaySource {
                id: 1,
                width: 1920,
                height: 1200,
            }],
            windows: vec![WindowSource {
                id: 11,
                title: "Editor".to_owned(),
                application: player(),
                application_name: "Player".to_owned(),
                width: 800,
                height: 600,
            }],
            applications: vec![ApplicationSource {
                id: player(),
                name: "Player".to_owned(),
            }],
        };
        SharePicker::new(ScreenAccess::Allowed, Listed::Sources(sources), 1)
    }

    #[test]
    fn a_share_needs_a_picture_and_a_sound_that_the_person_chose() {
        let mut picker = picker();
        // Nothing is chosen at the start, and nothing is chosen for the person.
        assert_eq!(picker.chosen(), None);
        picker.picture = Some(PictureSource::Display(1));
        assert_eq!(picker.chosen(), None);
        picker.sound = Some(SoundKind::None);
        assert_eq!(
            picker.chosen(),
            Some((PictureSource::Display(1), SoundChoice::None))
        );
        picker.sound = Some(SoundKind::System);
        assert_eq!(
            picker.chosen(),
            Some((PictureSource::Display(1), SoundChoice::System))
        );
        // The sound of one application with a screen needs the application.
        picker.sound = Some(SoundKind::Application);
        assert_eq!(picker.chosen(), None);
        picker.application = Some(player());
        assert_eq!(
            picker.chosen(),
            Some((
                PictureSource::Display(1),
                SoundChoice::Application(player())
            ))
        );
        // An application that the system does not list is not shared.
        picker.application = Some(ApplicationId {
            process_id: 8,
            bundle_id: "example.other".to_owned(),
        });
        assert_eq!(picker.chosen(), None);
    }

    #[test]
    fn the_sound_of_one_application_with_a_window_is_the_application_of_that_window() {
        let mut picker = picker();
        picker.picture = Some(PictureSource::Window(11));
        picker.sound = Some(SoundKind::Application);
        assert_eq!(
            picker.chosen(),
            Some((
                PictureSource::Window(11),
                SoundChoice::Application(player())
            ))
        );
        // A window that the list does not hold is not shared.
        picker.picture = Some(PictureSource::Window(12));
        assert_eq!(picker.chosen(), None);
        // Without a list nothing is shared.
        picker.picture = Some(PictureSource::Window(11));
        picker.listed = Listed::Reading;
        assert_eq!(picker.chosen(), None);
    }

    #[test]
    fn a_new_picture_ends_the_choice_of_one_application_and_keeps_the_other_choices() {
        let other = ApplicationId {
            process_id: 9,
            bundle_id: "example.other".to_owned(),
        };
        let mut picker = picker();
        if let Listed::Sources(sources) = &mut picker.listed {
            sources.applications.push(ApplicationSource {
                id: other.clone(),
                name: "Other".to_owned(),
            });
        }
        // The person chose the sound of "Other" with the screen, and then picks a window
        // of "Player". The sound of "Player" was not chosen: the sound is asked again.
        picker.pick_picture(PictureSource::Display(1));
        picker.sound = Some(SoundKind::Application);
        picker.application = Some(other);
        picker.pick_picture(PictureSource::Window(11));
        assert_eq!((picker.sound, picker.application.clone()), (None, None));
        assert_eq!(picker.chosen(), None);
        // The same holds from a window to a screen.
        picker.sound = Some(SoundKind::Application);
        assert!(picker.chosen().is_some());
        picker.pick_picture(PictureSource::Display(1));
        assert_eq!(picker.sound, None);
        // The choices that name no application hold for every picture, and so does the
        // same picture picked again.
        for kind in [SoundKind::None, SoundKind::System] {
            picker.sound = Some(kind);
            picker.pick_picture(PictureSource::Window(11));
            assert_eq!(picker.sound, Some(kind));
        }
        picker.sound = Some(SoundKind::Application);
        picker.pick_picture(PictureSource::Window(11));
        assert_eq!(picker.sound, Some(SoundKind::Application));
    }
    #[test]
    fn a_portal_window_requires_an_explicit_application_and_unavailable_sound_is_not_offered() {
        let first = ApplicationId {
            process_id: 0,
            bundle_id: "portal:player.one".into(),
        };
        let second = ApplicationId {
            process_id: 0,
            bundle_id: "portal:player.two".into(),
        };
        let sources = ShareSources {
            portal: vec![ShareKind::Window],
            applications: vec![
                ApplicationSource {
                    id: first,
                    name: "First player".into(),
                },
                ApplicationSource {
                    id: second.clone(),
                    name: "Second player".into(),
                },
            ],
            ..ShareSources::default()
        };
        let mut picker = SharePicker::new(ScreenAccess::AskOnStart, Listed::Sources(sources), 1);
        picker.pick_picture(PictureSource::Portal(ShareKind::Window));
        assert!(picker.chosen().is_none());
        assert_eq!(
            picker.sound_kinds(),
            [SoundKind::None, SoundKind::Application]
        );
        picker.sound = Some(SoundKind::System);
        assert!(picker.chosen().is_none());
        picker.sound = Some(SoundKind::Application);
        assert!(picker.chosen().is_none());
        picker.application = Some(second.clone());
        assert_eq!(
            picker.chosen(),
            Some((
                PictureSource::Portal(ShareKind::Window),
                SoundChoice::Application(second)
            ))
        );
        picker.pick_picture(PictureSource::Portal(ShareKind::Screen));
        assert!(picker.chosen().is_none());
    }
}

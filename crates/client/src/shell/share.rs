//! Screen shares in the window (ADR 0738): the dialog in which a person chooses what to
//! share and its sound, and the stage of a call with a share: the bar of shares, the line
//! about the watched share, its picture, and the strip of the people.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    RenderImage, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    StyledText, Window, canvas, div, img, prelude::FluentBuilder,
};
use junto_client_core::protocol::call::ShareAudio;
use junto_media::calls::{ActiveCall, CallsView, Seat};
use junto_media::engine::ShareId;
use junto_media::share::picture::PictureFrame;
use junto_media::share::{
    ApplicationId, CaptureError, PictureSource, ScreenAccess, ShareKind, ShareSources, SoundChoice,
};

use super::call::tile_marks;
use super::channels::occupant_name;
use super::{Layer, Shell, prepared};
use crate::assets::Icon;
use crate::state::view::{CallPanel, Microphone, ShareCard, ShareStage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, DrawnPicture, Notice, PersonTile, ScrollArea, Segmented, StyleExt,
    dialog_footer, focus_ring, focus_ring_inside, icon, tooltip,
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
    /// The call and the seat that the dialog was opened for. What the person chooses in it
    /// is shared there and nowhere else (ADR 0788).
    seat: Seat,
}

impl SharePicker {
    fn new(access: ScreenAccess, listed: Listed, read: u64, seat: Seat) -> Self {
        Self {
            access,
            listed,
            picture: None,
            sound: None,
            application: None,
            scroll: ScrollHandle::new(),
            read,
            seat,
        }
    }

    /// True while the device is in the call, at the seat, that the dialog was opened for.
    pub(crate) fn belongs_to(&self, calls: &CallsView) -> bool {
        calls.call.as_ref().map(ActiveCall::seat).as_ref() == Some(&self.seat)
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

    fn sound_help(&self, sources: &ShareSources) -> SharedString {
        let shared_window = match self.picture {
            Some(PictureSource::Window(id)) => {
                sources.windows.iter().find(|window| window.id == id)
            }
            _ => None,
        };
        let window_application = shared_window.map(|window| window.application_name.clone());
        let started_the_client = shared_window
            .is_some_and(|window| sources.started_the_client.contains(&window.application));
        match (self.sound, &window_application) {
            (None, _) => text::SOUND_CHOICE_HELP.into(),
            (Some(SoundKind::None), _) => text::SOUND_NONE_HELP.into(),
            (Some(SoundKind::System), _) => text::SOUND_SYSTEM_HELP.into(),
            (Some(SoundKind::Application), _) if sources.creators_unknown => {
                text::SOUND_CREATORS_UNKNOWN.into()
            }
            (Some(SoundKind::Application), Some(name)) if started_the_client => {
                text::sound_includes_junto(name).into()
            }
            (Some(SoundKind::Application), Some(name)) => text::sound_of_window(name).into(),
            (Some(SoundKind::Application), None) => text::SOUND_APPLICATION_HELP.into(),
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
                // macOS and Windows list the applications with a window, and offer the
                // choice also with an empty list, which states why it is empty. Linux
                // lists the applications that play now.
                SoundKind::Application => {
                    cfg!(any(target_os = "macos", target_os = "windows"))
                        || !sources.applications.is_empty()
                        || !sources.windows.is_empty()
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
            (SoundKind::Application, PictureSource::Window(id)) => {
                let application = &sources
                    .windows
                    .iter()
                    .find(|window| window.id == id)?
                    .application;
                // Its sound includes the sound of the call, or can include it (ADR 0706); the
                // dialog states why.
                if sources.creators_unknown || sources.started_the_client.contains(application) {
                    return None;
                }
                SoundChoice::Application(application.clone())
            }
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
    pub main: Option<(ShareId, DrawnPicture)>,
    pub small: HashMap<ShareId, (Arc<PictureFrame>, Arc<RenderImage>)>,
}

/// Waits until one of the three sources of pictures has a newer one, or the window asks
/// for all pictures again, and returns which sources have a newer picture and whether the
/// window asked. Absent when a source has ended: the calls are gone.
async fn newer_pictures<E>(
    main: impl Future<Output = Result<(), E>>,
    small: impl Future<Output = Result<(), E>>,
    cameras: impl Future<Output = Result<(), E>>,
    again: impl Future<Output = ()>,
) -> Option<([bool; 3], bool)> {
    use std::task::Poll;
    let (mut main, mut small, mut cameras, mut again) = (
        std::pin::pin!(main),
        std::pin::pin!(small),
        std::pin::pin!(cameras),
        std::pin::pin!(again),
    );
    std::future::poll_fn(|cx| {
        let polled = [
            main.as_mut().poll(cx),
            small.as_mut().poll(cx),
            cameras.as_mut().poll(cx),
        ];
        if polled
            .iter()
            .any(|source| matches!(source, Poll::Ready(Err(_))))
        {
            return Poll::Ready(None);
        }
        let asked = again.as_mut().poll(cx).is_ready();
        let newer = polled.map(|source| source.is_ready());
        if asked || newer.contains(&true) {
            Poll::Ready(Some((newer, asked)))
        } else {
            Poll::Pending
        }
    })
    .await
}

/// A picture of the media engine as an image of the window. The image takes the pixels of
/// the picture without a copy. The picture is made on a blocking thread of the runtime, not
/// on the thread of the window (ADR 0730).
pub(crate) fn render_image(frame: PictureFrame) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl Shell {
    /// Follows the pictures of the call of this device: the watched share, the small
    /// pictures of the shares and the cameras. A picture that arrives is converted, with
    /// the newest picture of each other source that has a new one, on a blocking thread of
    /// the runtime of the client; then the follower asks for the next frame of the display,
    /// and that frame puts the converted pictures in place of the ones that the window
    /// holds. One conversion and one drawing run at a time, so the pictures of all sources
    /// together cause at most one drawing per frame, and a picture that a newer one
    /// replaced before its conversion started is not converted (ADR 0793). An image that
    /// is replaced is released from the GPU of every window (ADR 0774): a window keeps
    /// each image that it drew until it is told otherwise.
    pub(crate) fn follow_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let calls = state.calls().clone();
        let runtime = state.client().runtime().handle().clone();
        let (mut main, mut small, mut cameras) = (
            calls.main_picture(),
            calls.small_pictures(),
            calls.camera_pictures(),
        );
        let again = Arc::new(tokio::sync::Notify::new());
        self.pictures_again = again.clone();
        let software_video = self.software_video;
        let follow = cx.spawn_in(window, async move |this, cx| {
            // Without a graphics processor the cameras are taken at most once in 200 ms,
            // whatever the shares do: after a take the cameras rest for that time, and
            // the pictures of the shares are taken as they arrive.
            let mut rest = None;
            loop {
                let cameras_newer = async {
                    match rest.as_mut() {
                        Some(rest) => {
                            rest.await;
                            Ok(())
                        }
                        None => cameras.changed().await,
                    }
                };
                let Some((mut sources, asked)) = newer_pictures(
                    main.changed(),
                    small.changed(),
                    cameras_newer,
                    again.notified(),
                )
                .await
                else {
                    break;
                };
                if sources[2] {
                    if rest.take().is_some() {
                        // The rest has ended. A picture of a camera that arrived in it is
                        // found in the next turn.
                        sources[2] = false;
                    } else if software_video {
                        rest = Some(
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(200)),
                        );
                    }
                }
                // The window draws the call again: it holds no picture, and takes all.
                if asked {
                    sources = [true; 3];
                }
                if sources == [false; 3] {
                    continue;
                }
                let wanted = prepared::Wanted {
                    main: sources[0].then(|| main.borrow_and_update().clone()),
                    small: sources[1].then(|| small.borrow_and_update().clone()),
                    cameras: sources[2].then(|| cameras.borrow_and_update().clone()),
                };
                // A window that does not draw the call converts nothing and asks for no
                // frame. When it starts to draw the call it asks for all pictures again.
                let Ok(held) = this.update(cx, |this, cx| {
                    let held = this.draws_call(cx).then(|| this.held_pictures());
                    this.pictures_converting = held.is_some();
                    held
                }) else {
                    break;
                };
                let Some(held) = held else {
                    continue;
                };
                let Ok(converted) = runtime.spawn_blocking(move || wanted.prepare(&held)).await
                else {
                    let _ = this.update(cx, |this, _| this.pictures_converting = false);
                    break;
                };
                let (taken, frame) = futures_channel::oneshot::channel::<()>();
                let asked = this.update_in(cx, |this, window, cx| {
                    this.pictures_converting = false;
                    this.pictures_prepared = Some(converted);
                    let shell = cx.entity().downgrade();
                    window.on_next_frame(move |_, cx| {
                        let _ = shell.update(cx, |this, cx| this.take_pictures(cx));
                        let _ = taken.send(());
                    });
                });
                // Until the frame has taken the pictures, further ones ask for nothing.
                if asked.is_err() || frame.await.is_err() {
                    break;
                }
            }
        });
        self.picture_tasks = vec![follow];
    }

    /// The pictures that the window holds, so that a conversion leaves them as they are.
    fn held_pictures(&self) -> prepared::Held {
        prepared::Held {
            main: self.main_taken.clone(),
            small: self
                .pictures
                .small
                .iter()
                .map(|(share, (frame, _))| (*share, frame.clone()))
                .collect(),
            cameras: self
                .cameras
                .iter()
                .map(|(id, (frame, _))| (*id, frame.clone()))
                .collect(),
        }
    }

    /// Puts the pictures that the follower converted in place of the ones that the window
    /// holds, in the frame of the display that was asked for.
    fn take_pictures(&mut self, cx: &mut Context<Self>) {
        let Some(converted) = self.pictures_prepared.take() else {
            return;
        };
        if !self.draws_call(cx) {
            return;
        }
        let mut drawn = false;
        if let Some(main) = converted.main {
            // A frame that was asked for a picture that an earlier frame took draws nothing.
            let same = match (&main.frame, &self.main_taken) {
                (Some(now), Some(taken)) => Arc::ptr_eq(now, taken),
                (None, None) => true,
                _ => false,
            };
            if !same {
                self.main_taken = main.frame;
                let shown =
                    (main.shown).and_then(|(share, picture)| Some((share, picture.picture()?)));
                self.show_main(shown, cx);
                drawn = true;
            }
        }
        if let Some(small) = converted.small {
            drawn |= self.show_small(small, cx);
        }
        if let Some(cameras) = converted.cameras {
            drawn |= self.show_cameras(cameras, cx);
        }
        #[cfg(feature = "test-support")]
        {
            self.picture_draws += usize::from(drawn);
        }
        #[cfg(not(feature = "test-support"))]
        let _ = drawn;
    }

    /// Follows whether this window draws the call. A window that stops drawing it releases
    /// its pictures; a window that starts asks the follower for all pictures, which it
    /// converts off the thread of the window.
    pub(crate) fn refresh_picture_owner(&mut self, cx: &mut Context<Self>) {
        let drawing = self.draws_call(cx);
        if self.drawing_call == drawing {
            return;
        }
        self.drawing_call = drawing;
        if drawing {
            self.pictures_again.notify_one();
        } else {
            self.pictures_prepared = None;
            self.main_taken = None;
            self.show_main(None, cx);
            self.show_small(HashMap::new(), cx);
            self.show_cameras(HashMap::new(), cx);
        }
    }

    pub(crate) fn show_main(
        &mut self,
        shown: Option<(ShareId, DrawnPicture)>,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, before)) = std::mem::replace(&mut self.pictures.main, shown)
            && let Some(image) = before.image()
        {
            crate::images::release(image.clone(), cx);
        }
        cx.notify();
    }

    /// Holds the small pictures of these shares: a picture that the window holds is kept,
    /// and a converted one is taken. Returns whether the window holds other pictures than
    /// before, and is drawn for them.
    pub(crate) fn show_small(
        &mut self,
        pictures: prepared::ConvertedSmall,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut released = Vec::new();
        self.pictures.small.retain(|share, (frame, image)| {
            let kept = pictures
                .get(share)
                .is_some_and(|(now, _)| Arc::ptr_eq(now, frame));
            if !kept {
                released.push(image.clone());
            }
            kept
        });
        let mut changed = !released.is_empty();
        for (share, (frame, image)) in pictures {
            if !self.pictures.small.contains_key(&share)
                && let Some(image) = image
            {
                self.pictures.small.insert(share, (frame, image));
                changed = true;
            }
        }
        for image in released {
            crate::images::release(image, cx);
        }
        if changed {
            cx.notify();
        }
        changed
    }

    /// The share button of the strip of the call: it opens the dialog, or ends the share
    /// that runs.
    pub(crate) fn toggle_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The button does what it shows: the view that the window draws decides.
        let sharing = self
            .drawn_view(cx)
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
        // The dialog belongs to the call that the window shows when the person opens it.
        let Some(seat) = self.drawn_view(cx).controls.seat else {
            return;
        };
        let now = self.state.read(cx).media_calls().call;
        if now.as_ref().map(ActiveCall::seat).as_ref() != Some(&seat) {
            // The device is in another call than the one that is drawn, or in none.
            if now.is_some() {
                let told = text::unused_action(junto_media::calls::Unused::Share);
                self.state.update(cx, |state, cx| state.tell_call(told, cx));
            }
            return;
        }
        let calls = self.state.read(cx).calls().clone();
        self.share_reads += 1;
        let read = self.share_reads;
        if !calls.can_share() {
            let listed = Listed::Failed(CaptureError::Unsupported);
            let picker = SharePicker::new(ScreenAccess::NotAllowed, listed, read, seat);
            self.open_layer(Layer::SharePicker(Box::new(picker)), window, cx);
            return;
        }
        let access = calls.screen_access();
        let picker = SharePicker::new(access, Listed::Reading, read, seat);
        self.open_layer(Layer::SharePicker(Box::new(picker)), window, cx);
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
        self.share_picker_sources().is_some()
    }

    /// What the system lists in the dialog of a share, when the dialog has read it.
    pub fn share_picker_sources(&self) -> Option<&ShareSources> {
        match &self.layer {
            Layer::SharePicker(picker) => picker.sources(),
            _ => None,
        }
    }

    /// The sound description used by the open share dialog for its current selection.
    pub fn share_picker_sound_help(&self) -> Option<SharedString> {
        match &self.layer {
            Layer::SharePicker(picker) => Some(picker.sound_help(picker.sources()?)),
            _ => None,
        }
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
        let main = main
            .and_then(|(share, frame)| Some((share, DrawnPicture::Image(render_image(frame)?))));
        self.show_main(main, cx);
        let small = small
            .into_iter()
            .map(|(share, frame)| {
                let image = render_image(frame.clone());
                (share, (Arc::new(frame), image))
            })
            .collect();
        self.show_small(small, cx);
    }

    /// True while the follower converts pictures, or converted pictures wait for the next
    /// frame of the display. A test waits for both to end: the conversion runs on a thread
    /// of the runtime (ADR 0793).
    #[cfg(feature = "test-support")]
    pub fn pictures_pending(&self) -> bool {
        self.pictures_converting || self.pictures_prepared.is_some()
    }

    /// The images of the pictures of the call that the window holds now: the watched
    /// share, the small pictures of the shares and the cameras that are drawn as images.
    #[cfg(feature = "test-support")]
    pub fn picture_images(&self) -> Vec<Arc<RenderImage>> {
        let main = (self.pictures.main.iter()).filter_map(|(_, picture)| picture.image());
        let small = self.pictures.small.values().map(|(_, image)| image);
        let cameras = self
            .cameras
            .values()
            .filter_map(|(_, picture)| picture.image());
        main.chain(small).chain(cameras).cloned().collect()
    }

    fn picker(&mut self) -> Option<&mut SharePicker> {
        match &mut self.layer {
            Layer::SharePicker(picker) => Some(picker),
            _ => None,
        }
    }

    /// Starts the share that the dialog describes at the seat that the dialog was opened
    /// for, and closes the dialog, as a click on its button does.
    pub fn start_chosen_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker() else {
            return;
        };
        let Some((picture, sound)) = picker.chosen() else {
            return;
        };
        let seat = picker.seat.clone();
        self.state
            .update(cx, |state, _| state.start_share(&seat, picture, sound));
        self.close_layer(window, cx);
    }

    /// Closes the dialog of a share when the device is not in the call, at the seat, that
    /// the dialog was opened for, and tells the person that nothing is shared.
    pub(crate) fn drop_stale_share_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Layer::SharePicker(picker) = &self.layer else {
            return;
        };
        // The calls as the state of the application has taken them: the state puts away
        // what it told about a call when the call ends, and that is done by then.
        let calls = self.state.read(cx).shown_calls();
        if picker.belongs_to(&calls) {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.tell_call(text::SHARE_DIALOG_CLOSED, cx)
        });
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
        let shared_window = match picture {
            Some(PictureSource::Window(id)) => {
                sources.windows.iter().find(|window| window.id == id)
            }
            _ => None,
        };
        let window_application = shared_window.map(|window| window.application_name.clone());
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
        let sound_help = picker.sound_help(&sources);
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
                    let empty = if sources.creators_unknown {
                        text::SOUND_CREATORS_UNKNOWN
                    } else {
                        text::NO_APPLICATIONS
                    };
                    listed.push(note(empty.into()).into_any_element());
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
        panel: &CallPanel,
        stage: &ShareStage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let faces = self.shown_faces();
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
                                        super::people::avatar(&faces, share.account_id, 16.0)
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
                .child(super::people::avatar(&faces, share.account_id, 18.0).radius(2.0))
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
                .map(|(_, picture)| picture.clone())
        });
        let calls = self.state.read(cx).calls().clone();
        let viewport = div()
            .id("share-viewport")
            .debug_selector(|| "share-viewport".to_owned())
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
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
                Some(picture) => viewport.child(
                    div()
                        .id("share-picture")
                        .role(Role::Image)
                        .aria_label(watched.map(title_of).unwrap_or_default())
                        .debug_selector(|| "share-picture".to_owned())
                        .relative()
                        .size_full()
                        .min_h_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .inset(u(0.0))
                                .size_full()
                                .child(picture.element()),
                        ),
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

        // The setting affects only the tiles; the member list stays complete.
        let people = panel.people.as_slice();
        let hidden = panel.hide_without_video
            && people
                .iter()
                .any(|person| self.camera_image(person).is_some());
        let tiles: Vec<AnyElement> = {
            people
                .iter()
                .filter(|occupant| !hidden || self.camera_image(occupant).is_some())
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
                    .picture(faces.image(occupant.account_id))
                    .in_strip()
                    .sidebar(self.detached)
                    .camera_view(self.camera_request(occupant, cx))
                    .camera_picture(self.camera_image(occupant))
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
        let ids: Vec<_> = people.iter().map(|person| person.account_id).collect();
        let cameras: Vec<_> = people
            .iter()
            .filter(|person| self.camera_image(person).is_some())
            .map(|person| person.account_id)
            .collect();
        let hidden_ids = super::camera::hidden_people(&ids, &cameras, hidden);
        let hidden_count = hidden_ids.len();
        let hidden_note = (hidden_count > 0).then(|| {
            let (strong, rest) = text::people_hidden(hidden_count);
            let line = format!("{strong}{rest}");
            let avatars = people
                .iter()
                .filter(|person| hidden_ids.contains(&person.account_id))
                .take(3)
                .map(|occupant| {
                    super::people::avatar(&faces, occupant.account_id, 20.0).radius(2.0)
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
                .when(self.detached, |note| {
                    note.flex_col().items_start().w_full().ml(u(0.0)).mb(u(0.0))
                })
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
            .when(self.detached, |strip| {
                strip
                    .flex_col()
                    .w(u(112.0))
                    .h_full()
                    .px(u(8.0))
                    .items_start()
                    .overflow_y_scroll()
            })
            .children(tiles)
            .children(hidden_note);

        div()
            .id("call-stage")
            .relative()
            .overflow_hidden()
            .debug_selector(|| "call-stage".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(Hsla::from(colors.stage))
            .children(bar)
            .children(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .when(!self.detached, |area| area.flex_col())
                    .child(viewport)
                    .child(strip),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::call::CallPlace;
    use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
    use junto_media::calls::{CallTarget, Phase};
    use junto_media::engine::{MicrophoneState, OutputState, ShareState};
    use junto_media::permission::MicrophoneAccess;
    use junto_media::share::{ApplicationSource, DisplaySource, WindowSource};

    use super::*;

    /// The pixels that the conversion of a picture writes are the pixels of the image of the
    /// window: neither the conversion nor the image copies them.
    #[test]
    fn an_image_holds_the_pixels_that_the_conversion_wrote() {
        let pixels =
            junto_media::share::synthetic::two_colours(64, 36, (200, 30, 30), (30, 30, 200));
        let frame = junto_media::share::picture::convert(&pixels, (64, 36)).unwrap();
        let written = frame.bgra.as_ptr();
        let image = render_image(frame).unwrap();
        assert_eq!(image.as_bytes(0).unwrap().as_ptr(), written);
        let picture = junto_media::share::picture::VideoPicture::new(&pixels, false, (32, 18))
            .unwrap()
            .bgra()
            .unwrap();
        let written = picture.bgra.as_ptr();
        let image = render_image(picture).unwrap();
        assert_eq!(image.as_bytes(0).unwrap().as_ptr(), written);
        assert_eq!(image.size(0), gpui::size(32.into(), 18.into()));
    }

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
            started_the_client: Vec::new(),
            creators_unknown: false,
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
        SharePicker::new(ScreenAccess::Allowed, Listed::Sources(sources), 1, seat())
    }

    fn seat() -> Seat {
        Seat {
            target: CallTarget {
                server_id: ServerId::generate(),
                place: CallPlace::Channel {
                    workspace_id: WorkspaceId::generate(),
                    channel_id: ChannelId::generate(),
                },
            },
            identity: "seat".to_owned(),
        }
    }

    fn in_call(seat: &Seat) -> CallsView {
        CallsView {
            call: Some(ActiveCall {
                camera: junto_media::camera::CameraState::Off,
                cameras: Vec::new(),
                target: seat.target,
                identity: seat.identity.clone(),
                phase: Phase::Connected,
                interrupted_at: None,
                microphone: MicrophoneState::Running,
                access: MicrophoneAccess::Allowed,
                input_device: None,
                output: OutputState::Running,
                output_device: None,
                microphone_block: None,
                sound_block: None,
                share: ShareState::Off,
                shares: Vec::new(),
                watched: None,
            }),
            ..CallsView::default()
        }
    }

    #[test]
    fn a_share_dialog_belongs_to_the_call_and_the_seat_of_its_opening() {
        let picker = picker();
        let seat = picker.seat.clone();
        assert!(picker.belongs_to(&in_call(&seat)));
        // A token for the same seat keeps the dialog: the phase is another one.
        let mut reconnecting = in_call(&seat);
        reconnecting.call.as_mut().unwrap().phase = Phase::Reconnecting;
        assert!(picker.belongs_to(&reconnecting));
        // A moderator moved the person, or another window joined another call.
        let moved = Seat {
            target: self::seat().target,
            ..seat.clone()
        };
        assert!(!picker.belongs_to(&in_call(&moved)));
        // The same call at a new seat.
        let replaced = Seat {
            identity: "seat-2".to_owned(),
            ..seat
        };
        assert!(!picker.belongs_to(&in_call(&replaced)));
        assert!(!picker.belongs_to(&CallsView::default()));
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
    fn the_sound_of_a_window_whose_application_started_junto_is_not_shared() {
        let mut picker = picker();
        if let Listed::Sources(sources) = &mut picker.listed {
            sources.started_the_client.push(player());
        }
        picker.picture = Some(PictureSource::Window(11));
        picker.sound = Some(SoundKind::Application);
        assert_eq!(picker.chosen(), None);
        // The other choices of sound stay.
        picker.sound = Some(SoundKind::System);
        assert_eq!(
            picker.chosen(),
            Some((PictureSource::Window(11), SoundChoice::System))
        );
    }

    #[test]
    fn without_the_applications_that_started_junto_no_sound_of_one_application_is_shared() {
        let mut picker = picker();
        if let Listed::Sources(sources) = &mut picker.listed {
            sources.creators_unknown = true;
        }
        picker.picture = Some(PictureSource::Window(11));
        picker.sound = Some(SoundKind::Application);
        assert_eq!(picker.chosen(), None);
        picker.sound = Some(SoundKind::System);
        assert_eq!(
            picker.chosen(),
            Some((PictureSource::Window(11), SoundChoice::System))
        );
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
        let mut picker = SharePicker::new(
            ScreenAccess::AskOnStart,
            Listed::Sources(sources),
            1,
            seat(),
        );
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

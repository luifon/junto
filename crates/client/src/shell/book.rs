//! What ADR 0780 adds to the window: the unread sign of the head of a server, the items of
//! the server menu that change the order of the server book, the notice of a newer build,
//! and the section "Version" of the settings.

use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use gpui_base::input::InputState;
use junto_client_core::version::Incompatibility;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::admin::{AdminOpen, AdminPage};
use crate::state::book::{CheckShown, Step};
use crate::state::view::{ServerRow, ShellView};
use crate::text::book as txt;
use crate::theme::{Theme, u};
use crate::ui::{
    Badge, Button, Field, MenuItem, Notice, StyleExt, focus_ring, icon, tooltip, unread_dot,
};

/// What the window holds of the update notice.
pub(crate) struct BookUi {
    /// The address of the page of a release, as text that a person can select and that
    /// takes no change.
    page: Entity<InputState>,
    /// The address that the field holds.
    page_shown: Option<String>,
    /// The address that was given to the clipboard while the settings are open.
    copied: Option<String>,
    /// The position of the open section of the settings.
    settings_scroll: ScrollHandle,
    /// The position of the list of the sections of the settings.
    pub(crate) settings_nav_scroll: ScrollHandle,
    /// The keyboard focus of the update notice.
    notice: FocusHandle,
}

impl BookUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let page = Field::state("", window, cx);
        page.update(cx, |field, cx| field.set_readonly(true, cx));
        Self {
            page,
            page_shown: None,
            copied: None,
            settings_scroll: ScrollHandle::new(),
            settings_nav_scroll: ScrollHandle::new(),
            notice: cx.focus_handle().tab_stop(true),
        }
    }
}

/// The name of the head of a server for the accessibility tree: the name of the server,
/// what the head shows as unread, and the join requests that wait.
pub(crate) fn head_label(row: &ServerRow) -> SharedString {
    let mut label = row.name.clone();
    if let Some(sign) = super::channels::unread_sign(row.head_unread) {
        label = format!("{label}, {}", sign.label);
    }
    if row.waiting_join_requests > 0 {
        label = format!(
            "{label}, {}",
            crate::text::admin::waiting(AdminPage::JoinRequests, row.waiting_join_requests)
        );
    }
    label.into()
}

/// The unread sign of the head of a server in the expanded server book: the sign of the
/// channel list, for what no row of a workspace shows.
pub(crate) fn head_sign(row: &ServerRow, index: usize, cx: &App) -> Option<AnyElement> {
    let sign = super::channels::unread_sign(row.head_unread)?;
    let mark = match sign.mentions {
        Some(count) => Badge::new(count).into_any_element(),
        None => div()
            .flex_none()
            .mr(u(2.0))
            .child(unread_dot(cx))
            .into_any_element(),
    };
    Some(
        div()
            .flex_none()
            .debug_selector(move || format!("server-head-unread-{index}"))
            .child(mark)
            .into_any_element(),
    )
}

impl Shell {
    /// The count of the join requests that wait for the person on the head of a server in
    /// the expanded server book. A press opens the page of the join requests (ADR 0761).
    pub(crate) fn render_waiting_sign(
        &mut self,
        index: usize,
        row: &ServerRow,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let waiting = row.waiting_join_requests;
        if waiting == 0 {
            return None;
        }
        let server_id = row.id;
        let label: SharedString =
            crate::text::admin::waiting(AdminPage::JoinRequests, waiting).into();
        Some(
            div()
                .id(("server-waiting", index))
                .flex_none()
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(label.clone())
                .debug_selector(move || format!("server-waiting-{index}"))
                .tooltip(tooltip(label))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.state
                        .update(cx, |state, cx| state.select_server(server_id, cx));
                    let open = AdminOpen::Server {
                        server_id,
                        page: AdminPage::JoinRequests,
                    };
                    this.open_admin(open, window, cx);
                }))
                .child(
                    Badge::new(crate::text::unread_count(waiting as u64))
                        .icon(crate::text::admin::page_icon(AdminPage::JoinRequests)),
                )
                .into_any_element(),
        )
    }

    /// Opens the settings at the section "Version", which is the last one.
    pub fn open_version_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.book.copied = None;
        self.open_settings(window, cx);
        self.open_settings_section(super::settings::SettingsSection::About, window, cx);
    }

    /// The settings scroll position used by the window tests.
    #[cfg(feature = "test-support")]
    pub fn settings_scroll_handle(&self) -> ScrollHandle {
        self.book.settings_scroll.clone()
    }

    pub(crate) fn settings_scroll(&self) -> &ScrollHandle {
        &self.book.settings_scroll
    }

    /// What the settings held of this window ends with them: they open at their start
    /// the next time, and "Copied" is told while the settings that copied are open.
    fn settings_closed(&mut self) {
        if !matches!(self.layer, Layer::Settings) {
            self.book.copied = None;
            self.book.settings_scroll.set_offset(gpui::Point::default());
        }
    }

    /// The items of the menu of the selected server that arrange the server book
    /// (ADR 0780, ADR 0775): one step up and one step down, among the servers of its group
    /// or among the top-level parts over a whole group; the choice of its group; the
    /// collapse of its workspaces; and mark as read. An item at the end that it leads to
    /// stays in the menu and keeps the keyboard focus; it does nothing and says why.
    pub(crate) fn render_move_items(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(row) = view.servers.iter().find(|row| row.selected) else {
            return Vec::new();
        };
        let server_id = row.id;
        let shelves = view.shelves();
        let item = junto_client_core::arrangement::Item::Server(server_id);
        let at_end = |up: bool| junto_client_core::arrangement::step(&shelves, item, up).is_none();
        let in_group = row.group.is_some();
        let mut items: Vec<AnyElement> = [
            (Step::Up, "move-server-up", "menu-move-up", at_end(true)),
            (
                Step::Down,
                "move-server-down",
                "menu-move-down",
                at_end(false),
            ),
        ]
        .into_iter()
        .map(|(step, id, selector, at_end)| {
            MenuItem::new(id, txt::move_title(step))
                .detail(txt::server_move_help(step, at_end, in_group))
                .disabled(at_end)
                .selector(selector)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.move_server(server_id, step, cx));
                }))
                .into_any_element()
        })
        .collect();
        items.push(
            MenuItem::new("server-group", txt::MOVE_TO_GROUP)
                .icon(Icon::FolderOpen)
                .detail(txt::MOVE_TO_GROUP_HELP)
                .end_icon(Icon::ChevronRight)
                .selector("menu-server-group")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_layer(Layer::GroupChoice(server_id), window, cx);
                }))
                .into_any_element(),
        );
        if !row.workspaces.is_empty() {
            let collapsed = row.workspaces_collapsed;
            items.push(
                MenuItem::new(
                    "server-workspaces",
                    if collapsed {
                        txt::EXPAND_WORKSPACES
                    } else {
                        txt::COLLAPSE_WORKSPACES
                    },
                )
                .icon(if collapsed {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                })
                .detail(if collapsed {
                    txt::EXPAND_WORKSPACES_HELP
                } else {
                    txt::COLLAPSE_WORKSPACES_HELP
                })
                .selector("menu-server-workspaces")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.arrange(
                            junto_client_core::arrangement::Change::CollapseWorkspaces {
                                server: server_id,
                                collapsed: !collapsed,
                            },
                            cx,
                        );
                    });
                }))
                .into_any_element(),
            );
        }
        let unread = row.unread.any();
        items.push(
            MenuItem::new("mark-server-read", txt::MARK_SERVER_READ)
                .icon(Icon::CheckDouble)
                .detail(if unread {
                    txt::MARK_SERVER_READ_HELP
                } else {
                    txt::NOTHING_UNREAD
                })
                .disabled(!unread)
                .selector("menu-mark-server-read")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state
                        .update(cx, |state, _| state.mark_server_read(server_id, None));
                    this.close_layer(window, cx);
                }))
                .into_any_element(),
        );
        items
    }

    /// Why the selected server was not moved. It follows the last item of the menu, which
    /// opens downwards, so a refusal that arrives moves no item (ADR 0730).
    pub(crate) fn render_move_error(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let failed = view
            .servers
            .iter()
            .find(|row| row.selected)
            .and_then(|row| self.state.read(cx).move_failure(row.id))
            .map(str::to_owned);
        crate::ui::notice_slot("move-server-error-slot")
            .at_end()
            .px(u(8.0))
            .children(failed.map(|failed| Notice::error(failed).selector("move-server-error")))
            .into_any_element()
    }

    /// The notice of a newer build: one line of the channel column, directly above the
    /// profile strip. The whole line is one control, which opens the section "Version" of
    /// the settings. Nothing while the update check names no newer build.
    pub(crate) fn render_update_notice(
        &mut self,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.settings_closed();
        let update = view.update?;
        let install = self.state.read(cx).supports_update_install();
        let colors = theme.colors;
        let ring = focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        // The sentence of the line names no version: the name and the tooltip do.
        let label: SharedString = txt::update_available(update.version).into();
        Some(
            div()
                .id("update-notice")
                .role(Role::Button)
                .aria_label(label.clone())
                .debug_selector(|| "update-notice".to_owned())
                .tooltip(tooltip(label))
                .track_focus(&self.book.notice)
                .flex()
                .flex_none()
                .items_center()
                .gap(u(7.0))
                .h(u(30.0))
                .mx(u(8.0))
                .mb(u(8.0))
                .px(u(9.0))
                .rounded(u(theme.metrics.radius))
                .border_1()
                .border_color(Hsla::from(colors.cypress_wash_rule))
                .bg(Hsla::from(colors.cypress_wash))
                .typeface(theme.types.control, theme)
                .text_size(u(13.0))
                .ink(colors.cypress_ink)
                .cursor_pointer()
                .focus_visible(move |style| style.shadow(ring))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.state.update(cx, |state, cx| state.install_update(cx));
                    this.open_version_settings(window, cx);
                }))
                .child(icon(Icon::Download, 14.0, colors.cypress_ink))
                .child(div().flex_1().min_w_0().truncate().child(txt::NEW_VERSION))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .underline()
                        .child(if install { txt::UPDATE } else { txt::DETAILS }),
                )
                .into_any_element(),
        )
    }

    /// The sentence of a server that this build cannot use, with the newer version of
    /// Junto when the server needs a newer client and one is known.
    pub(crate) fn incompatibility_detail(
        &self,
        reason: Incompatibility,
        detail: String,
        cx: &App,
    ) -> String {
        match reason {
            Incompatibility::ClientTooOld { .. } => {
                let available = self.state.read(cx).available_update();
                txt::needs_update(&detail, available.map(|update| update.version))
            }
            _ => detail,
        }
    }

    /// The section "Version" of the settings: the version of this build, where the update
    /// check stands, and that this version installs nothing by itself.
    pub(crate) fn render_version_settings(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let version = self.state.read(cx).version_view();
        let colors = theme.colors;
        let page = match &version.check {
            CheckShown::Available { page, .. } => page.clone(),
            _ => None,
        };
        // The field holds the address that the state names now.
        if self.book.page_shown != page {
            let shown = page.clone().unwrap_or_default();
            self.book
                .page
                .update(cx, |field, cx| field.set_value(shown, window, cx));
            self.book.page_shown.clone_from(&page);
            self.book.copied = None;
        }
        let sentence = |text: SharedString, selector: &'static str| {
            div()
                .flex_none()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .debug_selector(move || selector.to_owned())
                .child(text)
        };
        let mut parts: Vec<AnyElement> = vec![
            div()
                .mt(u(16.0))
                .mb(u(5.0))
                .typeface(theme.types.label, theme)
                .ink(colors.bronze)
                .debug_selector(|| "version-section".to_owned())
                .child(txt::VERSION)
                .into_any_element(),
            div()
                .flex_none()
                .typeface(theme.types.control, theme)
                .font_weight(FontWeight::SEMIBOLD)
                .ink(colors.ink)
                .debug_selector(|| "version-build".to_owned())
                .child(txt::this_build(version.build))
                .into_any_element(),
            crate::ui::notice_slot("version-state-slot")
                .child(sentence(
                    txt::check_shown(&version.check).into(),
                    "version-state",
                ))
                .into_any_element(),
        ];
        if let Some(address) = page {
            let copied = self.book.copied.as_deref() == Some(address.as_str());
            parts.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(8.0))
                    .mt(u(8.0))
                    .child(div().flex_1().min_w_0().child(
                        Field::new(&self.book.page, txt::RELEASE_PAGE).selector("release-page"),
                    ))
                    .child(
                        Button::new("copy-release-page")
                            .label(if copied {
                                txt::ADDRESS_COPIED
                            } else {
                                txt::COPY_ADDRESS
                            })
                            .accessible_label(txt::COPY_ADDRESS)
                            .selector("copy-release-page")
                            .h(u(32.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // The clipboard is written by the click and at no other
                                // moment. No browser is opened.
                                cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
                                this.book.copied = Some(address.clone());
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
            parts.push(
                sentence(txt::RELEASE_PAGE_HELP.into(), "release-page-help")
                    .mt(u(5.0))
                    .into_any_element(),
            );
        }
        if version.can_check {
            parts.push(
                div()
                    .flex()
                    .flex_none()
                    .mt(u(8.0))
                    .child(
                        Button::new("check-update")
                            .label(if version.checking {
                                txt::CHECKING
                            } else {
                                txt::CHECK_NOW
                            })
                            .icon(Icon::Retry)
                            .disabled(version.checking)
                            .selector("check-update")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.check_for_update(cx));
                            })),
                    )
                    .into_any_element(),
            );
        }
        if self.state.read(cx).supports_update_install()
            && matches!(
                version.check,
                crate::state::book::CheckShown::Available { .. }
            )
        {
            let can_install = self.state.read(cx).can_install_update();
            parts.push(
                Button::new("install-update")
                    .label(txt::INSTALL_UPDATE)
                    .disabled(!can_install)
                    .selector("install-update")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.install_update(cx));
                    }))
                    .into_any_element(),
            );
            if let Some(status) = self
                .state
                .read(cx)
                .update_install_status()
                .map(str::to_owned)
            {
                parts.push(
                    sentence(status.into(), "update-install-status")
                        .mt(u(8.0))
                        .into_any_element(),
                );
            }
        } else {
            parts.push(
                sentence(
                    self.state
                        .read(cx)
                        .update_install_status()
                        .unwrap_or(txt::NO_INSTALL)
                        .to_owned()
                        .into(),
                    "version-no-install",
                )
                .mt(u(8.0))
                .into_any_element(),
            );
        }
        parts
    }

    /// The address of the release page that the settings show as text, for a test.
    #[cfg(feature = "test-support")]
    pub fn release_page_shown(&self, cx: &App) -> String {
        self.book.page.read(cx).value().to_string()
    }

    /// True when the settings say that the address was copied.
    #[cfg(feature = "test-support")]
    pub fn release_page_was_copied(&self) -> bool {
        self.book.copied.is_some()
    }

    /// True while the update notice holds the keyboard focus.
    #[cfg(feature = "test-support")]
    pub fn update_notice_is_focused(&self, window: &Window) -> bool {
        self.book.notice.is_focused(window)
    }

    /// True while the settings are open.
    #[cfg(feature = "test-support")]
    pub fn is_settings_open(&self) -> bool {
        matches!(self.layer, Layer::Settings)
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::ids::ServerId;

    use super::*;
    use crate::state::view::{ServerStatus, Unread};
    use crate::text;

    fn row(head_unread: Unread) -> ServerRow {
        ServerRow {
            id: ServerId::generate(),
            name: "Harbor".to_owned(),
            address: "harbor.example".to_owned(),
            logo: None,
            status: ServerStatus::Ready,
            selected: false,
            has_recovery: false,
            unread: head_unread,
            head_unread,
            waiting_join_requests: 0,
            workspaces: Vec::new(),
            group: None,
            workspaces_collapsed: false,
        }
    }

    /// The lines of ADR 0732, "States of a server".
    #[test]
    fn every_state_of_a_server_has_its_line_in_the_server_book() {
        use junto_client_core::flow::access::Notice;

        use crate::shell::rail::status_line;

        let line = |status: ServerStatus, has_recovery: bool| {
            let row = ServerRow {
                status,
                has_recovery,
                ..row(Unread::default())
            };
            status_line(&row).map(|(_, line)| line)
        };
        let states = [
            (ServerStatus::Connecting, "connecting…"),
            (ServerStatus::Reconnecting, "reconnecting…"),
            (ServerStatus::LoggedOut, "not logged in"),
            (ServerStatus::LoggedOutByPerson, "logged out"),
            (
                ServerStatus::WaitingForApproval {
                    name: "Luca".to_owned(),
                },
                "waiting for approval",
            ),
            (
                ServerStatus::WaitingForLink {
                    code: "ABCD-EFGH".to_owned(),
                },
                "waiting for a device link",
            ),
            (ServerStatus::NoAccount(None), "not joined"),
            (
                ServerStatus::NoAccount(Some(Notice::Removed)),
                "removed from the server",
            ),
            (
                ServerStatus::NoAccount(Some(Notice::RequestDenied)),
                "request denied",
            ),
            (
                ServerStatus::NoAccount(Some(Notice::RequestExpired)),
                "request expired",
            ),
            (
                ServerStatus::NoAccount(Some(Notice::LinkExpired)),
                "link expired",
            ),
            (
                ServerStatus::NoAccount(Some(Notice::KeyMissing)),
                "device key missing",
            ),
            (ServerStatus::Banned, "banned"),
            (ServerStatus::Inactive, "you left this server"),
            (ServerStatus::Deleted, "account deleted"),
            (ServerStatus::Unreachable, "cannot be reached"),
            (
                ServerStatus::Incompatible(Incompatibility::ClientTooOld { server_oldest: 3 }),
                "needs a newer Junto",
            ),
            (
                ServerStatus::Incompatible(Incompatibility::ServerTooOld { server_newest: 1 }),
                "server is too old",
            ),
            (
                ServerStatus::Incompatible(Incompatibility::NoCommonVersion),
                "not compatible",
            ),
            (
                ServerStatus::Incompatible(Incompatibility::NoVersions),
                "not compatible",
            ),
            (ServerStatus::OtherServer, "another server answers"),
        ];
        for (status, expected) in states {
            assert_eq!(line(status.clone(), false), Some(expected), "{status:?}");
            // A state without lists is told also when a recovery waits.
            assert_eq!(line(status.clone(), true), Some(expected), "{status:?}");
        }
        // A server that shows its lists, and one without a connection by its mode, show
        // their address, or the recovery notice.
        for status in [ServerStatus::Ready, ServerStatus::Standby] {
            assert_eq!(line(status.clone(), false), None);
            assert_eq!(line(status, true), Some("recovery notice"));
        }
    }

    #[test]
    fn the_head_of_a_server_names_what_it_shows_as_unread() {
        assert_eq!(head_label(&row(Unread::default())), "Harbor");
        let plain = Unread {
            messages: 3,
            mentions: 0,
            sign: true,
            muted: false,
        };
        let label = head_label(&row(plain));
        assert!(label.starts_with("Harbor, "), "{label}");
        assert_eq!(
            label.as_ref(),
            format!("Harbor, {}", text::unread_tooltip(3, 0))
        );
        // A mention outranks plain unread: the sign is the count of the mentions.
        let mention = Unread {
            messages: 3,
            mentions: 2,
            sign: true,
            muted: false,
        };
        let sign = crate::shell::channels::unread_sign(mention).unwrap();
        assert_eq!(
            sign.mentions.as_deref(),
            Some(text::mention_badge(2).as_str())
        );
        assert!(
            crate::shell::channels::unread_sign(plain)
                .unwrap()
                .mentions
                .is_none()
        );
    }
}

//! The search of the window (ADR 0743, view H of the design): the field of the header, and
//! the side panel with the filters and the results.

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::{ChannelId, ServerId};

use super::notices::{Excerpt, place_icon, side_panel};
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::search::{
    DateFilter, Filters, ResultRow, ResultTarget, SearchPanel, SearchShown,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Field, FieldKind, Menu, MenuItem, Notice, ScrollArea, SelectBox,
    SideHeader, StatePanel, StyleExt, Toggle, icon,
};

/// One of the three filters of a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Channel,
    Author,
    Date,
}

/// The width of the search field in the header, in design pixels, in the full and in the
/// short form of the header (ADR 0730).
pub(crate) fn search_width(compact: bool) -> f32 {
    if compact { 130.0 } else { 190.0 }
}

impl Shell {
    /// Puts text into the search field of the header, as typing does.
    pub fn type_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.search_field
            .update(cx, |field, cx| field.set_value(text, window, cx));
    }

    /// Moves the keyboard focus into the search field of the header.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_field(&self.search_field.clone(), window, cx);
    }

    /// The text of the search field.
    pub fn search_text(&self, cx: &gpui::App) -> String {
        self.search_field.read(cx).value().to_string()
    }

    /// Searches the shown server for the text of the field.
    pub fn submit_search(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(server_id) = self.state.read(cx).view().selected.map(|server| server.id) else {
            return;
        };
        let text = self.search_text(cx);
        self.members_open = false;
        self.state
            .update(cx, |state, cx| state.search(server_id, &text, cx));
        cx.notify();
    }

    /// Scrolls the results of the search to their end.
    pub fn show_end_of_results(&mut self, cx: &mut Context<Self>) {
        self.search_scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Ends the search and empties its field.
    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| state.close_search(cx));
        self.type_search("", window, cx);
        if matches!(self.layer, Layer::SearchFilter(_)) {
            self.close_layer(window, cx);
        }
        cx.notify();
    }

    /// Changes the filters of the search and runs it again.
    pub fn set_search_filters(&mut self, filters: Filters, cx: &mut Context<Self>) {
        self.search_date_invalid = false;
        self.state
            .update(cx, |state, cx| state.set_search_filters(filters, cx));
        cx.notify();
    }

    /// The search field of the header of the main area (view H of the design).
    pub(crate) fn render_search_field(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let searching = self
            .view
            .as_ref()
            .is_some_and(|view| view.search_text.is_some());
        let field = Field::new(&self.search_field, text::SEARCH_FIELD)
            .kind(FieldKind::Find)
            .selector("search-field");
        let field = if searching || !self.search_text(cx).is_empty() {
            field.trailing(
                div().flex_none().pr(u(2.0)).child(
                    Button::new("clear-search")
                        .kind(ButtonKind::Ghost)
                        .icon(Icon::X)
                        .tooltip(text::CLOSE_SEARCH)
                        .selector("clear-search")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_search(window, cx);
                        })),
                ),
            )
        } else {
            field
        };
        div()
            .flex_none()
            .w(u(search_width(compact)))
            .child(field)
            .into_any_element()
    }

    fn open_search_filter(
        &mut self,
        kind: FilterKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A click on the box of an open list closes the list: the press closed it, and
        // the click must not open it again.
        if std::mem::take(&mut self.filter_closed_by_press) {
            return;
        }
        self.open_layer(Layer::SearchFilter(kind), window, cx);
    }

    /// The list of the choices of one filter, under its box.
    fn render_filter_menu(
        &mut self,
        kind: FilterKind,
        panel: &SearchPanel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filters = panel.filters;
        let item = |selector: String,
                    index: usize,
                    title: SharedString,
                    chosen: bool,
                    with: Filters,
                    cx: &mut Context<Self>| {
            div()
                .flex_none()
                .debug_selector(move || selector.clone())
                .child(
                    MenuItem::new(("filter", index), title)
                        .icon(if chosen { Icon::Check } else { Icon::Minus })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_search_filters(with, cx);
                            this.close_layer(window, cx);
                        })),
                )
                .into_any_element()
        };
        let (label, items): (&'static str, Vec<AnyElement>) = match kind {
            FilterKind::Channel => {
                let mut items = vec![item(
                    "search-channel:all".to_owned(),
                    0,
                    text::FILTER_ALL_CHANNELS.into(),
                    filters.channel.is_none(),
                    Filters {
                        channel: None,
                        ..filters
                    },
                    cx,
                )];
                for (index, (channel_id, name, _)) in panel.channels.iter().enumerate() {
                    // A channel whose history the person may not read is no filter
                    // (ADR 0790).
                    if panel.unreadable.contains(channel_id) {
                        continue;
                    }
                    items.push(item(
                        format!("search-channel:{channel_id}"),
                        index + 1,
                        name.clone().into(),
                        filters.channel == Some(*channel_id),
                        Filters {
                            channel: Some(*channel_id),
                            ..filters
                        },
                        cx,
                    ));
                }
                (text::FILTER_CHANNEL, items)
            }
            FilterKind::Author => {
                let mut items = vec![item(
                    "search-author:anyone".to_owned(),
                    0,
                    text::FILTER_ANYONE.into(),
                    filters.author.is_none(),
                    Filters {
                        author: None,
                        ..filters
                    },
                    cx,
                )];
                for (index, (account_id, name)) in panel.authors.iter().enumerate() {
                    items.push(item(
                        format!("search-author:{account_id}"),
                        index + 1,
                        name.clone().into(),
                        filters.author == Some(*account_id),
                        Filters {
                            author: Some(*account_id),
                            ..filters
                        },
                        cx,
                    ));
                }
                (text::FILTER_AUTHOR, items)
            }
            FilterKind::Date => (
                text::FILTER_DATE,
                DateFilter::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, date)| {
                        item(
                            format!("search-date-{index}"),
                            index,
                            text::date_filter(date).into(),
                            filters.date == date,
                            Filters { date, ..filters },
                            cx,
                        )
                    })
                    .collect(),
            ),
        };
        div()
            .absolute()
            .left_0()
            .top(u(32.0))
            .child(deferred(
                anchored().snap_to_window().child(
                    Menu::new("search-filter-menu", label, self.layer_focus())
                        .width(220.0)
                        .selector("search-filter-menu")
                        .on_close(Self::handler(cx, |this, window, cx| {
                            this.filter_closed_by_press = !window.last_input_was_keyboard();
                            this.close_layer(window, cx);
                        }))
                        .child(
                            div()
                                .id("filter-choices")
                                .flex()
                                .flex_col()
                                .max_h(u(320.0))
                                .overflow_y_scroll()
                                .children(items),
                        )
                        .child(div().h(u(4.0))),
                ),
            ))
            .into_any_element()
    }

    /// The filters of a search, under the header of its panel.
    fn render_filters(
        &mut self,
        panel: &SearchPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let open = match &self.layer {
            Layer::SearchFilter(kind) => Some(*kind),
            _ => None,
        };
        let boxes: [(FilterKind, &'static str, SharedString, &'static str); 3] = [
            (
                FilterKind::Channel,
                text::FILTER_CHANNEL,
                panel
                    .channel
                    .as_ref()
                    .map_or(text::FILTER_ALL_CHANNELS.into(), |(_, name)| {
                        name.clone().into()
                    }),
                "search-channel",
            ),
            (
                FilterKind::Author,
                text::FILTER_AUTHOR,
                panel
                    .author
                    .as_ref()
                    .map_or(text::FILTER_ANYONE.into(), |(_, name)| name.clone().into()),
                "search-author",
            ),
            (
                FilterKind::Date,
                text::FILTER_DATE,
                text::date_filter(panel.date).into(),
                "search-date",
            ),
        ];
        let mut row = div()
            .flex()
            .flex_none()
            .gap(u(6.0))
            .pt(u(8.0))
            .px(u(14.0))
            .pb(u(10.0))
            .bg(Hsla::from(colors.limestone))
            .border_b_1()
            .border_color(Hsla::from(colors.rule));
        for (kind, label, value, selector) in boxes {
            let menu = (open == Some(kind)).then(|| self.render_filter_menu(kind, panel, cx));
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .mb(u(3.0))
                            .text_size(u(12.5))
                            .line_height(u(16.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .ink(colors.bronze)
                            .child(label),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                SelectBox::new(selector, label, value)
                                    .selector(selector)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_search_filter(kind, window, cx);
                                    })),
                            )
                            .children(menu),
                    ),
            );
        }
        let filters = panel.filters;
        let mut content = div().flex().flex_col().gap(u(6.0)).px(u(14.0)).pb(u(8.0));
        let options: [(&'static str, &'static str, bool, Filters); 5] = [
            (
                "search-has-file",
                text::search::HAS_FILE,
                filters.has_file,
                Filters {
                    has_file: !filters.has_file,
                    ..filters
                },
            ),
            (
                "search-has-link",
                text::search::HAS_LINK,
                filters.has_link,
                Filters {
                    has_link: !filters.has_link,
                    ..filters
                },
            ),
            (
                "search-has-picture",
                text::search::HAS_PICTURE,
                filters.has_picture,
                Filters {
                    has_picture: !filters.has_picture,
                    ..filters
                },
            ),
            (
                "search-mentions-me",
                text::search::MENTIONS_ME,
                filters.mentions_me,
                Filters {
                    mentions_me: !filters.mentions_me,
                    ..filters
                },
            ),
            (
                "search-direct",
                text::search::DIRECT_MESSAGES,
                filters.direct,
                Filters {
                    direct: !filters.direct,
                    ..filters
                },
            ),
        ];
        for (selector, title, on, next) in options {
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(title)
                    .child(
                        Toggle::new(selector, title, on, (text::ON, text::OFF))
                            .held(match panel.capabilities {
                                Some(supported)
                                    if supported.filters
                                        && (!panel.text.is_empty() || supported.filter_only) =>
                                {
                                    None
                                }
                                Some(supported) if supported.filters => {
                                    Some(text::search::FILTERS_NEED_TEXT)
                                }
                                Some(_) => Some(text::search::FILTERS_UNSUPPORTED),
                                None => Some(text::search::SUPPORT_UNKNOWN),
                            })
                            .selector(selector)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_search_filters(next, cx);
                            })),
                    ),
            );
        }
        content = content
            .child(
                div()
                    .flex()
                    .gap(u(6.0))
                    .child(
                        div().flex_1().min_w_0().child(text::search::FROM).child(
                            Field::new(&self.search_from, "From date").selector("search-from"),
                        ),
                    )
                    .child(div().flex_1().min_w_0().child(text::search::THROUGH).child(
                        Field::new(&self.search_through, "Through date").selector("search-through"),
                    )),
            )
            .child(
                Button::new("search-apply-dates")
                    .label(text::search::APPLY_DATES)
                    .selector("search-apply-dates")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let format = time::macros::format_description!("[year]-[month]-[day]");
                        let from =
                            time::Date::parse(this.search_from.read(cx).value().trim(), format);
                        let through =
                            time::Date::parse(this.search_through.read(cx).value().trim(), format);
                        if let (Ok(from), Ok(through)) = (from, through)
                            && from <= through
                            && through.next_day().is_some()
                        {
                            this.search_date_invalid = false;
                            this.set_search_filters(
                                Filters {
                                    date: DateFilter::Range { from, through },
                                    ..filters
                                },
                                cx,
                            );
                        } else {
                            this.search_date_invalid = true;
                            cx.notify();
                        }
                    })),
            );
        if self.search_date_invalid {
            content = content.child(
                div()
                    .debug_selector(|| "search-date-invalid".to_owned())
                    .child(text::search::DATES_HELP),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(row)
            .child(content)
            .into_any_element()
    }

    /// One result. `index` is its place in the list, and `number` its place among the
    /// results of its kind, which names a line of a transcript for a test.
    fn render_result(
        &mut self,
        position: (usize, usize),
        row: &ResultRow,
        panel: &SearchPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (index, number) = position;
        let colors = theme.colors;
        let server_id = panel.server_id;
        let (action, tag, selector): (SharedString, Option<SharedString>, String) = match row.target
        {
            ResultTarget::Message(message_id) => (
                if panel.going == Some(message_id) {
                    text::MESSAGES_LOADING.into()
                } else {
                    text::GO_TO_MESSAGE.into()
                },
                None,
                format!("search-result:{message_id}"),
            ),
            ResultTarget::Transcript { .. } => (
                text::OPEN_TRANSCRIPT.into(),
                Some(text::RECORDING_RESULT.into()),
                format!("search-transcript-{number}"),
            ),
        };
        let shown = Excerpt {
            icon: place_icon(&row.place),
            place: text::place_name(&row.place).into(),
            day: row.day.clone().into(),
            action: Some(action.clone()),
            seed: row.author_id.as_uuid().as_u128(),
            picture: self.faces(server_id).image(row.author_id),
            presence: Some(self.faces(server_id).presence(row.author_id)),
            name_color: self
                .faces(server_id)
                .in_workspace(row.workspace_id)
                .color(row.author_id),
            profile: Some(crate::ui::OpenProfile {
                server: server_id,
                account: row.author_id,
            }),
            author: row.author.clone().into(),
            account: self.excerpt_account(
                server_id,
                row.author_id,
                super::account_menus::AccountSource::Search(row.target),
                cx,
            ),
            time: row.time.clone().into(),
            tag,
            text: row.text.clone().into(),
            marks: row.marks.clone(),
            unread: false,
        };
        let label: SharedString = format!(
            "{}, {}: {}",
            row.author,
            text::place_name(&row.place),
            row.text
        )
        .into();
        let ring = crate::ui::focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let target = row.target;
        let owner = self
            .state
            .read(cx)
            .server(server_id)
            .and_then(|server| server.owner());
        let copied = row.text.clone();
        let id: SharedString = format!(
            "search-actions:{owner:?}:{target:?}:{}:{:?}:{:?}:{:?}",
            panel.text,
            panel.channel.as_ref().map(|(id, _)| id),
            panel.author.as_ref().map(|(id, _)| id),
            panel.filters
        )
        .into();
        let focus = crate::ui::row_focus(id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let (frame, actions) = crate::ui::collect_actions(|| {
            crate::ui::RowAction::new(
                action.clone(),
                cx.listener(move |this, _, _, cx| {
                    this.go_to_result(server_id, target, cx);
                }),
            )
            .register();
            crate::ui::RowAction::new(
                text::COPY_TEXT,
                cx.listener(move |this, _, _, cx| {
                    this.copy_text(copied.clone(), cx);
                }),
            )
            .register();
            crate::ui::RowAction::new(
                text::COPY_TARGET_LINK,
                cx.listener(move |this, _, _, cx| {
                    let state = this.state.read(cx);
                    let link = match target {
                        ResultTarget::Message(id) => state.message_link(server_id, id),
                        ResultTarget::Transcript { recording_id, .. } => {
                            state.recording_link(server_id, recording_id)
                        }
                    };
                    if let Some(link) = link {
                        this.copy_text(link, cx);
                    }
                }),
            )
            .register();
            div()
                .id((
                    match row.target {
                        ResultTarget::Message(_) => "result",
                        ResultTarget::Transcript { .. } => "transcript-result",
                    },
                    index,
                ))
                .role(Role::ListItem)
                .aria_label(label)
                .debug_selector(move || selector.clone())
                .flex_none()
                .border_t_1()
                .border_color(Hsla::from(colors.rule_soft))
                .cursor_pointer()
                .track_focus(&focus)
                .hover(move |style| style.bg(Hsla::from(colors.row_hover)))
                .focus_visible(move |style| style.shadow(ring))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.go_to_result(server_id, target, cx);
                }))
                .child(shown.element(theme, cx))
        });
        crate::ui::bind_row_action_menu(
            frame,
            id,
            crate::ui::RowMenu::overlay(focused),
            actions,
            window,
            cx,
        )
        .into_any_element()
    }

    /// Goes to what a result of a search names.
    pub(crate) fn go_to_result(
        &mut self,
        server_id: ServerId,
        target: ResultTarget,
        cx: &mut Context<Self>,
    ) {
        self.members_open = false;
        let file_call = self
            .view
            .as_ref()
            .and_then(|view| view.lists())
            .is_some_and(|lists| {
                lists.call.is_some() && lists.search.as_ref().is_some_and(|panel| panel.files)
            });
        if file_call {
            self.state.update(cx, |state, cx| state.hide_search(cx));
            self.open_call_panel(cx);
            self.show_call_tab(super::call::CallTab::Chat, cx);
        }
        self.state.update(cx, |state, cx| match target {
            ResultTarget::Message(message_id) => state.go_to_result(message_id, cx),
            ResultTarget::Transcript {
                workspace_id,
                channel_id,
                recording_id,
                line,
            } => state.open_transcript(
                server_id,
                workspace_id,
                channel_id,
                recording_id,
                Some(line),
                cx,
            ),
        });
        cx.notify();
    }

    /// The side panel of a search: its header, its filters, and its results as messages
    /// and as lines of transcripts.
    pub(crate) fn render_search_panel(
        &mut self,
        panel: &SearchPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let files = panel.files;
        let found = if files {
            panel.results.iter().map(|row| row.attachments.len()).sum()
        } else {
            panel.results.len()
        };
        let note: SharedString = match &panel.state {
            SearchShown::Searching if found == 0 => text::SEARCHING.into(),
            _ if files && found == 1 => "1 file".into(),
            _ if files => format!("{found} files").into(),
            _ => text::results_for(found, panel.more, &panel.text).into(),
        };
        let header = SideHeader::new(if files {
            text::search::FILES
        } else {
            text::SEARCH
        })
        .title_bar()
        .icon(if files {
            Icon::FolderOpen
        } else {
            Icon::Search
        })
        .note(note)
        .trailing(
            Button::new("close-search")
                .kind(ButtonKind::Ghost)
                .icon(Icon::X)
                .tooltip(text::CLOSE_SEARCH)
                .selector("close-search")
                .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
        );
        // The Files panel of a voice channel has the tabs Files and Recordings (ADR 0784).
        let voice_channel = panel
            .channel
            .as_ref()
            .filter(|_| files)
            .and_then(|(channel_id, _)| {
                panel
                    .channels
                    .iter()
                    .find(|(id, ..)| id == channel_id)
                    .filter(|(.., kind)| {
                        *kind == junto_client_core::protocol::structure::ChannelKind::Voice
                    })
                    .map(|(id, ..)| *id)
            })
            .zip(
                self.view
                    .as_ref()
                    .and_then(|view| view.workspace())
                    .map(|w| w.id),
            );
        let recordings_tab = voice_channel.is_some() && self.files_recordings;
        let tabs = voice_channel.map(|(channel_id, workspace_id)| {
            let server_id = panel.server_id;
            let files_tab =
                crate::ui::Tab::new("files-tab-files", Icon::FolderOpen, text::FILES_TAB)
                    .on(!recordings_tab)
                    .selector("files-tab-files")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_recordings = false;
                        cx.notify();
                    }));
            let recordings =
                crate::ui::Tab::new("files-tab-recordings", Icon::Record, text::RECORDINGS_TAB)
                    .on(recordings_tab)
                    .selector("files-tab-recordings")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.files_recordings = true;
                        this.search_scroll.set_offset(gpui::Point::default());
                        this.state.update(cx, |state, cx| {
                            state.show_channel_recordings(server_id, workspace_id, channel_id, cx)
                        });
                    }));
            div().flex_none().px(u(10.0)).child(crate::ui::TabRow::new(
                "files-tabs",
                text::FILES_TABS,
                [files_tab, recordings],
            ))
        });
        let filters = (!files).then(|| self.render_filters(panel, &theme, cx));
        let unsupported = panel
            .capabilities
            .filter(|supported| !files && !supported.filters)
            .map(|_| {
                Notice::new(text::search::SOME_FILTERS_UNSUPPORTED)
                    .selector("search-filters-unsupported")
            });
        let again = |cx: &mut Context<Self>| {
            Button::new("search-again")
                .icon(Icon::Retry)
                .label(text::SEARCH_AGAIN)
                .selector("search-again")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.search_again(cx));
                }))
        };
        let go_failed = panel.go_failed.clone().map(|reason| {
            div().flex_none().px(u(14.0)).pt(u(10.0)).child(
                Notice::error(reason).selector("search-go-failed").action(
                    Button::new("dismiss-search-failure")
                        .label(text::DISMISS)
                        .selector("dismiss-search-failure")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.dismiss_search_failure(cx));
                        })),
                ),
            )
        });
        // The results of archived channels whose list could not be read (ADR 0762).
        let server_id = panel.server_id;
        let archive_failed = (!panel.archive_failed.is_empty()).then(|| {
            let workspaces = panel.archive_failed.clone();
            div().flex_none().px(u(14.0)).pt(u(10.0)).child(
                Notice::error(text::ARCHIVED_RESULTS_NOT_READ)
                    .selector("search-archive-failed")
                    .action(
                        Button::new("search-archive-retry")
                            .label(text::TRY_AGAIN)
                            .selector("search-archive-retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_archived_lists_again(server_id, &workspaces, cx);
                                });
                            })),
                    ),
            )
        });
        let recordings =
            voice_channel
                .filter(|_| recordings_tab)
                .map(|(channel_id, workspace_id)| {
                    self.render_channel_recordings(
                        panel.server_id,
                        workspace_id,
                        channel_id,
                        &theme,
                        cx,
                    )
                });
        let body: AnyElement = match (&panel.state, recordings) {
            (_, Some(recordings)) => recordings,
            (state, None) => match state {
                SearchShown::Searching if found == 0 => div()
                    .id("search-running")
                    .debug_selector(|| "search-running".to_owned())
                    .px(u(14.0))
                    .pt(u(12.0))
                    .typeface(theme.types.small, &theme)
                    .ink(colors.ink_2)
                    .child(text::SEARCHING)
                    .into_any_element(),
                SearchShown::Nothing => {
                    let empty = StatePanel::new(
                        if !files && panel.text.is_empty() && panel.filters == Filters::default() {
                            text::search::CHOOSE_FILTERS
                        } else if files && panel.previous_files {
                            text::search::NO_OLDER_FILES
                        } else if files {
                            text::search::NO_FILES
                        } else {
                            text::SEARCH_NOTHING_TITLE
                        },
                        if !files && panel.text.is_empty() && panel.filters == Filters::default() {
                            text::search::CHOOSE_FILTERS_HELP
                        } else if files && panel.previous_files {
                            text::search::NO_OLDER_FILES_HELP
                        } else if files {
                            text::search::NO_FILES_HELP
                        } else {
                            text::SEARCH_NOTHING
                        },
                    )
                    .selector("search-nothing");
                    if files && panel.previous_files {
                        empty
                            .action(
                                Button::new("previous-files")
                                    .label(text::search::NEWER_FILES)
                                    .selector("previous-files")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.search_scroll.set_offset(gpui::Point::default());
                                        this.state.update(cx, |state, cx| state.previous_files(cx));
                                    })),
                            )
                            .into_any_element()
                    } else {
                        empty.into_any_element()
                    }
                }
                SearchShown::Refused(reason) => {
                    StatePanel::new(text::SEARCH_REFUSED_TITLE, reason.clone())
                        .icon(Icon::Warning)
                        .selector("search-refused")
                        .into_any_element()
                }
                SearchShown::Failed(reason) => {
                    StatePanel::new(text::SEARCH_FAILED_TITLE, reason.clone())
                        .icon(Icon::Warning)
                        .selector("search-failed")
                        .action(again(cx))
                        .into_any_element()
                }
                SearchShown::Searching | SearchShown::Results => {
                    // One list in the order of the server: the rows of a later page join it
                    // after the rows before, and no listed row moves (ADR 0730, ADR 0743).
                    let mut rows: Vec<AnyElement> = Vec::with_capacity(panel.results.len());
                    let mut transcripts = 0;
                    for (index, row) in panel.results.iter().enumerate() {
                        let number = match row.target {
                            ResultTarget::Message(_) => index,
                            ResultTarget::Transcript { .. } => {
                                transcripts += 1;
                                transcripts - 1
                            }
                        };
                        rows.push(self.render_result(
                            (index, number),
                            row,
                            panel,
                            &theme,
                            window,
                            cx,
                        ));
                        rows.extend(self.listed_files(panel.server_id, row, &theme, window, cx));
                    }
                    // "Show more results" stands under the list, at a place of its own while
                    // results are listed: a page that arrives does not move it.
                    let more = div().flex().flex_none().px(u(14.0)).py(u(8.0)).child(
                        Button::new("more-results")
                            .label(if panel.reading_more {
                                text::READING_MORE_RESULTS
                            } else {
                                if files {
                                    text::search::OLDER_FILES
                                } else {
                                    text::SHOW_MORE_RESULTS
                                }
                            })
                            .disabled(panel.reading_more || !panel.can_read_more)
                            .selector("more-results")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if files {
                                    this.search_scroll.set_offset(gpui::Point::default());
                                }
                                this.state
                                    .update(cx, |state, cx| state.read_more_results(cx));
                            })),
                    );
                    let previous = files.then(|| {
                        div().flex().flex_none().px(u(14.0)).child(
                            Button::new("previous-files")
                                .label(text::search::NEWER_FILES)
                                .selector("previous-files")
                                .disabled(!panel.previous_files || panel.reading_more)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.search_scroll.set_offset(gpui::Point::default());
                                    this.state.update(cx, |state, cx| state.previous_files(cx));
                                })),
                        )
                    });
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .children(previous)
                        .child(
                            ScrollArea::new("search-results", &self.search_scroll, text::SEARCH)
                                .selector("search-results")
                                .children(rows)
                                .child(div().flex_none().h(u(6.0))),
                        )
                        .child(more)
                        .into_any_element()
                }
            },
        };
        side_panel(
            &theme,
            text::SEARCH,
            if files {
                "file-list-panel"
            } else {
                "search-panel"
            },
            self.panel_width(
                junto_client_core::settings::Panel::Search,
                theme.metrics.side_panel_width,
                cx,
            ),
        )
        .child(header)
        .children(tabs)
        .children(filters)
        .children(unsupported)
        .child(
            crate::ui::notice_slot("search-go-failed-slot")
                .children(go_failed)
                .children(archive_failed),
        )
        .child(body)
        .into_any_element()
    }

    /// The tab "Recordings" of the Files panel of a voice channel (ADR 0784): its
    /// recordings, newest first, each with its transcript, its card in the chat and its
    /// files.
    /// The files of a recording of the tab "Recordings", each with a control that saves it,
    /// and the downloads with their progress, as the card of the recording has them.
    fn render_channel_recording_files(
        &mut self,
        index: usize,
        card: &crate::state::recorded::RecordingCard,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::state::RecordedFile;
        use crate::state::recorded::{CardState, FileKind};
        let colors = theme.colors;
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let panel = div()
            .flex()
            .flex_col()
            .gap(u(4.0))
            .debug_selector(move || format!("channel-recording-files:{recording_id}"));
        let detail = match &card.state {
            CardState::Shown(detail) => detail,
            _ => {
                return panel
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::RECORDING_READING),
                    )
                    .into_any_element();
            }
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for (position, file) in detail.files.iter().enumerate() {
            let (segment_id, stem) = (file.segment_id, file.stem.clone());
            let title = super::recorded::file_title(file);
            rows.push(
                div()
                    .debug_selector(move || {
                        format!("channel-recording-file:{recording_id}:{position}")
                    })
                    .child(
                        Button::new(("channel-recording-file", index * 1000 + position))
                            .kind(ButtonKind::Ghost)
                            .icon(match file.kind {
                                FileKind::Video => Icon::Play,
                                FileKind::Microphone => Icon::Microphone,
                                FileKind::ShareSound => Icon::Screen,
                            })
                            .label(format!("{title} · {}", text::file_size(file.bytes)))
                            .when_some(file.problem.clone(), |button, problem| {
                                button.tooltip(problem)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.save_recording_file(
                                        server_id,
                                        recording_id,
                                        RecordedFile::Segment(segment_id),
                                        &stem,
                                        false,
                                        cx,
                                    );
                                });
                            })),
                    )
                    .into_any_element(),
            );
            if let Some(progress) = &file.download
                && progress.stage != junto_client_core::transfers::DownloadStage::Cancelled
            {
                let control = self.recorded_download(
                    card,
                    RecordedFile::Segment(segment_id),
                    index * 1000 + position,
                    progress,
                    theme,
                    cx,
                );
                rows.push(Self::download_row(title, control, position, theme));
            }
        }
        panel.children(rows).into_any_element()
    }

    fn render_channel_recordings(
        &mut self,
        server_id: ServerId,
        workspace_id: junto_client_core::protocol::ids::WorkspaceId,
        channel_id: ChannelId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let state = self.state.read(cx);
        // The list of this channel, read for the account that the device has now. A tab
        // that stays open while another voice channel is shown reads that channel.
        let Some(list) = state
            .channel_recordings_of(server_id, workspace_id, channel_id)
            .cloned()
        else {
            let app = self.state.downgrade();
            cx.defer(move |cx| {
                let _ = app.update(cx, |state, cx| {
                    if state
                        .channel_recordings_of(server_id, workspace_id, channel_id)
                        .is_none()
                    {
                        state.show_channel_recordings(server_id, workspace_id, channel_id, cx);
                    }
                });
            });
            return div().into_any_element();
        };
        let files_card = list
            .files_open
            .and_then(|recording_id| state.channel_recording_card(recording_id));
        let address = state.server(server_id).map(|server| server.entry().address);
        let rows: Vec<AnyElement> = list
            .recordings
            .iter()
            .enumerate()
            .map(|(index, recording)| {
                let id = recording.id;
                let when = self.state.read(cx).recording_time(recording);
                let length = recording
                    .recording_since
                    .zip(recording.ended_at)
                    .and_then(|(since, ended)| std::time::Duration::try_from(ended - since).ok())
                    .map(text::admin::recordings::length);
                let detail = [Some(text::recording_state_word(recording.state).to_owned()), length]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let address = address.clone();
                let saved = matches!(
                    recording.state,
                    junto_client_core::protocol::recording::RecordingState::Ready
                        | junto_client_core::protocol::recording::RecordingState::PartlyFailed
                );
                let files = files_card
                    .as_ref()
                    .filter(|card| card.recording_id == id)
                    .map(|card| self.render_channel_recording_files(index, card, theme, cx));
                div()
                    .id(("channel-recording", index))
                    .role(Role::ListItem)
                    .aria_label(format!("{}, {when}", text::RECORDING))
                    .debug_selector(move || format!("channel-recording:{id}"))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(u(4.0))
                    .px(u(14.0))
                    .py(u(8.0))
                    .border_t_1()
                    .border_color(Hsla::from(colors.rule_soft))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(7.0))
                            .typeface(theme.types.control_strong, theme)
                            .child(icon(Icon::Record, 14.0, colors.cinnabar))
                            .child(div().min_w_0().truncate().child(when)),
                    )
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(detail),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(u(6.0))
                            .child(
                                div()
                                    .debug_selector(move || format!("channel-recording-card:{id}"))
                                    .child(
                                        Button::new(("channel-recording-card", index))
                                            .kind(ButtonKind::Ghost)
                                            .label(text::OPEN_RECORDING_CARD)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(address) = &address {
                                                    this.state.update(cx, |state, cx| {
                                                        state.open_recording_link(address, id, cx);
                                                    });
                                                }
                                            })),
                                    ),
                            )
                            .when(
                                recording.transcript
                                    != junto_client_core::protocol::transcript::TranscriptState::None,
                                |actions| {
                                    actions.child(
                                        div()
                                            .debug_selector(move || {
                                                format!("channel-recording-transcript:{id}")
                                            })
                                            .child(
                                                Button::new(("channel-recording-transcript", index))
                                                    .kind(ButtonKind::Ghost)
                                                    .label(text::TRANSCRIPT)
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.state.update(cx, |state, cx| {
                                                            state.open_transcript(
                                                                server_id,
                                                                workspace_id,
                                                                channel_id,
                                                                id,
                                                                None,
                                                                cx,
                                                            );
                                                        });
                                                    })),
                                            ),
                                    )
                                },
                            )
                            // The files of the recording, saved through the flow of its card
                            // (ADR 0784).
                            .when(saved, |actions| {
                                actions.child(
                                    div()
                                        .debug_selector(move || {
                                            format!("channel-recording-download:{id}")
                                        })
                                        .child(
                                            Button::new(("channel-recording-download", index))
                                                .kind(ButtonKind::Ghost)
                                                .icon(Icon::Download)
                                                .label(text::DOWNLOAD)
                                                .tooltip(text::RECORDING_FILES)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |state, cx| {
                                                        state.toggle_channel_recording_files(id, cx);
                                                    });
                                                })),
                                        ),
                                )
                            }),
                    )
                    .children(files)
                    .into_any_element()
            })
            .collect();
        let end: AnyElement = if list.reading {
            div()
                .px(u(14.0))
                .py(u(8.0))
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .debug_selector(|| "channel-recordings-reading".to_owned())
                .child(text::READING_RECORDINGS)
                .into_any_element()
        } else if let Some(failed) = list.failed.clone() {
            div()
                .px(u(14.0))
                .py(u(8.0))
                .child(
                    Notice::error(failed)
                        .selector("channel-recordings-failed")
                        .action(
                            Button::new("channel-recordings-retry")
                                .label(text::TRY_AGAIN)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.show_channel_recordings(
                                            server_id,
                                            workspace_id,
                                            channel_id,
                                            cx,
                                        );
                                    });
                                })),
                        ),
                )
                .into_any_element()
        } else if list.recordings.is_empty() && list.older.is_none() {
            StatePanel::new(text::NO_RECORDINGS_TITLE, text::NO_RECORDINGS)
                .selector("channel-recordings-empty")
                .into_any_element()
        } else if list.older.is_some() {
            div()
                .px(u(14.0))
                .py(u(8.0))
                .child(
                    Button::new("channel-recordings-older")
                        .label(text::OLDER_RECORDINGS)
                        .selector("channel-recordings-older")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.read_older_channel_recordings(cx));
                        })),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };
        ScrollArea::new(
            "channel-recordings",
            &self.search_scroll,
            text::RECORDINGS_TAB,
        )
        .selector("channel-recordings")
        .children(rows)
        .child(end)
        .into_any_element()
    }
}

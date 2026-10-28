//! The search of the window (ADR 0743, view H of the design): the field of the header, and
//! the side panel with the filters and the results.

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::ServerId;

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
    SideHeader, StatePanel, StyleExt,
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
        let filters = Filters {
            channel: panel.channel.as_ref().map(|(channel_id, _)| *channel_id),
            author: panel.author.as_ref().map(|(account_id, _)| *account_id),
            date: panel.date,
        };
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
                    if panel.unreadable.contains(channel_id) {
                        let selector = format!("search-channel-refused:{channel_id}");
                        items.push(
                            div()
                                .flex_none()
                                .debug_selector(move || selector.clone())
                                .child(
                                    MenuItem::new(("filter", index + 1), name.clone())
                                        .icon(Icon::Lock)
                                        .disabled(true)
                                        .detail(text::not_permitted(
                                            WorkspacePermission::ReadMessageHistory,
                                        )),
                                )
                                .into_any_element(),
                        );
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

    /// The three filters of a search, under the header of its panel.
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
        row.into_any_element()
    }

    /// One result. `index` is its place in the list, and `number` its place among the
    /// results of its kind, which names a line of a transcript for a test.
    fn render_result(
        &mut self,
        index: usize,
        number: usize,
        row: &ResultRow,
        panel: &SearchPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            action: Some(action),
            seed: row.author_id.as_uuid().as_u128(),
            picture: self.faces(server_id).image(row.author_id),
            author: row.author.clone().into(),
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
            .focusable()
            .tab_stop(true)
            .hover(move |style| style.bg(Hsla::from(colors.row_hover)))
            .focus_visible(move |style| style.shadow(ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.go_to_result(server_id, target, cx);
            }))
            .child(shown.element(theme, cx))
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
        let _ = window;
        let theme = theme(cx);
        let colors = theme.colors;
        let found = panel.results.len();
        let note: SharedString = match &panel.state {
            SearchShown::Searching if found == 0 => text::SEARCHING.into(),
            _ => text::results_for(found, panel.more, &panel.text).into(),
        };
        let header = SideHeader::new(text::SEARCH)
            .title_bar()
            .icon(Icon::Search)
            .note(note)
            .trailing(
                Button::new("close-search")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_SEARCH)
                    .selector("close-search")
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
            );
        let filters = self.render_filters(panel, &theme, cx);
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
        let body: AnyElement = match &panel.state {
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
                StatePanel::new(text::SEARCH_NOTHING_TITLE, text::SEARCH_NOTHING)
                    .selector("search-nothing")
                    .into_any_element()
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
                    rows.push(self.render_result(index, number, row, panel, &theme, cx));
                }
                // "Show more results" stands under the list, at a place of its own while
                // results are listed: a page that arrives does not move it.
                let more = div().flex().flex_none().px(u(14.0)).py(u(8.0)).child(
                    Button::new("more-results")
                        .label(if panel.reading_more {
                            text::READING_MORE_RESULTS
                        } else {
                            text::SHOW_MORE_RESULTS
                        })
                        .disabled(panel.reading_more || !panel.can_read_more)
                        .selector("more-results")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.read_more_results(cx));
                        })),
                );
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        ScrollArea::new("search-results", &self.search_scroll, text::SEARCH)
                            .selector("search-results")
                            .children(rows)
                            .child(div().flex_none().h(u(6.0))),
                    )
                    .child(more)
                    .into_any_element()
            }
        };
        side_panel(
            &theme,
            text::SEARCH,
            "search-panel",
            self.panel_width(
                junto_client_core::settings::Panel::Search,
                theme.metrics.side_panel_width,
                cx,
            ),
        )
        .child(header)
        .child(filters)
        .child(
            crate::ui::notice_slot("search-go-failed-slot")
                .children(go_failed)
                .children(archive_failed),
        )
        .child(body)
        .into_any_element()
    }
}

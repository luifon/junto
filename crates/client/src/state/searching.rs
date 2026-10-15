//! What the application does for a search (ADR 0743): it asks the server for the pages of
//! the search of the window, and reads the message of a result for the jump to it.

use gpui::Context;
use junto_client_core::api::{ApiError, ErrorCategory, SearchQuery};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{MessageId, ServerId};
use time::OffsetDateTime;

use super::AppState;
use super::notices::Jump;
use super::search::{Filters, Hit, MOST_HITS, Progress, Search};
use super::view::{ServerSnapshot, shown_model};
use crate::text;

/// The longest text of a search in bytes (ADR 0409).
const MOST_QUERY_BYTES: usize = 1_024;

/// How a failed request of a search is told: a refusal with its reason, or no answer.
fn failure(error: &ApiError) -> Progress {
    if error.is_unavailable() {
        return Progress::Failed(text::flow_error(&FlowError::from(error)));
    }
    Progress::Refused(match error.code() {
        Some(ErrorCode::InvalidRequest) => text::SEARCH_TEXT_REFUSED.to_owned(),
        _ => text::flow_error(&FlowError::from(error)),
    })
}

impl AppState {
    /// Searches a server for a text, with the filters of the search of before. The side
    /// panel shows the search in place of a thread, the pinned messages and the notices.
    pub fn search(&mut self, server_id: ServerId, text: &str, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            self.close_search(cx);
            return;
        }
        let filters = self
            .known
            .search
            .as_ref()
            .filter(|search| search.server_id == server_id)
            .map(|search| search.filters)
            .unwrap_or_default();
        self.start_search(server_id, text, filters, cx);
    }

    /// Changes the filters of the search and runs it again.
    pub fn set_search_filters(&mut self, filters: Filters, cx: &mut Context<Self>) {
        let Some(search) = &self.known.search else {
            return;
        };
        let (server_id, text) = (search.server_id, search.text.clone());
        self.start_search(server_id, text, filters, cx);
    }

    /// Runs the search of the window again from its first page, after a failure.
    pub fn search_again(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.known.search else {
            return;
        };
        let (server_id, text, filters) = (search.server_id, search.text.clone(), search.filters);
        self.start_search(server_id, text, filters, cx);
    }

    fn start_search(
        &mut self,
        server_id: ServerId,
        text: String,
        filters: Filters,
        cx: &mut Context<Self>,
    ) {
        self.selection.thread = None;
        self.selection.pins = None;
        self.selection.notices = None;
        self.selection.transcript = None;
        self.known.transcript_line = None;
        self.known.fork = None;
        // The results of the same text stay on screen while its next answer is on its
        // way; another text starts with an empty panel.
        let held = self
            .known
            .search
            .take()
            .filter(|search| search.server_id == server_id && search.text == text);
        self.known.search = Some(Search {
            server_id,
            text,
            filters,
            progress: Progress::Searching,
            hits: held.map(|search| search.hits).unwrap_or_default(),
            more: false,
            reading_more: false,
            open: true,
            going: None,
            go_failed: None,
        });
        self.search_next = None;
        self.ask_search(false, cx);
        self.changed(cx);
    }

    /// Reads the first page of the search of the window again and changes nothing else
    /// of the window: the side panel, a jump that is on its way and the filters stay.
    /// The read results stay on screen until the answer replaces them.
    fn read_search_again(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.known.search else {
            return;
        };
        search.progress = Progress::Searching;
        search.more = false;
        search.reading_more = false;
        self.search_next = None;
        self.ask_search(false, cx);
    }

    /// Reads the page after the read ones.
    pub fn read_more_results(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.known.search else {
            return;
        };
        if !search.more
            || search.hits.len() >= MOST_HITS
            || search.reading_more
            || search.progress != Progress::Done
        {
            return;
        }
        search.reading_more = true;
        self.ask_search(true, cx);
        self.changed(cx);
    }

    /// Sends the request of the search of the window: its first page, or the page after
    /// the read ones. An answer is used only while its request is the last one.
    fn ask_search(&mut self, more: bool, cx: &mut Context<Self>) {
        self.search_asks += 1;
        let number = self.search_asks;
        let Some(search) = &mut self.known.search else {
            return;
        };
        if search.text.len() > MOST_QUERY_BYTES {
            search.progress = Progress::Refused(text::SEARCH_TEXT_TOO_LONG.to_owned());
            search.hits.clear();
            return;
        }
        let Some(server) = self.client.server(search.server_id) else {
            self.known.search = None;
            return;
        };
        let query = SearchQuery {
            text: search.text.clone(),
            channel_id: search.filters.channel,
            author_id: search.filters.author,
            from: search
                .filters
                .date
                .from(OffsetDateTime::now_utc(), self.known.zone),
            until: None,
            limit: None,
            cursor: if more { self.search_next.clone() } else { None },
        };
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let answered = api.search(&query).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_asks != number {
                    return;
                }
                let Some(search) = &mut this.known.search else {
                    return;
                };
                search.reading_more = false;
                match answered {
                    Ok(page) => {
                        if !more {
                            search.hits.clear();
                        }
                        for hit in page.results {
                            let hit = Hit::from(hit);
                            if search.hits.len() < MOST_HITS && !search.hits.contains(&hit) {
                                search.hits.push(hit);
                            }
                        }
                        search.more = page.next.is_some();
                        search.progress = Progress::Done;
                        this.search_next = page.next;
                    }
                    Err(error) => {
                        // A further page that failed leaves the read pages.
                        if more {
                            search.go_failed = Some(text::flow_error(&FlowError::from(&error)));
                        } else {
                            search.hits.clear();
                            search.more = false;
                            search.progress = failure(&error);
                        }
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
    }

    /// Takes the search out of the side panel. Its results stay until the next search.
    pub fn hide_search(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.known.search.as_mut().filter(|search| search.open) {
            search.open = false;
            self.changed(cx);
        }
    }

    /// Ends the search of the window.
    pub fn close_search(&mut self, cx: &mut Context<Self>) {
        if self.known.search.take().is_some() {
            self.search_asks += 1;
            self.search_next = None;
            self.changed(cx);
        }
    }

    /// The search belongs to a server: it ends when another server is shown.
    pub(super) fn end_search_of_other_server(&mut self, shown: Option<ServerId>) {
        if self
            .known
            .search
            .as_ref()
            .is_some_and(|search| Some(search.server_id) != shown)
        {
            self.known.search = None;
            self.search_asks += 1;
            self.search_next = None;
        }
    }

    /// The state of a server was replaced: the first page of its search is read again,
    /// because each page is decided by what the account can read at the time of its
    /// request (ADR 0409). A search that the side panel does not show sends no request:
    /// its results are dropped, and the next search of its text reads them.
    pub(super) fn search_after_reload(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let Some(search) = self
            .known
            .search
            .as_mut()
            .filter(|search| search.server_id == server_id)
        else {
            return;
        };
        if !matches!(search.progress, Progress::Done | Progress::Searching) {
            return;
        }
        if search.open {
            self.read_search_again(cx);
        } else {
            search.hits.clear();
            search.more = false;
            search.reading_more = false;
            search.progress = Progress::Done;
            self.search_asks += 1;
            self.search_next = None;
        }
    }

    /// What the account may see on the server of the search changed (ADR 0748): the
    /// results leave at once, the answer of a read that started before is not taken, and
    /// the search is read again as after a reload.
    pub(super) fn search_after_permission_change(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = self
            .known
            .search
            .as_mut()
            .filter(|search| search.server_id == server_id)
        else {
            return;
        };
        search.hits.clear();
        search.more = false;
        search.reading_more = false;
        self.search_asks += 1;
        self.search_next = None;
        self.search_after_reload(server_id, cx);
    }

    /// The channel of the filter left the state of its server: the filter is taken
    /// away, and the search is read again without it.
    pub(super) fn drop_lost_search_filter(
        &mut self,
        servers: &[ServerSnapshot],
        cx: &mut Context<Self>,
    ) {
        let Some(search) = &mut self.known.search else {
            return;
        };
        let Some(channel_id) = search.filters.channel else {
            return;
        };
        let held = servers
            .iter()
            .find(|server| server.id == search.server_id)
            .and_then(shown_model)
            .is_none_or(|model| model.channel(channel_id).is_some());
        if held {
            return;
        }
        search.filters.channel = None;
        if search.open {
            self.read_search_again(cx);
        }
    }

    /// Goes to the message of a result. The result names its message and its place; the
    /// position and the thread of the message are read first. The person who goes to
    /// another place before the answer stays there.
    pub fn go_to_result(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(search) = &mut self.known.search else {
            return;
        };
        let server_id = search.server_id;
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        search.going = Some(message_id);
        search.go_failed = None;
        self.result_asks += 1;
        let number = self.result_asks;
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let read = api.message(message_id).await;
            let _ = this.update(cx, |this, cx| {
                this.result_answers += 1;
                // Only the last read counts, and only for the search that asked for it.
                if this.result_asks != number {
                    return;
                }
                let Some(search) = this.known.search.as_mut().filter(|search| {
                    search.server_id == server_id && search.going == Some(message_id)
                }) else {
                    return;
                };
                search.going = None;
                match read {
                    // A server is not trusted to answer for the message that was asked
                    // for (ADR 0714).
                    Ok(message) if message.id != message_id => {
                        search.go_failed = Some(text::RESULT_GONE.to_owned());
                        this.changed(cx);
                    }
                    Ok(message) => {
                        let jump = Jump::to(server_id, &message);
                        this.go_to(jump, cx);
                    }
                    Err(error) => {
                        if error.category() == Some(ErrorCategory::NotFound) {
                            // The message is gone, or the account can no longer read it.
                            search.hits.retain(|hit| hit.message_id != Some(message_id));
                            search.go_failed = Some(text::RESULT_GONE.to_owned());
                        } else {
                            search.go_failed = Some(text::flow_error(&FlowError::from(&error)));
                        }
                        this.changed(cx);
                    }
                }
            });
        })
        .detach();
        self.changed(cx);
    }

    /// How many reads of the message of a search result have ended. A test waits for
    /// the end of a read with it.
    #[cfg(feature = "test-support")]
    pub fn result_answers(&self) -> u64 {
        self.result_answers
    }

    pub fn dismiss_search_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = &mut self.known.search {
            search.go_failed = None;
            self.changed(cx);
        }
    }

    /// Draws this search in place of what a server answered. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_search_fixture(&mut self, search: Option<Search>, cx: &mut Context<Self>) {
        self.known.search = search;
        self.changed(cx);
    }
}

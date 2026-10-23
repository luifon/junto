//! The variable-height list of an administration page: it lays out the rows in view and
//! keeps the keys, the focus and the scroll position of the rows by their key (ADR 0761).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, ElementId, FocusHandle, Global, Hsla, InteractiveElement,
    IntoElement, KeyBinding, ListAlignment, ListOffset, ListState, ParentElement, Pixels, Role,
    ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Window,
    canvas, div, list, prelude::FluentBuilder, px,
};
use gpui_base::{Scrollbar, ScrollbarMode};
use junto_client_core::protocol::ids::AccountId;

use crate::shell::Shell;
use crate::state::admin::view::{AdminView, Scope};
use crate::theme::{theme, u};

gpui::actions!(
    admin_list,
    [
        NextControl,
        PreviousControl,
        NextRow,
        PreviousRow,
        PageUp,
        PageDown,
        FirstRow,
        LastRow
    ]
);

/// The height of a row that was not laid out yet, in pixels of the design. The scroll
/// bar and the reveal of a distant row use it until the row has its real height.
const ESTIMATE: f32 = 40.0;

/// The frames in which a row that the keyboard asked for is revealed again until it is
/// drawn in view. The estimate can place it outside the view the first time.
const ATTEMPTS: u8 = 8;

struct Bindings;
impl Global for Bindings {}

pub(crate) fn init(cx: &mut App) {
    if cx.try_global::<Bindings>().is_none() {
        cx.set_global(Bindings);
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("AdminList")),
            KeyBinding::new("shift-tab", PreviousControl, Some("AdminList")),
            KeyBinding::new("down", NextRow, Some("AdminList")),
            KeyBinding::new("up", PreviousRow, Some("AdminList")),
            KeyBinding::new("pageup", PageUp, Some("AdminListArea")),
            KeyBinding::new("pagedown", PageDown, Some("AdminListArea")),
            KeyBinding::new("home", FirstRow, Some("AdminListArea")),
            KeyBinding::new("end", LastRow, Some("AdminListArea")),
        ]);
    }
}

/// A view that draws one [`PageList`].
pub(crate) trait ListHost: Sized + 'static {
    fn page_list(&mut self) -> &mut PageList;

    /// The accounts of the rows that the list laid out, in the order of the rows, after a
    /// frame in which they changed.
    fn accounts_laid_out(&mut self, _accounts: Vec<AccountId>, _cx: &mut Context<Self>) {}

    /// Moves the focus from the focused element to the next or the previous tab stop
    /// outside the list, as Tab and Shift+Tab do.
    fn leave_list(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
    }
}

impl ListHost for Shell {
    fn page_list(&mut self) -> &mut PageList {
        &mut self.admin.list
    }

    /// The window reads the profiles of the accounts that the page draws (ADR 0783).
    fn accounts_laid_out(&mut self, accounts: Vec<AccountId>, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.admin_accounts_laid_out(accounts, cx));
    }

    /// The layer keeps the focus inside it: the Tab action of the root of the window
    /// moves the focus as it does from any control of the layer.
    fn leave_list(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let name = if forward {
            "root::Tab"
        } else {
            "root::TabPrev"
        };
        match cx.build_action(name, None) {
            Ok(action) => window.dispatch_action(action, cx),
            Err(_) if forward => window.focus_next(cx),
            Err(_) => window.focus_prev(cx),
        }
    }
}

/// One row of a list, as the page states it. The key stays with the row while the list
/// changes. `interactive` is false only for a row that holds no control that takes the
/// keyboard focus; the page derives it from the data that builds the controls of the row.
pub(crate) struct Row {
    key: String,
    interactive: bool,
    item: bool,
    account: Option<AccountId>,
}

impl Row {
    /// A row that is an item of the list for assistive technology.
    pub(crate) fn new(key: impl Into<String>, interactive: bool) -> Self {
        Self {
            key: key.into(),
            interactive,
            item: true,
            account: None,
        }
    }

    /// The row draws this account with its picture.
    pub(crate) fn account(mut self, account_id: AccountId) -> Self {
        self.account = Some(account_id);
        self
    }

    /// A part of the page in the list, such as its header or its footer, which is no item
    /// of the list for assistive technology.
    pub(crate) fn part(mut self) -> Self {
        self.item = false;
        self
    }
}

#[derive(Clone)]
struct Entry {
    key: SharedString,
    interactive: bool,
    /// The place of the row among the items of the list, from 1.
    position: Option<usize>,
    /// The element of the row. Not a tab stop: Tab reaches the controls in the row.
    focus: FocusHandle,
    /// An empty element at the end of the row, from which Shift+Tab reaches the last
    /// control of the row.
    end: FocusHandle,
    account: Option<AccountId>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    /// The first control of the row.
    First,
    /// The last control of the row.
    Last,
    /// The element of the row.
    Row,
    /// No move: the row that holds the focus is shown.
    Keep,
}

/// A row that the keyboard moves the focus to, once the row is drawn in view.
struct Pending {
    key: SharedString,
    target: Target,
    /// The focus when the move was asked for. A focus that changed in the meantime ends
    /// the move.
    from: Option<FocusHandle>,
    frames: u8,
}

/// What the list measures in one frame and uses before the next.
#[derive(Default)]
struct Frame {
    /// The width of the list and the rem size for which the estimate was applied.
    measured: Cell<Option<(Pixels, Pixels)>>,
    wheel: Cell<Wheel>,
    /// The rows laid out in the frame.
    laid_out: Cell<usize>,
    /// The accounts of the rows laid out in the frame, each with the place of its row, and
    /// those that the host was told.
    accounts: RefCell<Vec<(usize, AccountId)>>,
    told: RefCell<Vec<AccountId>>,
}

/// The wheel steps in lines since the list was drawn, from the scroll position at which
/// it was drawn.
#[derive(Clone, Copy, Default)]
struct Wheel {
    top: ListOffset,
    lines: f32,
    line_height: Pixels,
}

pub(crate) struct PageList {
    pub(crate) state: ListState,
    pub(crate) drawn: bool,
    pub(crate) requested_end: bool,
    rows: Rc<Vec<Entry>>,
    items: usize,
    pending: Option<Pending>,
    view: Option<AdminView>,
    /// The element that holds the list, its entries and its scroll bar.
    area: FocusHandle,
    /// The tab stops before and after the rows. They are drawn while the focus is outside
    /// the rows of a list that has rows, so that Tab and Shift+Tab enter the list at a row
    /// in view, also when no row in view has a control.
    start: FocusHandle,
    finish: FocusHandle,
    frame: Rc<Frame>,
    /// The tab stop before (true) or after (false) the rows that the focus reached and that
    /// the list has answered.
    entered: Option<bool>,
    /// The next change of the rows starts the view at the first item.
    restart: bool,
}

impl PageList {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(800.0)),
            drawn: false,
            requested_end: false,
            rows: Rc::default(),
            items: 0,
            pending: None,
            view: None,
            area: cx.focus_handle().tab_stop(false),
            start: cx.focus_handle().tab_stop(true),
            finish: cx.focus_handle().tab_stop(true),
            frame: Rc::default(),
            entered: None,
            restart: false,
        }
    }

    /// Takes the rows of the page for this frame. A row whose key stays keeps its focus
    /// handle and its measured height. Each run of removed or added rows is replaced on its
    /// own, and the view keeps the first row in view, or its place (ADR 0761).
    pub(crate) fn take(
        &mut self,
        view: Option<&AdminView>,
        rows: Vec<Row>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let unchanged = self.rows.len() == rows.len()
            && self.rows.iter().zip(&rows).all(|(entry, row)| {
                entry.key == row.key
                    && entry.interactive == row.interactive
                    && entry.position.is_some() == row.item
                    && entry.account == row.account
            });
        if !unchanged {
            self.replace(rows, window, cx);
            if std::mem::take(&mut self.restart)
                && let Some(first) = self.rows.iter().position(|entry| entry.position.is_some())
            {
                self.state.scroll_to(ListOffset {
                    item_ix: first,
                    offset_in_item: px(0.0),
                });
            }
        }
        if self.view.as_ref() != view {
            self.state.remeasure_items(0..self.rows.len());
            self.view = view.cloned();
        }
        if self.requested_end {
            self.state.scroll_to_end();
            self.requested_end = false;
        }
    }

    fn replace(&mut self, rows: Vec<Row>, window: &mut Window, cx: &mut App) {
        let old = Rc::clone(&self.rows);
        let at: HashMap<&str, usize> = old
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.key.as_ref(), index))
            .collect();
        let found: Vec<Option<usize>> = rows
            .iter()
            .map(|row| at.get(row.key.as_str()).copied())
            .collect();
        let kept = kept_in_place(&found);
        let mut handles: HashMap<&str, (FocusHandle, FocusHandle)> = old
            .iter()
            .map(|entry| (entry.key.as_ref(), (entry.focus.clone(), entry.end.clone())))
            .collect();
        let mut position = 0;
        let entries: Vec<Entry> = rows
            .into_iter()
            .map(|row| {
                let (focus, end) = handles.remove(row.key.as_str()).unwrap_or_else(|| {
                    (
                        cx.focus_handle().tab_stop(false),
                        cx.focus_handle().tab_stop(false),
                    )
                });
                position += usize::from(row.item);
                Entry {
                    key: row.key.into(),
                    interactive: row.interactive,
                    position: row.item.then_some(position),
                    focus,
                    end,
                    account: row.account,
                }
            })
            .collect();

        // The new place of each old row that stays in place.
        let mut moved_to = vec![None; old.len()];
        for &(before, after) in &kept {
            moved_to[before] = Some(after);
        }

        // Each run between two rows that stay is replaced, the last run first, so that the
        // places of the runs before it stay valid.
        let mut bounds = vec![(0, 0)];
        bounds.extend(kept.iter().map(|&(before, after)| (before + 1, after + 1)));
        let ends = kept
            .iter()
            .copied()
            .chain(std::iter::once((old.len(), entries.len())));
        let runs: Vec<_> = bounds
            .into_iter()
            .zip(ends)
            .filter(|((old_start, new_start), (old_end, new_end))| {
                old_start != old_end || new_start != new_end
            })
            .collect();

        // Where the view goes, read before the change: the first row in view stays at its
        // place when it stays in place. When it is removed, the row after the removed rows
        // stays where it was in view; when rows replace it, the first of them goes to the
        // top of the view.
        let viewport = self.state.viewport_bounds();
        let shown = |index: usize| {
            self.state
                .bounds_for_item(index)
                .filter(|bounds| bounds.intersects(&viewport))
        };
        let top = self.state.logical_scroll_top();
        let anchor = if top.item_ix >= old.len() {
            None
        } else if let Some(after) = moved_to[top.item_ix] {
            Some((
                ListOffset {
                    item_ix: after,
                    offset_in_item: top.offset_in_item,
                },
                px(0.0),
            ))
        } else {
            runs.iter()
                .find(|((old_start, _), (old_end, _))| {
                    (*old_start..*old_end).contains(&top.item_ix)
                })
                .map(|&((_, new_start), (old_end, new_end))| {
                    let next = (new_start == new_end && old_end < old.len())
                        .then(|| shown(old_end))
                        .flatten();
                    let above = next.map_or(px(0.0), |bounds| bounds.top() - viewport.top());
                    (
                        ListOffset {
                            item_ix: new_start,
                            offset_in_item: px(0.0),
                        },
                        above,
                    )
                })
        };
        // A focused row in view that moves, such as by a move with the keyboard, is shown
        // after the change. A focused row out of view stays out of view.
        let focused = old
            .iter()
            .position(|entry| entry.focus.contains_focused(window, cx))
            .filter(|&index| shown(index).is_some())
            .and_then(|index| {
                let after = found.iter().position(|&before| before == Some(index))?;
                (moved_to[index] != Some(index)).then_some(after)
            });

        for &((old_start, new_start), (old_end, new_end)) in runs.iter().rev() {
            self.state.splice_focusable(
                old_start..old_end,
                entries[new_start..new_end]
                    .iter()
                    .map(|entry| Some(entry.focus.clone())),
            );
        }
        if !runs.is_empty() {
            if let Some((_, rem)) = self.frame.measured.get() {
                self.estimate(rem);
            }
            if let Some((offset, above)) = anchor {
                self.state.scroll_to(offset);
                self.state.scroll_by(-above);
            }
        }
        self.items = position;
        self.rows = Rc::new(entries);
        if let Some(index) = focused
            && self.pending.is_none()
        {
            let from = window.focused(cx);
            self.pend(index, Target::Keep, from);
        }
    }

    /// Gives every row that was not laid out the estimated height at this rem size.
    fn estimate(&self, rem: Pixels) {
        let _ = self
            .state
            .clone()
            .with_uniform_item_height(u(ESTIMATE).to_pixels(rem));
    }

    fn index(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|entry| entry.key.as_ref() == key)
    }

    fn next_interactive(&self, index: usize, forward: bool) -> Option<usize> {
        if forward {
            (index + 1..self.rows.len()).find(|&index| self.rows[index].interactive)
        } else {
            (0..index).rev().find(|&index| self.rows[index].interactive)
        }
    }

    /// The places of the rows that the last frame drew in the view: those that it drew
    /// whole, and those that it drew whole or in part.
    fn in_view(&self) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let viewport = self.state.viewport_bounds();
        let first = self.state.logical_scroll_top().item_ix;
        let end = (first..self.rows.len())
            .take_while(|&index| {
                self.state
                    .bounds_for_item(index)
                    .is_some_and(|bounds| bounds.top() < viewport.bottom())
            })
            .last()
            .map_or(first, |last| last + 1);
        let whole = |index: &usize| {
            self.state.bounds_for_item(*index).is_some_and(|bounds| {
                bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom()
            })
        };
        let drawn_whole = match ((first..end).find(whole), (first..end).rev().find(whole)) {
            (Some(start), Some(last)) => start..last + 1,
            _ => end..end,
        };
        (drawn_whole, first..end)
    }

    /// The first (`forward`) or the last row that `fits`: among the rows that the view shows
    /// whole, then among the rows that it shows in part, and otherwise in the whole list. A
    /// move of the focus to a row that the view cuts scrolls the view (ADR 0761).
    fn first_fitting(&self, forward: bool, fits: impl Fn(&Entry) -> bool) -> Option<usize> {
        let (whole, shown) = self.in_view();
        let find = |range: std::ops::Range<usize>| {
            let range = range.start.min(self.rows.len())..range.end.min(self.rows.len());
            let rows = &self.rows[range.clone()];
            if forward {
                rows.iter().position(&fits)
            } else {
                rows.iter().rposition(&fits)
            }
            .map(|index| range.start + index)
        };
        find(whole)
            .or_else(|| find(shown))
            .or_else(|| find(0..self.rows.len()))
    }

    /// Moves the focus to a row when it is drawn in view, and reveals it now.
    fn pend(&mut self, index: usize, target: Target, from: Option<FocusHandle>) {
        self.state.scroll_to_reveal_item(index);
        self.pending = Some(Pending {
            key: self.rows[index].key.clone(),
            target,
            from,
            frames: ATTEMPTS,
        });
    }

    /// The next change of the rows starts the view at the first item of the list, such as
    /// after the person reads the list again. [`Self::end_restart`] ends the request when
    /// the read ended without a change.
    pub(crate) fn start_at_first_item(&mut self) {
        self.restart = true;
    }

    pub(crate) fn end_restart(&mut self) {
        self.restart = false;
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn focus_first(&mut self, window: &Window, cx: &App) {
        if let Some(index) = (0..self.rows.len()).find(|&index| self.rows[index].interactive) {
            self.pend(index, Target::First, window.focused(cx));
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn laid_out(&self) -> usize {
        self.frame.laid_out.get()
    }

    #[cfg(test)]
    fn row_focus(&self, key: &str) -> Option<FocusHandle> {
        Some(self.rows[self.index(key)?].focus.clone())
    }
}

/// The longest series of rows that keep their order: the pairs of old and new places of
/// the rows that stay in place. `found` holds the old place of each new row.
fn kept_in_place(found: &[Option<usize>]) -> Vec<(usize, usize)> {
    // The ends of the longest increasing series of each length, as places in `found`.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; found.len()];
    for (after, before) in found.iter().enumerate() {
        let Some(before) = *before else { continue };
        let length = tails.partition_point(|&tail| found[tail] < Some(before));
        previous[after] = length.checked_sub(1).map(|length| tails[length]);
        if length == tails.len() {
            tails.push(after);
        } else {
            tails[length] = after;
        }
    }
    let mut kept = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(after) = at {
        if let Some(before) = found[after] {
            kept.push((before, after));
        }
        at = previous[after];
    }
    kept.reverse();
    kept
}

/// Whether the focus is on a control in the row, not on the row or its end.
fn inside(entry: &Entry, window: &Window, cx: &App) -> bool {
    entry.focus.contains_focused(window, cx)
        && !entry.focus.is_focused(window)
        && !entry.end.is_focused(window)
}

fn restore(from: Option<&FocusHandle>, window: &mut Window, cx: &mut App) {
    match from {
        Some(handle) => window.focus(handle, cx),
        None => window.blur(cx),
    }
}

/// Tab or Shift+Tab in the row with `key`.
fn traverse<V: ListHost>(
    host: &mut V,
    key: &str,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let list = host.page_list();
    let Some(index) = list.index(key) else {
        cx.propagate();
        return;
    };
    let entry = list.rows[index].clone();
    let from = window.focused(cx);
    if forward {
        window.focus_next(cx);
    } else {
        window.focus_prev(cx);
    }
    if inside(&entry, window, cx) {
        return;
    }
    match list.next_interactive(index, forward) {
        Some(next) if inside(&list.rows[next], window, cx) => {}
        Some(next) => {
            restore(from.as_ref(), window, cx);
            let target = if forward { Target::First } else { Target::Last };
            list.pend(next, target, from);
            cx.notify();
        }
        // The focus leaves the list as for any other control.
        None => {
            restore(from.as_ref(), window, cx);
            cx.propagate();
        }
    }
}

/// Down or Up on the focused element of a row.
fn step<V: ListHost>(
    host: &mut V,
    key: &str,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let list = host.page_list();
    let Some(index) = list.index(key) else {
        cx.propagate();
        return;
    };
    if !list.rows[index].focus.is_focused(window) {
        cx.propagate();
        return;
    }
    let next = if forward {
        Some(index + 1).filter(|&next| next < list.rows.len())
    } else {
        index.checked_sub(1)
    };
    if let Some(next) = next {
        list.pend(next, Target::Row, window.focused(cx));
        cx.notify();
    }
}

/// The focus is on the tab stop before or after the rows: it goes on to the first or the
/// last row in view that can hold a control, or, when no row in view can, to the first or
/// the last such row of the list. Answered once for each arrival of the focus.
fn enter(list: &mut PageList, window: &Window, cx: &App) {
    let reached = if list.start.is_focused(window) {
        Some(true)
    } else if list.finish.is_focused(window) {
        Some(false)
    } else {
        None
    };
    if reached == list.entered {
        return;
    }
    list.entered = reached;
    let Some(forward) = reached else { return };
    let target = if forward { Target::First } else { Target::Last };
    // Without a row that can hold a control, the focus goes to an item of the list itself,
    // by the same rule, from which the keys scroll the list.
    let found = match list.first_fitting(forward, |entry| entry.interactive) {
        Some(index) => Some((index, target)),
        None => list
            .first_fitting(forward, |entry| entry.position.is_some())
            .or_else(|| {
                if forward {
                    (!list.rows.is_empty()).then_some(0)
                } else {
                    list.rows.len().checked_sub(1)
                }
            })
            .map(|index| (index, Target::Row)),
    };
    if let Some((index, target)) = found {
        list.pend(index, target, window.focused(cx));
    }
}

/// Continues a move of the focus after the frame in which its row was drawn, or not.
fn resume<V: ListHost>(host: &mut V, painted: bool, window: &mut Window, cx: &mut Context<V>) {
    let list = host.page_list();
    let Some(pending) = list.pending.take() else {
        return;
    };
    let Some(index) = list.index(&pending.key) else {
        return;
    };
    if window.focused(cx) != pending.from {
        return;
    }
    if !painted {
        if let Some(frames) = pending.frames.checked_sub(1) {
            list.state.scroll_to_reveal_item(index);
            list.pending = Some(Pending { frames, ..pending });
            cx.notify();
            return;
        }
        // The row was not drawn in the view. From a tab stop before or after the rows,
        // the move leaves the list, so that the focus does not stay on an empty element.
        let forward = if pending.from.as_ref() == Some(&list.start) {
            true
        } else if pending.from.as_ref() == Some(&list.finish) {
            false
        } else {
            return;
        };
        let edge = if forward { &list.finish } else { &list.start };
        window.focus(edge, cx);
        host.leave_list(forward, window, cx);
        return;
    }
    let entry = list.rows[index].clone();
    // The row is laid out now: its real height reveals it whole, where its estimate may
    // have left a part of it outside the view.
    list.state.scroll_to_reveal_item(index);
    cx.notify();
    let forward = match pending.target {
        Target::Keep => return,
        Target::Row => {
            window.focus(&entry.focus, cx);
            return;
        }
        Target::First => {
            window.focus(&entry.focus, cx);
            window.focus_next(cx);
            true
        }
        Target::Last => {
            window.focus(&entry.end, cx);
            window.focus_prev(cx);
            false
        }
    };
    if inside(&entry, window, cx) {
        return;
    }
    // The row holds no control that takes the focus now, such as when all of them are
    // disabled: the move goes on to the next row, or leaves the list.
    restore(pending.from.as_ref(), window, cx);
    if let Some(next) = list.next_interactive(index, forward) {
        list.pend(next, pending.target, pending.from);
        cx.notify();
        return;
    }
    // From a tab stop before or after the rows, the move leaves from the other one, so
    // that the focus does not return to the rows.
    if pending.from.as_ref() == Some(&list.start) || pending.from.as_ref() == Some(&list.finish) {
        let edge = if forward { &list.finish } else { &list.start };
        window.focus(edge, cx);
    }
    host.leave_list(forward, window, cx);
}

/// The element of the list: the rows in view, drawn by `render`, with the scroll bar.
pub(crate) fn element<V: ListHost>(
    host: &mut V,
    label: SharedString,
    render: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let entity = cx.entity().downgrade();
    let view = cx.entity_id();
    let page = host.page_list();
    page.drawn = true;
    enter(page, window, cx);
    let state = page.state.clone();
    let rows = Rc::clone(&page.rows);
    let handles: Vec<FocusHandle> = rows.iter().map(|entry| entry.focus.clone()).collect();
    let items = page.items;
    let frame = Rc::clone(&page.frame);
    frame.laid_out.set(0);
    frame.accounts.borrow_mut().clear();
    let pending = page
        .pending
        .as_ref()
        .and_then(|pending| page.index(&pending.key));
    let in_rows = page.area.contains_focused(window, cx)
        && !page.start.is_focused(window)
        && !page.finish.is_focused(window);
    let entries = !page.rows.is_empty() && !in_rows;
    let (area, start, finish) = (page.area.clone(), page.start.clone(), page.finish.clone());
    let colors = theme(cx).colors;

    let count = rows.len();
    let laid_out = Rc::clone(&frame);
    let render = cx.processor(move |this: &mut V, index: usize, window, cx| {
        laid_out.laid_out.set(laid_out.laid_out.get() + 1);
        let content = render(this, index, window, cx);
        let Some(entry) = rows.get(index) else {
            return content;
        };
        // A row can be laid out twice in a frame, to measure it and to draw it.
        if let Some(account) = entry.account {
            let mut accounts = laid_out.accounts.borrow_mut();
            if !accounts.iter().any(|(_, held)| *held == account) {
                accounts.push((index, account));
            }
        }
        let key = entry.key.clone();
        div()
            .id(ElementId::Name(key.clone()))
            .track_focus(&entry.focus)
            .debug_selector({
                let key = key.clone();
                move || format!("admin-row-{key}")
            })
            .key_context("AdminList")
            .when_some(entry.position, |row, position| {
                row.role(Role::ListItem)
                    .aria_position_in_set(position)
                    .aria_size_of_set(items)
            })
            .on_action({
                let key = key.clone();
                cx.listener(move |this, _: &NextControl, window, cx| {
                    traverse(this, &key, true, window, cx)
                })
            })
            .on_action({
                let key = key.clone();
                cx.listener(move |this, _: &PreviousControl, window, cx| {
                    traverse(this, &key, false, window, cx)
                })
            })
            .on_action({
                let key = key.clone();
                cx.listener(move |this, _: &NextRow, window, cx| step(this, &key, true, window, cx))
            })
            .on_action({
                let key = key.clone();
                cx.listener(move |this, _: &PreviousRow, window, cx| {
                    step(this, &key, false, window, cx)
                })
            })
            .px(u(18.0))
            .pt(if index == 0 { u(14.0) } else { u(0.0) })
            .pb(if index + 1 == count { u(18.0) } else { u(0.0) })
            .child(content)
            .child(
                div()
                    .id(ElementId::Name(format!("{key}-end").into()))
                    .track_focus(&entry.end)
                    .size_0(),
            )
            .into_any_element()
    });

    // Drawn after the list: the estimate after a change of width or rem size, which the
    // list resets, the scroll position for the wheel, and the check of a pending focus.
    let measure = {
        let state = state.clone();
        let shown = state.clone();
        let frame = Rc::clone(&frame);
        let accounts = Rc::clone(&frame);
        canvas(
            move |_, window, _| {
                let width = state.viewport_bounds().size.width;
                let rem = window.rem_size();
                let measured = frame.measured.get();
                if measured != Some((width, rem)) {
                    // At another scale every height differs: each row takes the estimate
                    // at the new scale until it is laid out, at the same place in view.
                    if let Some((_, before)) = measured.filter(|(_, before)| *before != rem) {
                        let top = state.logical_scroll_top();
                        state.splice_focusable(
                            0..state.item_count(),
                            handles.iter().cloned().map(Some),
                        );
                        state.scroll_to(ListOffset {
                            item_ix: top.item_ix,
                            offset_in_item: top.offset_in_item * (rem.as_f32() / before.as_f32()),
                        });
                    }
                    let _ = state
                        .clone()
                        .with_uniform_item_height(u(ESTIMATE).to_pixels(rem));
                    frame.measured.set(Some((width, rem)));
                }
                frame.wheel.set(Wheel {
                    top: state.logical_scroll_top(),
                    lines: 0.0,
                    line_height: window.line_height(),
                });
            },
            move |_, (), window, cx| {
                // The host learns the accounts of the drawn rows after the frame, so that a
                // change of its state does not happen while the window draws. The list lays
                // out rows above the first row in view, and at the end of the list, upward.
                let mut rows = accounts.accounts.borrow().clone();
                rows.sort_unstable_by_key(|(index, _)| *index);
                let laid_out: Vec<AccountId> =
                    rows.into_iter().map(|(_, account)| account).collect();
                if *accounts.told.borrow() != laid_out {
                    accounts.told.replace(laid_out.clone());
                    let host = entity.clone();
                    cx.defer(move |cx| {
                        let _ = host.update(cx, |host, cx| host.accounts_laid_out(laid_out, cx));
                    });
                }
                let Some(index) = pending else { return };
                let viewport = shown.viewport_bounds();
                let painted = shown
                    .bounds_for_item(index)
                    .is_some_and(|bounds| bounds.intersects(&viewport));
                window.on_next_frame(move |window, cx| {
                    let _ = entity.update(cx, |host, cx| resume(host, painted, window, cx));
                });
            },
        )
        .absolute()
        .size_0()
    };

    // The list scrolls a wheel step in lines by 20 pixels; the line height of the list
    // replaces that distance, as in other scrolled elements.
    let wheel = {
        let state = state.clone();
        let frame = Rc::clone(&frame);
        move |event: &ScrollWheelEvent, _: &mut Window, cx: &mut App| {
            let mut wheel = frame.wheel.get();
            match event.delta {
                ScrollDelta::Lines(lines) => {
                    wheel.lines += lines.y;
                    frame.wheel.set(wheel);
                    state.scroll_to(wheel.top);
                    state.scroll_by(-wheel.line_height * wheel.lines);
                    cx.notify(view);
                }
                ScrollDelta::Pixels(_) => {
                    wheel.lines = 0.0;
                    frame.wheel.set(wheel);
                }
            }
        }
    };

    // A tab stop holds the focus until the list moves it on: it carries the name of the
    // list.
    let named = label.clone();
    let entry = |id: &'static str, handle: FocusHandle| {
        entries.then(|| {
            div()
                .id(id)
                .role(Role::Group)
                .aria_label(named.clone())
                .track_focus(&handle)
                .size_0()
        })
    };
    div()
        .id("admin-list-area")
        .key_context("AdminListArea")
        .track_focus(&area)
        .on_action({
            let state = state.clone();
            move |_: &PageUp, _, cx| {
                state.scroll_by(-state.viewport_bounds().size.height * 0.8);
                cx.notify(view);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &PageDown, _, cx| {
                state.scroll_by(state.viewport_bounds().size.height * 0.8);
                cx.notify(view);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &FirstRow, _, cx| {
                state.scroll_to(ListOffset::default());
                cx.notify(view);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &LastRow, _, cx| {
                state.scroll_to_end();
                cx.notify(view);
            }
        })
        .relative()
        .size_full()
        .children(entry("admin-list-start", start))
        .child(
            div()
                .id("admin-page")
                .role(Role::List)
                .aria_label(label)
                .debug_selector(|| "admin-page".to_owned())
                .size_full()
                .on_scroll_wheel(wheel)
                .child(list(state.clone(), render).size_full()),
        )
        .child(measure)
        .children(entry("admin-list-finish", finish))
        .child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(&state)
                    .viewport_from_layout()
                    .mode(ScrollbarMode::Always)
                    .styles(|styles| {
                        styles
                            .track(|track| track.bg(gpui::transparent_black()).width(px(10.0)))
                            .track_hover(|track| track.bg(gpui::transparent_black()))
                            .track_active(|track| track.bg(gpui::transparent_black()))
                            .thumb(|bar| {
                                bar.bg(Hsla::from(colors.rule))
                                    .width(px(4.0))
                                    .radius(px(2.0))
                                    .inset(px(3.0))
                            })
                            .thumb_hover(|bar| bar.bg(Hsla::from(colors.ink_3)))
                            .thumb_active(|bar| bar.bg(Hsla::from(colors.ink_3)))
                    }),
            ),
        )
        .into_any_element()
}

impl Shell {
    #[cfg(feature = "test-support")]
    pub fn focus_admin_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.admin.list.focus_first(window, cx);
        cx.notify();
    }

    /// The key of the row of the open page of the administration that holds the focused
    /// control.
    #[cfg(feature = "test-support")]
    pub fn admin_focused_row(&self, window: &Window, cx: &App) -> Option<String> {
        self.admin
            .list
            .rows
            .iter()
            .find(|entry| inside(entry, window, cx))
            .map(|entry| entry.key.to_string())
    }

    /// The rows that the open page of the administration laid out in its last frame,
    /// including the rows measured beside the view.
    #[cfg(feature = "test-support")]
    pub fn admin_rows_laid_out(&self) -> usize {
        self.admin.list.laid_out()
    }

    pub(crate) fn render_admin_list(
        &mut self,
        render: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match self.admin.list.view.as_ref().map(|view| &view.scope) {
            Some(Scope::Server { selected, .. }) => crate::text::admin::page_name(*selected),
            Some(Scope::Channel(settings)) => {
                crate::text::admin::channel_page_name(settings.selected)
            }
            None => crate::text::admin::TITLE,
        };
        element(self, label.into(), render, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gpui::{
        Action as _, Entity, KeyBinding, Keystroke, ListOffset, Pixels, Render, ScrollDelta,
        ScrollWheelEvent, TestAppContext, VisualTestContext, point,
    };

    use super::*;

    gpui::actions!(admin_list_test, [Next, Previous, Press]);

    #[derive(Clone)]
    struct Line {
        key: String,
        height: f32,
        control: bool,
        part: bool,
    }

    fn line(index: usize, height: f32, control: bool) -> Line {
        Line {
            key: format!("r{index:04}"),
            height,
            control,
            part: false,
        }
    }

    /// A page with a header, `count` entries whose keys start with `prefix`, and a footer,
    /// as the audit log.
    fn page(prefix: &str, count: usize) -> Vec<Line> {
        let part = |key: &str| Line {
            key: key.to_owned(),
            height: 60.0,
            control: true,
            part: true,
        };
        std::iter::once(part("header"))
            .chain((0..count).map(|index| Line {
                key: format!("{prefix}{index:04}"),
                height: 30.0,
                control: true,
                part: false,
            }))
            .chain(std::iter::once(part("footer")))
            .collect()
    }

    fn selector(name: String) -> &'static str {
        Box::leak(name.into_boxed_str())
    }

    struct Host {
        list: PageList,
        lines: Vec<Line>,
        controls: HashMap<String, FocusHandle>,
        before: FocusHandle,
        after: FocusHandle,
        pressed: Vec<String>,
        focused: Vec<Option<FocusHandle>>,
        width: Pixels,
        height: Pixels,
        /// The accounts that the list told, at each time.
        told: Vec<Vec<AccountId>>,
    }

    impl ListHost for Host {
        fn page_list(&mut self) -> &mut PageList {
            &mut self.list
        }

        fn accounts_laid_out(&mut self, accounts: Vec<AccountId>, _: &mut Context<Self>) {
            self.told.push(accounts);
        }
    }

    /// The account that a row of a test draws.
    fn account(key: &str) -> AccountId {
        let number = key.bytes().fold(1u128, |number, byte| {
            number.wrapping_mul(257).wrapping_add(u128::from(byte))
        });
        AccountId::from_uuid(uuid::Uuid::from_u128(number))
    }

    impl Host {
        fn set(&mut self, lines: Vec<Line>, cx: &mut Context<Self>) {
            for line in lines.iter().filter(|line| line.control) {
                self.controls
                    .entry(line.key.clone())
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            self.lines = lines;
            cx.notify();
        }
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.focused.push(window.focused(cx));
            let rows = self
                .lines
                .iter()
                .map(|line| {
                    let row = Row::new(line.key.clone(), line.control);
                    if line.part {
                        row.part()
                    } else {
                        row.account(account(&line.key))
                    }
                })
                .collect();
            self.list.take(None, rows, window, cx);
            let lines = self.lines.clone();
            let controls = self.controls.clone();
            let list = element(
                self,
                "Rows".into(),
                move |_, index, _, cx| {
                    let line = &lines[index];
                    let mut row = div()
                        .h(px(line.height))
                        .debug_selector(|| format!("row-{}", line.key));
                    if let Some(control) = controls.get(&line.key) {
                        let key = line.key.clone();
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("control-{}", line.key)))
                                .key_context("TestControl")
                                .track_focus(control)
                                .size(px(10.0))
                                .on_action(cx.listener(move |this: &mut Host, _: &Press, _, _| {
                                    this.pressed.push(key.clone());
                                })),
                        );
                    }
                    row.into_any_element()
                },
                window,
                cx,
            );
            div()
                .key_context("TestRoot")
                .on_action(cx.listener(|_, _: &Next, window, cx| window.focus_next(cx)))
                .on_action(cx.listener(|_, _: &Previous, window, cx| window.focus_prev(cx)))
                .size_full()
                .flex()
                .flex_col()
                .line_height(px(32.0))
                .child(div().id("before").track_focus(&self.before).size(px(10.0)))
                .child(div().w(self.width).h(self.height).child(list))
                .child(div().id("after").track_focus(&self.after).size(px(10.0)))
        }
    }

    fn open(cx: &mut TestAppContext, lines: Vec<Line>) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::theme::install(crate::theme::Theme::stone(), cx);
            init(cx);
            cx.bind_keys([
                KeyBinding::new("tab", Next, Some("TestRoot")),
                KeyBinding::new("shift-tab", Previous, Some("TestRoot")),
                KeyBinding::new("enter", Press, Some("TestControl")),
            ]);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let mut host = Host {
                list: PageList::new(cx),
                lines: Vec::new(),
                controls: HashMap::new(),
                before: cx.focus_handle().tab_stop(true),
                after: cx.focus_handle().tab_stop(true),
                pressed: Vec::new(),
                focused: Vec::new(),
                width: px(400.0),
                height: px(300.0),
                told: Vec::new(),
            };
            host.set(lines, cx);
            host
        });
        settle(cx);
        (host, cx)
    }

    /// Runs the application until it has no work, with the frames that it asks for. The
    /// test platform has no display, so a frame that a window asks for is given here.
    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..50 {
            cx.run_until_parked();
            if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
                cx.run_until_parked();
                return;
            }
        }
        panic!("the window asks for frames without end");
    }

    fn press(cx: &mut VisualTestContext, keys: &str) {
        cx.simulate_keystrokes(keys);
        settle(cx);
    }

    fn redraw(host: &Entity<Host>, cx: &mut VisualTestContext) {
        host.update(cx, |_, cx| cx.notify());
        settle(cx);
    }

    fn scroll_to(host: &Entity<Host>, cx: &mut VisualTestContext, item_ix: usize, offset: f32) {
        host.update(cx, |host, _| {
            host.list.state.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(offset),
            })
        });
        redraw(host, cx);
    }

    fn focus(
        host: &Entity<Host>,
        cx: &mut VisualTestContext,
        handle: impl Fn(&Host) -> FocusHandle,
    ) {
        let handle = host.read_with(cx, |host, _| handle(host));
        cx.update(|window, cx| window.focus(&handle, cx));
        settle(cx);
    }

    fn in_view(cx: &mut VisualTestContext, key: &str) -> bool {
        let viewport = cx.debug_bounds("admin-page").unwrap();
        cx.debug_bounds(selector(format!("row-{key}")))
            .is_some_and(|row| row.top() >= viewport.top() && row.bottom() <= viewport.bottom())
    }

    fn pressed_after_enter(host: &Entity<Host>, cx: &mut VisualTestContext) -> Vec<String> {
        host.update(cx, |host, _| host.pressed.clear());
        press(cx, "enter");
        host.read_with(cx, |host, _| host.pressed.clone())
    }

    /// The host learns the accounts of the rows that the list lays out after each frame in
    /// which they changed, and only those (ADR 0783).
    #[gpui::test]
    fn the_host_learns_the_accounts_of_the_rows_that_the_list_lays_out(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines);
        let told = |host: &Entity<Host>, cx: &mut VisualTestContext| {
            host.read_with(cx, |host, _| host.told.clone())
        };
        let first = told(&host, cx).last().cloned().unwrap();
        let laid_out = host.read_with(cx, |host, _| host.list.laid_out());
        assert!(first.contains(&account("r0000")));
        assert!(
            !first.is_empty() && first.len() <= laid_out,
            "{}",
            first.len()
        );
        assert!(!first.contains(&account("r1999")));

        // A frame that lays out the same rows tells nothing.
        let times = told(&host, cx).len();
        redraw(&host, cx);
        assert_eq!(told(&host, cx).len(), times);

        scroll_to(&host, cx, 1500, 0.0);
        let after = told(&host, cx).last().cloned().unwrap();
        assert_eq!(told(&host, cx).len(), times + 1);
        assert!(after.contains(&account("r1500")));
        assert!(!after.contains(&account("r0000")));
    }

    /// The host learns the accounts of the laid-out rows in the order of the rows, also
    /// where the list lays them out in another order: the rows above the first row in
    /// view, and the rows that fill the view upward from the end of the list (ADR 0783).
    #[gpui::test]
    fn the_host_learns_the_laid_out_accounts_in_the_order_of_the_rows(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, true)).collect();
        let places: HashMap<AccountId, usize> = lines
            .iter()
            .enumerate()
            .map(|(place, line)| (account(&line.key), place))
            .collect();
        let (host, cx) = open(cx, lines);
        for (item_ix, offset) in [(1999, 0.0), (900, 10.0), (0, 0.0)] {
            scroll_to(&host, cx, item_ix, offset);
            let told = host.read_with(cx, |host, _| host.told.last().cloned().unwrap());
            let order: Vec<usize> = told.iter().map(|account| places[account]).collect();
            assert!(
                order.is_sorted() && !order.is_empty(),
                "at {item_ix}: {order:?}"
            );
        }
    }

    /// Tab and Shift+Tab from outside enter the list at the first and the last row that the
    /// view shows whole: the rows that the view cuts at its top and its bottom are not
    /// entered, so the view does not move (ADR 0761).
    #[gpui::test]
    fn tab_enters_the_list_at_the_rows_that_the_view_shows_whole(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines);
        scroll_to(&host, cx, 900, 10.0);
        let before = cx.debug_bounds("row-r0905");
        assert!(before.is_some());
        let cut = |cx: &mut VisualTestContext, key: &str| {
            cx.debug_bounds(selector(format!("row-{key}"))).is_some() && !in_view(cx, key)
        };
        assert!(cut(cx, "r0900") && cut(cx, "r0910"));
        assert!(in_view(cx, "r0901") && in_view(cx, "r0909"));
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0901"]);
        assert_eq!(cx.debug_bounds("row-r0905"), before);
        focus(&host, cx, |host| host.after.clone());
        press(cx, "shift-tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0909"]);
        assert_eq!(cx.debug_bounds("row-r0905"), before);
    }

    /// When no row that the view shows whole can hold a control, Tab and Shift+Tab from
    /// outside enter the list at the rows that the view cuts, which a small scroll shows,
    /// and not at the first or the last row of the list (ADR 0761).
    #[gpui::test]
    fn tab_enters_a_cut_row_before_a_row_outside_the_view(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000)
            .map(|index| line(index, 30.0, [0, 900, 910, 1999].contains(&index)))
            .collect();
        let (host, cx) = open(cx, lines);
        scroll_to(&host, cx, 900, 10.0);
        assert!(in_view(cx, "r0901") && in_view(cx, "r0909") && !in_view(cx, "r0900"));
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0900"]);
        assert!(in_view(cx, "r0905"));
        scroll_to(&host, cx, 900, 10.0);
        focus(&host, cx, |host| host.after.clone());
        press(cx, "shift-tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0910"]);
        assert!(in_view(cx, "r0905"));
    }

    /// A row that is taller than the view is never shown whole: Tab enters the list at the
    /// row that the view shows in part, and not at a row far away.
    #[gpui::test]
    fn tab_enters_a_row_taller_than_the_view_that_the_view_shows(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..20).map(|index| line(index, 500.0, true)).collect();
        let (host, cx) = open(cx, lines);
        scroll_to(&host, cx, 5, 100.0);
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0005"]);
    }

    /// Point 1 of the review: a row above the view that moves elsewhere moves no row in
    /// view, and every row keeps its focus handle.
    #[gpui::test]
    fn a_row_that_moves_from_above_the_view_moves_no_row_in_view(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines.clone());
        scroll_to(&host, cx, 900, 10.0);
        let handles = |host: &Entity<Host>, cx: &mut VisualTestContext| {
            host.read_with(cx, |host, _| {
                ["r0002", "r0900", "r1500"].map(|key| host.list.row_focus(key))
            })
        };
        let before = (cx.debug_bounds("row-r0905"), handles(&host, cx));
        assert!(before.0.is_some());
        let mut moved = lines.clone();
        let renamed = moved.remove(2);
        moved.push(renamed);
        host.update(cx, |host, cx| host.set(moved, cx));
        settle(cx);
        assert_eq!(cx.debug_bounds("row-r0905"), before.0);
        assert_eq!(handles(&host, cx), before.1);
    }

    /// Point 2 of the review: the focused control of a row that a keyboard move takes out
    /// of view keeps the keys, and its row is shown.
    #[gpui::test]
    fn a_focused_row_that_moves_out_of_view_keeps_the_keys(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..200).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines.clone());
        focus(&host, cx, |host| host.controls["r0005"].clone());
        let mut moved = lines.clone();
        let row = moved.remove(5);
        moved.insert(50, row);
        host.update(cx, |host, cx| host.set(moved, cx));
        settle(cx);
        assert_eq!(pressed_after_enter(&host, cx), ["r0005"]);
        assert!(in_view(cx, "r0005"));
    }

    /// Point 4 of the review: Tab and Shift+Tab enter the list from outside when the rows
    /// in view have no control.
    #[gpui::test]
    fn tab_enters_the_list_when_the_rows_in_view_have_no_control(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000)
            .map(|index| line(index, 30.0, index == 0 || index == 1999))
            .collect();
        let (host, cx) = open(cx, lines);
        scroll_to(&host, cx, 900, 0.0);
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r0000"]);
        assert!(in_view(cx, "r0000"));
        scroll_to(&host, cx, 900, 0.0);
        focus(&host, cx, |host| host.after.clone());
        press(cx, "shift-tab");
        assert_eq!(pressed_after_enter(&host, cx), ["r1999"]);
        assert!(in_view(cx, "r1999"));
    }

    /// Point 5 of the review: Tab to a row far away keeps the focus in the list until the
    /// row is drawn in view.
    #[gpui::test]
    fn tab_to_a_row_far_away_keeps_the_focus_in_the_list(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000)
            .map(|index| line(index, 90.0, index == 0 || index == 300))
            .collect();
        let (host, cx) = open(cx, lines);
        focus(&host, cx, |host| host.controls["r0000"].clone());
        host.update(cx, |host, _| host.focused.clear());
        press(cx, "tab");
        let (before, after, focused) = host.read_with(cx, |host, _| {
            (
                host.before.clone(),
                host.after.clone(),
                host.focused.clone(),
            )
        });
        assert!(
            !focused
                .iter()
                .flatten()
                .any(|handle| *handle == before || *handle == after),
            "the focus left the list on its way"
        );
        assert_eq!(pressed_after_enter(&host, cx), ["r0300"]);
        assert!(in_view(cx, "r0300"));
    }

    /// Point 6 of the review: a Tab whose row was not drawn moves no focus that the
    /// person placed afterwards.
    #[gpui::test]
    fn a_tab_that_did_not_end_moves_no_later_focus(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000)
            .map(|index| line(index, 30.0, index == 0 || index == 1000))
            .collect();
        let (host, cx) = open(cx, lines);
        focus(&host, cx, |host| host.controls["r0000"].clone());
        let before = host.read_with(cx, |host, _| host.before.clone());
        cx.update(|window, cx| {
            window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx);
            host.update(cx, |host, _| {
                host.list.state.scroll_to(ListOffset::default());
            });
            window.focus(&before, cx);
        });
        settle(cx);
        scroll_to(&host, cx, 995, 0.0);
        scroll_to(&host, cx, 990, 0.0);
        cx.update(|window, cx| assert!(before.is_focused(window), "{:?}", window.focused(cx)));
    }

    /// Point 7 of the review: the height estimate of rows not drawn stays after the first
    /// frame, after a change of width and after a change of the rem size.
    #[gpui::test]
    fn the_estimate_of_rows_not_drawn_follows_the_width_and_the_rem_size(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 40.0, false)).collect();
        let (host, cx) = open(cx, lines);
        let content = |host: &Entity<Host>, cx: &mut VisualTestContext| {
            host.read_with(cx, |host, _| {
                host.list.state.max_offset_for_scrollbar().y + px(300.0)
            })
        };
        let near =
            |value: Pixels, expected: f32| (value.as_f32() - expected).abs() < expected * 0.05;
        assert!(
            near(content(&host, cx), 80_000.0),
            "{:?}",
            content(&host, cx)
        );
        host.update(cx, |host, cx| {
            host.width = px(500.0);
            cx.notify();
        });
        settle(cx);
        assert!(
            near(content(&host, cx), 80_000.0),
            "{:?}",
            content(&host, cx)
        );
        cx.update(|window, _| window.set_rem_size(px(20.0)));
        redraw(&host, cx);
        assert!(
            near(content(&host, cx), 100_000.0),
            "{:?}",
            content(&host, cx)
        );
    }

    /// Point 8 of the review: a focused row without a control moves the focus to the next
    /// and the previous row with Down and Up, and shows it.
    #[gpui::test]
    fn up_and_down_move_the_focus_between_rows(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..200).map(|index| line(index, 30.0, false)).collect();
        let (host, cx) = open(cx, lines);
        focus(&host, cx, |host| host.list.row_focus("r0003").unwrap());
        press(cx, "down");
        let focused = |host: &Entity<Host>, cx: &mut VisualTestContext, key: &str| {
            let handle = host.read_with(cx, |host, _| host.list.row_focus(key).unwrap());
            cx.update(|window, _| handle.is_focused(window))
        };
        assert!(focused(&host, cx, "r0004"));
        for _ in 0..20 {
            press(cx, "down");
        }
        assert!(focused(&host, cx, "r0024"));
        assert!(in_view(cx, "r0024"));
        press(cx, "up");
        assert!(focused(&host, cx, "r0023"));
    }

    /// Point 9 of the review: a Tab that reaches a row of the last frame after the list was
    /// replaced, such as by a change of the page, does not panic.
    #[gpui::test]
    fn a_tab_after_the_list_was_replaced_in_the_same_frame(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..20).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines);
        focus(&host, cx, |host| host.controls["r0005"].clone());
        cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.list = PageList::new(cx);
                host.lines.clear();
                cx.notify();
            });
            window.dispatch_action(NextControl.boxed_clone(), cx);
        });
        settle(cx);
    }

    /// Point 14 of the review: a step of a wheel that counts lines scrolls by the line
    /// height of the list.
    #[gpui::test]
    fn a_wheel_step_in_lines_scrolls_by_the_line_height(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, false)).collect();
        let (host, cx) = open(cx, lines);
        let page = cx.debug_bounds("admin-page").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: page.center(),
            delta: ScrollDelta::Lines(point(0.0, -2.0)),
            ..Default::default()
        });
        settle(cx);
        let offset = host.read_with(cx, |host, _| {
            host.list.state.scroll_px_offset_for_scrollbar()
        });
        assert_eq!(offset.y, px(-64.0));
    }

    /// Check of the corrections, point 2: the focused control of a row that moves one place
    /// down, out of the view, keeps the keys, and its row is shown.
    #[gpui::test]
    fn a_focused_row_moved_one_place_down_out_of_view_is_shown(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..200).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines.clone());
        let last = (0..20)
            .rev()
            .find(|&index| in_view(cx, &format!("r{index:04}")))
            .unwrap();
        let key = format!("r{last:04}");
        focus(&host, cx, |host| host.controls[&key].clone());
        let mut moved = lines.clone();
        moved.swap(last, last + 1);
        host.update(cx, |host, cx| host.set(moved, cx));
        settle(cx);
        assert!(in_view(cx, &key));
        assert_eq!(pressed_after_enter(&host, cx), [key]);
    }

    /// Check of the corrections, N1: a focused row out of view that another change moves
    /// moves no row in view.
    #[gpui::test]
    fn a_focused_row_out_of_view_that_moves_moves_no_row_in_view(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines.clone());
        focus(&host, cx, |host| host.controls["r0002"].clone());
        scroll_to(&host, cx, 900, 10.0);
        let before = cx.debug_bounds("row-r0905");
        assert!(before.is_some());
        let mut moved = lines.clone();
        let row = moved.remove(2);
        moved.push(row);
        host.update(cx, |host, cx| host.set(moved, cx));
        settle(cx);
        assert_eq!(cx.debug_bounds("row-r0905"), before);
        assert_eq!(pressed_after_enter(&host, cx), ["r0002"]);
    }

    /// Check of the corrections, point 4: Tab and Shift+Tab enter a list whose rows hold no
    /// control at its first or its last row, from which the keys scroll the list.
    #[gpui::test]
    fn tab_enters_a_list_without_controls_at_its_first_or_last_row(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, false)).collect();
        let (host, cx) = open(cx, lines);
        let focused = |host: &Entity<Host>, cx: &mut VisualTestContext, key: &str| {
            let handle = host.read_with(cx, |host, _| host.list.row_focus(key).unwrap());
            cx.update(|window, _| handle.is_focused(window))
        };
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        assert!(focused(&host, cx, "r0000"));
        press(cx, "end");
        assert!(in_view(cx, "r1999"));
        focus(&host, cx, |host| host.after.clone());
        press(cx, "shift-tab");
        assert!(focused(&host, cx, "r1999"));
        press(cx, "home");
        assert!(in_view(cx, "r0000"));
    }

    /// Check of the corrections, N2: rows that replace the rows in view, as the entries
    /// after "Continue", start at the top of the view with the first of them, also when a
    /// row that stays, such as the footer, is in view.
    #[gpui::test]
    fn rows_that_replace_the_rows_in_view_start_at_the_top_of_the_view(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, page("a", 50));
        host.update(cx, |host, _| host.list.state.scroll_to_end());
        redraw(&host, cx);
        assert!(in_view(cx, "footer"));
        host.update(cx, |host, cx| host.set(page("b", 50), cx));
        settle(cx);
        let top = host.read_with(cx, |host, _| host.list.state.logical_scroll_top());
        assert_eq!((top.item_ix, top.offset_in_item), (1, px(0.0)));
        assert!(in_view(cx, "b0000"));
    }

    /// Check of the corrections, N2: after the page asks for it, as at "Read again", the
    /// next change of the rows starts the view at the first item, also when the change only
    /// removes rows.
    #[gpui::test]
    fn a_read_again_starts_the_view_at_the_first_item(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, page("a", 500));
        host.update(cx, |host, _| host.list.state.scroll_to_end());
        redraw(&host, cx);
        assert!(in_view(cx, "footer"));
        host.update(cx, |host, cx| {
            host.list.start_at_first_item();
            host.set(page("a", 50), cx);
        });
        settle(cx);
        let top = host.read_with(cx, |host, _| host.list.state.logical_scroll_top());
        assert_eq!((top.item_ix, top.offset_in_item), (1, px(0.0)));
        assert!(in_view(cx, "a0000"));
    }

    /// Check of the corrections, low point 1: Tab and Shift+Tab from outside enter the list
    /// at the first and the last row in view that holds a control, and at the first and
    /// the last item in view of a list without controls, and the view does not move.
    #[gpui::test]
    fn tab_enters_the_list_at_the_rows_in_view(cx: &mut TestAppContext) {
        for control in [true, false] {
            let lines: Vec<Line> = (0..2000).map(|index| line(index, 30.0, control)).collect();
            let (host, cx) = open(cx, lines);
            scroll_to(&host, cx, 900, 0.0);
            let before = cx.debug_bounds("row-r0905");
            assert!(before.is_some());
            let last = (900..1000)
                .rev()
                .map(|index| format!("r{index:04}"))
                .find(|key| in_view(cx, key))
                .unwrap();
            let entered = |host: &Entity<Host>, cx: &mut VisualTestContext, key: &str| {
                if control {
                    pressed_after_enter(host, cx) == [key]
                } else {
                    let handle = host.read_with(cx, |host, _| host.list.row_focus(key).unwrap());
                    cx.update(|window, _| handle.is_focused(window))
                }
            };
            focus(&host, cx, |host| host.before.clone());
            press(cx, "tab");
            assert!(entered(&host, cx, "r0900"), "control {control}");
            assert_eq!(cx.debug_bounds("row-r0905"), before);
            focus(&host, cx, |host| host.after.clone());
            press(cx, "shift-tab");
            assert!(entered(&host, cx, &last), "control {control}");
            assert_eq!(cx.debug_bounds("row-r0905"), before);
        }
    }

    /// Check of the corrections, low point 3: when the row that Tab enters is not drawn in
    /// the view, the focus leaves the list and does not stay on its empty tab stop.
    #[gpui::test]
    fn tab_into_a_list_that_draws_no_row_in_view_leaves_it(cx: &mut TestAppContext) {
        let lines: Vec<Line> = (0..20).map(|index| line(index, 30.0, true)).collect();
        let (host, cx) = open(cx, lines);
        host.update(cx, |host, cx| {
            host.height = px(0.0);
            cx.notify();
        });
        settle(cx);
        focus(&host, cx, |host| host.before.clone());
        press(cx, "tab");
        let (after, start) =
            host.read_with(cx, |host, _| (host.after.clone(), host.list.start.clone()));
        cx.update(|window, cx| {
            assert!(!start.is_focused(window));
            assert!(after.is_focused(window), "{:?}", window.focused(cx));
        });
    }

    #[test]
    fn the_rows_kept_in_place_are_the_longest_series_in_order() {
        // Old places of the new rows: row 2 moved to the end, a new row in front.
        let found = [None, Some(0), Some(1), Some(3), Some(4), Some(2)];
        assert_eq!(kept_in_place(&found), [(0, 1), (1, 2), (3, 3), (4, 4)]);
        assert_eq!(kept_in_place(&[Some(1), Some(0)]).len(), 1);
        assert!(kept_in_place(&[None, None]).is_empty());
    }
}

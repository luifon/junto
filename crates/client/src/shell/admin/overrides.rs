//! The pages "Permissions" of a channel and of a category (ADR 0301, ADR 0304): the roles
//! and the people with a rule at the place, a search that adds a role or a member, and for
//! the shown subject one line per permission with the three choices "Allow", "Neutral" and
//! "Deny". A channel in a category states whether it takes the rules of the category.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Entity, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window, div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{ChannelId, ServerId};
use junto_client_core::protocol::structure::RuleSubject;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::list::Row;
use super::{Mode, Question, group_label, line, line_text, named, page_title, refusal, sentence};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::state::admin::view::overrides::{
    Candidate, CategoryPermissionsPage, Choice, PermissionLine, PermissionsPage, RuleList,
    SubjectKind, SubjectLine, SyncLine, Visibility, group_of,
};
use crate::state::admin::view::{AdminView, CategorySettings, ChannelSettings, Listed};
use crate::state::admin::{AskKind, ChannelPage, RulesOf};
use crate::text;
use crate::text::admin::overrides::Place;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Chip, Field, FieldEvent, StyleExt};

/// The most roles and members that the search lists at once.
const MOST_MATCHES: usize = 8;

/// The subject whose permissions the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expanded {
    /// The person chose no subject yet: the first one, which is the "everyone" role.
    First,
    Subject(RuleSubject),
    Nothing,
}

/// What the pages of the rules hold of the window itself.
pub(crate) struct Ui {
    expanded: Expanded,
    /// The search for a role or a member to add is open.
    adding: bool,
    /// The search of "Who can see this channel" is open: a choice allows "view channel".
    adding_viewer: bool,
    /// The full list of the rules of a channel ("Advanced permissions") is open.
    advanced: bool,
    /// A change of who sees a synced channel waits for the answer to the question whether
    /// the channel stops taking the rules of its category (ADR 0761).
    pending: Option<Pending>,
    pub(crate) search: Entity<InputState>,
}

impl Ui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let search = Field::state(text::admin::overrides::ADD_HELP, window, cx);
        let subscription = Field::observe(&search, window, cx, |_: &mut Shell, event, _, cx| {
            if event == FieldEvent::Changed {
                cx.notify();
            }
        });
        let ui = Self {
            expanded: Expanded::First,
            adding: false,
            adding_viewer: false,
            advanced: false,
            pending: None,
            search,
        };
        (ui, vec![subscription])
    }
}

/// A permission that a line of choices shows.
pub(crate) trait Shown: Copy + 'static {
    /// The name of the permission in the names of the controls, for a test.
    fn key(self) -> &'static str;
    fn label(self) -> &'static str;
    /// What the permission allows at a place.
    fn help(self, place: Place) -> &'static str;
    /// The permission that makes a channel visible: its refusal names sight.
    fn sees(self) -> bool;
}

impl Shown for WorkspacePermission {
    fn key(self) -> &'static str {
        self.name()
    }
    fn label(self) -> &'static str {
        text::admin::overrides::name(self)
    }
    fn help(self, place: Place) -> &'static str {
        match place {
            Place::Channel => text::admin::overrides::help(self),
            // The help of a role describes a permission without naming one channel.
            Place::Category | Place::Workspace | Place::Server => {
                text::admin::roles::workspace_help_of(self)
            }
        }
    }
    fn sees(self) -> bool {
        self == Self::ViewChannel
    }
}

impl Shown for ServerPermission {
    fn key(self) -> &'static str {
        self.name()
    }
    fn label(self) -> &'static str {
        text::admin::overrides::server_name(self)
    }
    fn help(self, _: Place) -> &'static str {
        text::admin::overrides::server_help(self)
    }
    fn sees(self) -> bool {
        false
    }
}

/// A change of who sees a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Private(bool),
    Allow(RuleSubject),
    Remove(RuleSubject),
}

/// What a click on a choice sends.
pub(crate) type OnChoose<P> = Rc<dyn Fn(&mut Shell, P, Choice, &mut Context<Shell>)>;

/// The selector part of a subject: `role-<id>` or `person-<id>`.
fn subject_name(subject: RuleSubject) -> String {
    match subject {
        RuleSubject::Role { role_id } => format!("role-{role_id}"),
        RuleSubject::Person { account_id } => format!("person-{account_id}"),
    }
}

/// The identifier of a subject, which names its controls for a test.
fn subject_id(subject: RuleSubject) -> uuid::Uuid {
    match subject {
        RuleSubject::Role { role_id } => role_id.as_uuid(),
        RuleSubject::Person { account_id } => account_id.as_uuid(),
    }
}

/// The place of rules that a page changes.
#[derive(Clone, Copy)]
struct Editor {
    server_id: ServerId,
    at: RulesOf,
    place: Place,
    /// The part of the client core that holds the rules, for a read that failed.
    part: AdminPart,
}

impl Shell {
    pub(crate) fn reset_overrides_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.admin.overrides.expanded = Expanded::First;
        self.admin.overrides.adding = false;
        self.admin.overrides.adding_viewer = false;
        self.admin.overrides.advanced = false;
        self.admin.overrides.pending = None;
        self.admin
            .overrides
            .search
            .update(cx, |field, cx| field.set_value("", window, cx));
    }

    /// Opens or closes the search for a role or a member to add.
    pub fn set_rules_adding(&mut self, adding: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.admin.overrides.adding = adding;
        self.admin
            .overrides
            .search
            .update(cx, |field, cx| field.set_value("", window, cx));
        if adding {
            self.focus_field(&self.admin.overrides.search.clone(), window, cx);
        }
        self.admin.list.state.remeasure();
        cx.notify();
    }

    /// Shows the permissions of a role or a person of the open page of rules, or of none.
    pub fn show_rule_subject(&mut self, subject: Option<RuleSubject>, cx: &mut Context<Self>) {
        self.admin.overrides.expanded = subject.map_or(Expanded::Nothing, Expanded::Subject);
        self.admin.list.state.remeasure();
        cx.notify();
    }

    /// Puts text into the search for a role or a member, as typing does.
    pub fn type_rules_search(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = typed.to_owned();
        self.admin
            .overrides
            .search
            .update(cx, |field, cx| field.set_value(typed, window, cx));
        cx.notify();
    }

    /// One of the three choices of a permission. The choice of now has the pressed look
    /// and takes no click.
    pub(crate) fn render_rule_choice<P: Shown>(
        &mut self,
        line: &PermissionLine<P>,
        (index, choice): (usize, Choice),
        (place, held): (Place, Option<SharedString>),
        on_choose: &OnChoose<P>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let on = line.current == choice;
        let word = text::admin::overrides::choice_word(choice);
        let permission = line.permission;
        let held: Option<SharedString> = line
            .refused(choice)
            .map(|reason| {
                text::admin::overrides::refused(
                    reason,
                    permission.label(),
                    place,
                    permission.sees(),
                )
                .into()
            })
            .or(held);
        let pressed = theme.colors.limestone_lo;
        let mut button = Button::new((SharedString::from(format!("override-{word}")), index))
            .label(text::admin::overrides::choice(choice))
            .accessible_label(text::admin::overrides::choice_label(
                permission.label(),
                choice,
                on,
            ))
            .h(u(28.0))
            .px(u(10.0))
            .disabled(held.is_some())
            .when(on, |button| button.bg(Hsla::from(pressed)).shadow_none());
        if let Some(reason) = held {
            button = button.tooltip(reason);
        }
        if !on {
            let on_choose = on_choose.clone();
            button = button.on_click(cx.listener(move |this, _, _, cx| {
                on_choose(this, permission, choice, cx);
            }));
        }
        named(format!("override-{}-{word}", permission.key()), button)
    }

    /// One permission: its name, what it allows, and the three choices. A choice that the
    /// grant ceiling refuses is disabled, and the line states the reason. `held` is why
    /// every choice of the line waits or is locked.
    pub(crate) fn render_rule_permission<P: Shown>(
        &mut self,
        line: &PermissionLine<P>,
        index: usize,
        (place, held): (Place, Option<SharedString>),
        on_choose: &OnChoose<P>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let permission = line.permission;
        let reason = line.reason().map(|reason| {
            text::admin::overrides::refused(reason, permission.label(), place, permission.sees())
        });
        let mut choices = div().flex().flex_none().gap(u(4.0)).mt(u(1.0));
        for choice in Choice::ALL {
            choices = choices.child(self.render_rule_choice(
                line,
                (index, choice),
                (place, held.clone()),
                on_choose,
                theme,
                cx,
            ));
        }
        div()
            .flex()
            .flex_none()
            .items_start()
            .gap(u(10.0))
            .py(u(6.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule_soft))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.control_strong, theme)
                            .ink(colors.ink)
                            .child(permission.label()),
                    )
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(permission.help(place)),
                    )
                    .children(reason.map(|reason| {
                        sentence(reason, theme)
                            .mt(u(3.0))
                            .ink(colors.ink)
                            .debug_selector(|| "override-ceiling".to_owned())
                    })),
            )
            .child(choices)
            .into_any_element()
    }

    /// One role or person: its name, what its rule holds, and its actions. The shown
    /// subject has its permissions under the row.
    fn render_rule_subject(
        &mut self,
        view: &AdminView,
        editor: Editor,
        subject: &SubjectLine,
        shown: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Editor {
            server_id,
            at,
            place,
            ..
        } = editor;
        let key = subject.subject;
        let what = AskKind::SetRule(at, key);
        let sending = view.sending(what);
        let id = subject_id(key);
        let number = id.as_u128() as u64;
        let name = if subject.name.is_empty() {
            text::admin::overrides::UNKNOWN_MEMBER.to_owned()
        } else {
            subject.name.clone()
        };
        let summary = text::admin::overrides::summary(subject.allowed(), subject.denied());
        let mut row = line(theme)
            .child(match key {
                RuleSubject::Person { account_id } => super::line_identity(
                    crate::shell::people::person_label(
                        &self.faces(server_id),
                        account_id,
                        name.clone(),
                        "rule-person",
                    ),
                    Some(summary.into()),
                    theme,
                ),
                _ => super::line_identity(
                    crate::ui::role_name(name.clone(), subject.color, theme),
                    Some(summary.into()),
                    theme,
                ),
            })
            .child(Chip::new(text::admin::overrides::kind(subject.kind)));
        let locked = subject.locked.map(text::admin::overrides::locked);
        // The removal of a rule removes its denies: the grant ceiling decides it.
        let blocked = subject.blocks_removal();
        let cannot_remove = (subject.has_rule && !blocked.is_empty())
            .then(|| text::admin::overrides::removal_refused(&blocked, place));
        if subject.has_rule {
            let held: Option<SharedString> = locked
                .map(SharedString::from)
                .or_else(|| cannot_remove.clone().map(SharedString::from))
                .or_else(|| sending.then(|| text::admin::SENDING.into()));
            let button = Button::new(("override-clear", number))
                .label(text::admin::overrides::REMOVE)
                .h(u(28.0))
                .px(u(10.0))
                .disabled(held.is_some())
                .tooltip(held.unwrap_or_else(|| text::admin::overrides::REMOVE_HELP.into()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.clear_rule(server_id, (at, key), cx);
                    });
                }));
            row = row.child(named(format!("override-clear-{id}"), button));
        }
        let toggle = Button::new(("override-subject", number))
            .icon(if shown {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .label(if shown {
                text::admin::overrides::HIDE
            } else {
                text::admin::overrides::SHOW
            })
            .accessible_label(if shown {
                text::admin::overrides::hide_subject(&name)
            } else {
                text::admin::overrides::show_subject(&name)
            })
            .h(u(28.0))
            .px(u(10.0))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.admin.overrides.expanded = if shown {
                    Expanded::Nothing
                } else {
                    Expanded::Subject(key)
                };
                this.admin.list.state.remeasure();
                cx.notify();
            }));
        row = row.child(named(format!("rules-show-{id}"), toggle));

        let mut block = div()
            .id(SharedString::from(format!("override-result-{id}")))
            .flex()
            .flex_col()
            .flex_none()
            .debug_selector(move || format!("rules-subject-{}", subject_name(key)))
            .child(row);
        if let Some(reason) = locked {
            block = block.child(
                sentence(reason, theme)
                    .mt(u(4.0))
                    .ink(theme.colors.ink)
                    .debug_selector(move || format!("rules-locked-{id}")),
            );
        } else if let Some(reason) = cannot_remove {
            block = block.child(sentence(reason, theme).mt(u(4.0)).ink(theme.colors.ink));
        }
        if shown {
            let held: Option<SharedString> = locked
                .map(SharedString::from)
                .or_else(|| sending.then(|| text::admin::SENDING.into()));
            let on_choose: OnChoose<WorkspacePermission> =
                Rc::new(move |this: &mut Shell, permission, choice, cx| {
                    this.state.update(cx, |state, cx| {
                        state.set_rule_choice(server_id, (at, key), permission, choice, cx);
                    });
                });
            let mut permissions = div().flex().flex_col().flex_none().pl(u(14.0));
            let mut group = None;
            for (index, permission) in subject.permissions.iter().enumerate() {
                let of = group_of(permission.permission);
                if of != group {
                    group = of;
                    if let Some(of) = of {
                        permissions = permissions
                            .child(group_label(text::admin::overrides::group_name(of), theme));
                    }
                }
                permissions = permissions.child(self.render_rule_permission(
                    permission,
                    index,
                    (place, held.clone()),
                    &on_choose,
                    theme,
                    cx,
                ));
            }
            block = block.child(permissions);
        }
        block
            .children(refusal(view.refused(what), "override-refused"))
            .into_any_element()
    }

    /// The label of the subjects with the control that adds one. While the search is
    /// open: the field and the roles and members that match the typed text.
    fn render_rule_adding(
        &mut self,
        list: &RuleList,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let adding = self.admin.overrides.adding;
        let mut column = div().flex().flex_col().flex_none().child(
            div()
                .flex()
                .flex_none()
                .items_end()
                .gap(u(8.0))
                .child(
                    group_label(text::admin::overrides::SUBJECTS, theme)
                        .flex_1()
                        .min_w_0(),
                )
                .child(named(
                    "rules-add".to_owned(),
                    Button::new("rules-add")
                        .icon(if adding { Icon::X } else { Icon::Plus })
                        .label(if adding {
                            text::CANCEL
                        } else {
                            text::admin::overrides::ADD
                        })
                        .h(u(28.0))
                        .px(u(10.0))
                        .mb(u(4.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_rules_adding(!adding, window, cx);
                        })),
                )),
        );
        if !adding {
            return column.into_any_element();
        }
        column = column.child(
            div().flex_none().pt(u(4.0)).child(
                Field::new(&self.admin.overrides.search, text::admin::overrides::SEARCH)
                    .selector("rules-search"),
            ),
        );
        let typed = self
            .admin
            .overrides
            .search
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let matches: Vec<&Candidate> = list
            .candidates
            .iter()
            .filter(|candidate| {
                !(list.people_full && matches!(candidate.subject, RuleSubject::Person { .. }))
            })
            .filter(|candidate| candidate.matches(&typed))
            .collect();
        for candidate in matches.iter().take(MOST_MATCHES) {
            let subject = candidate.subject;
            let kind = match subject {
                RuleSubject::Role { .. } => SubjectKind::Role,
                RuleSubject::Person { .. } => SubjectKind::Person,
            };
            let number = subject_id(subject).as_u128() as u64;
            column = column.child(
                line(theme)
                    .child(match subject {
                        RuleSubject::Person { account_id } => super::line_identity(
                            crate::shell::people::person_label(
                                &self.shown_faces(),
                                account_id,
                                candidate.name.clone(),
                                "rule-candidate",
                            ),
                            None,
                            theme,
                        ),
                        _ => super::line_identity(
                            crate::ui::role_name(candidate.name.clone(), candidate.color, theme),
                            None,
                            theme,
                        ),
                    })
                    .child(Chip::new(text::admin::overrides::kind(kind)))
                    .child(named(
                        format!("rules-add-{}", subject_name(subject)),
                        Button::new(("rules-add-subject", number))
                            .label(text::admin::overrides::ADD_ONE)
                            .accessible_label(text::admin::overrides::add_subject(
                                &candidate.name,
                                kind,
                            ))
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.admin.overrides.expanded = Expanded::Subject(subject);
                                this.set_rules_adding(false, window, cx);
                                this.state
                                    .update(cx, |state, cx| state.add_rule_subject(subject, cx));
                            })),
                    )),
            );
        }
        let more = matches.len().saturating_sub(MOST_MATCHES);
        if more > 0 {
            column = column
                .child(sentence(text::admin::overrides::more_matches(more), theme).py(u(6.0)));
        }
        if matches.is_empty() {
            column = column.child(sentence(text::admin::overrides::NO_MATCH, theme).py(u(6.0)));
        }
        if !list.members_read {
            column =
                column.child(sentence(text::admin::overrides::MEMBERS_READING, theme).py(u(3.0)));
        }
        if list.people_full {
            column = column.child(sentence(text::admin::overrides::PEOPLE_FULL, theme).py(u(3.0)));
        }
        column.into_any_element()
    }

    /// The rows of an editor of rules: the header of the page, the row that adds a
    /// subject, and one row for each subject.
    fn render_rules(
        &mut self,
        view: &AdminView,
        editor: Editor,
        (rules, folded): (&Listed<RuleList>, bool),
        header: impl Fn(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A folded list of rules shows its header alone ("Advanced permissions").
        let list = rules.loaded().cloned().filter(|_| !folded);
        let subjects = list
            .as_ref()
            .map(|list| list.subjects.clone())
            .unwrap_or_default();
        let mut rows = vec![Row::part("permissions-header", true)];
        if list.is_some() {
            rows.push(Row::part("rules-add-row", true));
        }
        rows.extend(subjects.iter().map(|row| {
            let name = subject_name(row.subject);
            Row::item(format!("override-{name}"), true, name)
        }));
        self.admin.list.take(Some(view), rows, window, cx);
        let view = view.clone();
        let failure = match rules {
            Listed::Failed(failure) => Some(Some(*failure)),
            Listed::Loading => Some(None),
            Listed::Loaded(_) => None,
        };
        self.render_admin_list(
            move |this, index, window, cx| {
                let theme = theme(cx);
                if index == 0 {
                    let head = header(this, window, cx);
                    let mut column = div().flex().flex_col().child(head);
                    if let Some(failure) = failure {
                        column = column.child(this.render_reading(
                            editor.server_id,
                            editor.part,
                            failure,
                            &theme,
                            cx,
                        ));
                    }
                    return column.into_any_element();
                }
                let Some(list) = list.as_ref() else {
                    return div().into_any_element();
                };
                if index == 1 {
                    return this.render_rule_adding(list, &theme, cx);
                }
                let expanded = match this.admin.overrides.expanded {
                    Expanded::First => subjects.first().map(|subject| subject.subject),
                    Expanded::Subject(subject) => Some(subject),
                    Expanded::Nothing => None,
                };
                let row = &subjects[index - 2];
                this.render_rule_subject(
                    &view,
                    editor,
                    row,
                    expanded == Some(row.subject),
                    &theme,
                    cx,
                )
            },
            window,
            cx,
        )
    }

    pub(crate) fn render_permissions_page(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        page: &PermissionsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let channel_id = settings.channel_id;
        let editor = Editor {
            server_id: view.server_id,
            at: RulesOf::Channel(channel_id),
            place: Place::Channel,
            part: AdminPart::Rules(channel_id),
        };
        let archived = settings.archived;
        let sync = page.sync.clone();
        let header_view = view.clone();
        let visibility = page
            .rules
            .loaded()
            .map(crate::state::admin::view::overrides::visibility);
        let list = page.rules.loaded().cloned();
        let settings = settings.clone();
        let folded = !self.admin.overrides.advanced;
        self.render_rules(
            view,
            editor,
            (&page.rules, folded),
            move |this, _, cx| {
                let theme = theme(cx);
                let mut column = div()
                    .flex()
                    .flex_col()
                    .child(this.render_permissions_header(
                        &header_view,
                        channel_id,
                        archived,
                        sync.as_ref(),
                        cx,
                    ));
                if let (Some(visibility), Some(list)) = (&visibility, &list) {
                    column = column.child(this.render_visibility(
                        &header_view,
                        &settings,
                        sync.as_ref(),
                        (visibility, list),
                        &theme,
                        cx,
                    ));
                }
                let open = this.admin.overrides.advanced;
                column
                    .child(
                        div().flex().flex_none().mt(u(14.0)).child(named(
                            if open {
                                "rules-advanced-open".to_owned()
                            } else {
                                "rules-advanced".to_owned()
                            },
                            Button::new("rules-advanced")
                                .kind(crate::ui::ButtonKind::Ghost)
                                .icon(if open {
                                    Icon::ChevronDown
                                } else {
                                    Icon::ChevronRight
                                })
                                .label(text::admin::overrides::ADVANCED)
                                .h(u(28.0))
                                .px(u(6.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.admin.overrides.advanced = !open;
                                    cx.notify();
                                })),
                        )),
                    )
                    .into_any_element()
            },
            window,
            cx,
        )
    }

    /// "Private channel" and, while it is on, "Who can see this channel": the roles and the
    /// people with an allow of "view channel", each with "Remove", and the search that adds
    /// one; a person whom a rule under "Advanced permissions" denies it is marked
    /// (ADR 0761). A change writes one permission of one rule (ADR 0304).
    fn render_visibility(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        sync: Option<&SyncLine>,
        (visibility, list): (&Visibility, &RuleList),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, channel_id) = (view.server_id, settings.channel_id);
        let at = RulesOf::Channel(channel_id);
        let synced = sync.is_some_and(|sync| sync.synced);
        let settings_sync = sync.map(|sync| sync.category_name.clone());
        let private = visibility.private;
        let waiting = view.asks.iter().any(|ask| {
            matches!(ask.what, AskKind::SetRule(of, _) if of == at)
                && matches!(ask.state, crate::state::admin::AskState::Sending)
        });
        let held = (!settings.may_manage)
            .then_some(text::admin::CHANNEL_MANAGE_NEEDS)
            .or(waiting.then_some(text::admin::SENDING));
        let mut column = div().flex().flex_col().flex_none().mt(u(10.0)).child(
            line(theme)
                .child(line_text(
                    text::admin::PRIVATE_CHANNEL.to_owned(),
                    Some(text::admin::PRIVATE_CHANNEL_HELP.into()),
                    theme,
                ))
                .child(named(
                    "channel-private".to_owned(),
                    crate::ui::Toggle::new(
                        "channel-private",
                        text::admin::PRIVATE_CHANNEL,
                        private,
                        (text::ON, text::OFF),
                    )
                    .held(held)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_visibility(
                            (server_id, channel_id),
                            synced,
                            Pending::Private(!private),
                            cx,
                        );
                    })),
                )),
        );
        if let Some(pending) = self.admin.overrides.pending {
            let category = settings_sync.clone().unwrap_or_default();
            column = column.child(
                self.render_row_question(
                    Question {
                        sentence: text::admin::overrides::stop_sync_question(&category).into(),
                        confirm: text::admin::overrides::STOP_SYNC_CONFIRM,
                        confirm_selector: "visibility-confirm",
                        cancel_selector: "visibility-cancel",
                        sending: waiting,
                    },
                    theme,
                    cx,
                    move |this, _, cx| {
                        this.admin.overrides.pending = None;
                        this.send_visibility((server_id, channel_id), pending, cx);
                    },
                )
                .debug_selector(|| "visibility-question".to_owned()),
            );
        }
        if !private {
            return column.into_any_element();
        }
        let adding = self.admin.overrides.adding_viewer;
        column = column.child(
            div()
                .flex()
                .flex_none()
                .items_end()
                .gap(u(8.0))
                .child(
                    group_label(text::admin::overrides::WHO_SEES, theme)
                        .flex_1()
                        .min_w_0(),
                )
                .when(settings.may_manage, |row| {
                    row.child(named(
                        "viewers-add".to_owned(),
                        Button::new("viewers-add")
                            .icon(if adding { Icon::X } else { Icon::Plus })
                            .label(if adding {
                                text::CANCEL
                            } else {
                                text::admin::overrides::ADD_VIEWER
                            })
                            .h(u(28.0))
                            .px(u(10.0))
                            .mb(u(4.0))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.admin.overrides.adding_viewer = !adding;
                                this.admin.overrides.adding = false;
                                this.admin
                                    .overrides
                                    .search
                                    .update(cx, |field, cx| field.set_value("", window, cx));
                                if !adding {
                                    this.focus_field(
                                        &this.admin.overrides.search.clone(),
                                        window,
                                        cx,
                                    );
                                }
                                cx.notify();
                            })),
                    ))
                }),
        );
        if adding {
            column = column.child(
                div().flex_none().pt(u(4.0)).child(
                    Field::new(&self.admin.overrides.search, text::admin::overrides::SEARCH)
                        .selector("viewers-search"),
                ),
            );
            let typed = self
                .admin
                .overrides
                .search
                .read(cx)
                .value()
                .trim()
                .to_owned();
            let shown: Vec<RuleSubject> = visibility
                .viewers
                .iter()
                .map(|viewer| viewer.subject)
                .collect();
            let candidates: Vec<(RuleSubject, String)> = list
                .subjects
                .iter()
                .filter(|line| line.kind != SubjectKind::Everyone)
                .map(|line| (line.subject, line.name.clone()))
                .chain(
                    list.candidates
                        .iter()
                        .map(|candidate| (candidate.subject, candidate.name.clone())),
                )
                .filter(|(subject, _)| !shown.contains(subject))
                .filter(|(_, name)| {
                    junto_client_core::model::comparable_name(name)
                        .contains(&junto_client_core::model::comparable_name(&typed))
                })
                .collect();
            for (subject, name) in candidates.iter().take(MOST_MATCHES) {
                let subject = *subject;
                let number = subject_id(subject).as_u128() as u64;
                column = column.child(
                    line(theme)
                        .child(line_text(name.clone(), None, theme))
                        .child(named(
                            format!("viewers-add-{}", subject_name(subject)),
                            Button::new(("viewers-add-subject", number))
                                .label(text::admin::overrides::ADD_ONE)
                                .h(u(28.0))
                                .px(u(10.0))
                                .disabled(held.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.admin.overrides.adding_viewer = false;
                                    this.change_visibility(
                                        (server_id, channel_id),
                                        synced,
                                        Pending::Allow(subject),
                                        cx,
                                    );
                                })),
                        )),
                );
            }
            if candidates.is_empty() {
                column = column.child(sentence(text::admin::overrides::NO_MATCH, theme).py(u(6.0)));
            }
        }
        if visibility.viewers.is_empty() {
            column = column.child(
                sentence(text::admin::overrides::ONLY_ADMINISTRATORS_SEE, theme)
                    .debug_selector(|| "viewers-none".to_owned()),
            );
        }
        for viewer in &visibility.viewers {
            let subject = viewer.subject;
            let mut row = line(theme)
                .debug_selector({
                    let key = subject_name(subject);
                    move || format!("viewer-{key}")
                })
                .child(line_text(
                    viewer.name.clone(),
                    Some(text::admin::overrides::kind(viewer.kind).into()),
                    theme,
                ));
            if viewer.denied {
                row = row.child(Chip::new(text::admin::overrides::DENIED_BY_ADVANCED));
            } else if settings.may_manage {
                row = row.child(super::row_action(
                    ("viewer-remove", subject_id(subject).as_u128() as u64),
                    text::admin::overrides::REMOVE_VIEWER,
                    format!("viewer-remove-{}", subject_name(subject)),
                    held,
                    cx.listener(move |this, _, _, cx| {
                        this.change_visibility(
                            (server_id, channel_id),
                            synced,
                            Pending::Remove(subject),
                            cx,
                        );
                    }),
                ));
            }
            column = column.child(row);
        }
        column.into_any_element()
    }

    fn render_permissions_header(
        &mut self,
        view: &AdminView,
        channel_id: ChannelId,
        archived: bool,
        sync: Option<&SyncLine>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::channel_page_name(ChannelPage::Permissions),
                text::admin::overrides::HELP,
                &theme,
            ))
            .child(sentence(text::admin::overrides::ORDER, &theme));
        if archived {
            // The rules of an archived channel can be changed (ADR 0410).
            column = column.child(sentence(text::admin::overrides::ARCHIVED, &theme).mt(u(3.0)));
        }
        if let Some(sync) = sync {
            column = column.child(self.render_sync_line(view, channel_id, sync, &theme, cx));
        }
        column.into_any_element()
    }

    /// Whether the channel takes the rules of its category, with "Sync again" for a
    /// channel with its own rules. The sync asks first: it removes the own rules.
    fn render_sync_line(
        &mut self,
        view: &AdminView,
        channel_id: ChannelId,
        sync: &SyncLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let what = AskKind::SyncRules(channel_id);
        let category = sync.category_name.clone();
        let content: AnyElement = if self.admin.mode == Some(Mode::SyncRules) && !sync.synced {
            self.render_row_question(
                Question {
                    sentence: text::admin::overrides::sync_question(&category).into(),
                    confirm: text::admin::overrides::SYNC_AGAIN_CONFIRM,
                    confirm_selector: "rules-sync-confirm",
                    cancel_selector: "rules-sync-cancel",
                    sending: view.sending(what),
                },
                theme,
                cx,
                move |this, _, cx| {
                    this.admin_sent(what);
                    this.state.update(cx, |state, cx| {
                        state.sync_rules(server_id, channel_id, cx);
                    });
                },
            )
            .into_any_element()
        } else {
            let mut row = div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(line_text(
                            text::admin::overrides::sync_line(&category, sync.synced),
                            None,
                            theme,
                        ))
                        .child(sentence(
                            text::admin::overrides::sync_help(&category, sync.synced),
                            theme,
                        )),
                );
            if !sync.synced {
                row = row.child(named(
                    "rules-sync-again".to_owned(),
                    Button::new("rules-sync-again")
                        .label(text::admin::overrides::SYNC_AGAIN)
                        .h(u(28.0))
                        .px(u(10.0))
                        .disabled(view.sending(what))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_admin_mode(Some(Mode::SyncRules), "", window, cx);
                        })),
                ));
            }
            row.into_any_element()
        };
        let selector = if sync.synced {
            "rules-synced"
        } else {
            "rules-not-synced"
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .mt(u(8.0))
            .child(
                self.mode_row(
                    &[Mode::SyncRules],
                    line(theme)
                        .debug_selector(move || selector.to_owned())
                        .child(content),
                ),
            )
            .children(refusal(view.refused(what), "rules-sync-refused"))
            .into_any_element()
    }

    pub(crate) fn render_category_permissions_page(
        &mut self,
        view: &AdminView,
        settings: &CategorySettings,
        page: &CategoryPermissionsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (workspace_id, category_id) = (settings.workspace_id, settings.category_id);
        let editor = Editor {
            server_id: view.server_id,
            at: RulesOf::Category(workspace_id, category_id),
            place: Place::Category,
            part: AdminPart::CategoryRules(workspace_id, category_id),
        };
        let synced = page.synced;
        let name = settings.name.clone();
        let header_view = view.clone();
        self.render_rules(
            view,
            editor,
            (&page.rules, false),
            move |this, _, cx| {
                let theme = theme(cx);
                div()
                    .flex()
                    .flex_col()
                    .child(page_title(
                        text::admin::overrides::CATEGORY_SETTINGS,
                        text::admin::overrides::CATEGORY_HELP,
                        &theme,
                    ))
                    .child(this.render_category_name(&header_view, category_id, &name, &theme, cx))
                    .child(group_label(text::admin::overrides::PERMISSIONS, &theme))
                    .child(sentence(text::admin::overrides::ORDER, &theme))
                    .child(
                        sentence(text::admin::overrides::synced_count(synced), &theme)
                            .mt(u(8.0))
                            .ink(theme.colors.ink)
                            .debug_selector(|| "category-synced-count".to_owned()),
                    )
                    .child(sentence(text::admin::overrides::SYNCED_HELP, &theme).mt(u(3.0)))
                    .into_any_element()
            },
            window,
            cx,
        )
    }

    /// The name of a category with "Rename", on the page of the category, which holds its
    /// name and its permissions together (ADR 0761).
    fn render_category_name(
        &mut self,
        view: &AdminView,
        category_id: junto_client_core::protocol::ids::CategoryId,
        name: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let what = AskKind::UpdateCategory(category_id);
        let content: AnyElement = if self.admin.mode == Some(Mode::RenameCategory(category_id)) {
            self.render_form(
                super::Form {
                    field: super::FormField::Name,
                    label: text::admin::CATEGORY_NAME,
                    field_selector: "category-name",
                    send: text::SAVE,
                    sending: view.sending(what),
                    help: None,
                    held: None,
                    extra: Vec::new(),
                },
                theme,
                cx,
            )
            .into_any_element()
        } else {
            let shown = name.to_owned();
            line(theme)
                .child(line_text(name.to_owned(), None, theme))
                .child(super::row_action(
                    (
                        "category-page-rename",
                        category_id.as_uuid().as_u128() as u64,
                    ),
                    text::admin::RENAME,
                    "category-page-rename".to_owned(),
                    None,
                    cx.listener(move |this, _, window, cx| {
                        this.set_admin_mode(
                            Some(Mode::RenameCategory(category_id)),
                            &shown,
                            window,
                            cx,
                        );
                    }),
                ))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(group_label(text::admin::CATEGORY_NAME, theme))
            .child(self.mode_row(&[Mode::RenameCategory(category_id)], content))
            .children(refusal(view.refused(what), "category-page-refused"))
            .into_any_element()
    }

    /// A change of who sees a channel: sent at once, or, for a channel that takes the rules
    /// of its category, after the question whether it stops taking them (ADR 0761).
    fn change_visibility(
        &mut self,
        place: (ServerId, ChannelId),
        synced: bool,
        change: Pending,
        cx: &mut Context<Self>,
    ) {
        if synced {
            self.admin.overrides.pending = Some(change);
            cx.notify();
            return;
        }
        self.send_visibility(place, change, cx);
    }

    fn send_visibility(
        &mut self,
        (server_id, channel_id): (ServerId, ChannelId),
        change: Pending,
        cx: &mut Context<Self>,
    ) {
        let at = RulesOf::Channel(channel_id);
        self.state.update(cx, |state, cx| match change {
            Pending::Private(private) => state.set_private(server_id, channel_id, private, cx),
            Pending::Allow(subject) => state.set_rule_choice(
                server_id,
                (at, subject),
                WorkspacePermission::ViewChannel,
                Choice::Allow,
                cx,
            ),
            Pending::Remove(subject) => state.set_rule_choice(
                server_id,
                (at, subject),
                WorkspacePermission::ViewChannel,
                Choice::Neutral,
                cx,
            ),
        });
    }
}

impl Shell {
    /// Shows or folds the advanced permissions of the page of a channel.
    pub fn show_advanced_permissions(&mut self, open: bool, cx: &mut Context<Self>) {
        self.admin.overrides.advanced = open;
        cx.notify();
    }
}

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
    SubjectKind, SubjectLine, SyncLine,
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
            .child(line_text(name.clone(), Some(summary.into()), theme))
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
            for (index, permission) in subject.permissions.iter().enumerate() {
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
                    .child(line_text(candidate.name.clone(), None, theme))
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
        rules: &Listed<RuleList>,
        header: impl Fn(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let list = rules.loaded().cloned();
        let subjects = list
            .as_ref()
            .map(|list| list.subjects.clone())
            .unwrap_or_default();
        let mut rows = vec![Row::new("permissions-header", true).part()];
        if list.is_some() {
            rows.push(Row::new("rules-add-row", true).part());
        }
        rows.extend(
            subjects
                .iter()
                .map(|row| Row::new(format!("override-{}", subject_name(row.subject)), true)),
        );
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
        self.render_rules(
            view,
            editor,
            &page.rules,
            move |this, _, cx| {
                this.render_permissions_header(
                    &header_view,
                    channel_id,
                    archived,
                    sync.as_ref(),
                    cx,
                )
            },
            window,
            cx,
        )
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
        self.render_rules(
            view,
            editor,
            &page.rules,
            move |_, _, cx| {
                let theme = theme(cx);
                div()
                    .flex()
                    .flex_col()
                    .child(page_title(
                        text::admin::overrides::PERMISSIONS,
                        text::admin::overrides::CATEGORY_HELP,
                        &theme,
                    ))
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
}

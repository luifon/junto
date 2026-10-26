use std::sync::Arc;

use junto_client_core::Reach;
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::StreamStatus;
use junto_client_core::model::{
    AdminModel, AdminPart, Collection, GlossaryList, LoadFailure, PlaceId, Reading, ServerModel,
    Timeline, WorkspaceSettings,
};
use junto_client_core::protocol::account::OwnAccount;
use junto_client_core::protocol::discovery::Retention;
use junto_client_core::protocol::events::Cursor;
use junto_client_core::protocol::ids::{
    AccountId, CategoryId, ChannelId, DeviceId, ServerId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{
    ArchivePeriod, ArchivedChannel, Category, Channel, ChannelKind, ChannelOverride, Workspace,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use junto_client_core::version::SupportedVersions;
use time::{OffsetDateTime, UtcOffset};

use super::*;
use crate::state::admin::{AskKind, AskState};
use crate::state::feed::Zone;

fn time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

/// One server with the default workspace "Studio" and the workspace "Lab". "Studio" has
/// the category "Projects" with the channel "plans", and the channels "general" and
/// "Daily" (voice) without a category. The account holds the role "leads".
struct Scene {
    server_id: ServerId,
    own: AccountId,
    studio: WorkspaceId,
    lab: WorkspaceId,
    everyone: WorkspaceRoleId,
    leads: WorkspaceRoleId,
    projects: CategoryId,
    general: ChannelId,
    plans: ChannelId,
    daily: ChannelId,
    snapshot: Snapshot,
    admin: AdminModel,
}

fn channel(
    workspace_id: WorkspaceId,
    kind: ChannelKind,
    name: &str,
    position: i32,
    category_id: Option<CategoryId>,
) -> Channel {
    Channel {
        id: ChannelId::generate(),
        workspace_id,
        category_id,
        kind,
        name: name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    }
}

fn scene() -> Scene {
    let own = AccountId::generate();
    let (studio, lab) = (WorkspaceId::generate(), WorkspaceId::generate());
    let projects = CategoryId::generate();
    let role = |name: &str, permissions: &[WorkspacePermission], position| WorkspaceRole {
        position,
        id: WorkspaceRoleId::generate(),
        workspace_id: studio,
        name: name.to_owned(),
        permissions: permissions.to_vec(),
        is_everyone: position == 0,
    };
    let everyone = role("everyone", &[WorkspacePermission::ViewChannel], 0);
    let leads = role("leads", &[], 1);
    let general = channel(studio, ChannelKind::Text, "general", 0, None);
    let daily = channel(studio, ChannelKind::Voice, "Daily", 1, None);
    let plans = channel(studio, ChannelKind::Text, "plans", 2, Some(projects));
    let workspace = |id, name: &str, is_default| Workspace {
        id,
        name: name.to_owned(),
        is_default,
    };
    let snapshot = Snapshot {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 1,
        },
        account: OwnAccount {
            id: own,
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: vec![studio],
        },
        workspaces: vec![
            WorkspaceState {
                workspace: workspace(lab, "Lab", false),
                own_role_ids: Vec::new(),
                roles: Vec::new(),
                categories: Vec::new(),
                channels: Vec::new(),
                calls: Vec::new(),
                permissions: Vec::new(),
                channel_permissions: Vec::new(),
                notification_default: None,
            },
            WorkspaceState {
                workspace: workspace(studio, "Studio", true),
                own_role_ids: vec![leads.id],
                roles: vec![everyone.clone(), leads.clone()],
                categories: vec![Category {
                    id: projects,
                    workspace_id: studio,
                    name: "Projects".to_owned(),
                    position: 0,
                }],
                channels: vec![general.clone(), daily.clone(), plans.clone()],
                calls: Vec::new(),
                permissions: Vec::new(),
                channel_permissions: Vec::new(),
                notification_default: None,
            },
        ],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        sent_call_invitations: Vec::new(),
        call_moves: Vec::new(),
        entry_requests: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
    };
    Scene {
        server_id: ServerId::generate(),
        own,
        studio,
        lab,
        everyone: everyone.id,
        leads: leads.id,
        projects,
        general: general.id,
        plans: plans.id,
        daily: daily.id,
        snapshot,
        admin: AdminModel::default(),
    }
}

impl Scene {
    fn studio_mut(&mut self) -> &mut WorkspaceState {
        &mut self.snapshot.workspaces[1]
    }

    /// Gives the role of the account these permissions.
    fn lead_holds(&mut self, permissions: &[WorkspacePermission]) {
        let leads = self.leads;
        for role in &mut self.studio_mut().roles {
            if role.id == leads {
                role.permissions = permissions.to_vec();
            }
        }
    }

    fn model(&self) -> ServerModel {
        let mut model =
            ServerModel::from_snapshot(crate::state::fixtures::from_roles(self.snapshot.clone()));
        model.admin = Arc::new(self.admin.clone());
        model
    }

    fn server(&self) -> ServerSnapshot {
        ServerSnapshot {
            id: self.server_id,
            name: "Acme".to_owned(),
            address: "acme.example:7443".to_owned(),
            keep_connected: true,
            reach: Reach::Reachable {
                version: SupportedVersions::THIS_BUILD
                    .negotiate(&[SupportedVersions::THIS_BUILD.current()])
                    .unwrap(),
            },
            access: AccessState::LoggedIn {
                account_id: self.own,
                device_id: DeviceId::generate(),
                session_expires_at: time(),
                recoveries: Vec::new(),
                previous_account: None,
            },
            in_progress: None,
            error: None,
            stream: StreamStatus::Live,
            model: Some(Arc::new(self.model())),
            server_version: None,
        }
    }

    fn known() -> Known {
        Known {
            zone: Zone::Fixed(UtcOffset::UTC),
            ..Known::default()
        }
    }

    fn view(&self, open: AdminOpen) -> Option<AdminView> {
        derive(
            &[self.server()],
            &Selection::default(),
            &Self::known(),
            &AdminState::default(),
            open,
        )
    }

    fn page(&self, page: AdminPage) -> (Vec<NavGroup>, AdminPage, ServerPage) {
        let open = AdminOpen::Server {
            server_id: self.server_id,
            page,
        };
        match self.view(open).unwrap().scope {
            Scope::Server {
                nav,
                selected,
                page,
            } => (nav, selected, page),
            Scope::Channel(_) => panic!("a page of the server was opened"),
        }
    }

    fn settings(&self, channel_id: ChannelId, page: ChannelPage) -> Option<ChannelSettings> {
        let open = AdminOpen::Channel {
            server_id: self.server_id,
            workspace_id: self.studio,
            channel_id,
            page,
        };
        self.view(open).map(|view| match view.scope {
            Scope::Channel(settings) => *settings,
            Scope::Server { .. } => panic!("the settings of a channel were opened"),
        })
    }

    fn archived(&self, channel_id: ChannelId) -> ArchivedChannel {
        let channel = self
            .snapshot
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.channels.iter())
            .find(|channel| channel.id == channel_id)
            .unwrap()
            .clone();
        ArchivedChannel {
            channel,
            archived_at: time(),
            deletes_at: Some(time() + time::Duration::days(30)),
        }
    }

    /// Takes a channel out of the channel list and puts it into the loaded archived list.
    fn archive(&mut self, channel_id: ChannelId) -> ArchivedChannel {
        let archived = self.archived(channel_id);
        self.studio_mut()
            .channels
            .retain(|channel| channel.id != channel_id);
        self.admin.archived = vec![(self.studio, Collection::Loaded(vec![archived.clone()]))];
        archived
    }
}

fn pages(nav: &[NavGroup]) -> Vec<AdminPage> {
    nav.iter().flat_map(|group| group.pages.clone()).collect()
}

#[test]
fn a_member_without_permissions_has_the_pages_that_every_member_may_use() {
    let scene = scene();
    let (nav, selected, page) = scene.page(AdminPage::Workspaces);
    assert_eq!(
        pages(&nav),
        [
            AdminPage::Workspaces,
            AdminPage::Archive,
            AdminPage::Members,
            AdminPage::Recordings,
            AdminPage::Glossary,
            AdminPage::Tokens,
            AdminPage::Exports
        ]
    );
    assert_eq!(nav[1].title, NavTitle::Workspace("Studio".to_owned()));
    assert_eq!(selected, AdminPage::Workspaces);
    let ServerPage::Workspaces(page) = page else {
        panic!("the page of the workspaces was opened");
    };
    assert!(!page.may_create && !page.may_manage && !page.is_owner);
    let names: Vec<(&str, bool, bool, bool)> = page
        .rows
        .iter()
        .map(|row| (row.name.as_str(), row.is_default, row.shown, row.member))
        .collect();
    assert_eq!(
        names,
        [("Lab", false, false, false), ("Studio", true, true, true)]
    );
}

#[test]
fn each_server_permission_adds_its_page() {
    for (permission, page) in [
        (ServerPermission::BanMembers, AdminPage::People),
        (ServerPermission::CreateInvites, AdminPage::Invites),
        (
            ServerPermission::ApproveJoinRequests,
            AdminPage::JoinRequests,
        ),
        (ServerPermission::ViewServerAuditLog, AdminPage::AuditLog),
    ] {
        let mut scene = scene();
        scene.snapshot.account.server_permissions = vec![permission];
        let (nav, selected, _) = scene.page(page);
        let shown = pages(&nav);
        assert!(shown.contains(&page), "{permission:?}");
        assert_eq!(selected, page);
        assert!(!shown.contains(&AdminPage::Channels));
    }
    let mut scene = scene();
    scene.snapshot.account.server_permissions = vec![ServerPermission::CreateWorkspace];
    let (_, _, page) = scene.page(AdminPage::Workspaces);
    assert!(matches!(page, ServerPage::Workspaces(page) if page.may_create && !page.may_manage));
}

#[test]
fn an_administrator_and_the_owner_have_every_page() {
    let every = [
        AdminPage::Workspaces,
        AdminPage::Server,
        AdminPage::People,
        AdminPage::Invites,
        AdminPage::JoinRequests,
        AdminPage::Tokens,
        AdminPage::ServerRoles,
        AdminPage::AuditLog,
        AdminPage::Channels,
        AdminPage::Roles,
        AdminPage::Archive,
        AdminPage::Members,
        AdminPage::Notifications,
        AdminPage::Recordings,
        AdminPage::Glossary,
        AdminPage::Exports,
    ];
    let mut scene = scene();
    scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
    let (nav, _, page) = scene.page(AdminPage::Workspaces);
    assert_eq!(pages(&nav), every);
    assert!(
        matches!(page, ServerPage::Workspaces(page) if page.may_create && page.may_manage && !page.is_owner)
    );
    let mut scene = self::scene();
    scene.snapshot.account.is_owner = true;
    let (nav, _, page) = scene.page(AdminPage::Workspaces);
    assert_eq!(pages(&nav), every);
    assert!(matches!(page, ServerPage::Workspaces(page) if page.is_owner));
}

#[test]
fn the_roles_of_the_workspace_add_the_channels_page_and_the_audit_log() {
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let (nav, selected, page) = scene.page(AdminPage::Channels);
    assert!(pages(&nav).contains(&AdminPage::Channels));
    assert!(!pages(&nav).contains(&AdminPage::AuditLog));
    assert_eq!(selected, AdminPage::Channels);
    assert!(matches!(page, ServerPage::Channels(page) if page.may_manage));

    let mut scene = self::scene();
    scene.lead_holds(&[WorkspacePermission::ViewAuditEntries]);
    let (nav, _, _) = scene.page(AdminPage::Workspaces);
    assert!(pages(&nav).contains(&AdminPage::AuditLog));
}

#[test]
fn a_page_that_the_person_may_not_use_gives_way_to_the_workspaces() {
    let scene = scene();
    for page in [
        AdminPage::People,
        AdminPage::Invites,
        AdminPage::JoinRequests,
        AdminPage::AuditLog,
        AdminPage::Channels,
    ] {
        let (_, selected, shown) = scene.page(page);
        assert_eq!(selected, AdminPage::Workspaces, "{page:?}");
        assert!(matches!(shown, ServerPage::Workspaces(_)));
    }
}

#[test]
fn the_channels_page_lists_the_groups_in_the_order_of_the_channel_list() {
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let second = Category {
        id: CategoryId::generate(),
        workspace_id: scene.studio,
        name: "Rooms".to_owned(),
        position: 1,
    };
    scene.studio_mut().categories.push(second.clone());
    // The category of this channel is not in the state: the channel has none on screen.
    let stray = channel(
        scene.studio,
        ChannelKind::Text,
        "stray",
        5,
        Some(CategoryId::generate()),
    );
    scene.studio_mut().channels.push(stray.clone());
    let (_, _, page) = scene.page(AdminPage::Channels);
    let ServerPage::Channels(page) = page else {
        panic!("the channels page was opened");
    };
    assert_eq!(page.workspace_id, scene.studio);
    assert_eq!(page.groups.len(), 3);
    let names = |group: &ChannelGroup| -> Vec<(String, bool, bool)> {
        group
            .channels
            .iter()
            .map(|line| (line.name.clone(), line.first, line.last))
            .collect()
    };
    assert_eq!(page.groups[0].category, None);
    assert_eq!(
        names(&page.groups[0]),
        [
            ("general".to_owned(), true, false),
            ("Daily".to_owned(), false, false),
            ("stray".to_owned(), false, true)
        ]
    );
    let projects = page.groups[1].category.as_ref().unwrap();
    assert_eq!(
        (projects.id, projects.first, projects.last),
        (scene.projects, true, false)
    );
    assert_eq!(names(&page.groups[1]), [("plans".to_owned(), true, true)]);
    let rooms = page.groups[2].category.as_ref().unwrap();
    assert_eq!(
        (rooms.id, rooms.first, rooms.last),
        (second.id, false, true)
    );
    assert!(page.groups[2].channels.is_empty());
    assert_eq!(
        page.channel(scene.plans).unwrap().category_id,
        Some(scene.projects)
    );
    assert_eq!(page.channel(stray.id).unwrap().category_id, None);
    assert_eq!(page.categories().count(), 2);
}

#[test]
fn a_channel_in_which_the_person_does_not_manage_takes_no_change_on_the_channels_page() {
    use junto_client_core::protocol::structure::ChannelPermissions;
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let lines = |scene: &Scene| {
        let (_, _, page) = scene.page(AdminPage::Channels);
        let ServerPage::Channels(page) = page else {
            panic!("the page of the channels is not offered");
        };
        [scene.general, scene.daily, scene.plans].map(|channel_id| {
            let line = page.channel(channel_id).unwrap();
            (line.may_manage, line.moves_up, line.moves_down)
        })
    };
    // The list of the workspace holds for every channel: "general" is the first channel
    // of its group and "Daily" the last, and "plans" is alone in its category.
    let here = Err(Unplanned::Nothing);
    assert_eq!(
        lines(&scene),
        [
            (true, here, Ok(())),
            (true, Ok(()), here),
            (true, here, here)
        ]
    );
    // An override denies "manage channel" in "Daily": the channel takes no change, and no
    // move of a neighbour that would change its position either (ADR 0790). "general"
    // still goes after it, to the free position after the last one, which changes no
    // other channel.
    let daily = scene.daily;
    scene.studio_mut().channel_permissions = vec![ChannelPermissions {
        channel_id: daily,
        permissions: vec![WorkspacePermission::ViewChannel],
    }];
    assert_eq!(
        lines(&scene),
        [
            (true, here, Ok(())),
            (false, Err(Unplanned::NotManaged), here),
            (true, here, here)
        ]
    );
}

#[test]
fn each_page_holds_open_the_collections_that_it_shows() {
    let mut scene = scene();
    let model = scene.model();
    let server = |page| AdminOpen::Server {
        server_id: scene.server_id,
        page,
    };
    let studio = Some(scene.studio);
    assert!(needs(&server(AdminPage::Workspaces), &model, studio).is_empty());
    assert!(needs(&server(AdminPage::Channels), &model, studio).is_empty());
    // The order of the server roles decides who can be removed and banned (ADR 0791).
    assert_eq!(
        needs(&server(AdminPage::People), &model, studio),
        [AdminPart::Accounts, AdminPart::ServerRoles]
    );
    assert_eq!(
        needs(&server(AdminPage::Invites), &model, studio),
        [AdminPart::Invites]
    );
    assert_eq!(
        needs(&server(AdminPage::JoinRequests), &model, studio),
        [AdminPart::JoinRequests]
    );
    assert_eq!(
        needs(&server(AdminPage::Tokens), &model, studio),
        [AdminPart::AccessTokens]
    );
    assert_eq!(
        needs(&server(AdminPage::Exports), &model, studio),
        [AdminPart::Exports, AdminPart::Archived(scene.studio)]
    );
    assert_eq!(
        needs(&server(AdminPage::Exports), &model, None),
        [AdminPart::Exports]
    );
    assert!(needs(&server(AdminPage::Members), &model, studio).is_empty());
    // A recording of an archived channel is named by the archived list.
    assert_eq!(
        needs(&server(AdminPage::Recordings), &model, studio),
        [AdminPart::Archived(scene.studio)]
    );
    assert!(needs(&server(AdminPage::Recordings), &model, None).is_empty());
    // A member reads the archived list and not the settings of an administrator.
    assert_eq!(
        needs(&server(AdminPage::Archive), &model, studio),
        [AdminPart::Archived(scene.studio)]
    );
    assert!(needs(&server(AdminPage::Archive), &model, None).is_empty());
    let channel = |page| AdminOpen::Channel {
        server_id: scene.server_id,
        workspace_id: scene.studio,
        channel_id: scene.general,
        page,
    };
    // The overrides need "manage channel".
    assert_eq!(
        needs(&channel(ChannelPage::Overview), &model, studio),
        [AdminPart::Archived(scene.studio)]
    );

    scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
    let model = scene.model();
    assert_eq!(
        needs(&server(AdminPage::People), &model, studio),
        [AdminPart::Accounts, AdminPart::ServerRoles]
    );
    // An administrator stands in a workspace at a server role (ADR 0309), and the
    // server states what the administrator may do with the roles of the workspace
    // (ADR 0311).
    assert_eq!(
        needs(&server(AdminPage::Members), &model, studio),
        [
            AdminPart::Accounts,
            AdminPart::ServerRoles,
            AdminPart::RoleActions(scene.studio)
        ]
    );
    assert_eq!(
        needs(&server(AdminPage::Archive), &model, studio),
        [
            AdminPart::Archived(scene.studio),
            AdminPart::WorkspaceSettings(scene.studio),
            AdminPart::DirectRetention
        ]
    );
    for page in [ChannelPage::Overview, ChannelPage::Permissions] {
        assert_eq!(
            needs(&channel(page), &model, studio),
            [
                AdminPart::Archived(scene.studio),
                AdminPart::Overrides(scene.general)
            ]
        );
    }
    assert_eq!(
        needs(&channel(ChannelPage::Archive), &model, studio),
        [AdminPart::Archived(scene.studio)]
    );
}

#[test]
fn the_archive_page_shows_each_state_of_its_collections() {
    let mut scene = scene();
    let archive = |scene: &Scene| match scene.page(AdminPage::Archive).2 {
        ServerPage::Archive(page) => page,
        _ => panic!("the archive page was opened"),
    };
    // A member: the list is on its way, and no settings.
    let page = archive(&scene);
    assert_eq!(page.channels, Listed::Loading);
    assert_eq!(page.settings, None);
    assert_eq!(page.direct_retention, None);
    assert!(!page.may_manage);

    scene.admin.archived = vec![(scene.studio, Collection::Failed(LoadFailure::Unavailable))];
    assert_eq!(
        archive(&scene).channels,
        Listed::Failed(LoadFailure::Unavailable)
    );

    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let archived = scene.archive(scene.plans);
    let page = archive(&scene);
    assert!(page.may_manage);
    let rows = page.channels.loaded().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, scene.plans);
    assert_eq!(rows[0].name, "plans");
    assert_eq!(rows[0].archived_at, archived.archived_at);
    assert_eq!(rows[0].deletes_at, archived.deletes_at);

    // An administrator reads the settings.
    scene.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
    let page = archive(&scene);
    assert_eq!(page.settings, Some(Listed::Loading));
    assert_eq!(page.direct_retention, Some(Listed::Loading));
    scene.admin.settings = vec![(
        scene.studio,
        Collection::Loaded(WorkspaceSettings {
            archive_period: ArchivePeriod { days: Some(30) },
            file_retention: Retention { days: None },
            recording_retention: Retention { days: Some(90) },
        }),
    )];
    scene.admin.direct_retention = Collection::Loaded(Retention { days: Some(14) });
    let page = archive(&scene);
    assert_eq!(
        page.settings,
        Some(Listed::Loaded(WorkspaceDays {
            archive_period: Some(30),
            file_retention: None,
            recording_retention: Some(90),
        }))
    );
    assert_eq!(page.direct_retention, Some(Listed::Loaded(Some(14))));
}

#[test]
fn the_settings_of_a_channel_show_what_the_roles_of_the_person_allow() {
    let mut scene = scene();
    // A member without "manage channel" has the page of the archive and the export only.
    let settings = scene
        .settings(scene.general, ChannelPage::Overview)
        .unwrap();
    assert_eq!(settings.pages, [ChannelPage::Archive]);
    assert_eq!(settings.selected, ChannelPage::Archive);
    assert!(!settings.may_manage && !settings.archived);
    let ChannelPageView::Archive(page) = &settings.page else {
        panic!("the page of the archive is shown");
    };
    assert_eq!(page.archived, None);
    assert!(!page.may_export && !page.permanent);

    scene.lead_holds(&[
        WorkspacePermission::ManageChannel,
        WorkspacePermission::ExportChannel,
    ]);
    let settings = scene
        .settings(scene.general, ChannelPage::Overview)
        .unwrap();
    assert_eq!(
        settings.pages,
        [
            ChannelPage::Overview,
            ChannelPage::Permissions,
            ChannelPage::Archive
        ]
    );
    assert_eq!(settings.name, "general");
    let ChannelPageView::Overview(page) = &settings.page else {
        panic!("the overview is shown");
    };
    assert_eq!(page.category_id, None);
    assert_eq!(page.categories, [(scene.projects, "Projects".to_owned())]);
    assert_eq!(page.voice, None);
    // The overrides are not read yet.
    assert_eq!(page.privacy, Privacy::Unknown);
    let settings = scene.settings(scene.general, ChannelPage::Archive).unwrap();
    assert!(matches!(&settings.page, ChannelPageView::Archive(page) if page.may_export));

    // A voice channel has the settings of its room.
    let daily = scene.daily;
    for channel in &mut scene.studio_mut().channels {
        if channel.id == daily {
            channel.user_limit = Some(8);
            channel.hides_occupants = true;
            channel.permanent_messages = true;
        }
    }
    let settings = scene.settings(daily, ChannelPage::Overview).unwrap();
    assert_eq!(settings.kind, ChannelKind::Voice);
    let ChannelPageView::Overview(page) = &settings.page else {
        panic!("the overview is shown");
    };
    assert!(page.permanent);
    assert_eq!(
        page.voice,
        Some(VoiceSettings {
            user_limit: Some(8),
            hides_occupants: true
        })
    );
}

#[test]
fn a_channel_is_private_when_its_everyone_role_is_denied_the_view() {
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let privacy = |scene: &Scene| match scene
        .settings(scene.general, ChannelPage::Overview)
        .unwrap()
        .page
    {
        ChannelPageView::Overview(page) => page.privacy,
        _ => panic!("the overview is shown"),
    };
    scene.admin.overrides = vec![(scene.general, Collection::Loading)];
    assert_eq!(privacy(&scene), Privacy::Unknown);
    scene.admin.overrides = vec![(scene.general, Collection::Loaded(Vec::new()))];
    assert_eq!(privacy(&scene), Privacy::Open);
    // A deny of another role, and another deny of the "everyone" role, keep it open.
    scene.admin.overrides = vec![(
        scene.general,
        Collection::Loaded(vec![
            ChannelOverride {
                role_id: scene.leads,
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
            ChannelOverride {
                role_id: scene.everyone,
                allow: Vec::new(),
                deny: vec![WorkspacePermission::MentionEveryone],
            },
        ]),
    )];
    assert_eq!(privacy(&scene), Privacy::Open);
    scene.admin.overrides = vec![(
        scene.general,
        Collection::Loaded(vec![ChannelOverride {
            role_id: scene.everyone,
            allow: Vec::new(),
            deny: vec![WorkspacePermission::ViewChannel],
        }]),
    )];
    assert_eq!(privacy(&scene), Privacy::Private);
}

#[test]
fn the_settings_of_an_archived_channel_show_its_archive_and_a_gone_channel_shows_nothing() {
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let archived = scene.archive(scene.plans);
    let settings = scene.settings(scene.plans, ChannelPage::Archive).unwrap();
    assert!(settings.archived);
    assert_eq!(settings.name, "plans");
    let ChannelPageView::Archive(page) = &settings.page else {
        panic!("the page of the archive is shown");
    };
    assert_eq!(
        page.archived,
        Some((archived.archived_at, archived.deletes_at))
    );

    // The channel is in no list: its settings have nothing to show.
    scene.admin.archived.clear();
    assert_eq!(scene.settings(scene.plans, ChannelPage::Archive), None);
    // A channel of another workspace is not the channel of these settings.
    let open = AdminOpen::Channel {
        server_id: scene.server_id,
        workspace_id: scene.lab,
        channel_id: scene.general,
        page: ChannelPage::Archive,
    };
    assert_eq!(scene.view(open), None);
    // A server that the client does not hold shows nothing.
    let open = AdminOpen::Server {
        server_id: ServerId::generate(),
        page: AdminPage::Workspaces,
    };
    assert_eq!(scene.view(open), None);
}

#[test]
fn the_view_tells_the_request_of_the_person() {
    let scene = scene();
    let open = AdminOpen::Server {
        server_id: scene.server_id,
        page: AdminPage::Workspaces,
    };
    let mut admin = AdminState {
        requests: vec![(
            1,
            Ask {
                what: AskKind::CreateWorkspace,
                state: AskState::Sending,
            },
        )],
        ..AdminState::default()
    };
    let view = derive(
        &[scene.server()],
        &Selection::default(),
        &Scene::known(),
        &admin,
        open,
    )
    .unwrap();
    assert!(view.sending(AskKind::CreateWorkspace));
    assert!(!view.sending(AskKind::RedeemInvite));
    assert_eq!(view.refused(AskKind::CreateWorkspace), None);
    admin.requests = vec![(
        2,
        Ask {
            what: AskKind::DeleteWorkspace(scene.lab),
            state: AskState::Refused("No.".to_owned()),
        },
    )];
    let view = derive(
        &[scene.server()],
        &Selection::default(),
        &Scene::known(),
        &admin,
        open,
    )
    .unwrap();
    assert_eq!(
        view.refused(AskKind::DeleteWorkspace(scene.lab)),
        Some("No.")
    );
    assert_eq!(view.refused(AskKind::DeleteWorkspace(scene.studio)), None);
    assert!(!view.sending(AskKind::DeleteWorkspace(scene.lab)));
}

#[test]
fn the_main_area_shows_a_selected_archived_channel_as_archived() {
    let mut scene = scene();
    scene.lead_holds(&[WorkspacePermission::ManageChannel]);
    let archived = scene.archive(scene.plans);
    let mut model = scene.model();
    // The route of the channel stated what the account holds in it (ADR 0790).
    Arc::make_mut(&mut model.admin).channel_permissions = vec![(
        scene.plans,
        Collection::Loaded(vec![
            WorkspacePermission::ViewChannel,
            WorkspacePermission::ManageChannel,
        ]),
    )];
    model.timelines.push(Arc::new(Timeline {
        place: PlaceId::Channel(scene.plans),
        messages: Vec::new(),
        more_before: false,
        more_after: false,
        reading: Reading::default(),
        loaded: true,
        failed: None,
        stale: false,
        unread_after: None,
        root: None,
        forks: Vec::new(),
        anchor: None,
    }));
    let mut server = scene.server();
    server.model = Some(Arc::new(model));
    let mut selection = Selection::default();
    selection
        .channels
        .insert((scene.server_id, scene.studio), scene.plans);
    let view = crate::state::view::derive(&[server.clone()], &selection, &Scene::known());
    let workspace = view.workspace().unwrap();
    let channel = workspace.channel.as_ref().unwrap();
    assert_eq!((channel.id, channel.name.as_str()), (scene.plans, "plans"));
    assert!(channel.selected && channel.voice.is_none());
    // The channel is in no section of the channel list.
    assert!(
        workspace
            .sections
            .iter()
            .all(|section| section.rows.iter().all(|row| row.id != scene.plans))
    );
    let state = workspace.archived.unwrap();
    assert_eq!(state.archived_at, archived.archived_at);
    assert_eq!(state.deletes_at, archived.deletes_at);
    assert!(state.may_restore);
    assert!(view.lists().unwrap().feed.is_some());

    // Another selection shows a channel of the list, which is not archived.
    let view = crate::state::view::derive(&[server], &Selection::default(), &Scene::known());
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.channel.as_ref().unwrap().id, scene.general);
    assert_eq!(workspace.archived, None);
}

#[test]
fn the_thread_and_the_pins_of_an_archived_channel_offer_no_change() {
    use junto_client_core::model::PinList;
    use junto_client_core::protocol::ids::MessageId;
    use junto_client_core::protocol::message::{Block, Document, Inline, Message, MessagePlace};

    let mut scene = scene();
    scene.lead_holds(&[
        WorkspacePermission::ManageChannel,
        WorkspacePermission::ManageMessages,
    ]);
    scene.archive(scene.plans);
    let place = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.plans,
    };
    let message = |position: u64, thread_root: Option<MessageId>| Message {
        id: MessageId::generate(),
        place,
        position,
        author_id: Some(scene.own),
        author_name: Some("Luca".to_owned()),
        created_at: time(),
        edited_at: None,
        document: Some(Document {
            blocks: vec![Block::Paragraph {
                content: vec![Inline::Text {
                    text: "text".to_owned(),
                    marks: Vec::new(),
                }],
            }],
        }),
        system: None,
        reply_to: None,
        permanent: false,
        deleted: false,
        attachments: Vec::new(),
        reactions: Vec::new(),
        thread_root,
        reply_count: 0,
        last_reply: None,
        reply_authors: Vec::new(),
        pinned: false,
        files_removed_by_retention: false,
    };
    let mut root = message(1, None);
    root.reply_count = 1;
    root.pinned = true;
    let reply = message(2, Some(root.id));
    let timeline = |place: PlaceId, messages: Vec<Message>, root: Option<&Message>| Timeline {
        place,
        messages: messages.into_iter().map(Arc::new).collect(),
        more_before: false,
        more_after: false,
        reading: Reading::default(),
        loaded: true,
        failed: None,
        stale: false,
        unread_after: None,
        root: root.cloned().map(Arc::new),
        forks: Vec::new(),
        anchor: None,
    };
    let thread = PlaceId::Thread {
        channel_id: scene.plans,
        root_id: root.id,
    };
    let mut model = scene.model();
    model.timelines = vec![
        Arc::new(timeline(
            PlaceId::Channel(scene.plans),
            vec![root.clone()],
            None,
        )),
        Arc::new(timeline(thread, vec![reply], Some(&root))),
    ];
    model.pins = vec![PinList {
        channel_id: scene.plans,
        messages: junto_client_core::model::Loadable::Loaded(vec![Arc::new(root.clone())]),
        failed: None,
    }];
    let mut server = scene.server();
    server.model = Some(Arc::new(model));
    let mut selection = Selection::default();
    selection
        .channels
        .insert((scene.server_id, scene.studio), scene.plans);
    selection.thread = Some((scene.server_id, scene.plans, root.id));
    let offers = |row: &crate::state::feed::MessageRow| {
        row.can_edit || row.can_delete || row.can_pin || row.can_thread
    };
    let messages = |feed: &crate::state::feed::Feed| -> Vec<crate::state::feed::MessageRow> {
        feed.rows
            .iter()
            .filter_map(|row| match row {
                crate::state::feed::FeedRow::Message(row) => Some(row.clone()),
                _ => None,
            })
            .collect()
    };
    let view = crate::state::view::derive(&[server.clone()], &selection, &Scene::known());
    let workspace = view.workspace().unwrap();
    assert!(workspace.archived.is_some());
    let panel = workspace.thread.as_ref().unwrap();
    assert!(!panel.may_fork, "an archived channel takes no fork");
    assert!(!offers(panel.root.as_ref().unwrap()));
    let replies = messages(panel.feed.as_ref().unwrap());
    assert_eq!(replies.len(), 1);
    assert!(replies.iter().all(|row| !offers(row)));

    // The pin list of the archived channel offers no unpin.
    selection.thread = None;
    selection.pins = Some((scene.server_id, scene.plans));
    let view = crate::state::view::derive(&[server], &selection, &Scene::known());
    let pins = view.workspace().unwrap().pins.as_ref().unwrap();
    let crate::state::view::PinsState::Loaded(rows) = &pins.state else {
        panic!("the pin list is loaded");
    };
    assert_eq!(rows.len(), 1);
    assert!(rows.iter().all(|row| !offers(row)));
}

#[test]
fn an_account_without_the_administrator_permission_has_the_page_of_its_own_tokens() {
    let scene = scene();
    let (nav, selected, page) = scene.page(AdminPage::Tokens);
    assert_eq!(selected, AdminPage::Tokens);
    assert!(matches!(page, ServerPage::Tokens(_)));
    // The page stands with the things of the account, not with the things of the server.
    let own = nav
        .iter()
        .find(|group| group.title == NavTitle::Own)
        .unwrap();
    assert_eq!(own.pages, [AdminPage::Tokens, AdminPage::Exports]);
    assert!(!nav[0].pages.contains(&AdminPage::Tokens));
}

#[test]
fn an_archived_voice_channel_offers_no_recording_removal() {
    use crate::state::feed::FeedRow;
    use junto_client_core::protocol::ids::{MessageId, RecordingId};
    use junto_client_core::protocol::message::{Message, MessagePlace, SystemMessage};

    for archived in [false, true] {
        let mut scene = scene();
        scene.lead_holds(&[WorkspacePermission::ManageRecordings]);
        if archived {
            scene.archive(scene.daily);
        }
        let mut model = scene.model();
        let message = Message {
            id: MessageId::generate(),
            place: MessagePlace::Channel {
                workspace_id: scene.studio,
                channel_id: scene.daily,
            },
            position: 1,
            author_id: None,
            author_name: None,
            created_at: time(),
            edited_at: None,
            document: None,
            system: Some(SystemMessage {
                action: "recording_ready".to_owned(),
                subject_id: None,
                actor_id: None,
                recording_id: Some(RecordingId::generate()),
                duration_seconds: None,
                name: None,
                previous_name: None,
            }),
            reply_to: None,
            permanent: false,
            deleted: false,
            attachments: Vec::new(),
            reactions: Vec::new(),
            thread_root: None,
            reply_count: 0,
            last_reply: None,
            pinned: false,
            files_removed_by_retention: false,
            reply_authors: Vec::new(),
        };
        model.timelines.push(Arc::new(Timeline {
            place: PlaceId::Channel(scene.daily),
            messages: vec![Arc::new(message)],
            more_before: false,
            more_after: false,
            reading: Reading::default(),
            loaded: true,
            failed: None,
            stale: false,
            unread_after: None,
            root: None,
            forks: Vec::new(),
            anchor: None,
        }));
        let mut server = scene.server();
        server.model = Some(Arc::new(model));
        let mut selection = Selection::default();
        selection
            .channels
            .insert((scene.server_id, scene.studio), scene.daily);
        let view = crate::state::view::derive(&[server], &selection, &Scene::known());
        let feed = view.lists().unwrap().feed.as_ref().unwrap();
        let card = feed
            .rows
            .iter()
            .find_map(|row| match row {
                FeedRow::Recording(row) => Some(&row.card),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            view.may_remove_recording(scene.server_id, card.recording_id),
            !archived
        );
        assert_eq!(card.may_remove, !archived);
    }
}

#[test]
fn every_member_has_the_page_of_the_recordings_of_the_shown_workspace() {
    let scene = scene();
    let (nav, selected, page) = scene.page(AdminPage::Recordings);
    assert_eq!(selected, AdminPage::Recordings);
    let ServerPage::Recordings(page) = page else {
        panic!("the page of the recordings was opened");
    };
    assert_eq!(page.workspace_id, scene.studio);
    assert_eq!(page.workspace_name, "Studio");
    assert_eq!(page.state, recordings::RecordingsRead::NotRead);
    // The page stands with the things of the workspace.
    assert!(nav[1].pages.contains(&AdminPage::Recordings));
    assert!(!nav[0].pages.contains(&AdminPage::Recordings));

    // An account that sees no workspace has no such page: the first page is shown.
    let mut scene = self::scene();
    scene.snapshot.workspaces.clear();
    scene.snapshot.account.workspace_ids.clear();
    let (nav, selected, page) = scene.page(AdminPage::Recordings);
    assert!(!pages(&nav).contains(&AdminPage::Recordings));
    assert_eq!(selected, AdminPage::Workspaces);
    assert!(matches!(page, ServerPage::Workspaces(_)));
}

#[test]
fn every_member_reads_the_glossary_and_the_page_tells_why_it_takes_no_change() {
    let mut scene = scene();
    let studio = Some(scene.studio);
    let model = scene.model();
    let open = AdminOpen::Server {
        server_id: scene.server_id,
        page: AdminPage::Glossary,
    };
    assert_eq!(
        needs(&open, &model, studio),
        [AdminPart::Glossary(scene.studio)]
    );
    assert!(needs(&open, &model, None).is_empty());
    let glossary = |scene: &Scene, admin: &AdminState| match derive(
        &[scene.server()],
        &Selection::default(),
        &Scene::known(),
        admin,
        open,
    )
    .unwrap()
    .scope
    {
        Scope::Server {
            page: ServerPage::Glossary(page),
            ..
        } => page,
        other => panic!("the glossary page was not shown: {other:?}"),
    };
    let text = crate::text::admin::glossary::NOT_READ;
    let page = glossary(&scene, &AdminState::default());
    assert_eq!(page.terms, Listed::Loading);
    assert_eq!(page.workspace_name, "Studio");
    // A member reads it without "manage glossary".
    assert_eq!(
        page.held(),
        Some(crate::text::admin::glossary::MANAGE_NEEDS)
    );
    scene.lead_holds(&[WorkspacePermission::ManageGlossary]);
    assert_eq!(glossary(&scene, &AdminState::default()).held(), Some(text));

    let terms = vec!["Quorvex".to_owned(), "Lumitrax".to_owned()];
    scene.admin.glossaries = vec![(
        scene.studio,
        Collection::Loaded(GlossaryList {
            terms: terms.clone(),
            cut: false,
        }),
    )];
    let page = glossary(&scene, &AdminState::default());
    assert_eq!(page.terms, Listed::Loaded(terms.clone()));
    assert_eq!(page.held(), None);

    // After a change of the person, the page shows the list of its answer until the state
    // shows a read that started after the answer arrived.
    let answered_at = std::time::Instant::now();
    let admin = AdminState {
        glossary_answered: Some(crate::state::admin::glossary::Answered {
            server_id: scene.server_id,
            workspace_id: scene.studio,
            terms: vec!["Quorvex".to_owned()],
            at: answered_at,
        }),
        ..AdminState::default()
    };
    let part = AdminPart::Glossary(scene.studio);
    let read = |shown| junto_client_core::model::ReadTimes {
        started: shown,
        shown,
    };
    for shown in [None, Some(answered_at)] {
        scene.admin.read_times = vec![(part, read(shown))];
        let page = glossary(&scene, &admin);
        assert_eq!(page.terms, Listed::Loaded(vec!["Quorvex".to_owned()]));
        assert_eq!(page.held(), None);
    }
    let later = answered_at + std::time::Duration::from_millis(1);
    scene.admin.read_times = vec![(part, read(Some(later)))];
    assert_eq!(
        glossary(&scene, &admin).terms,
        Listed::Loaded(terms.clone())
    );

    // A list that the client holds in part takes no change.
    scene.admin.glossaries = vec![(
        scene.studio,
        Collection::Loaded(GlossaryList { terms, cut: true }),
    )];
    let page = glossary(&scene, &AdminState::default());
    assert!(page.cut);
    assert_eq!(page.held(), Some(crate::text::admin::glossary::CUT));
}

/// A page of the channels: "x" without a category, which the person manages, and the
/// category "Projects" with the channels at `positions`, which the person does not manage.
/// The identifiers order equal positions: "x" is 75, and the channels of `positions` have
/// the identifiers that `ids` states.
fn page_of_moves(
    positions: &[i32],
    ids: &[u128],
) -> (ChannelsPage, ChannelId, CategoryId, Vec<ChannelId>) {
    let x = ChannelId::from_uuid(uuid::Uuid::from_u128(75));
    let projects = CategoryId::generate();
    let line = |id, name: &str, category_id, position, may_manage| ChannelLine {
        id,
        name: name.to_owned(),
        kind: ChannelKind::Text,
        category_id,
        position,
        place: ChannelPlace {
            category_id,
            position,
        },
        permanent: false,
        first: false,
        last: false,
        may_manage,
        moves_up: Err(Unplanned::Nothing),
        moves_down: Err(Unplanned::Nothing),
    };
    let others: Vec<ChannelId> = ids
        .iter()
        .map(|id| ChannelId::from_uuid(uuid::Uuid::from_u128(*id)))
        .collect();
    let page = ChannelsPage {
        workspace_id: WorkspaceId::generate(),
        workspace_name: "Studio".to_owned(),
        may_manage: true,
        groups: vec![
            ChannelGroup {
                category: None,
                channels: vec![line(x, "x", None, 0, true)],
            },
            ChannelGroup {
                category: Some(CategoryLine {
                    id: projects,
                    name: "Projects".to_owned(),
                    position: 0,
                    first: true,
                    last: true,
                }),
                channels: others
                    .iter()
                    .zip(positions)
                    .map(|(id, position)| line(*id, "other", Some(projects), *position, false))
                    .collect(),
            },
        ],
        refusal: None,
    };
    (page, x, projects, others)
}

#[test]
fn a_move_names_where_each_channel_was_read_and_needs_every_channel_that_it_changes() {
    // Between two channels that the person does not manage there is a free position: only
    // the moved channel changes, and the move is offered. The two channels next to the
    // place follow as checks of where the page read them, which need no "manage channel".
    let (page, x, projects, others) = page_of_moves(&[10, 20], &[100, 50]);
    let between = page.plan(x, Some(projects), 1).unwrap();
    let at = |position| ChannelPlace {
        category_id: Some(projects),
        position,
    };
    let check = |channel_id, position| ChannelMove {
        channel_id,
        from: at(position),
        to: at(position),
    };
    assert_eq!(
        between,
        [
            ChannelMove {
                channel_id: x,
                from: ChannelPlace {
                    category_id: None,
                    position: 0,
                },
                to: at(11),
            },
            check(others[0], 10),
            check(others[1], 20),
        ]
    );
    // Every place of the group is offered: before the first, between, after the last. At
    // an end one channel is next to the place.
    assert!((0..=2).all(|to| page.takes(x, Some(projects), to)));
    assert_eq!(
        page.plan(x, Some(projects), 2).unwrap()[1..],
        [check(others[1], 20)]
    );
    // Without a free position, and with identifiers that do not order "x" beside either
    // channel at its position, the other channels would change: no move is offered.
    let (page, x, projects, _) = page_of_moves(&[10, 11], &[100, 50]);
    assert_eq!(page.plan(x, Some(projects), 1), Err(Unplanned::NotManaged));
    assert!(!page.takes(x, Some(projects), 1));
    // With identifiers that order "x" after the first channel, it shares its position.
    let (page, x, projects, _) = page_of_moves(&[10, 11], &[60, 50]);
    assert_eq!(
        page.plan(x, Some(projects), 1).unwrap()[0].to,
        ChannelPlace {
            category_id: Some(projects),
            position: 10,
        }
    );
    // A drop at its own place moves nothing.
    assert_eq!(page.plan(x, None, 0), Err(Unplanned::Nothing));
}

#[test]
fn a_move_takes_the_way_that_changes_only_channels_that_the_person_manages() {
    // "a" and "b" share position 0 and "c" is at 1; the person manages "z" and "b" only.
    // Between "a" and "b", "z" takes 0 and "b" moves to 1, beside "c": "a" and "c" stay.
    let projects = CategoryId::generate();
    let id = |value: u128| ChannelId::from_uuid(uuid::Uuid::from_u128(value));
    let (a, b, c, z) = (id(1), id(2), id(3), id(26));
    let line =
        |id: ChannelId, category_id: Option<CategoryId>, position: i32, may_manage| ChannelLine {
            id,
            name: "channel".to_owned(),
            kind: ChannelKind::Text,
            category_id,
            position,
            place: ChannelPlace {
                category_id,
                position,
            },
            permanent: false,
            first: false,
            last: false,
            may_manage,
            moves_up: Err(Unplanned::Nothing),
            moves_down: Err(Unplanned::Nothing),
        };
    let page = ChannelsPage {
        workspace_id: WorkspaceId::generate(),
        workspace_name: "Studio".to_owned(),
        may_manage: true,
        groups: vec![
            ChannelGroup {
                category: None,
                channels: vec![line(z, None, 0, true)],
            },
            ChannelGroup {
                category: Some(CategoryLine {
                    id: projects,
                    name: "Projects".to_owned(),
                    position: 0,
                    first: true,
                    last: true,
                }),
                channels: vec![
                    line(a, Some(projects), 0, false),
                    line(b, Some(projects), 0, true),
                    line(c, Some(projects), 1, false),
                ],
            },
        ],
        refusal: None,
    };
    let at = |position| ChannelPlace {
        category_id: Some(projects),
        position,
    };
    let moves = page.plan(z, Some(projects), 1).unwrap();
    let changed: Vec<(ChannelId, ChannelPlace)> = moves
        .iter()
        .filter(|held| held.to != held.from)
        .map(|held| (held.channel_id, held.to))
        .collect();
    assert_eq!(changed, [(z, at(0)), (b, at(1))]);
    // "a" before the place and "b" after it are checked; "b" through its own move.
    assert!(
        moves
            .iter()
            .any(|held| held.channel_id == a && held.to == held.from)
    );
}

#[test]
fn a_move_names_at_most_the_channels_that_one_request_names() {
    // "x" without a category, and a category of 401 channels at consecutive positions, all
    // managed by the person, whose identifiers fall by 2 from one channel to the next: "x"
    // fits beside no channel at its position at the place 201.
    let projects = CategoryId::generate();
    let id = |value: u128| ChannelId::from_uuid(uuid::Uuid::from_u128(value));
    let line = |id: ChannelId, category_id: Option<CategoryId>, position: i32| ChannelLine {
        id,
        name: format!("c{position}"),
        kind: ChannelKind::Text,
        category_id,
        position,
        place: ChannelPlace {
            category_id,
            position,
        },
        permanent: false,
        first: false,
        last: false,
        may_manage: true,
        moves_up: Err(Unplanned::Nothing),
        moves_down: Err(Unplanned::Nothing),
    };
    let x = id(1599);
    let page = ChannelsPage {
        workspace_id: WorkspaceId::generate(),
        workspace_name: "Studio".to_owned(),
        may_manage: true,
        groups: vec![
            ChannelGroup {
                category: None,
                channels: vec![line(x, None, 0)],
            },
            ChannelGroup {
                category: Some(CategoryLine {
                    id: projects,
                    name: "Projects".to_owned(),
                    position: 0,
                    first: true,
                    last: true,
                }),
                channels: (0..=400)
                    .map(|at| line(id(2 * (1000 - at as u128)), Some(projects), at))
                    .collect(),
            },
        ],
        refusal: None,
    };
    // To the end and to the first place: one move, and the check of the channel next to it.
    assert_eq!(page.plan(x, Some(projects), 401).unwrap().len(), 2);
    assert_eq!(page.plan(x, Some(projects), 0).unwrap().len(), 2);
    // Elsewhere "x" shares the position of a channel next to it: one move and two checks.
    assert_eq!(page.plan(x, Some(projects), 300).unwrap().len(), 3);
    // At the place 201, every way moves at least 200 channels besides "x", and with the two
    // checks the request would name more than 200: the place is not offered, and the page
    // states why.
    assert_eq!(page.plan(x, Some(projects), 201), Err(Unplanned::TooMany));
    assert!(!page.takes(x, Some(projects), 201));
    assert!(
        crate::text::admin::MOVE_TOO_MANY.contains(&MOST_CHANNEL_MOVES.to_string()),
        "the reason names the bound of the route"
    );
    // While a refused move is drawn, no move is offered.
    let shown = ChannelsPage {
        refusal: Some((x, "refused".to_owned())),
        ..page
    };
    assert_eq!(
        shown.plan(x, Some(projects), 401),
        Err(Unplanned::RefusalShown)
    );
}

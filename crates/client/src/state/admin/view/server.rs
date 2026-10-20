//! The page of the server itself: its logo (ADR 0101) and the limits of the files of
//! messages (ADR 0408), which an administrator changes.

use std::sync::Arc;

use gpui::RenderImage;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::message::AttachmentLimits;

use super::Listed;

use crate::state::view::{Known, ServerSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettingsPage {
    /// The name of the server. It names the image of the logo.
    pub name: String,
    /// The server has a logo now, as far as this device knows: an event stated one, or no
    /// event stated the logo and the server book names one or a read found one.
    pub has_logo: bool,
    /// The image of the logo, when the window holds it.
    pub image: Option<Arc<RenderImage>>,
    /// The person is an administrator: set and remove.
    pub may_change: bool,
    /// The largest file and the number of files of a message that the server accepts.
    pub limits: Listed<AttachmentLimits>,
}

pub fn settings(server: &ServerSnapshot, model: &ServerModel, known: &Known) -> ServerSettingsPage {
    let logo = known.logos.of(server.id);
    ServerSettingsPage {
        name: server.name.clone(),
        has_logo: logo.exists,
        image: logo.image,
        may_change: model.is_administrator(),
        limits: Listed::of(&model.admin.attachment_limits, Clone::clone),
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::Reach;
    use junto_client_core::flow::access::AccessState;
    use junto_client_core::live::StreamStatus;
    use junto_client_core::protocol::ServerPermission;
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{AccountId, DeviceId, ServerId, StreamEpoch};
    use junto_client_core::protocol::snapshot::Snapshot;
    use junto_client_core::version::SupportedVersions;
    use time::OffsetDateTime;

    use super::*;
    use crate::state::admin::view::{NavTitle, Scope, ServerPage, derive, nav};
    use crate::state::admin::{AdminOpen, AdminPage, AdminState};
    use crate::state::logo::{Logo, Logos, Told};
    use crate::state::people::{AVATAR_SIDE, PictureHash, decode};
    use crate::state::view::Selection;

    fn time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn logo() -> Logo {
        let picture = image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 40, 200, 255]));
        let mut bytes = Vec::new();
        picture
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        Logo {
            hash: PictureHash::of(&bytes),
            image: decode(&bytes, AVATAR_SIDE).unwrap(),
        }
    }

    /// The state of a server without a workspace for this account.
    fn model(account: OwnAccount) -> ServerModel {
        ServerModel::from_snapshot(Snapshot {
            cursor: Cursor {
                epoch: StreamEpoch::generate(),
                position: 1,
            },
            account,
            workspaces: Vec::new(),
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
        })
    }

    fn account(is_owner: bool, server_permissions: Vec<ServerPermission>) -> OwnAccount {
        OwnAccount {
            id: AccountId::generate(),
            name: "Luca".to_owned(),
            avatar: None,
            is_owner,
            server_role_ids: Vec::new(),
            server_permissions,
            workspace_ids: Vec::new(),
        }
    }

    fn server(model: ServerModel) -> ServerSnapshot {
        ServerSnapshot {
            id: ServerId::generate(),
            name: "Acme Studio".to_owned(),
            address: "acme.example:7443".to_owned(),
            keep_connected: true,
            reach: Reach::Reachable {
                version: SupportedVersions::THIS_BUILD
                    .negotiate(&[SupportedVersions::THIS_BUILD.current()])
                    .unwrap(),
            },
            access: AccessState::LoggedIn {
                account_id: model.account.id,
                device_id: DeviceId::generate(),
                session_expires_at: time(),
                recoveries: Vec::new(),
                previous_account: None,
            },
            in_progress: None,
            error: None,
            stream: StreamStatus::Live,
            model: Some(Arc::new(model)),
        }
    }

    /// The page "Server" as the administration of `server` shows it with these logos.
    fn page(server: &ServerSnapshot, logos: &Logos) -> (AdminPage, ServerPage) {
        let known = Known {
            logos: logos.shown(),
            ..Known::default()
        };
        let open = AdminOpen::Server {
            server_id: server.id,
            page: AdminPage::Server,
        };
        let view = derive(
            std::slice::from_ref(server),
            &Selection::default(),
            &known,
            &AdminState::default(),
            open,
        )
        .unwrap();
        match view.scope {
            Scope::Server { selected, page, .. } => (selected, page),
            Scope::Channel(_) => panic!("a page of the server was opened"),
        }
    }

    fn settings_of(server: &ServerSnapshot, logos: &Logos) -> ServerSettingsPage {
        match page(server, logos) {
            (AdminPage::Server, ServerPage::Server(page)) => page,
            other => panic!("the page of the server is not shown: {other:?}"),
        }
    }

    #[test]
    fn the_page_of_the_server_shows_the_logo_that_the_window_holds() {
        let server = server(model(account(true, Vec::new())));
        let held = logo();
        let mut logos = Logos::default();
        logos.hold(server.id, held.clone());
        assert_eq!(
            settings_of(&server, &logos),
            ServerSettingsPage {
                name: "Acme Studio".to_owned(),
                has_logo: true,
                image: Some(held.image),
                may_change: true,
                limits: Listed::Loading,
            }
        );
    }

    #[test]
    fn the_page_states_a_logo_whose_image_the_window_does_not_hold() {
        let server = server(model(account(true, Vec::new())));
        // The server book names a logo, and its image is not read yet.
        let mut logos = Logos::default();
        logos.want(server.id, Told::NotStated { named: true });
        let page = settings_of(&server, &logos);
        assert!(page.has_logo);
        assert_eq!(page.image, None);
        // An event stated a logo.
        let mut logos = Logos::default();
        logos.want(server.id, Told::Present(Some(logo().hash)));
        assert!(settings_of(&server, &logos).has_logo);
    }

    #[test]
    fn the_page_of_a_server_without_a_logo_has_none() {
        let server = server(model(account(true, Vec::new())));
        // Nothing is known of the server, the server book names no logo, and an event
        // stated that the logo was removed.
        let nothing = Logos::default();
        let mut not_named = Logos::default();
        not_named.want(server.id, Told::NotStated { named: false });
        let mut removed = Logos::default();
        removed.hold(server.id, logo());
        removed.want(server.id, Told::Absent);
        for logos in [nothing, not_named, removed] {
            let page = settings_of(&server, &logos);
            assert!(!page.has_logo);
            assert_eq!(page.image, None);
        }
    }

    #[test]
    fn only_an_administrator_has_the_page_of_the_server() {
        let pages = |account: OwnAccount| {
            let groups = nav(&model(account), None);
            assert_eq!(groups[0].title, NavTitle::Server);
            groups[0].pages.clone()
        };
        // The page is the second one of the group of the server.
        let owner = pages(account(true, Vec::new()));
        assert_eq!(owner[..2], [AdminPage::Workspaces, AdminPage::Server]);
        let administrator = pages(account(false, vec![ServerPermission::Administrator]));
        assert_eq!(
            administrator[..2],
            [AdminPage::Workspaces, AdminPage::Server]
        );
        let member = pages(account(false, vec![ServerPermission::CreateInvites]));
        assert!(!member.contains(&AdminPage::Server));

        // An administrator may change the logo.
        let server_of = |account: OwnAccount| server(model(account));
        let administrator = server_of(account(false, vec![ServerPermission::Administrator]));
        assert!(settings_of(&administrator, &Logos::default()).may_change);
        // The page of a person who is no administrator gives way to the first one.
        let member = server_of(account(false, Vec::new()));
        let (selected, shown) = page(&member, &Logos::default());
        assert_eq!(selected, AdminPage::Workspaces);
        assert!(matches!(shown, ServerPage::Workspaces(_)));
    }
}

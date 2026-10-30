use super::*;
use crate::admin_pages::{invite, joined};

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_group_call_carries_sound_and_keeps_its_chat_when_detached(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Group calls");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
    let group = world
        .run(admin.handle.api().create_group(
            "Project".to_owned(),
            vec![window.own_account(), third.account().unwrap()],
        ))
        .unwrap();
    window.until("group row", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    window.click_named(format!("direct-row:{}", group.id));
    window.until("group call control", |window| {
        window.shows("start-group-call")
    });
    window.click("start-group-call");
    window.until("group invitation", |_| {
        admin.handle.live().view().model.is_some_and(|model| {
            model.incoming_invitations().any(|invitation| {
                invitation.place
                    == CallPlace::Group {
                        conversation_id: group.id,
                    }
            })
        })
    });
    let invitation = admin
        .handle
        .live()
        .view()
        .model
        .unwrap()
        .incoming_invitations()
        .find(|invitation| {
            invitation.place
                == CallPlace::Group {
                    conversation_id: group.id,
                }
        })
        .unwrap()
        .clone();
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Group {
            conversation_id: group.id,
        },
    };
    world
        .run(caller.calls.join_invited(target, invitation.id))
        .unwrap();
    window.until("group connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Window,
        "joining the group",
    );
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Administrator,
        "joining the group",
    );
    assert!(matches!(
        window.view().lists().unwrap().call.as_ref().unwrap().place,
        junto_client::state::view::PanelPlace::Direct(_)
    ));
    window.click("detach-call");
    window.until("group detached", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.call_view())
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .unwrap()
            .id,
        group.id
    );
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.reads_messages(
                admin.handle.id(),
                junto_client_core::model::PlaceId::Direct(group.id)
            ))
    );
    window.click("return-call");
    window.until("group returned", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
    window.until("group call ended", |window| window.view().call.is_none());
    assert_eq!(
        window
            .view()
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .unwrap()
            .id,
        group.id
    );
}

use super::*;
use junto_client_core::protocol::ids::ConversationId;
use junto_client_core::protocol::message::{MessagePlace, MessageState};

/// The conversation of Luca and Ana.
fn conversation() -> ConversationId {
    id(900)
}

/// "Acme Studio" with the conversation of Luca and Ana and their direct-message call. Ana
/// is in the call when `answered` holds.
fn acme_with_ana(answered: bool) -> ServerSnapshot {
    let (luca, ana) = (person("Luca"), person("Ana"));
    let place = CallPlace::direct(luca, ana);
    let CallPlace::Direct {
        low_account_id,
        high_account_id,
    } = place
    else {
        return acme();
    };
    let place_state = MessageState {
        place: MessagePlace::Direct {
            conversation_id: conversation(),
            low_account_id,
            high_account_id,
        },
        last_read: 0,
        latest: 0,
        unread: 0,
        mentions: 0,
        unread_sign: false,
        notification_mentions: 0,
        muted: false,
    };
    let call = Call {
        id: id(901),
        place,
        started_at: Some(at()),
        lock: None,
        meeting: None,
        hides_occupants: false,
        participants: if answered {
            vec![participant("Luca"), participant("Ana")]
        } else {
            vec![participant("Luca")]
        },
        recording: None,
    };
    let mut server = acme();
    server.model = Some(acme_model_of(
        |_| {},
        move |snapshot| {
            snapshot.messages.push(place_state);
            snapshot.direct_calls.push(call);
        },
    ));
    server
}

/// The state of the calls while this device is in the direct-message call with Ana.
fn with_ana() -> MediaSnapshot {
    with_call(in_call(Phase::Connected), |call| {
        call.target = CallTarget {
            server_id: id(1),
            place: CallPlace::direct(person("Luca"), person("Ana")),
        };
    })
}

fn show_conversation(
    tab: CallTab,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) {
    move |shell, window, cx| {
        shell.state().clone().update(cx, |state, cx| {
            state.select_conversation(id(1), conversation(), cx)
        });
        shell.open(Open::CallTab(tab), window, cx);
    }
}

/// The page of a direct-message call (ADR 0735).
pub(super) fn draw(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::call("direct-call-ringing", with_ana()).servers(vec![acme_with_ana(false)]),
        show_conversation(CallTab::Members),
    )?;
    studio.draw(
        Screen::call("direct-call-chat", with_ana()).servers(vec![acme_with_ana(true)]),
        show_conversation(CallTab::Chat),
    )?;
    studio.draw(
        Screen::call("direct-call-members", with_ana()).servers(vec![acme_with_ana(true)]),
        show_conversation(CallTab::Members),
    )?;
    studio.draw(
        Screen::call("direct-call-settings", with_ana()).servers(vec![acme_with_ana(true)]),
        show_conversation(CallTab::Settings),
    )
}

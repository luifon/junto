use super::*;
use junto_client_core::{
    book::EntryKey,
    protocol::{ids::ConversationId, message::GroupConversation},
};

#[test]
fn membership_pruning_removes_unexpired_group_activity_without_changing_other_groups() {
    let mut model = crate::state::fixtures::model_at(1);
    let server = ServerId::generate();
    let own = model.account.id;
    let first = AccountId::generate();
    let second = AccountId::generate();
    let group = ConversationId::generate();
    let other = ConversationId::generate();
    model.group_conversations = [group, other]
        .into_iter()
        .map(|id| GroupConversation {
            id,
            name: "Project".to_owned(),
            account_ids: vec![own, first, second],
        })
        .collect();
    let owner = Owner {
        entry: EntryKey {
            server_id: server,
            generation: 1,
        },
        account_id: own,
    };
    let now = Instant::now();
    let until = now + Duration::from_millis(u64::from(TYPING_LIFETIME_MS));
    let mut activity = Activity::default();
    for (place, account) in [(group, first), (group, second), (other, first)] {
        activity.people.insert(
            (server, PlaceId::Direct(place), account),
            Person {
                owner,
                name: "Writer".to_owned(),
                until,
            },
        );
    }
    activity.keep(&HashMap::from([(server, (owner, Some(&model)))]), now);
    assert_eq!(activity.people.len(), 3);
    model.group_conversations[0]
        .account_ids
        .retain(|account| *account != first);
    assert!(activity.people[&(server, PlaceId::Direct(group), first)].until > now);
    activity.keep(&HashMap::from([(server, (owner, Some(&model)))]), now);
    assert!(
        !activity
            .people
            .contains_key(&(server, PlaceId::Direct(group), first))
    );
    assert!(
        activity
            .people
            .contains_key(&(server, PlaceId::Direct(group), second))
    );
    assert!(
        activity
            .people
            .contains_key(&(server, PlaceId::Direct(other), first))
    );
    model.group_conversations.remove(0);
    model.account.is_owner = true;
    assert!(activity.people[&(server, PlaceId::Direct(group), second)].until > now);
    activity.keep(&HashMap::from([(server, (owner, Some(&model)))]), now);
    assert!(
        !activity
            .people
            .contains_key(&(server, PlaceId::Direct(group), second))
    );
    assert!(
        activity
            .people
            .contains_key(&(server, PlaceId::Direct(other), first))
    );
    model.group_conversations[0]
        .account_ids
        .retain(|account| *account != own);
    assert!(activity.people[&(server, PlaceId::Direct(other), first)].until > now);
    activity.keep(&HashMap::from([(server, (owner, Some(&model)))]), now);
    assert!(activity.people.is_empty());
}

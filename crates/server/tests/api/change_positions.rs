//! What the positions of the events of one change tell a recipient (ADR 0010, ADR 0011).

use junto_protocol::events::{Event, Level};
use junto_protocol::ids::AccountId;

use crate::support::{Member, TestServer};

/// Records in one transaction one event for each account of `accounts`, in that order.
async fn one_change(server: &TestServer, owner: &Member, accounts: &[AccountId]) {
    let mut transaction = server.db.begin().await.unwrap();
    for (index, account) in accounts.iter().enumerate() {
        let event = Event::DeviceRenamed {
            device_id: owner.device_id,
            name: format!("name {index}"),
        };
        junto_server::events::tell_account(&mut transaction, server.clock.now(), *account, &event)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
}

/// The positions of the events that one recipient receives from a change, counted from the
/// cursor before the change, and the distance of the snapshot after it.
async fn observed(hidden: usize) -> (Vec<u64>, u64) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let before = server.snapshot(&owner).await.cursor;
    let mut stream = server.stream(&owner).await;
    stream.subscribe(Level::Full, Some(before)).await;
    let mut accounts = vec![owner.account_id];
    accounts.extend(std::iter::repeat_n(other.account_id, hidden));
    accounts.push(owner.account_id);
    one_change(&server, &owner, &accounts).await;
    let after = server.snapshot(&owner).await.cursor;
    let received = server
        .traffic(&owner, &mut stream)
        .await
        .into_iter()
        .map(|(id, _)| id - before.position)
        .collect();
    (received, after.position - before.position)
}

#[tokio::test]
async fn two_events_of_one_change_tell_nothing_of_the_events_between_them() {
    let without = observed(0).await;
    assert_eq!(without.0.len(), 2);
    assert_eq!(observed(3).await, without);
}

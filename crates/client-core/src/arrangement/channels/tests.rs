use super::*;

#[test]
fn favourites_are_placed_ordered_collapsed_and_round_trip_as_data() {
    let mut layout = Layout::default();
    let (first, second, third) = (
        ChannelId::generate(),
        ChannelId::generate(),
        ChannelId::generate(),
    );
    for channel in [first, second, third] {
        layout
            .apply(Change::Place {
                channel,
                to: Location::Favourites,
                before: None,
            })
            .unwrap();
    }
    layout
        .apply(Change::MoveChannel {
            channel: third,
            up: true,
        })
        .unwrap();
    assert_eq!(layout.favourites, [first, third, second]);
    layout
        .apply(Change::Place {
            channel: second,
            to: Location::Favourites,
            before: Some(first),
        })
        .unwrap();
    assert_eq!(layout.favourites, [second, first, third]);
    layout
        .apply(Change::Place {
            channel: second,
            to: Location::Default,
            before: None,
        })
        .unwrap();
    assert_eq!(layout.location(second), Location::Default);
    assert_eq!(layout.location(first), Location::Favourites);
    layout.apply(Change::Collapse { collapsed: true }).unwrap();
    assert!(layout.favourites_collapsed);
    assert_eq!(
        serde_json::from_str::<Layout>(&serde_json::to_string(&layout).unwrap()).unwrap(),
        layout
    );
}

#[test]
fn invalid_or_excessive_changes_are_atomic_and_stored_data_is_checked() {
    let mut layout = Layout::default();
    let channel = ChannelId::generate();
    layout
        .apply(Change::Place {
            channel,
            to: Location::Favourites,
            before: None,
        })
        .unwrap();
    let before = layout.clone();
    assert_eq!(
        layout.apply(Change::Place {
            channel: ChannelId::generate(),
            to: Location::Favourites,
            before: Some(ChannelId::generate()),
        }),
        Err(Error::NoChannel)
    );
    assert_eq!(
        layout.apply(Change::MoveChannel {
            channel: ChannelId::generate(),
            up: true,
        }),
        Err(Error::NoChannel)
    );
    assert_eq!(layout, before);
    layout.favourites.push(channel);
    assert_eq!(layout.check(), Err(Error::Duplicate));
    layout.favourites = (0..MOST_CHANNELS).map(|_| ChannelId::generate()).collect();
    let before = layout.clone();
    assert_eq!(
        layout.apply(Change::Place {
            channel,
            to: Location::Favourites,
            before: None
        }),
        Err(Error::Limit)
    );
    assert_eq!(layout, before);
}

#[test]
fn the_named_sections_of_version_9_go_and_a_layout_of_only_sections_goes_with_them() {
    let (account, other, workspace, kept, favourite, placed) = (
        "0191a0f0-0000-7000-8000-000000000001",
        "0191a0f0-0000-7000-8000-000000000002",
        "0191a0f0-0000-7000-8000-000000000003",
        "0191a0f0-0000-7000-8000-000000000004",
        ChannelId::generate(),
        ChannelId::generate(),
    );
    let section = serde_json::json!({
        "id": "0191a0f0-0000-7000-8000-000000000005",
        "name": "Projects",
        "collapsed": true,
        "channels": [placed],
    });
    let mut layouts = serde_json::json!({
        account: {
            workspace: {"favourites": [favourite], "favourites_collapsed": true, "sections": [section]},
            kept: {"sections": [section]},
        },
        other: {workspace: {"sections": [section]}},
    });
    without_sections(&mut layouts);
    assert_eq!(
        layouts,
        serde_json::json!({
            account: {workspace: {"favourites": [favourite], "favourites_collapsed": true}},
        })
    );
}

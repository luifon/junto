use super::*;

#[test]
fn channels_have_one_personal_place_and_moves_preserve_manual_order() {
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
    let section = SectionId::generate();
    layout
        .apply(Change::Create {
            id: section,
            name: " Projects ".to_owned(),
            channel: Some(first),
        })
        .unwrap();
    assert_eq!(layout.location(first), Location::Section(section));
    assert_eq!(layout.favourites, [third, second]);
    layout
        .apply(Change::Place {
            channel: second,
            to: Location::Section(section),
            before: Some(first),
        })
        .unwrap();
    assert_eq!(layout.sections[0].channels, [second, first]);
    layout
        .apply(Change::Place {
            channel: second,
            to: Location::Default,
            before: None,
        })
        .unwrap();
    assert_eq!(layout.location(second), Location::Default);
    layout.apply(Change::Remove(section)).unwrap();
    assert_eq!(layout.location(first), Location::Default);
}

#[test]
fn sections_are_named_ordered_collapsed_and_round_trip_as_data() {
    let mut layout = Layout::default();
    let (first, second) = (SectionId::generate(), SectionId::generate());
    for (id, name) in [(first, "Planning"), (second, "Research")] {
        layout
            .apply(Change::Create {
                id,
                name: name.to_owned(),
                channel: None,
            })
            .unwrap();
    }
    layout
        .apply(Change::Rename {
            id: first,
            name: "Roadmap".to_owned(),
        })
        .unwrap();
    layout
        .apply(Change::MoveSection {
            id: second,
            up: true,
        })
        .unwrap();
    layout
        .apply(Change::Collapse {
            location: Location::Section(first),
            collapsed: true,
        })
        .unwrap();
    layout
        .apply(Change::Collapse {
            location: Location::Favourites,
            collapsed: true,
        })
        .unwrap();
    assert_eq!(layout.sections[0].id, second);
    assert_eq!(layout.sections[1].name, "Roadmap");
    assert!(layout.sections[1].collapsed);
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
    for name in [
        String::new(),
        "x".repeat(LONGEST_NAME + 1),
        "a\0b".to_owned(),
    ] {
        let before = layout.clone();
        assert_eq!(
            layout.apply(Change::Create {
                id: SectionId::generate(),
                name,
                channel: Some(channel)
            }),
            Err(Error::InvalidName)
        );
        assert_eq!(layout, before);
    }
    let before = layout.clone();
    assert_eq!(
        layout.apply(Change::Place {
            channel,
            to: Location::Section(SectionId::generate()),
            before: None
        }),
        Err(Error::NoSection)
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
    layout.favourites.clear();
    for _ in 0..MOST_SECTIONS {
        layout
            .apply(Change::Create {
                id: SectionId::generate(),
                name: "Section".to_owned(),
                channel: None,
            })
            .unwrap();
    }
    let before = layout.clone();
    assert_eq!(
        layout.apply(Change::Create {
            id: SectionId::generate(),
            name: "Excess".to_owned(),
            channel: None
        }),
        Err(Error::Limit)
    );
    assert_eq!(layout, before);
}

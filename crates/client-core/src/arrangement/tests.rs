use super::*;

fn servers(count: usize) -> Vec<ServerId> {
    (0..count).map(|_| ServerId::generate()).collect()
}

/// The book as text: a server is its number in `ids`, a group is its servers in brackets
/// with its name.
fn shape(ids: &[ServerId], order: &[ServerId], arrangement: &Arrangement) -> String {
    let number = |server: &ServerId| {
        ids.iter()
            .position(|id| id == server)
            .map_or("?".to_owned(), |index| index.to_string())
    };
    arrangement
        .shelves(order)
        .iter()
        .map(|shelf| match shelf {
            Shelf::Server(server) => number(server),
            Shelf::Group { group, servers } => format!(
                "{}[{}]",
                group.name,
                servers.iter().map(number).collect::<Vec<_>>().join(" ")
            ),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct Book {
    ids: Vec<ServerId>,
    order: Vec<ServerId>,
    arrangement: Arrangement,
}

impl Book {
    fn new(count: usize) -> Self {
        let ids = servers(count);
        Self {
            order: ids.clone(),
            ids,
            arrangement: Arrangement::default(),
        }
    }

    fn id(&self, number: usize) -> ServerId {
        self.ids[number]
    }

    fn change(&mut self, change: Change) -> Result<(), ArrangeError> {
        self.arrangement.apply(&mut self.order, change)
    }

    fn shape(&self) -> String {
        shape(&self.ids, &self.order, &self.arrangement)
    }

    fn group(&self, number: usize) -> GroupId {
        self.arrangement.group_of(self.id(number)).unwrap()
    }

    /// Names the group of a server, so that the shape shows it.
    fn name(&mut self, number: usize, name: &str) {
        let group = self.group(number);
        self.change(Change::RenameGroup {
            group,
            name: name.to_owned(),
        })
        .unwrap();
    }
}

#[test]
fn servers_move_before_and_after_each_other_and_to_the_end() {
    let mut book = Book::new(4);
    let (zero, two, three) = (book.id(0), book.id(2), book.id(3));
    book.change(Change::MoveServer {
        server: three,
        to: Spot::Before(Item::Server(zero)),
    })
    .unwrap();
    assert_eq!(book.shape(), "3 0 1 2");
    book.change(Change::MoveServer {
        server: three,
        to: Spot::After(Item::Server(two)),
    })
    .unwrap();
    assert_eq!(book.shape(), "0 1 2 3");
    book.change(Change::MoveServer {
        server: zero,
        to: Spot::End,
    })
    .unwrap();
    assert_eq!(book.shape(), "1 2 3 0");
    // Next to itself: nothing changes.
    book.change(Change::MoveServer {
        server: zero,
        to: Spot::Before(Item::Server(zero)),
    })
    .unwrap();
    assert_eq!(book.shape(), "1 2 3 0");
    assert!(book.arrangement.is_empty());
}

#[test]
fn a_server_dropped_on_another_makes_a_group_at_the_place_of_the_other() {
    let mut book = Book::new(4);
    let (one, three) = (book.id(1), book.id(3));
    book.change(Change::GroupWith {
        server: three,
        target: one,
    })
    .unwrap();
    book.name(1, "G");
    assert_eq!(book.shape(), "0 G[1 3] 2");
    // The servers of a group follow each other in the file of the book.
    assert_eq!(book.order, [book.id(0), one, three, book.id(2)]);
    // A third server dropped on a server of the group joins the group after it.
    book.change(Change::GroupWith {
        server: book.id(0),
        target: one,
    })
    .unwrap();
    assert_eq!(book.shape(), "G[1 0 3] 2");
    // Dropped on itself: nothing changes.
    book.change(Change::GroupWith {
        server: one,
        target: one,
    })
    .unwrap();
    assert_eq!(book.shape(), "G[1 0 3] 2");
}

#[test]
fn servers_move_into_out_of_and_within_a_group() {
    let mut book = Book::new(5);
    let ids: Vec<ServerId> = book.ids.clone();
    book.change(Change::GroupWith {
        server: ids[2],
        target: ids[1],
    })
    .unwrap();
    book.name(1, "G");
    let group = book.group(1);
    assert_eq!(book.shape(), "0 G[1 2] 3 4");
    // Into the group at its end, and next to a server of the group.
    book.change(Change::MoveServer {
        server: ids[4],
        to: Spot::InGroup(group),
    })
    .unwrap();
    assert_eq!(book.shape(), "0 G[1 2 4] 3");
    book.change(Change::MoveServer {
        server: ids[0],
        to: Spot::Before(Item::Server(ids[2])),
    })
    .unwrap();
    assert_eq!(book.shape(), "G[1 0 2 4] 3");
    // Within the group.
    book.change(Change::MoveServer {
        server: ids[4],
        to: Spot::Before(Item::Server(ids[1])),
    })
    .unwrap();
    assert_eq!(book.shape(), "G[4 1 0 2] 3");
    // Out of the group: next to the group, or next to a server outside it.
    book.change(Change::MoveServer {
        server: ids[0],
        to: Spot::After(Item::Group(group)),
    })
    .unwrap();
    assert_eq!(book.shape(), "G[4 1 2] 0 3");
    book.change(Change::MoveServer {
        server: ids[2],
        to: Spot::End,
    })
    .unwrap();
    assert_eq!(book.shape(), "G[4 1] 0 3 2");
    book.change(Change::LeaveGroup { server: ids[4] }).unwrap();
    assert_eq!(book.shape(), "G[1] 4 0 3 2");
    // The last server leaves: the group goes.
    book.change(Change::MoveServer {
        server: ids[1],
        to: Spot::End,
    })
    .unwrap();
    assert_eq!(book.shape(), "4 0 3 2 1");
    assert!(book.arrangement.groups.is_empty());
    assert_eq!(
        book.change(Change::RenameGroup {
            group,
            name: "Gone".to_owned()
        }),
        Err(ArrangeError::NoGroup(group))
    );
}

#[test]
fn the_only_server_of_a_group_is_not_moved_into_its_own_group() {
    let mut book = Book::new(3);
    let (zero, one) = (book.id(0), book.id(1));
    book.change(Change::NewGroup {
        server: one,
        name: "G".to_owned(),
    })
    .unwrap();
    let group = book.group(1);
    assert_eq!(book.shape(), "0 G[1] 2");
    let before = (book.order.clone(), book.arrangement.clone());
    assert_eq!(
        book.change(Change::MoveServer {
            server: one,
            to: Spot::InGroup(group),
        }),
        Err(ArrangeError::NoGroup(group))
    );
    assert_eq!((book.order.clone(), book.arrangement.clone()), before);
    book.change(Change::MoveServer {
        server: zero,
        to: Spot::After(Item::Group(group)),
    })
    .unwrap();
    assert_eq!(book.shape(), "G[1] 0 2");
}

#[test]
fn a_group_moves_as_a_whole_and_stays_outside_other_groups() {
    let mut book = Book::new(6);
    let ids = book.ids.clone();
    book.change(Change::GroupWith {
        server: ids[1],
        target: ids[0],
    })
    .unwrap();
    book.name(0, "A");
    book.change(Change::GroupWith {
        server: ids[4],
        target: ids[3],
    })
    .unwrap();
    book.name(3, "B");
    let (a, b) = (book.group(0), book.group(3));
    assert_eq!(book.shape(), "A[0 1] 2 B[3 4] 5");
    book.change(Change::MoveGroup {
        group: a,
        to: Spot::End,
    })
    .unwrap();
    assert_eq!(book.shape(), "2 B[3 4] 5 A[0 1]");
    // Next to a server of another group: next to that group.
    book.change(Change::MoveGroup {
        group: a,
        to: Spot::Before(Item::Server(ids[4])),
    })
    .unwrap();
    assert_eq!(book.shape(), "2 A[0 1] B[3 4] 5");
    book.change(Change::MoveGroup {
        group: a,
        to: Spot::InGroup(b),
    })
    .unwrap();
    assert_eq!(book.shape(), "2 B[3 4] A[0 1] 5");
    book.change(Change::MoveGroup {
        group: b,
        to: Spot::Before(Item::Server(ids[2])),
    })
    .unwrap();
    assert_eq!(book.shape(), "B[3 4] 2 A[0 1] 5");
    // Next to itself or one of its servers: nothing changes.
    book.change(Change::MoveGroup {
        group: b,
        to: Spot::After(Item::Server(ids[3])),
    })
    .unwrap();
    assert_eq!(book.shape(), "B[3 4] 2 A[0 1] 5");
    assert_eq!(book.order, [ids[3], ids[4], ids[2], ids[0], ids[1], ids[5]]);
}

#[test]
fn a_group_is_named_coloured_collapsed_and_dissolved_in_place() {
    let mut book = Book::new(4);
    let ids = book.ids.clone();
    book.change(Change::NewGroup {
        server: ids[1],
        name: "  Work\u{7}  ".to_owned(),
    })
    .unwrap();
    let group = book.group(1);
    assert_eq!(book.arrangement.group(group).unwrap().name, "Work");
    book.change(Change::MoveServer {
        server: ids[2],
        to: Spot::InGroup(group),
    })
    .unwrap();
    assert_eq!(book.shape(), "0 Work[1 2] 3");
    let long = "x".repeat(LONGEST_GROUP_NAME + 10);
    book.change(Change::RenameGroup { group, name: long })
        .unwrap();
    assert_eq!(
        book.arrangement.group(group).unwrap().name.chars().count(),
        LONGEST_GROUP_NAME
    );
    book.change(Change::RenameGroup {
        group,
        name: String::new(),
    })
    .unwrap();
    assert_eq!(book.arrangement.group(group).unwrap().name, "");
    book.change(Change::ColourGroup {
        group,
        colour: GroupColour::Cypress,
    })
    .unwrap();
    book.change(Change::CollapseGroup {
        group,
        collapsed: true,
    })
    .unwrap();
    let held = book.arrangement.group(group).unwrap();
    assert_eq!(held.colour, GroupColour::Cypress);
    assert!(held.collapsed);
    book.change(Change::DissolveGroup { group }).unwrap();
    assert_eq!(book.shape(), "0 1 2 3");
    assert!(book.arrangement.is_empty());
}

#[test]
fn a_new_group_of_a_server_in_a_group_stands_after_that_group() {
    let mut book = Book::new(3);
    let ids = book.ids.clone();
    book.change(Change::GroupWith {
        server: ids[1],
        target: ids[0],
    })
    .unwrap();
    book.name(0, "A");
    book.change(Change::NewGroup {
        server: ids[0],
        name: "B".to_owned(),
    })
    .unwrap();
    assert_eq!(book.shape(), "A[1] B[0] 2");
}

#[test]
fn the_workspaces_of_a_server_take_the_order_of_the_person() {
    let mut book = Book::new(2);
    let server = book.id(0);
    let shown: Vec<WorkspaceId> = (0..4).map(|_| WorkspaceId::generate()).collect();
    book.change(Change::MoveWorkspace {
        server,
        workspace: shown[3],
        to: WorkspaceSpot::Before(shown[0]),
        shown: shown.clone(),
    })
    .unwrap();
    let mut listed = shown.clone();
    book.arrangement
        .sort_workspaces(server, &mut listed, |id| *id);
    assert_eq!(listed, [shown[3], shown[0], shown[1], shown[2]]);
    // The order counts from what is listed in the order of the person.
    book.change(Change::MoveWorkspace {
        server,
        workspace: shown[0],
        to: WorkspaceSpot::After(shown[2]),
        shown: shown.clone(),
    })
    .unwrap();
    let mut listed = shown.clone();
    book.arrangement
        .sort_workspaces(server, &mut listed, |id| *id);
    assert_eq!(listed, [shown[3], shown[1], shown[2], shown[0]]);
    // A new workspace of the server follows the ones of the order.
    let new = WorkspaceId::generate();
    let mut listed = vec![new, shown[0], shown[1], shown[2], shown[3]];
    book.arrangement
        .sort_workspaces(server, &mut listed, |id| *id);
    assert_eq!(listed, [shown[3], shown[1], shown[2], shown[0], new]);
    // The other server keeps the order of its server.
    let mut other = shown.clone();
    book.arrangement
        .sort_workspaces(book.id(1), &mut other, |id| *id);
    assert_eq!(other, shown);
    assert_eq!(
        book.change(Change::MoveWorkspace {
            server,
            workspace: new,
            to: WorkspaceSpot::Before(shown[0]),
            shown: shown.clone(),
        }),
        Err(ArrangeError::NoWorkspace(new))
    );
}

#[test]
fn workspaces_and_groups_collapse_one_by_one_and_all_at_once() {
    let mut book = Book::new(3);
    let ids = book.ids.clone();
    book.change(Change::GroupWith {
        server: ids[2],
        target: ids[1],
    })
    .unwrap();
    let group = book.group(1);
    book.change(Change::CollapseWorkspaces {
        server: ids[0],
        collapsed: true,
    })
    .unwrap();
    assert!(book.arrangement.workspaces_collapsed(ids[0]));
    assert!(!book.arrangement.workspaces_collapsed(ids[1]));
    book.change(Change::CollapseEverything { collapsed: true })
        .unwrap();
    assert!(
        ids.iter()
            .all(|id| book.arrangement.workspaces_collapsed(*id))
    );
    assert!(book.arrangement.group(group).unwrap().collapsed);
    book.change(Change::CollapseEverything { collapsed: false })
        .unwrap();
    assert!(
        !ids.iter()
            .any(|id| book.arrangement.workspaces_collapsed(*id))
    );
    assert!(!book.arrangement.group(group).unwrap().collapsed);
    // A record that arranges nothing is not kept.
    assert_eq!(book.arrangement.servers.len(), 2);
}

#[test]
fn a_change_for_a_server_that_is_gone_changes_nothing() {
    let mut book = Book::new(2);
    let gone = ServerId::generate();
    let before = (book.order.clone(), book.arrangement.clone());
    for change in [
        Change::MoveServer {
            server: gone,
            to: Spot::End,
        },
        Change::MoveServer {
            server: book.id(0),
            to: Spot::Before(Item::Server(gone)),
        },
        Change::GroupWith {
            server: book.id(0),
            target: gone,
        },
        Change::NewGroup {
            server: gone,
            name: String::new(),
        },
        Change::CollapseWorkspaces {
            server: gone,
            collapsed: true,
        },
    ] {
        assert!(book.change(change.clone()).is_err(), "{change:?}");
        assert_eq!((book.order.clone(), book.arrangement.clone()), before);
    }
}

#[test]
fn a_removed_server_takes_its_record_and_an_empty_group_away() {
    let mut book = Book::new(3);
    let ids = book.ids.clone();
    book.change(Change::NewGroup {
        server: ids[1],
        name: "G".to_owned(),
    })
    .unwrap();
    book.change(Change::CollapseWorkspaces {
        server: ids[2],
        collapsed: true,
    })
    .unwrap();
    let left = [ids[0], ids[2]];
    let normalised = book.arrangement.normalised(&left);
    assert!(normalised.groups.is_empty());
    assert_eq!(normalised.servers.len(), 1);
    assert_eq!(shape(&ids, &left, &normalised), "0 2");
}

#[test]
fn the_number_of_groups_is_bounded() {
    let mut book = Book::new(MOST_GROUPS + 1);
    for number in 0..MOST_GROUPS {
        book.change(Change::NewGroup {
            server: book.id(number),
            name: String::new(),
        })
        .unwrap();
    }
    assert_eq!(
        book.change(Change::NewGroup {
            server: book.id(MOST_GROUPS),
            name: String::new(),
        }),
        Err(ArrangeError::TooManyGroups)
    );
    assert!(book.arrangement.check().is_ok());
}

#[test]
fn a_stored_arrangement_outside_its_bounds_is_refused() {
    let server = ServerId::generate();
    let place = ServerPlace::new(server);
    let twice = Arrangement {
        groups: Vec::new(),
        servers: vec![place.clone(), place.clone()],
    };
    assert!(twice.check().is_err());
    let long = Arrangement {
        groups: Vec::new(),
        servers: vec![ServerPlace {
            workspace_order: (0..=MOST_ORDERED_WORKSPACES)
                .map(|_| WorkspaceId::generate())
                .collect(),
            ..place
        }],
    };
    assert!(long.check().is_err());
    let named = Arrangement {
        groups: vec![ServerGroup {
            id: GroupId::generate(),
            name: " padded ".to_owned(),
            colour: GroupColour::Ochre,
            collapsed: false,
        }],
        servers: Vec::new(),
    };
    assert!(named.check().is_err());
}

//! The text of the page "Notifications" of a workspace (ADR 0761).

use junto_client_core::protocol::discovery::NotificationLevel;

use crate::state::admin::view::notifications::DefaultChoice;

pub const LEVEL: &str = "Level of the workspace";
pub const HOLDS_FOR: &str = "The level holds for a member who set no level for this workspace. A level that a \
     member set for the workspace or for one channel comes first.";

pub fn help(workspace: &str) -> String {
    format!("What the workspace “{workspace}” notifies its members about when they chose nothing.")
}

/// A choice as the control names it.
pub const fn choice(choice: DefaultChoice) -> &'static str {
    match choice {
        DefaultChoice::NotSet => "Not set",
        DefaultChoice::Level(NotificationLevel::All) => "All messages",
        DefaultChoice::Level(NotificationLevel::Mentions) => "Mentions only",
        DefaultChoice::Level(NotificationLevel::Nothing) => "Nothing",
    }
}

/// What holds for a member with the choice that the workspace has now.
pub const fn stated(choice: DefaultChoice) -> &'static str {
    match choice {
        DefaultChoice::NotSet => {
            "The workspace sets no level. The default of each member holds; a member without \
             one is notified of mentions only."
        }
        DefaultChoice::Level(NotificationLevel::All) => {
            "The workspace notifies its members of every message."
        }
        DefaultChoice::Level(NotificationLevel::Mentions) => {
            "The workspace notifies its members of mentions only."
        }
        DefaultChoice::Level(NotificationLevel::Nothing) => {
            "The workspace notifies its members of nothing."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_choice_has_a_name_and_a_sentence_of_its_own() {
        let names: Vec<&str> = DefaultChoice::ALL
            .iter()
            .map(|held| choice(*held))
            .collect();
        let sentences: Vec<&str> = DefaultChoice::ALL
            .iter()
            .map(|held| stated(*held))
            .collect();
        for list in [&names, &sentences] {
            for (index, text) in list.iter().enumerate() {
                assert!(!text.is_empty());
                assert!(!list[..index].contains(text), "{text}");
            }
        }
        assert_eq!(choice(DefaultChoice::NotSet), "Not set");
        assert!(stated(DefaultChoice::NotSet).contains("sets no level"));
        assert!(help("Studio").contains("“Studio”"));
    }
}

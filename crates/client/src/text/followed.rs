//! The text of followed threads (ADR 0412, ADR 0413): the control of the thread panel that
//! states whether the account follows the thread, and the row of the channel column that
//! offers the further followed threads of a channel.

pub const CONTROL: &str = "Whether you follow this thread";
pub const FOLLOWING_BY_CHOICE: &str = "You follow this thread.";
pub const NOT_FOLLOWING_BY_CHOICE: &str =
    "You do not follow this thread. Its replies are not counted as unread; a mention still is.";
pub const FOLLOWING_BY_RULE: &str =
    "You follow this thread because you wrote in it or were named in it.";
pub const NOT_FOLLOWING: &str = "You do not follow this thread.";
pub const FOLLOW: &str = "Follow";
pub const STOP_FOLLOWING: &str = "Stop following";
pub const USE_DEFAULT: &str = "Use the default";
pub const USE_DEFAULT_HELP: &str =
    "Remove your choice. You then follow the thread when you wrote in it or were named in it.";
pub const THREAD_GONE: &str = "This thread no longer exists.";
pub const ARCHIVED: &str =
    "This channel is archived. Your choice for this thread cannot be changed.";
pub const SHOW_MORE: &str = "Show more followed threads";

/// What the control states: the choice of the account when it made one, and what the rule
/// of the server gives otherwise.
pub const fn state(following: bool, setting: Option<bool>) -> &'static str {
    match (setting, following) {
        (Some(true), _) => FOLLOWING_BY_CHOICE,
        (Some(false), _) => NOT_FOLLOWING_BY_CHOICE,
        (None, true) => FOLLOWING_BY_RULE,
        (None, false) => NOT_FOLLOWING,
    }
}

/// The action that changes whether the account follows the thread.
pub const fn action(following: bool) -> &'static str {
    if following { STOP_FOLLOWING } else { FOLLOW }
}

/// The row that shows the further followed threads of a channel. `count`: how many the
/// client holds; absent while the server lists further ones.
pub fn show_more(count: Option<usize>) -> String {
    match count {
        None => SHOW_MORE.to_owned(),
        Some(1) => "Show 1 more followed thread".to_owned(),
        Some(count) => format!("Show {count} more followed threads"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_states_the_choice_and_the_rule_in_four_sentences() {
        assert_eq!(state(true, Some(true)), "You follow this thread.");
        assert_eq!(
            state(false, Some(false)),
            "You do not follow this thread. Its replies are not counted as unread; a mention \
             still is."
        );
        assert_eq!(
            state(true, None),
            "You follow this thread because you wrote in it or were named in it."
        );
        assert_eq!(state(false, None), "You do not follow this thread.");
    }

    #[test]
    fn the_action_is_the_opposite_of_what_holds() {
        assert_eq!(action(true), "Stop following");
        assert_eq!(action(false), "Follow");
    }

    #[test]
    fn the_row_of_further_threads_counts_them_when_the_client_holds_them_all() {
        assert_eq!(show_more(Some(1)), "Show 1 more followed thread");
        assert_eq!(show_more(Some(5)), "Show 5 more followed threads");
        assert_eq!(show_more(None), "Show more followed threads");
    }

    #[test]
    fn the_sentences_about_followed_threads_have_no_exclamation() {
        for sentence in [
            FOLLOWING_BY_CHOICE,
            NOT_FOLLOWING_BY_CHOICE,
            FOLLOWING_BY_RULE,
            NOT_FOLLOWING,
            USE_DEFAULT_HELP,
            THREAD_GONE,
            ARCHIVED,
        ] {
            assert!(!sentence.contains('!'), "{sentence}");
            assert!(sentence.ends_with('.'), "{sentence}");
        }
    }
}

//! The text of the glossary page of a workspace (ADR 0616).

pub const HELP: &str = "Names and terms that the speech model receives before it transcribes a \
     recording of this workspace, the most important first. It reads the first terms up to \
     a limit of 200 tokens; a transcript states which terms it used. Every member of the \
     workspace can read the glossary.";
pub const NEW_TERM: &str = "Add a term";
pub const TERM: &str = "Term";
pub const ADD: &str = "Add";
pub const TERMS: &str = "Terms, the most important first";
pub const EMPTY: &str = "The glossary has no term.";
pub const MANAGE_NEEDS: &str = "You do not hold “manage glossary” in this workspace.";
pub const CUT: &str = "The server holds more terms than this application reads. Change the \
     glossary with another client.";
pub const NOT_READ: &str = "The glossary is not read yet.";
pub const TERM_EMPTY: &str = "Type a term.";
pub const TERM_LONG: &str = "A term has at most 200 characters.";
pub const TERM_CHARACTERS: &str =
    "A term holds no line break, no control character, and not “<|” or “|>”.";
pub const TERM_TWICE: &str = "This term is in the glossary already.";
pub const FULL: &str = "The glossary holds 200 terms, which is the most. Remove one first.";
pub const CHANGED_MEANWHILE: &str = "The glossary was changed after this page showed it. The \
     list shows it as it is now: make your change again.";
pub const INVALID: &str = "The server did not accept the term. A term has 1 to 200 characters, \
     no line break and no “<|” or “|>”, and is in the glossary once.";

pub fn count(terms: usize) -> String {
    match terms {
        1 => "1 term".to_owned(),
        terms => format!("{terms} terms"),
    }
}

pub fn remove_question(term: &str) -> String {
    format!("Remove “{term}” from the glossary?")
}

/// The position of a term in the list, from 1.
pub fn position(index: usize) -> String {
    format!("{}.", index + 1)
}

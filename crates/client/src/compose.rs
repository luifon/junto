//! The text of the composer and the document of a message (ADR 0408, ADR 0740).
//!
//! A person types text with a small set of marks, and the server stores a typed document.
//! [`parse`] makes the document, and [`write`] makes the text of a document again when a
//! person edits a message. A mention is no mark: the composer holds it as a token with a
//! range of the text, because a name does not identify an account.
//!
//! The marks: `**bold**`, `*italic*` and `_italic_`, `~~struck~~`, `` `code` ``, a link as
//! `[text](address)` or as a bare address, and at the start of a line `> ` for a quote,
//! `- ` for a list item, `1. ` for a numbered item, `- [ ] ` and `- [x] ` for a check box,
//! and three backticks for a code block. A backslash before a mark makes it plain text.

use std::ops::Range;

use junto_client_core::link::Link;
use junto_client_core::protocol::ids::{AccountId, WorkspaceRoleId};
use junto_client_core::protocol::message::{Block, CheckItem, Document, Inline, Mark};

/// What a mention names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MentionTarget {
    Account(AccountId),
    Role(WorkspaceRoleId),
    Everyone,
}

/// A mention in the text of the composer: the bytes that show it, and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub range: Range<usize>,
    pub target: MentionTarget,
}

/// The limits of a document on the server (ADR 0408).
pub const MOST_BYTES: usize = 16 * 1024;
pub const MOST_BLOCKS: usize = 100;
pub const MOST_NODES: usize = 1000;
pub const MOST_MENTIONS: usize = 100;
/// The longest text of the composer that is read for marks. A longer text is taken as
/// plain text, and [`check`] refuses it for its length: without white space at its line
/// ends it cannot become a document that the server accepts.
pub const MOST_TEXT: usize = 4 * MOST_BYTES;
const LONGEST_LINK: usize = junto_client_core::link::LONGEST;
const LONGEST_LANGUAGE: usize = 32;

/// Why the server would refuse a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The document has no text. A message without text needs a file.
    Empty,
    /// The document is longer than 16 KiB in the form that the server stores.
    TooLong {
        bytes: usize,
    },
    TooManyBlocks {
        blocks: usize,
    },
    TooManyNodes {
        nodes: usize,
    },
    TooManyMentions {
        mentions: usize,
    },
}

/// The first private-use character that stands for a mention while the text is parsed.
const TOKEN_BASE: u32 = 0xF0000;
const TOKEN_LAST: u32 = 0xFFFFD;

fn token(index: usize) -> Option<char> {
    let code = TOKEN_BASE.checked_add(u32::try_from(index).ok()?)?;
    (code <= TOKEN_LAST).then(|| char::from_u32(code)).flatten()
}

fn token_index(character: char) -> Option<usize> {
    let code = character as u32;
    (TOKEN_BASE..=TOKEN_LAST)
        .contains(&code)
        .then(|| (code - TOKEN_BASE) as usize)
}

/// The text without what the server refuses in a text node: control characters other than
/// the line break and the tab. A line break of two characters becomes one.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|character| {
            (!character.is_control() || matches!(character, '\n' | '\t'))
                && token_index(*character).is_none()
        })
        .collect()
}

/// The text with one private-use character in place of each mention.
fn with_tokens(text: &str, mentions: &[Mention]) -> (String, Vec<MentionTarget>) {
    let mut ordered: Vec<&Mention> = mentions
        .iter()
        .filter(|mention| {
            mention.range.start < mention.range.end
                && text.is_char_boundary(mention.range.start)
                && text.is_char_boundary(mention.range.end)
                && mention.range.end <= text.len()
        })
        .collect();
    ordered.sort_by_key(|mention| mention.range.start);
    let mut made = String::with_capacity(text.len());
    let mut targets = Vec::new();
    let mut at = 0;
    for mention in ordered {
        if mention.range.start < at {
            continue;
        }
        let Some(mark) = token(targets.len()) else {
            break;
        };
        made.push_str(&clean(&text[at..mention.range.start]));
        made.push(mark);
        targets.push(mention.target);
        at = mention.range.end;
    }
    made.push_str(&clean(&text[at..]));
    (made, targets)
}

/// True when the server accepts the address of a link: HTTP, HTTPS or mailto, without a
/// user name and a password, without a control character, at most 2,048 bytes.
pub fn link_is_accepted(address: &str) -> bool {
    Link::parse(address).is_some()
}

fn language_is_accepted(language: &str) -> bool {
    !language.is_empty()
        && language.len() <= LONGEST_LANGUAGE
        && language
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte))
}

const MARK_ORDER: [Mark; 3] = [Mark::Bold, Mark::Italic, Mark::Strikethrough];

fn ordered(marks: &[Mark]) -> Vec<Mark> {
    MARK_ORDER
        .into_iter()
        .filter(|mark| marks.contains(mark))
        .collect()
}

struct Inlines<'a> {
    targets: &'a [MentionTarget],
    nodes: Vec<Inline>,
}

impl Inlines<'_> {
    fn text(&mut self, text: &str, marks: &[Mark]) {
        if text.is_empty() {
            return;
        }
        let marks = ordered(marks);
        if let Some(Inline::Text {
            text: held,
            marks: held_marks,
        }) = self.nodes.last_mut()
            && *held_marks == marks
        {
            held.push_str(text);
            return;
        }
        self.nodes.push(Inline::Text {
            text: text.to_owned(),
            marks,
        });
    }

    fn mention(&mut self, index: usize) {
        match self.targets.get(index) {
            Some(MentionTarget::Account(account_id)) => self.nodes.push(Inline::Account {
                account_id: *account_id,
            }),
            Some(MentionTarget::Role(role_id)) => {
                self.nodes.push(Inline::Role { role_id: *role_id });
            }
            Some(MentionTarget::Everyone) => self.nodes.push(Inline::Everyone),
            None => {}
        }
    }
}

const ESCAPED: &str = "\\*_~`[]()>-#.!+";

/// The runs of backticks of a text: the bytes at which they start, by the length of the
/// run. A code span ends at the next run of the length of its opening run; with this
/// table that run is found without a search of the text, so a text with many backticks
/// is not searched again for each of them.
type Backticks = std::collections::HashMap<usize, Vec<usize>>;

fn backticks(text: &str) -> Backticks {
    let bytes = text.as_bytes();
    let mut runs = Backticks::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let run = bytes[at..].iter().take_while(|byte| **byte == b'`').count();
            runs.entry(run).or_default().push(at);
            at += run;
        } else {
            at += 1;
        }
    }
    runs
}

#[cfg(test)]
thread_local! {
    /// The steps of the last reads of a text for marks, for the test of their bound.
    static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts steps of a read for marks. Only a test reads the count.
#[inline]
fn step(_count: usize) {
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + _count as u64));
}

/// One part of a text that is read for marks: the part, the byte of the whole text at
/// which it starts, and the runs of backticks of the whole text.
#[derive(Clone, Copy)]
struct Piece<'a> {
    text: &'a str,
    base: usize,
    runs: &'a Backticks,
}

impl Piece<'_> {
    /// The code span that opens at a byte of the part: the length of its runs of
    /// backticks, and the byte of the part at which the closing run starts. `None` when
    /// no run of that length follows in the part.
    fn code(&self, at: usize) -> Option<(usize, usize)> {
        let run = self.text[at..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        let after = self.base + at + run;
        let starts = self.runs.get(&run)?;
        let end = *starts.get(starts.partition_point(|start| *start < after))? - self.base;
        (end + run <= self.text.len()).then_some((run, end))
    }

    fn part(&self, range: Range<usize>) -> Self {
        Self {
            text: &self.text[range.clone()],
            base: self.base + range.start,
            runs: self.runs,
        }
    }
}

/// The end of a run of `count` copies of `mark` that closes a span opened before `from`:
/// the run is preceded by a character that is no white space.
fn closing(piece: Piece<'_>, from: usize, mark: char, count: usize) -> Option<usize> {
    let text = piece.text;
    let mut at = from;
    while at < text.len() {
        step(1);
        let character = text[at..].chars().next()?;
        if character == '\\' {
            at += 1 + text[at + 1..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        if character == '`' {
            // A code span is not searched for marks.
            at = match piece.code(at) {
                Some((run, end)) => end + run,
                None => at + text[at..].bytes().take_while(|byte| *byte == b'`').count(),
            };
            continue;
        }
        if character == mark {
            let run = text[at..].chars().take_while(|c| *c == mark).count();
            let before = text[..at].chars().next_back();
            let closes = before.is_some_and(|c| !c.is_whitespace());
            let word_after = text[at + run..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
            let fits = if mark == '_' { !word_after } else { true };
            if run >= count && closes && fits && at > from {
                // A longer run closes with its last characters: `**a*` + `*`.
                return Some(at + (run - count) * mark.len_utf8());
            }
            at += run * mark.len_utf8();
            continue;
        }
        at += character.len_utf8();
    }
    None
}

/// What stands at a `[`.
enum Bracket<'a> {
    /// `[text](address)`: the text, the address, and the length of the whole.
    Link(&'a str, &'a str, usize),
    /// No link starts at a `[` before this byte: the line has no `]`, or the `]` that
    /// every such `[` reaches is followed by no address.
    NoneBefore(usize),
}

fn bracket_link(text: &str) -> Bracket<'_> {
    let mut at = 1;
    let mut close = None;
    while at < text.len() {
        step(1);
        let Some(character) = text[at..].chars().next() else {
            break;
        };
        match character {
            '\\' => at += 1 + text[at + 1..].chars().next().map_or(0, char::len_utf8),
            ']' => {
                close = Some(at);
                break;
            }
            '\n' => break,
            _ => at += character.len_utf8(),
        }
    }
    let Some(close) = close else {
        return Bracket::NoneBefore(at.min(text.len()));
    };
    let none = Bracket::NoneBefore(close + 1);
    if !text[close + 1..].starts_with('(') {
        return none;
    }
    let start = close + 2;
    let mut depth = 0usize;
    let mut end = None;
    for (offset, character) in text[start..].char_indices() {
        step(1);
        // An address above the limit of the server is no link.
        if offset > LONGEST_LINK {
            return none;
        }
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = Some(start + offset);
                break;
            }
            ')' => depth -= 1,
            character if character.is_whitespace() => return none,
            _ => {}
        }
    }
    match end {
        Some(end) => Bracket::Link(&text[1..close], &text[start..end], end + 1),
        None => none,
    }
}

/// The length of a bare address at the start of `text`.
fn bare_link(text: &str) -> Option<usize> {
    if !(text.starts_with("https://") || text.starts_with("http://")) {
        return None;
    }
    // An address ends at white space. Text without white space that is longer than an
    // address can be is not searched to its end.
    let reach = text.floor_char_boundary(2 * LONGEST_LINK);
    step(reach);
    let end = text[..reach]
        .find(|character: char| character.is_whitespace() || token_index(character).is_some())
        .or_else(|| (reach == text.len()).then_some(reach))?;
    // A mark of the sentence after the address is not a part of it.
    let mut address = &text[..end];
    loop {
        let trimmed =
            address.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"', '*', '_', '~']);
        let trimmed = match trimmed.strip_suffix(')') {
            Some(inner) if inner.matches('(').count() < trimmed.matches(')').count() => inner,
            _ => trimmed,
        };
        if trimmed.len() == address.len() {
            break;
        }
        address = trimmed;
    }
    let host_starts = address.find("://").map(|at| at + 3)?;
    (address.len() > host_starts && link_is_accepted(address)).then_some(address.len())
}

fn unescaped(text: &str) -> String {
    let mut made = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && let Some(next) = characters.peek()
            && ESCAPED.contains(*next)
        {
            made.push(*next);
            characters.next();
        } else {
            made.push(character);
        }
    }
    made
}

/// How deep marks inside marks are read. A mark at a deeper place is plain text: the
/// three marks give no more than three levels of meaning.
const DEEPEST: usize = 8;

fn parse_inline(piece: Piece<'_>, marks: &[Mark], depth: usize, out: &mut Inlines<'_>) {
    let text = piece.text;
    let mut at = 0;
    let mut plain = 0;
    // The kinds of marks for which this part has no closing run after the place that
    // was searched: a later mark of the kind is not searched for again.
    let mut never_closed: Vec<(char, usize)> = Vec::new();
    // No link starts at a `[` before this byte.
    let mut no_link_before = 0;
    macro_rules! flush {
        ($end:expr) => {
            out.text(&text[plain..$end], marks);
        };
    }
    while at < text.len() {
        step(1);
        let rest = &text[at..];
        let Some(character) = rest.chars().next() else {
            break;
        };
        if let Some(index) = token_index(character) {
            flush!(at);
            out.mention(index);
            at += character.len_utf8();
            plain = at;
            continue;
        }
        match character {
            '\\' => {
                let next = rest[1..].chars().next();
                if let Some(next) = next.filter(|next| ESCAPED.contains(*next)) {
                    flush!(at);
                    let mut buffer = [0u8; 4];
                    out.text(next.encode_utf8(&mut buffer), marks);
                    at += 1 + next.len_utf8();
                    plain = at;
                    continue;
                }
            }
            '`' => {
                let Some((run, end)) = piece.code(at) else {
                    at += rest.bytes().take_while(|byte| *byte == b'`').count();
                    continue;
                };
                flush!(at);
                let mut code = &text[at + run..end];
                // One space at each end separates the content from a backtick in it.
                if code.len() > 2
                    && code.starts_with(' ')
                    && code.ends_with(' ')
                    && !code.trim().is_empty()
                {
                    code = &code[1..code.len() - 1];
                }
                if !code.is_empty() {
                    out.nodes.push(Inline::Code {
                        text: code.to_owned(),
                    });
                }
                at = end + run;
                plain = at;
                continue;
            }
            '[' if at >= no_link_before => match bracket_link(rest) {
                Bracket::Link(shown, address, length) => {
                    if link_is_accepted(address) && !shown.trim().is_empty() {
                        flush!(at);
                        out.nodes.push(Inline::Link {
                            text: unescaped(shown),
                            url: address.to_owned(),
                        });
                        at += length;
                        plain = at;
                        continue;
                    }
                    // Every `[` up to the `]` of this one reaches the same address.
                    no_link_before = at + shown.len() + 2;
                }
                Bracket::NoneBefore(end) => no_link_before = at + end,
            },
            'h' => {
                let starts_word = text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_alphanumeric());
                if starts_word && let Some(length) = bare_link(rest) {
                    flush!(at);
                    out.nodes.push(Inline::Link {
                        text: rest[..length].to_owned(),
                        url: rest[..length].to_owned(),
                    });
                    at += length;
                    plain = at;
                    continue;
                }
            }
            '*' | '_' | '~' => {
                let run = rest.chars().take_while(|c| *c == character).count();
                let (count, mark) = match (character, run) {
                    ('*', 2..) => (2, Some(Mark::Bold)),
                    ('*', _) => (1, Some(Mark::Italic)),
                    ('_', _) => (1, Some(Mark::Italic)),
                    ('~', 2..) => (2, Some(Mark::Strikethrough)),
                    _ => (run, None),
                };
                let inner = at + count;
                let opens = text[inner..]
                    .chars()
                    .next()
                    .is_some_and(|after| !after.is_whitespace());
                let word_before = text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric);
                let allowed = opens
                    && !(character == '_' && word_before)
                    && depth < DEEPEST
                    && !never_closed.contains(&(character, count));
                if let (Some(mark), true) = (mark, allowed) {
                    match closing(piece, inner, character, count) {
                        Some(end) => {
                            flush!(at);
                            let mut inner_marks = marks.to_vec();
                            if !inner_marks.contains(&mark) {
                                inner_marks.push(mark);
                            }
                            parse_inline(piece.part(inner..end), &inner_marks, depth + 1, out);
                            at = end + count;
                            plain = at;
                            continue;
                        }
                        None => never_closed.push((character, count)),
                    }
                }
                at += run;
                continue;
            }
            _ => {}
        }
        at += character.len_utf8();
    }
    flush!(text.len());
}

fn inline(text: &str, targets: &[MentionTarget]) -> Vec<Inline> {
    let mut out = Inlines {
        targets,
        nodes: Vec::new(),
    };
    let runs = backticks(text);
    let piece = Piece {
        text,
        base: 0,
        runs: &runs,
    };
    parse_inline(piece, &[], 0, &mut out);
    out.nodes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Blank,
    Fence,
    Quote,
    Check(bool),
    Bullet,
    Number,
    Plain,
}

/// The kind of a line and the byte at which its content starts.
fn line_kind(line: &str) -> (LineKind, usize) {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.is_empty() {
        return (LineKind::Blank, line.len());
    }
    if trimmed.starts_with("```") {
        return (LineKind::Fence, indent);
    }
    if let Some(rest) = trimmed.strip_prefix('>') {
        let space = usize::from(rest.starts_with(' '));
        return (LineKind::Quote, indent + 1 + space);
    }
    for bullet in ["- ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            for (mark, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
                if rest.starts_with(mark) {
                    return (LineKind::Check(checked), indent + bullet.len() + mark.len());
                }
            }
            for (mark, checked) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
                if rest == mark {
                    return (LineKind::Check(checked), line.len());
                }
            }
            return (LineKind::Bullet, indent + bullet.len());
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    // A number of a list has up to three digits: a line that starts with a year is text.
    if (1..=3).contains(&digits) {
        let rest = &trimmed[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return (LineKind::Number, indent + digits + 2);
        }
    }
    (LineKind::Plain, 0)
}

/// The document of a text of the composer. `mentions` are the tokens of the text.
pub fn parse(text: &str, mentions: &[Mention]) -> Document {
    read(text, mentions).0
}

/// The document of a text, and whether the text ends inside a code block that has no
/// closing line.
fn read(text: &str, mentions: &[Mention]) -> (Document, bool) {
    if text.len() > MOST_TEXT {
        let content = vec![Inline::Text {
            text: clean(text),
            marks: Vec::new(),
        }];
        let blocks = vec![Block::Paragraph { content }];
        return (Document { blocks }, false);
    }
    let (text, targets) = with_tokens(text, mentions);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut blocks = Vec::new();
    let mut open = false;
    let mut at = 0;
    while at < lines.len() {
        let (kind, start) = line_kind(lines[at]);
        match kind {
            LineKind::Blank => at += 1,
            LineKind::Fence => {
                let opening = lines[at][start..].trim_end();
                let ticks = opening.chars().take_while(|c| *c == '`').count();
                let language = opening[ticks..].trim();
                let mut code = Vec::new();
                at += 1;
                open = true;
                while at < lines.len() {
                    let line = lines[at].trim();
                    let closing = line.chars().take_while(|c| *c == '`').count();
                    at += 1;
                    if closing >= ticks && closing == line.len() {
                        open = false;
                        break;
                    }
                    code.push(lines[at - 1]);
                }
                let code = code.join("\n");
                if !code.trim().is_empty() {
                    blocks.push(Block::Code {
                        text: code,
                        language: language_is_accepted(language).then(|| language.to_owned()),
                    });
                }
            }
            LineKind::Quote | LineKind::Plain => {
                let mut content = Vec::new();
                while at < lines.len() {
                    let (next, start) = line_kind(lines[at]);
                    if next != kind {
                        break;
                    }
                    content.push(lines[at][start..].trim_end());
                    at += 1;
                }
                let content = inline(content.join("\n").trim(), &targets);
                if !content.is_empty() {
                    blocks.push(if kind == LineKind::Quote {
                        Block::Quote { content }
                    } else {
                        Block::Paragraph { content }
                    });
                }
            }
            LineKind::Bullet | LineKind::Number => {
                let mut items = Vec::new();
                while at < lines.len() {
                    let (next, start) = line_kind(lines[at]);
                    if next != kind {
                        break;
                    }
                    items.push(inline(lines[at][start..].trim(), &targets));
                    at += 1;
                }
                blocks.push(Block::List {
                    ordered: kind == LineKind::Number,
                    items,
                });
            }
            LineKind::Check(_) => {
                let mut items = Vec::new();
                while at < lines.len() {
                    let (LineKind::Check(checked), start) = line_kind(lines[at]) else {
                        break;
                    };
                    items.push(CheckItem {
                        checked,
                        content: inline(lines[at][start..].trim(), &targets),
                    });
                    at += 1;
                }
                blocks.push(Block::Checklist { items });
            }
        }
    }
    (Document { blocks }, open)
}

fn nodes_of(document: &Document) -> impl Iterator<Item = &Inline> {
    document.blocks.iter().flat_map(|block| {
        let nodes: Box<dyn Iterator<Item = &Inline>> = match block {
            Block::Paragraph { content } | Block::Quote { content } => Box::new(content.iter()),
            Block::Code { .. } => Box::new(std::iter::empty()),
            Block::List { items, .. } => Box::new(items.iter().flatten()),
            Block::Checklist { items } => Box::new(items.iter().flat_map(|item| &item.content)),
        };
        nodes
    })
}

/// True when the document has what the server takes as content: text that is not only
/// white space, or a mention.
pub fn has_content(document: &Document) -> bool {
    document.blocks.iter().any(|block| match block {
        Block::Code { text, .. } => !text.trim().is_empty(),
        _ => false,
    }) || nodes_of(document).any(|node| match node {
        Inline::Text { text, .. } | Inline::Code { text } | Inline::Link { text, .. } => {
            !text.trim().is_empty()
        }
        Inline::Account { .. } | Inline::Role { .. } | Inline::Everyone => true,
    })
}

/// Checks a document against the limits of the server before it is sent. A message with a
/// file can have an empty document.
pub fn check(document: &Document, has_files: bool) -> Result<(), Problem> {
    if !has_content(document) && !has_files {
        return Err(Problem::Empty);
    }
    let blocks = document.blocks.len();
    if blocks > MOST_BLOCKS {
        return Err(Problem::TooManyBlocks { blocks });
    }
    let nodes = nodes_of(document).count();
    if nodes > MOST_NODES {
        return Err(Problem::TooManyNodes { nodes });
    }
    let mut mentions: Vec<&Inline> = Vec::new();
    for node in nodes_of(document) {
        let mention = matches!(
            node,
            Inline::Account { .. } | Inline::Role { .. } | Inline::Everyone
        );
        if mention && !mentions.contains(&node) {
            mentions.push(node);
        }
    }
    if mentions.len() > MOST_MENTIONS {
        return Err(Problem::TooManyMentions {
            mentions: mentions.len(),
        });
    }
    let bytes = serde_json::to_vec(document).map_or(usize::MAX, |bytes| bytes.len());
    if bytes > MOST_BYTES {
        return Err(Problem::TooLong { bytes });
    }
    Ok(())
}

fn escape(text: &str, out: &mut String) {
    for character in text.chars() {
        if matches!(character, '\\' | '*' | '_' | '~' | '`' | '[' | ']') {
            out.push('\\');
        }
        out.push(character);
    }
}

/// The byte of a line of a paragraph at which a backslash keeps the line from starting a
/// block.
fn block_escape(line: &str) -> Option<usize> {
    let (kind, _) = line_kind(line);
    let indent = line.len() - line.trim_start().len();
    match kind {
        LineKind::Quote | LineKind::Bullet | LineKind::Check(_) => Some(indent),
        LineKind::Number => {
            let digits = line[indent..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            Some(indent + digits)
        }
        LineKind::Fence => Some(indent),
        LineKind::Blank | LineKind::Plain => None,
    }
}

struct Writer<'a> {
    text: String,
    mentions: Vec<Mention>,
    name: &'a dyn Fn(MentionTarget) -> String,
}

impl Writer<'_> {
    /// Writes a mention on one line: a control character of the name, such as a line break,
    /// is written as a space, so no line prefix or escape is put inside the mention.
    fn mention(&mut self, target: MentionTarget) {
        let start = self.text.len();
        self.text.push('@');
        self.text
            .extend((self.name)(target).chars().map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            }));
        self.mentions.push(Mention {
            range: start..self.text.len(),
            target,
        });
    }

    fn inline(&mut self, content: &[Inline]) {
        for node in content {
            match node {
                Inline::Text { text, marks } => {
                    let marks = ordered(marks);
                    let around: String = marks
                        .iter()
                        .map(|mark| match mark {
                            Mark::Bold => "**",
                            Mark::Italic => "*",
                            Mark::Strikethrough => "~~",
                        })
                        .collect();
                    // A mark must touch the text: white space at an end stays outside.
                    let inner = text.trim();
                    if marks.is_empty() || inner.is_empty() {
                        escape(text, &mut self.text);
                        continue;
                    }
                    let lead = &text[..text.len() - text.trim_start().len()];
                    let trail = &text[text.trim_end().len()..];
                    self.text.push_str(lead);
                    self.text.push_str(&around);
                    escape(inner, &mut self.text);
                    let closing: String = marks
                        .iter()
                        .rev()
                        .map(|mark| match mark {
                            Mark::Bold => "**",
                            Mark::Italic => "*",
                            Mark::Strikethrough => "~~",
                        })
                        .collect();
                    self.text.push_str(&closing);
                    self.text.push_str(trail);
                }
                Inline::Code { text } => {
                    let longest = text
                        .split(|character| character != '`')
                        .map(str::len)
                        .max()
                        .unwrap_or(0);
                    let fence = "`".repeat(longest + 1);
                    let pad = if text.starts_with('`') || text.ends_with('`') {
                        " "
                    } else {
                        ""
                    };
                    self.text
                        .push_str(&format!("{fence}{pad}{text}{pad}{fence}"));
                }
                Inline::Link { text, url } => {
                    let bare = text == url && bare_link(url) == Some(url.len());
                    if bare {
                        self.text.push_str(url);
                    } else {
                        self.text.push('[');
                        escape(text, &mut self.text);
                        self.text.push_str("](");
                        let balanced = url.matches('(').count() == url.matches(')').count();
                        if balanced {
                            self.text.push_str(url);
                        } else {
                            self.text
                                .push_str(&url.replace('(', "%28").replace(')', "%29"));
                        }
                        self.text.push(')');
                    }
                }
                Inline::Account { account_id } => {
                    self.mention(MentionTarget::Account(*account_id));
                }
                Inline::Role { role_id } => self.mention(MentionTarget::Role(*role_id)),
                Inline::Everyone => self.mention(MentionTarget::Everyone),
            }
        }
    }

    /// Writes inline content whose lines start with `prefix`. In a paragraph, a line
    /// that would start a block gets a backslash.
    fn lines(&mut self, content: &[Inline], prefix: &str, escape_starts: bool) {
        let start = self.text.len();
        self.inline(content);
        let written = self.text.split_off(start);
        // What is put into the written text: the byte before which it goes, and the text.
        let mut inserts: Vec<(usize, &str)> = Vec::new();
        let mut line_start = 0;
        for line in written.split('\n') {
            if !prefix.is_empty() {
                inserts.push((line_start, prefix));
            }
            if escape_starts && let Some(at) = block_escape(line) {
                inserts.push((line_start + at, "\\"));
            }
            line_start += line.len() + 1;
        }
        let mut at = 0;
        for (before, inserted) in &inserts {
            self.text.push_str(&written[at..*before]);
            self.text.push_str(inserted);
            at = *before;
        }
        self.text.push_str(&written[at..]);
        // The ranges of the mentions follow the bytes that were put before them.
        let moved = |offset: usize| -> usize {
            inserts
                .iter()
                .filter(|(before, _)| *before <= offset)
                .map(|(_, inserted)| inserted.len())
                .sum()
        };
        for mention in &mut self.mentions {
            if mention.range.start < start {
                continue;
            }
            let added = moved(mention.range.start - start);
            mention.range = mention.range.start + added..mention.range.end + added;
        }
    }
}

/// The bytes of a written text that show one block, and for a list the bytes of the
/// content of each of its items.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WrittenBlock {
    range: Range<usize>,
    items: Vec<Range<usize>>,
}

/// The text of the composer for a document, with its mentions and the place of each block
/// in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub text: String,
    pub mentions: Vec<Mention>,
    blocks: Vec<WrittenBlock>,
}

/// The text of the composer for a document. `name` gives the name that a mention shows,
/// without the `@`.
pub fn written(document: &Document, name: &dyn Fn(MentionTarget) -> String) -> Written {
    let mut writer = Writer {
        text: String::new(),
        mentions: Vec::new(),
        name,
    };
    let mut blocks = Vec::new();
    for (index, block) in document.blocks.iter().enumerate() {
        if index > 0 {
            writer.text.push_str("\n\n");
        }
        let start = writer.text.len();
        let mut items = Vec::new();
        match block {
            Block::Paragraph { content } => writer.lines(content, "", true),
            Block::Quote { content } => writer.lines(content, "> ", false),
            Block::Code { text, language } => {
                let longest = text
                    .split(|character| character != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                writer.text.push_str(&fence);
                writer
                    .text
                    .push_str(language.as_deref().unwrap_or_default());
                writer.text.push('\n');
                writer.text.push_str(text);
                writer.text.push('\n');
                writer.text.push_str(&fence);
            }
            Block::List {
                ordered,
                items: list,
            } => {
                for (number, item) in list.iter().enumerate() {
                    if number > 0 {
                        writer.text.push('\n');
                    }
                    let prefix = if *ordered {
                        format!("{}. ", number + 1)
                    } else {
                        "- ".to_owned()
                    };
                    writer.text.push_str(&prefix);
                    let from = writer.text.len();
                    writer.inline(&one_line(item));
                    items.push(from..writer.text.len());
                }
            }
            Block::Checklist { items: list } => {
                for (number, item) in list.iter().enumerate() {
                    if number > 0 {
                        writer.text.push('\n');
                    }
                    writer
                        .text
                        .push_str(if item.checked { "- [x] " } else { "- [ ] " });
                    let from = writer.text.len();
                    writer.inline(&one_line(&item.content));
                    items.push(from..writer.text.len());
                }
            }
        }
        blocks.push(WrittenBlock {
            range: start..writer.text.len(),
            items,
        });
    }
    Written {
        text: writer.text,
        mentions: writer.mentions,
        blocks,
    }
}

/// The text of the composer for a document, with its mentions.
pub fn write(
    document: &Document,
    name: &dyn Fn(MentionTarget) -> String,
) -> (String, Vec<Mention>) {
    let written = written(document, name);
    (written.text, written.mentions)
}

/// The mentions inside a range of a text, with their bytes counted from the start of the
/// range.
fn mentions_in(mentions: &[Mention], range: &Range<usize>) -> Vec<Mention> {
    let mut found: Vec<Mention> = mentions
        .iter()
        .filter(|mention| mention.range.start >= range.start && mention.range.end <= range.end)
        .map(|mention| Mention {
            range: mention.range.start - range.start..mention.range.end - range.start,
            target: mention.target,
        })
        .collect();
    found.sort_by_key(|mention| mention.range.start);
    found
}

/// The positions at the start and at the end of an old and a new list for which `same`
/// holds: the entries that a change in the middle left as they were.
fn kept_ends(old: usize, new: usize, same: impl Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let most = old.min(new);
    let mut pairs = Vec::new();
    let mut front = 0;
    while front < most && same(front, front) {
        pairs.push((front, front));
        front += 1;
    }
    let mut back = 0;
    while back < most - front && same(old - 1 - back, new - 1 - back) {
        pairs.push((old - 1 - back, new - 1 - back));
        back += 1;
    }
    pairs
}

/// Takes the items of a list of the original whose line the person did not change: the
/// items at the start and at the end that the composer reads from the unchanged line as
/// it reads them from the text of the person.
fn keep_items(old: &Block, written: &WrittenBlock, shown: &Written, new: &mut Block) {
    let as_read = |index: usize| -> Option<Vec<Inline>> {
        let range = written.items.get(index)?;
        let mentions = mentions_in(&shown.mentions, range);
        let (line, targets) = with_tokens(&shown.text[range.clone()], &mentions);
        Some(inline(line.trim(), &targets))
    };
    match (old, new) {
        (
            Block::List {
                ordered: was,
                items: old,
            },
            Block::List {
                ordered,
                items: new,
            },
        ) if was == ordered => {
            let pairs = kept_ends(old.len(), new.len(), |one, other| {
                as_read(one).as_ref() == Some(&new[other])
            });
            for (one, other) in pairs {
                new[other] = old[one].clone();
            }
        }
        (Block::Checklist { items: old }, Block::Checklist { items: new }) => {
            // The box is as the person set it. The text of the item is kept.
            let pairs = kept_ends(old.len(), new.len(), |one, other| {
                as_read(one).as_ref() == Some(&new[other].content)
            });
            for (one, other) in pairs {
                new[other].content = old[one].content.clone();
            }
        }
        _ => {}
    }
}

/// The document of an edited message. `shown` is the text that the composer got for
/// `original` at the start of the edit, and `text` with `mentions` is what the composer
/// holds now.
///
/// The text of the composer cannot state every document that the server accepts: a line
/// break in a list item, white space at the ends of a paragraph, an address that is no
/// link. So an edit changes only what the person changed. A block at the start or at the
/// end whose text and mentions are as they were shown is taken from `original`, and so
/// is an item of a changed list. The text between is read as the text of a new message.
pub fn parse_edit(
    original: &Document,
    shown: &Written,
    text: &str,
    mentions: &[Mention],
) -> Document {
    if text.len() > MOST_TEXT || shown.blocks.len() != original.blocks.len() {
        return parse(text, mentions);
    }
    let unchanged = |old: &Range<usize>, start: usize| {
        let new = start..start + old.len();
        text.get(new.clone()) == Some(&shown.text[old.clone()])
            && mentions_in(mentions, &new) == mentions_in(&shown.mentions, old)
    };
    // The blocks at the start: each stands unchanged before an empty line or the end.
    let mut before = 0;
    let mut middle_start = 0;
    for block in &shown.blocks {
        let end = block.range.end;
        let ends =
            text.len() == end || text.get(end..).is_some_and(|rest| rest.starts_with("\n\n"));
        if !(ends && unchanged(&block.range, block.range.start)) {
            break;
        }
        before += 1;
        middle_start = end;
    }
    // The blocks at the end: each stands unchanged after an empty line, at the distance
    // from the end of the text that it had.
    let mut after = 0;
    let mut middle_end = text.len();
    for block in shown.blocks[before..].iter().rev() {
        let Some(start) = (block.range.start + text.len()).checked_sub(shown.text.len()) else {
            break;
        };
        let preceded = (start == 0 && before == 0)
            || (start >= middle_start + 2
                && text.get(..start).is_some_and(|head| head.ends_with("\n\n")));
        if !(preceded && unchanged(&block.range, start)) {
            break;
        }
        after += 1;
        middle_end = start;
    }
    let between = |end: usize| {
        let range = middle_start..end;
        read(&text[range.clone()], &mentions_in(mentions, &range))
    };
    let (mut middle, open) = between(middle_end);
    if open && after > 0 {
        // A code block without its closing line takes the rest of the text, as in a new
        // message: no block after it is kept.
        after = 0;
        middle = between(text.len()).0;
    }
    let kept_after = original.blocks.len() - after;
    if let ([old], [new]) = (
        &original.blocks[before..kept_after],
        middle.blocks.as_mut_slice(),
    ) {
        keep_items(old, &shown.blocks[before], shown, new);
    }
    let mut blocks = original.blocks[..before].to_vec();
    blocks.append(&mut middle.blocks);
    blocks.extend_from_slice(&original.blocks[kept_after..]);
    Document { blocks }
}

/// The content of a list item on one line: a line break inside it would end the item.
fn one_line(content: &[Inline]) -> Vec<Inline> {
    content
        .iter()
        .map(|node| match node {
            Inline::Text { text, marks } => Inline::Text {
                text: text.replace('\n', " "),
                marks: marks.clone(),
            },
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: Vec::new(),
        }
    }

    fn marked(text: &str, marks: &[Mark]) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: marks.to_vec(),
        }
    }

    fn paragraph(content: Vec<Inline>) -> Block {
        Block::Paragraph { content }
    }

    fn one(text: &str) -> Vec<Inline> {
        let document = parse(text, &[]);
        assert_eq!(document.blocks.len(), 1, "{document:?}");
        match document.blocks.into_iter().next().unwrap() {
            Block::Paragraph { content } => content,
            other => panic!("no paragraph: {other:?}"),
        }
    }

    fn link(text: &str, url: &str) -> Inline {
        Inline::Link {
            text: text.to_owned(),
            url: url.to_owned(),
        }
    }

    #[test]
    fn marks_become_the_marks_of_text_nodes() {
        assert_eq!(one("plain text"), [plain("plain text")]);
        assert_eq!(
            one("The **v0.9** checklist is up. *Please* tick."),
            [
                plain("The "),
                marked("v0.9", &[Mark::Bold]),
                plain(" checklist is up. "),
                marked("Please", &[Mark::Italic]),
                plain(" tick."),
            ]
        );
        assert_eq!(
            one("~~old~~ new"),
            [marked("old", &[Mark::Strikethrough]), plain(" new")]
        );
        assert_eq!(
            one("_also italic_"),
            [marked("also italic", &[Mark::Italic])]
        );
        // Marks inside marks are one node with both, in a fixed order.
        assert_eq!(
            one("**bold and *both* **"),
            [
                plain("**bold and "),
                marked("both", &[Mark::Italic]),
                plain(" **")
            ]
        );
        assert_eq!(
            one("**bold *both***"),
            [
                marked("bold ", &[Mark::Bold]),
                marked("both", &[Mark::Bold, Mark::Italic])
            ]
        );
        assert_eq!(
            one("~~*a*~~"),
            [marked("a", &[Mark::Italic, Mark::Strikethrough])]
        );
    }

    #[test]
    fn a_mark_without_its_end_and_a_mark_inside_a_word_stay_text() {
        assert_eq!(one("2 * 3 * 4"), [plain("2 * 3 * 4")]);
        assert_eq!(one("a ** b"), [plain("a ** b")]);
        assert_eq!(one("*open"), [plain("*open")]);
        assert_eq!(one("snake_case_name"), [plain("snake_case_name")]);
        assert_eq!(one("tracks_days = 30"), [plain("tracks_days = 30")]);
        assert_eq!(one("~one tilde~"), [plain("~one tilde~")]);
        assert_eq!(one("\\*not italic\\*"), [plain("*not italic*")]);
        assert_eq!(one("a \\\\ b"), [plain("a \\ b")]);
        assert_eq!(one("C:\\temp"), [plain("C:\\temp")]);
    }

    #[test]
    fn code_is_not_parsed_and_keeps_its_backticks() {
        assert_eq!(
            one("Build: `cargo build --release`"),
            [
                plain("Build: "),
                Inline::Code {
                    text: "cargo build --release".to_owned()
                }
            ]
        );
        assert_eq!(
            one("`**not bold**`"),
            [Inline::Code {
                text: "**not bold**".to_owned()
            }]
        );
        assert_eq!(
            one("`` a ` b ``"),
            [Inline::Code {
                text: "a ` b".to_owned()
            }]
        );
        assert_eq!(one("a ` b"), [plain("a ` b")]);
        // A mark does not end inside code.
        assert_eq!(
            one("*a `*` b*"),
            [
                marked("a ", &[Mark::Italic]),
                Inline::Code {
                    text: "*".to_owned()
                },
                marked(" b", &[Mark::Italic])
            ]
        );
    }

    #[test]
    fn links_are_taken_when_the_server_accepts_their_address() {
        assert_eq!(
            one("See [the spec](https://example.test/spec)."),
            [
                plain("See "),
                link("the spec", "https://example.test/spec"),
                plain(".")
            ]
        );
        assert_eq!(
            one("Go to https://example.test/a(b)?c=1, then stop."),
            [
                plain("Go to "),
                link(
                    "https://example.test/a(b)?c=1",
                    "https://example.test/a(b)?c=1"
                ),
                plain(", then stop.")
            ]
        );
        assert_eq!(
            one("(https://example.test/x)"),
            [
                plain("("),
                link("https://example.test/x", "https://example.test/x"),
                plain(")")
            ]
        );
        assert_eq!(
            one("[mail](mailto:ana@example.test)"),
            [link("mail", "mailto:ana@example.test")]
        );
        // Another scheme, credentials and an empty text are no link.
        for text in [
            "[x](javascript:alert(1))",
            "[x](file:///etc/passwd)",
            "[x](https://user:secret@example.test/)",
            "[x](not an address)",
        ] {
            let content = one(text);
            assert!(
                content
                    .iter()
                    .all(|node| !matches!(node, Inline::Link { .. })),
                "{text}: {content:?}"
            );
        }
        assert!(link_is_accepted("http://example.test"));
        assert!(!link_is_accepted("ftp://example.test"));
        assert!(!link_is_accepted(&format!(
            "https://example.test/{}",
            "a".repeat(2048)
        )));
        assert_eq!(
            one("xhttps://example.test"),
            [plain("xhttps://example.test")]
        );
        assert_eq!(one("https://"), [plain("https://")]);
    }

    #[test]
    fn a_name_with_a_line_break_is_written_as_one_mention_on_one_line() {
        let name = |_: MentionTarget| "Ann\n- Bø".to_owned();
        let account_id = AccountId::generate();
        let document = Document {
            blocks: vec![
                paragraph(vec![
                    plain("to "),
                    Inline::Account { account_id },
                    plain(" now"),
                ]),
                Block::Quote {
                    content: vec![plain("to "), Inline::Account { account_id }],
                },
            ],
        };
        let shown = written(&document, &name);
        assert_eq!(shown.text, "to @Ann - Bø now\n\n> to @Ann - Bø");
        assert_eq!(shown.mentions.len(), 2);
        for mention in &shown.mentions {
            assert_eq!(shown.text.get(mention.range.clone()), Some("@Ann - Bø"));
        }
        assert_eq!(parse(&shown.text, &shown.mentions), document);
    }

    #[test]
    fn lines_become_the_blocks_of_the_document() {
        let text = "First paragraph\nwith two lines.\n\n> quoted\n> text\n\n- one\n- two\n\n1. first\n2. second\n\n- [ ] open\n- [x] done\n\n```rust\nfn main() {}\n\n  // indented\n```\nafter";
        let document = parse(text, &[]);
        assert_eq!(
            document.blocks,
            [
                paragraph(vec![plain("First paragraph\nwith two lines.")]),
                Block::Quote {
                    content: vec![plain("quoted\ntext")]
                },
                Block::List {
                    ordered: false,
                    items: vec![vec![plain("one")], vec![plain("two")]]
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![plain("first")], vec![plain("second")]]
                },
                Block::Checklist {
                    items: vec![
                        CheckItem {
                            checked: false,
                            content: vec![plain("open")]
                        },
                        CheckItem {
                            checked: true,
                            content: vec![plain("done")]
                        }
                    ]
                },
                Block::Code {
                    text: "fn main() {}\n\n  // indented".to_owned(),
                    language: Some("rust".to_owned())
                },
                paragraph(vec![plain("after")]),
            ]
        );
        // A code block without its end takes the rest. A language that the server
        // refuses is left out.
        assert_eq!(
            parse("```<html>\ncode", &[]).blocks,
            [Block::Code {
                text: "code".to_owned(),
                language: None
            }]
        );
        assert!(parse("```\n\n```", &[]).blocks.is_empty());
        assert!(parse("  \n\n \t ", &[]).blocks.is_empty());
        // A block mark needs its space, and a number is not a year.
        assert_eq!(one("-not a list"), [plain("-not a list")]);
        assert_eq!(one("2026. That year."), [plain("2026. That year.")]);
        assert_eq!(one("\\- not a list"), [plain("- not a list")]);
        assert_eq!(one("\\> not a quote"), [plain("> not a quote")]);
        assert_eq!(one("1\\. not a list"), [plain("1. not a list")]);
    }

    #[test]
    fn mentions_are_tokens_and_not_text() {
        let account_id = AccountId::generate();
        let role_id = WorkspaceRoleId::generate();
        let text = "Hello @Bruno and @leads, also @everyone and @Bruno.";
        let at = |name: &str, nth: usize| {
            let start = text.match_indices(name).nth(nth).unwrap().0;
            start..start + name.len()
        };
        let mentions = [
            Mention {
                range: at("@everyone", 0),
                target: MentionTarget::Everyone,
            },
            Mention {
                range: at("@Bruno", 0),
                target: MentionTarget::Account(account_id),
            },
            Mention {
                range: at("@leads", 0),
                target: MentionTarget::Role(role_id),
            },
        ];
        assert_eq!(
            parse(text, &mentions).blocks,
            [paragraph(vec![
                plain("Hello "),
                Inline::Account { account_id },
                plain(" and "),
                Inline::Role { role_id },
                plain(", also "),
                Inline::Everyone,
                // The second "@Bruno" has no token: it is text.
                plain(" and @Bruno."),
            ])]
        );
        // A mention inside a mark, and a range that is no range of the text.
        let text = "**@Bruno wins**";
        let mentions = [
            Mention {
                range: 2..8,
                target: MentionTarget::Account(account_id),
            },
            Mention {
                range: 40..50,
                target: MentionTarget::Everyone,
            },
        ];
        assert_eq!(
            parse(text, &mentions).blocks,
            [paragraph(vec![
                Inline::Account { account_id },
                marked(" wins", &[Mark::Bold]),
            ])]
        );
        // A private-use character that a person pasted names no mention.
        assert_eq!(one("a\u{F0000}b"), [plain("ab")]);
    }

    #[test]
    fn what_the_server_refuses_in_text_is_left_out() {
        assert_eq!(one("a\u{7}b\r\nc\td"), [plain("ab\nc\td")]);
    }

    #[test]
    fn a_document_is_checked_against_the_limits_of_the_server() {
        assert_eq!(check(&parse("  ", &[]), false), Err(Problem::Empty));
        assert_eq!(check(&parse("  ", &[]), true), Ok(()));
        assert_eq!(check(&parse("text", &[]), false), Ok(()));
        let long = "word ".repeat(4000);
        assert!(matches!(
            check(&parse(&long, &[]), false),
            Err(Problem::TooLong { bytes }) if bytes > MOST_BYTES
        ));
        let blocks = "p\n\n".repeat(101);
        assert_eq!(
            check(&parse(&blocks, &[]), false),
            Err(Problem::TooManyBlocks { blocks: 101 })
        );
        assert_eq!(check(&parse(&"p\n\n".repeat(100), &[]), false), Ok(()));
        let nodes = "a `b` ".repeat(501);
        assert!(matches!(
            check(&parse(&nodes, &[]), false),
            Err(Problem::TooManyNodes { nodes }) if nodes > MOST_NODES
        ));
        let many = Document {
            blocks: vec![paragraph(
                (0..101)
                    .map(|_| Inline::Account {
                        account_id: AccountId::generate(),
                    })
                    .collect(),
            )],
        };
        assert_eq!(
            check(&many, false),
            Err(Problem::TooManyMentions { mentions: 101 })
        );
        // The same mention many times is one mention.
        let account_id = AccountId::generate();
        let same = Document {
            blocks: vec![paragraph(vec![Inline::Account { account_id }; 200])],
        };
        assert_eq!(check(&same, false), Ok(()));
        let mention_only = Document {
            blocks: vec![paragraph(vec![Inline::Everyone])],
        };
        assert!(has_content(&mention_only));
    }

    #[test]
    fn the_text_of_a_document_gives_the_document_again() {
        let account_id = AccountId::generate();
        let role_id = WorkspaceRoleId::generate();
        let name = |target: MentionTarget| match target {
            MentionTarget::Account(_) => "Bruno Costa".to_owned(),
            MentionTarget::Role(_) => "leads".to_owned(),
            MentionTarget::Everyone => "everyone".to_owned(),
        };
        let documents = [
            Document {
                blocks: vec![
                    paragraph(vec![
                        plain("The "),
                        marked("v0.9", &[Mark::Bold]),
                        plain(" list: 2 * 3, snake_case, [x], a\\b, ~ok~ and "),
                        marked("both", &[Mark::Bold, Mark::Italic]),
                        plain(" then "),
                        Inline::Code {
                            text: "a ` b".to_owned(),
                        },
                        plain(" and "),
                        link("the (spec)", "https://example.test/a(b)"),
                        plain(" or https://example.test/x "),
                        link("https://example.test/y", "https://example.test/y"),
                        plain("."),
                    ]),
                    paragraph(vec![
                        plain("Hello "),
                        Inline::Account { account_id },
                        plain(", "),
                        Inline::Role { role_id },
                        plain(" and "),
                        Inline::Everyone,
                    ]),
                    paragraph(vec![
                        plain("- not a list "),
                        Inline::Account { account_id },
                        plain("\n> not a quote\n1. not numbered "),
                        Inline::Everyone,
                        plain("\n```\nplain"),
                    ]),
                    Block::Quote {
                        content: vec![
                            plain("quoted "),
                            Inline::Account { account_id },
                            plain("\nsecond line with "),
                            Inline::Everyone,
                        ],
                    },
                    Block::List {
                        ordered: false,
                        items: vec![
                            vec![plain("one "), marked("bold", &[Mark::Bold])],
                            vec![Inline::Account { account_id }, plain(" two")],
                        ],
                    },
                    Block::List {
                        ordered: true,
                        items: vec![vec![plain("first")], vec![plain("second")]],
                    },
                    Block::Checklist {
                        items: vec![
                            CheckItem {
                                checked: true,
                                content: vec![plain("done")],
                            },
                            CheckItem {
                                checked: false,
                                content: vec![plain("open "), Inline::Role { role_id }],
                            },
                        ],
                    },
                    Block::Code {
                        text: "fn main() {\n    let fence = \"```\";\n}".to_owned(),
                        language: Some("rust".to_owned()),
                    },
                    Block::Code {
                        text: "no language".to_owned(),
                        language: None,
                    },
                ],
            },
            Document { blocks: Vec::new() },
        ];
        for document in documents {
            let (text, mentions) = write(&document, &name);
            for mention in &mentions {
                assert!(text[mention.range.clone()].starts_with('@'), "{text}");
            }
            let again = parse(&text, &mentions);
            // A bare address in plain text becomes a link: that is the one difference.
            let expected = serde_json::to_string(&document)
                .unwrap()
                .replace(
                    r#"{"kind":"text","text":" or https://example.test/x ","marks":[]}"#,
                    r#"{"kind":"text","text":" or ","marks":[]},{"kind":"link","text":"https://example.test/x","url":"https://example.test/x"},{"kind":"text","text":" ","marks":[]}"#,
                );
            assert_eq!(serde_json::to_string(&again).unwrap(), expected, "{text}");
        }
    }

    /// The text of an edit: the first `find` of the shown text replaced by `put`, with
    /// the mentions after it moved.
    fn edit(shown: &Written, find: &str, put: &str) -> (String, Vec<Mention>) {
        let at = shown.text.find(find).unwrap();
        let text = shown.text.replacen(find, put, 1);
        let mentions = shown
            .mentions
            .iter()
            .map(|mention| {
                if mention.range.start < at {
                    return mention.clone();
                }
                let moved = |byte: usize| byte + put.len() - find.len();
                Mention {
                    range: moved(mention.range.start)..moved(mention.range.end),
                    target: mention.target,
                }
            })
            .collect();
        (text, mentions)
    }

    #[test]
    fn an_edit_keeps_the_blocks_that_the_person_did_not_change() {
        let name = |_: MentionTarget| "Ana".to_owned();
        let account_id = AccountId::generate();
        // A document of another client. The text of the composer cannot state the line
        // break in the list item, the empty line in the paragraph, the spaces at its
        // ends, the spaces in the code, and the address that is no link.
        let original = Document {
            blocks: vec![
                Block::List {
                    ordered: false,
                    items: vec![vec![plain("first\nsecond")], vec![plain("third")]],
                },
                paragraph(vec![
                    plain(" one\n\ntwo "),
                    Inline::Code {
                        text: " padded ".to_owned(),
                    },
                ]),
                paragraph(vec![
                    plain("see https://example.com now "),
                    Inline::Account { account_id },
                ]),
                paragraph(vec![plain("last")]),
            ],
        };
        let shown = written(&original, &name);
        assert_ne!(parse(&shown.text, &shown.mentions), original);
        // Nothing was changed.
        assert_eq!(
            parse_edit(&original, &shown, &shown.text, &shown.mentions),
            original
        );

        // The last block was changed.
        let text = format!("{} word", shown.text);
        let edited = parse_edit(&original, &shown, &text, &shown.mentions);
        assert_eq!(edited.blocks[..3], original.blocks[..3]);
        assert_eq!(edited.blocks[3..], [paragraph(vec![plain("last word")])]);

        // One item of the list was changed: the other item keeps its line break.
        let (text, mentions) = edit(&shown, "third", "3rd");
        let edited = parse_edit(&original, &shown, &text, &mentions);
        assert_eq!(
            edited.blocks[0],
            Block::List {
                ordered: false,
                items: vec![vec![plain("first\nsecond")], vec![plain("3rd")]],
            }
        );
        assert_eq!(edited.blocks[1..], original.blocks[1..]);

        // A block was put between two blocks.
        let (text, mentions) = edit(&shown, "\n\nlast", "\n\nnew\n\nlast");
        let edited = parse_edit(&original, &shown, &text, &mentions);
        assert_eq!(edited.blocks[..3], original.blocks[..3]);
        assert_eq!(
            edited.blocks[3..],
            [paragraph(vec![plain("new")]), original.blocks[3].clone()]
        );

        // The person removed the mention and left its text: that block is read again.
        let edited = parse_edit(&original, &shown, &shown.text, &[]);
        assert_eq!(edited.blocks[..2], original.blocks[..2]);
        assert_eq!(
            edited.blocks[2],
            paragraph(vec![
                plain("see "),
                link("https://example.com", "https://example.com"),
                plain(" now @Ana"),
            ])
        );
        assert_eq!(edited.blocks[3], original.blocks[3]);

        // A code block without its end takes the rest of the text, as in a new message:
        // no block after it is kept.
        let (text, mentions) = edit(&shown, "\n\nlast", "\n\n```\ncode\n\nlast");
        let edited = parse_edit(&original, &shown, &text, &mentions);
        assert_eq!(edited.blocks[..3], original.blocks[..3]);
        assert_eq!(
            edited.blocks[3..],
            [Block::Code {
                text: "code\n\nlast".to_owned(),
                language: None,
            }]
        );
    }

    #[test]
    fn a_backtick_in_a_link_does_not_take_the_code_after_it() {
        let code = |text: &str| Inline::Code {
            text: text.to_owned(),
        };
        assert_eq!(
            one("[`label](https://example.test) `code`"),
            [
                link("`label", "https://example.test"),
                plain(" "),
                code("code")
            ]
        );
        assert_eq!(
            one("https://example.test/`a and `code`"),
            [
                link("https://example.test/`a", "https://example.test/`a"),
                plain(" and "),
                code("code")
            ]
        );
        // A backslash before a backtick, and marks around code.
        assert_eq!(one("\\`a `b`"), [plain("`a "), code("b")]);
        assert_eq!(
            one("**bold `code*` end**"),
            [
                marked("bold ", &[Mark::Bold]),
                code("code*"),
                marked(" end", &[Mark::Bold])
            ]
        );
        assert_eq!(one("``a ` b`` `c"), [code("a ` b"), plain(" `c")]);
    }

    #[test]
    fn an_edit_of_one_check_box_keeps_the_other_ones() {
        let name = |_: MentionTarget| String::new();
        let item = |checked: bool, text: &str| CheckItem {
            checked,
            content: vec![plain(text)],
        };
        let original = Document {
            blocks: vec![Block::Checklist {
                items: vec![
                    item(true, "first\nline"),
                    item(false, "second"),
                    item(false, " third "),
                ],
            }],
        };
        let shown = written(&original, &name);
        let (text, mentions) = edit(&shown, "- [ ] second", "- [x] 2nd");
        let edited = parse_edit(&original, &shown, &text, &mentions);
        assert_eq!(
            edited.blocks,
            [Block::Checklist {
                items: vec![
                    item(true, "first\nline"),
                    item(true, "2nd"),
                    item(false, " third "),
                ],
            }]
        );
        // A box that the person ticks in the text keeps the text of its item.
        let (text, mentions) = edit(&shown, "- [x] first", "- [ ] first");
        let edited = parse_edit(&original, &shown, &text, &mentions);
        assert_eq!(
            edited.blocks,
            [Block::Checklist {
                items: vec![
                    item(false, "first\nline"),
                    item(false, "second"),
                    item(false, " third "),
                ],
            }]
        );
    }

    #[test]
    fn the_work_to_read_a_text_for_marks_grows_with_its_length() {
        // The steps that a read may take for each byte of the text. A text of 30,000
        // bytes that is searched again for each of its marks takes 5,000 and more.
        const PLAIN: u64 = 64;
        // Each start of a link is searched as far as the longest address reaches.
        const LINKS: u64 = 2 * LONGEST_LINK as u64 + 64;
        let texts = [
            // Marks that open and never close.
            ("*a ".repeat(10_000), PLAIN),
            ("_a ".repeat(10_000), PLAIN),
            ("~~a ".repeat(7_000), PLAIN),
            ("**a ".repeat(7_000), PLAIN),
            // Brackets without an end, and with an end that is no link.
            ("[".repeat(30_000), PLAIN),
            (format!("{}]", "[".repeat(30_000)), PLAIN),
            (format!("{}](javascript:x)", "[".repeat(30_000)), PLAIN),
            // Runs of backticks of every length, none of which closes.
            (
                (1..=240)
                    .map(|run| format!("{} ", "`".repeat(run)))
                    .collect(),
                PLAIN,
            ),
            // Marks inside marks.
            (
                format!("{}x{}", "*".repeat(15_000), "*".repeat(15_000)),
                PLAIN,
            ),
            (
                format!("{}x{}", "~".repeat(15_000), "~".repeat(15_000)),
                PLAIN,
            ),
            (
                format!("{}x{}", "_*".repeat(7_000), "*_".repeat(7_000)),
                PLAIN,
            ),
            // Addresses without an end.
            ("[a](x".repeat(6_000), LINKS),
            ("http://".repeat(4_000), LINKS),
            (format!("{} ", "http://\u{1}".repeat(3_000)), LINKS),
        ];
        for (text, most) in texts {
            assert!(text.len() <= MOST_TEXT);
            STEPS.with(|steps| steps.set(0));
            let document = parse(&text, &[]);
            let steps = STEPS.with(std::cell::Cell::get);
            assert!(!document.blocks.is_empty());
            assert!(
                steps <= most * text.len() as u64,
                "{steps} steps for {} bytes that start with {:?}",
                text.len(),
                &text[..12]
            );
        }
    }

    #[test]
    fn a_text_above_the_bound_is_not_read_for_marks() {
        let text = format!("{}x{}", "*".repeat(200_000), "*".repeat(200_000));
        let document = parse(&text, &[]);
        assert_eq!(document.blocks, [paragraph(vec![plain(&text)])]);
        assert!(matches!(
            check(&document, false),
            Err(Problem::TooLong { .. })
        ));
    }

    #[test]
    fn white_space_at_the_end_of_marked_text_stays_outside_the_mark() {
        let document = Document {
            blocks: vec![paragraph(vec![
                marked("bold ", &[Mark::Bold]),
                plain("plain"),
            ])],
        };
        let (text, _) = write(&document, &|_| String::new());
        assert_eq!(text, "**bold** plain");
    }
}

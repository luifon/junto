use super::{Drawing, Drawings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Query {
    pub start: usize,
    pub text: String,
    pub closed: bool,
}

fn boundary(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

fn code_at(text: &str) -> bool {
    let mut fence = None;
    let mut inline = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let run = trimmed.bytes().take_while(|byte| *byte == b'`').count();
        if run >= 3 && inline.is_none() {
            match fence {
                None => fence = Some(run),
                Some(open) if run >= open => fence = None,
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let bytes = line.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'\\' => at += 2,
                b'`' => {
                    let run = bytes[at..].iter().take_while(|byte| **byte == b'`').count();
                    match inline {
                        None => inline = Some(run),
                        Some(open) if open == run => inline = None,
                        _ => {}
                    }
                    at += run;
                }
                _ => at += 1,
            }
        }
    }
    fence.is_some() || inline.is_some()
}

pub(crate) fn query(text: &str, cursor: usize) -> Option<Query> {
    let before = text.get(..cursor)?;
    let last = before.rfind(':')?;
    let (start, closed) = if last + 1 == before.len() {
        before[..last]
            .rfind(':')
            .filter(|start| boundary(before, *start))
            .map_or((last, false), |start| (start, true))
    } else {
        (last, false)
    };
    let end = if closed {
        before.len() - 1
    } else {
        before.len()
    };
    let name = &before[start + 1..end];
    if !boundary(before, start)
        || name.len() > 128
        || (closed && name.is_empty())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+'))
        || code_at(&before[..start])
    {
        return None;
    }
    Some(Query {
        start,
        text: name.to_ascii_lowercase(),
        closed,
    })
}

fn name(emoji: &emojis::Emoji) -> String {
    let mut name = String::new();
    for character in emoji.name().chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('_') {
            name.push('_');
        }
    }
    name.trim_end_matches('_').to_owned()
}

pub(crate) fn label(emoji: &emojis::Emoji) -> String {
    emoji.shortcode().map_or_else(|| name(emoji), str::to_owned)
}

pub(crate) fn exact(query: &str) -> Option<&'static emojis::Emoji> {
    emojis::get_by_shortcode(query).or_else(|| super::table().find(|emoji| name(emoji) == query))
}

pub(crate) fn candidates(
    query: &str,
    recent: &[String],
    drawings: &Drawings,
) -> Vec<&'static emojis::Emoji> {
    let mut found = Vec::new();
    if query.is_empty() {
        found.extend(
            recent
                .iter()
                .filter_map(|value| emojis::get(value))
                .filter(|emoji| drawings.drawing(emoji) != Drawing::Broken),
        );
    }
    let mut ranked: Vec<_> = super::table()
        .filter_map(|emoji| {
            if drawings.drawing(emoji) == Drawing::Broken {
                return None;
            }
            let name = name(emoji);
            let score = if emoji.shortcodes().any(|shortcode| shortcode == query) || name == query {
                0
            } else if emoji
                .shortcodes()
                .any(|shortcode| shortcode.starts_with(query))
            {
                1
            } else if name.starts_with(query) {
                2
            } else if name.contains(query) {
                3
            } else {
                return None;
            };
            Some((score, emoji))
        })
        .collect();
    ranked.sort_by_key(|(score, _)| *score);
    for (_, emoji) in ranked {
        if !found.iter().any(|held| held.as_str() == emoji.as_str()) {
            found.push(emoji);
        }
        if found.len() >= 8 {
            break;
        }
    }
    found.truncate(8);
    found
}

pub(crate) fn used(document: &junto_client_core::protocol::message::Document) -> Vec<String> {
    use junto_client_core::protocol::message::{Block, Inline};
    fn text(found: &mut Vec<String>, value: &str) {
        for emoji in unicode_segmentation::UnicodeSegmentation::graphemes(value, true) {
            junto_client_core::emoji_history::remember(found, emoji);
        }
    }
    fn content(found: &mut Vec<String>, nodes: &[Inline]) {
        for (node, _) in junto_client_core::protocol::message::inline_nodes(nodes) {
            match node {
                Inline::Text { text: value, .. }
                | Inline::Code { text: value }
                | Inline::Link { text: value, .. } => text(found, value),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    for block in &document.blocks {
        match block {
            Block::Code { text: value, .. } => text(&mut found, value),
            Block::Heading { content: nodes, .. }
            | Block::Small { content: nodes }
            | Block::Paragraph { content: nodes }
            | Block::Quote { content: nodes } => content(&mut found, nodes),
            Block::List { items, .. } => {
                for nodes in items {
                    content(&mut found, nodes);
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    content(&mut found, &item.content);
                }
            }
            Block::Forward { .. } => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcode_requires_a_boundary_and_stays_literal_in_code_and_addresses() {
        for text in [
            "https://example.test/:smile:",
            "\\:smile:",
            "`code :smile:",
            "```rust\n:smile:",
            "time12:smile:",
            "::",
        ] {
            assert_eq!(query(text, text.len()), None, "{text}");
        }
        let text = "日本語 :thumbsup";
        assert_eq!(
            query(text, text.len()),
            Some(Query {
                start: "日本語 ".len(),
                text: "thumbsup".to_owned(),
                closed: false
            })
        );
        assert!(query(":smile:", 7).unwrap().closed);
        assert!(query("`code` :smile", 13).is_some());
        assert_eq!(query("é", 1), None);
    }

    #[test]
    fn submitted_unicode_choices_keep_whole_sequences_and_document_order() {
        let family = "👨‍👩‍👧‍👦";
        let document = crate::compose::parse(&format!("😀\n```text\n{family}\n```\n🎉"), &[]);
        assert_eq!(used(&document), ["🎉", family, "😀"]);
    }

    #[test]
    fn exact_shortcodes_rank_first_and_recent_choices_open_the_empty_query() {
        let smile = exact("smile").unwrap();
        assert_eq!(
            candidates("smile", &[], &Drawings::default())[0].as_str(),
            smile.as_str()
        );
        let party = exact("tada").unwrap();
        assert_eq!(
            candidates("", &[party.as_str().to_owned()], &Drawings::default())[0].as_str(),
            party.as_str()
        );
        assert!(candidates("a name that cannot match", &[], &Drawings::default()).is_empty());
    }
}

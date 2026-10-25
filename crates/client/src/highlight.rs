//! The syntax colours of a code block (ADR 0740).
//!
//! The library `synoptic` finds the tokens of a text with the rules of its language. This
//! module names the language of a block, bounds the text that is examined, and reduces the
//! tokens to the five colours of the design. A block comes from another account: a text
//! that the library cannot follow is drawn without colours.

use std::ops::Range;

use synoptic::TokOpt;

/// The longest code that is examined for its tokens, in bytes and in lines. A longer block
/// is drawn in one colour.
pub const MOST_BYTES: usize = 32 * 1024;
pub const MOST_LINES: usize = 1_000;

/// What a part of a code block is, as the design colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    Keyword,
    /// The name of a type, a tag, a key or a table.
    Type,
    /// The name of a function or of a macro.
    Function,
    /// A number or a text in quotation marks.
    Value,
    Comment,
}

/// The name of the rules of a language, by the name that a person writes after the three
/// backticks. Absent for a language without rules.
pub fn rules_of(language: &str) -> Option<&'static str> {
    let name = language.trim().to_lowercase();
    Some(match name.as_str() {
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "ruby" | "rb" => "rb",
        "lua" => "lua",
        "r" => "r",
        "go" | "golang" => "go",
        "javascript" | "js" | "jsx" | "mjs" | "node" => "js",
        "typescript" | "ts" | "tsx" => "ts",
        "dart" => "dart",
        "c" | "h" => "c",
        "c++" | "cpp" | "cxx" | "cc" | "hpp" => "cpp",
        "c#" | "cs" | "csharp" => "cs",
        "swift" => "swift",
        "json" | "jsonc" => "json",
        "kotlin" | "kt" => "kt",
        "java" => "java",
        "php" => "php",
        "scala" => "scala",
        "haskell" | "hs" => "hs",
        "css" | "scss" => "css",
        "html" | "htm" | "xhtml" | "vue" | "svelte" => "html",
        "markdown" | "md" => "md",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "shell" | "sh" | "bash" | "zsh" | "console" => "sh",
        "sql" | "postgres" | "postgresql" | "psql" => "sql",
        "xml" | "svg" => "xml",
        "diff" | "patch" => "diff",
        "asm" | "assembly" => "asm",
        "tex" | "latex" => "tex",
        "nu" | "nushell" => "nu",
        _ => return None,
    })
}

/// The colour of a token of the library, by its name there.
fn token(name: &str) -> Option<Token> {
    Some(match name {
        "keyword" | "kw" | "boolean" => Token::Keyword,
        "struct" | "namespace" | "attribute" | "tag" | "key" | "table" | "header" | "heading" => {
            Token::Type
        }
        "function" | "macro" => Token::Function,
        "digit" | "string" | "character" | "link" => Token::Value,
        "comment" => Token::Comment,
        _ => return None,
    })
}

/// The tokens of a code block in the order of the text. Each range lies on character
/// boundaries of `code`, and no two ranges overlap. Empty for a language without rules,
/// for a text above the bounds, and for a text that the library does not give back as it
/// was written.
pub fn tokens(code: &str, language: Option<&str>) -> Vec<(Range<usize>, Token)> {
    let Some(rules) = language.and_then(rules_of) else {
        return Vec::new();
    };
    if code.len() > MOST_BYTES {
        return Vec::new();
    }
    if code.contains(['\t', '\r']) {
        // The library counts a tab as several columns, and a carriage return is no part
        // of a line for it. The text is examined with a space in the place of each,
        // which has the same length in bytes.
        let spaced: String = code
            .chars()
            .map(|character| match character {
                '\t' | '\r' => ' ',
                other => other,
            })
            .collect();
        return refined(code, marked(&spaced, rules).unwrap_or_default(), rules);
    }
    refined(code, marked(code, rules).unwrap_or_default(), rules)
}

/// The languages in which a name that starts with a capital letter is the name of a type.
const CAPITAL_TYPES: [&str; 13] = [
    "rs", "ts", "js", "java", "kt", "cs", "swift", "cpp", "dart", "scala", "php", "hs", "py",
];

/// What this module adds to the tokens of the library: a token ends before white space,
/// a word token that lies inside a longer word is no token, and in the languages that
/// write a type with a capital letter, such a name is a type.
fn refined(
    code: &str,
    found: Vec<(Range<usize>, Token)>,
    rules: &'static str,
) -> Vec<(Range<usize>, Token)> {
    let letter_before = |at: usize| {
        code[..at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphanumeric() || before == '_')
    };
    let letter_after = |at: usize| {
        code[at..]
            .chars()
            .next()
            .is_some_and(|after| after.is_alphanumeric() || after == '_')
    };
    let mut marks: Vec<(Range<usize>, Token)> = Vec::with_capacity(found.len());
    let capitals = CAPITAL_TYPES.contains(&rules);
    let mut gap_start = 0;
    let names = |from: usize, to: usize, marks: &mut Vec<(Range<usize>, Token)>| {
        if !capitals || from >= to {
            return;
        }
        let mut start: Option<usize> = None;
        let gap = &code[from..to];
        let mut close = |start: &mut Option<usize>, end: usize| {
            if let Some(begin) = start.take() {
                let name = &gap[begin..end];
                let mut letters = name.chars();
                let capital = letters.next().is_some_and(char::is_uppercase);
                let lower = letters.any(char::is_lowercase);
                let whole = !letter_before(from + begin) && !letter_after(from + end);
                if capital && lower && whole {
                    marks.push((from + begin..from + end, Token::Type));
                }
            }
        };
        for (index, character) in gap.char_indices() {
            if character.is_alphanumeric() || character == '_' {
                start.get_or_insert(index);
            } else {
                close(&mut start, index);
            }
        }
        close(&mut start, gap.len());
    };
    for (range, token) in found {
        names(gap_start, range.start, &mut marks);
        gap_start = range.end;
        let end = range.start + code[range.clone()].trim_end().len();
        if end == range.start {
            continue;
        }
        let word = matches!(token, Token::Keyword | Token::Type | Token::Function);
        if word && (letter_before(range.start) || letter_after(end)) {
            continue;
        }
        marks.push((range.start..end, token));
    }
    names(gap_start, code.len(), &mut marks);
    marks
}

fn marked(code: &str, rules: &'static str) -> Option<Vec<(Range<usize>, Token)>> {
    let lines: Vec<String> = code.split('\n').map(str::to_owned).collect();
    if lines.len() > MOST_LINES {
        return None;
    }
    // A fault of the library costs the colours of one block and not the window.
    let found = crate::panics::handled_by_the_caller(|| {
        std::panic::catch_unwind(|| {
            let mut highlighter = synoptic::from_extension(rules, 1)?;
            highlighter.run(&lines);
            let mut marks = Vec::new();
            let mut start = 0;
            for (number, line) in lines.iter().enumerate() {
                let mut at = start;
                for part in highlighter.line(number, line) {
                    let (text, name) = match &part {
                        TokOpt::Some(text, name) => (text, Some(name.as_str())),
                        TokOpt::None(text) => (text, None),
                    };
                    let end = at + text.len();
                    // The library must give the text back as it was written.
                    if code.get(at..end) != Some(text.as_str()) {
                        return None;
                    }
                    if let Some(token) = name.and_then(token)
                        && end > at
                    {
                        marks.push((at..end, token));
                    }
                    at = end;
                }
                if at != start + line.len() {
                    return None;
                }
                start += line.len() + 1;
            }
            Some(marks)
        })
    });
    found.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(code: &str, language: &str) -> Vec<(String, Token)> {
        tokens(code, Some(language))
            .into_iter()
            .map(|(range, token)| (code[range].to_owned(), token))
            .collect()
    }

    fn has(marks: &[(String, Token)], text: &str, token: Token) -> bool {
        marks.iter().any(|(held, of)| held == text && *of == token)
    }

    #[test]
    fn the_code_of_the_design_gets_its_five_colours() {
        let code = "fn mix_tracks(tracks: &[Track]) -> Mixdown {\n    \
                    tracks.iter().filter(|t| !t.muted)   // skip silent tracks\n        \
                    .map(|t| t.resample(48_000)).collect()\n}";
        let marks = found(code, "Rust");
        assert!(has(&marks, "fn", Token::Keyword), "{marks:?}");
        assert!(has(&marks, "Track", Token::Type), "{marks:?}");
        assert!(has(&marks, "Mixdown", Token::Type), "{marks:?}");
        assert!(has(&marks, "48_000", Token::Value), "{marks:?}");
        assert!(
            has(&marks, "// skip silent tracks", Token::Comment),
            "{marks:?}"
        );
        assert!(
            marks.iter().any(|(_, token)| *token == Token::Function),
            "{marks:?}"
        );
    }

    #[test]
    fn a_language_is_named_as_people_write_it() {
        assert_eq!(rules_of("Rust"), Some("rs"));
        assert_eq!(rules_of(" TypeScript "), Some("ts"));
        assert_eq!(rules_of("C++"), Some("cpp"));
        assert_eq!(rules_of("TOML"), Some("toml"));
        assert_eq!(rules_of("bash"), Some("sh"));
        assert_eq!(rules_of("klingon"), None);
        assert_eq!(rules_of(""), None);
        // A block without a language, and one of a language without rules, has no colours.
        assert!(tokens("let a = 1;", None).is_empty());
        assert!(tokens("let a = 1;", Some("klingon")).is_empty());
        assert!(!tokens("let a = 1;", Some("rs")).is_empty());
        assert!(!tokens("[retention]\ntracks_days = 30", Some("TOML")).is_empty());
        assert!(!tokens("const a: number = 1;", Some("ts")).is_empty());
    }

    #[test]
    fn a_text_above_the_bounds_has_no_colours() {
        let long = "let a = 1;\n".repeat(MOST_LINES + 1);
        assert!(tokens(&long, Some("rs")).is_empty());
        let wide = format!("let a = \"{}\";", "x".repeat(MOST_BYTES));
        assert!(tokens(&wide, Some("rs")).is_empty());
        let within = "let a = 1;\n".repeat(MOST_LINES - 1);
        assert!(!tokens(&within, Some("rs")).is_empty());
    }

    /// The ranges lie on character boundaries, in the order of the text, without overlap.
    /// Returns how many tokens the text has.
    fn check(code: &str, language: &str) -> usize {
        let marks = tokens(code, Some(language));
        let mut end = 0;
        for (range, _) in &marks {
            assert!(range.start >= end && range.end > range.start, "{code:?}");
            assert!(range.end <= code.len(), "{code:?}");
            assert!(
                code.is_char_boundary(range.start) && code.is_char_boundary(range.end),
                "{code:?}"
            );
            end = range.end;
        }
        marks.len()
    }

    #[test]
    fn the_ranges_follow_the_text_also_with_tabs_and_letters_of_several_bytes() {
        let code = "fn açãO() {\n\tlet s = \"İstanbul — 東京\"; // ✓ done\r\n\tlet n = 4_2;\n}";
        check(code, "rs");
        let marks = found(code, "rs");
        assert!(has(&marks, "fn", Token::Keyword), "{marks:?}");
        assert!(has(&marks, "// ✓ done", Token::Comment), "{marks:?}");
        assert!(has(&marks, "4_2", Token::Value), "{marks:?}");
    }

    #[test]
    fn no_text_of_another_account_stops_the_window() {
        // Texts that a parser can stumble over, in every language with rules: marks that
        // open and never close, marks of other languages, and letters of several bytes.
        let pieces = [
            "\"", "'", "`", "/*", "*/", "//", "#", "<!--", "-->", "{{", "}}", "${", "\\", "\n",
            "\t", "\r\n", " ", "fn", "class", "0x", "1e9", "é", "東", "🙂", "\u{200d}", "<", ">",
            "</", "[", "]", "(", ")", "=", "---", "+++", "@@", "\u{0}",
        ];
        let languages = [
            "rs", "py", "rb", "lua", "r", "go", "js", "ts", "dart", "c", "cpp", "cs", "swift",
            "json", "kt", "java", "php", "scala", "hs", "css", "html", "md", "toml", "yaml", "sh",
            "sql", "xml", "diff", "asm", "tex", "nu",
        ];
        // A fixed sequence of numbers, so that each run examines the same texts.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for language in languages {
            assert!(rules_of(language).is_some(), "{language}");
            let mut found = 0;
            for _ in 0..120 {
                let length = (next() % 24) as usize;
                let code: String = (0..length)
                    .map(|_| pieces[(next() % pieces.len() as u64) as usize])
                    .collect();
                found += check(&code, language);
            }
            // The texts were examined: a library that gave up on each of them would
            // leave every one without a token.
            assert!(found > 0, "{language}");
        }
    }
}

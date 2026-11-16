//! Inline Markdown parser for bold and italic emphasis in script paragraphs.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StyledSpan {
    pub range: Range<usize>,
    pub style: SpanStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedParagraph {
    pub text: String,
    pub spans: Vec<StyledSpan>,
}

#[derive(Clone, Debug)]
enum Token {
    Text(String),
    Delim {
        kind: char,
        count: usize,
        original_count: usize,
        can_open: bool,
        can_close: bool,
    },
}

#[derive(Clone, Debug)]
struct DelimMatch {
    open_token: usize,
    close_token: usize,
    bold: bool,
}

/// Split a raw script into paragraphs separated by blank lines and parse inline
/// bold/italic Markdown in each paragraph.
pub fn parse_paragraphs(input: &str) -> Vec<ParsedParagraph> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current_lines.is_empty() {
                let joined = current_lines.join(" ");
                paragraphs.push(parse_inline(&joined));
                current_lines.clear();
            }
        } else {
            current_lines.push(trimmed);
        }
    }
    if !current_lines.is_empty() {
        let joined = current_lines.join(" ");
        paragraphs.push(parse_inline(&joined));
    }
    paragraphs
}

/// Parse inline `*italic*`, `_italic_`, `**bold**`, `__bold__`, and combined
/// `***bold italic***` / `**_bold italic_**` spans using CommonMark delimiter rules.
pub fn parse_inline(input: &str) -> ParsedParagraph {
    if !input.contains(['*', '_', '\\']) {
        return ParsedParagraph {
            text: input.to_string(),
            spans: Vec::new(),
        };
    }

    let mut tokens = tokenize(input);
    let matches = match_delimiters(&mut tokens);

    let mut text = String::with_capacity(input.len());
    let mut token_ranges: Vec<Range<usize>> = Vec::with_capacity(tokens.len());

    for token in &tokens {
        let start = text.len();
        match token {
            Token::Text(s) => text.push_str(s),
            Token::Delim { kind, count, .. } => {
                for _ in 0..*count {
                    text.push(*kind);
                }
            }
        }
        token_ranges.push(start..text.len());
    }

    if matches.is_empty() || text.is_empty() {
        return ParsedParagraph {
            text,
            spans: Vec::new(),
        };
    }

    // Build byte-indexed style events from matched delimiter ranges.
    let mut events: Vec<(usize, i32, i32)> = Vec::with_capacity(matches.len() * 2);
    for m in &matches {
        let start = token_ranges[m.open_token].end;
        let end = token_ranges[m.close_token].start;
        if start < end {
            let (db, di) = if m.bold { (1, 0) } else { (0, 1) };
            events.push((start, db, di));
            events.push((end, -db, -di));
        }
    }
    events.sort_by_key(|e| e.0);

    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut bold_depth = 0i32;
    let mut italic_depth = 0i32;
    let mut active_start: Option<usize> = None;
    let mut active_style = SpanStyle {
        bold: false,
        italic: false,
    };

    let mut i = 0;
    while i < events.len() {
        let pos = events[i].0;
        if let Some(start) = active_start {
            if start < pos {
                push_merged_span(&mut spans, start..pos, active_style);
            }
        }
        while i < events.len() && events[i].0 == pos {
            bold_depth += events[i].1;
            italic_depth += events[i].2;
            i += 1;
        }
        let next_style = SpanStyle {
            bold: bold_depth > 0,
            italic: italic_depth > 0,
        };
        if next_style.bold || next_style.italic {
            active_start = Some(pos);
            active_style = next_style;
        } else {
            active_start = None;
        }
    }

    ParsedParagraph { text, spans }
}

fn push_merged_span(spans: &mut Vec<StyledSpan>, range: Range<usize>, style: SpanStyle) {
    if let Some(last) = spans.last_mut() {
        if last.range.end == range.start && last.style == style {
            last.range.end = range.end;
            return;
        }
    }
    spans.push(StyledSpan { range, style });
}

fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < chars.len() {
        let (_, ch) = chars[i];
        if ch == '\\' {
            if let Some(&(_, next)) = chars.get(i + 1) {
                if next.is_ascii_punctuation() {
                    buf.push(next);
                    i += 2;
                    continue;
                }
            }
            buf.push('\\');
            i += 1;
            continue;
        }

        if ch == '*' || ch == '_' {
            if !buf.is_empty() {
                tokens.push(Token::Text(std::mem::take(&mut buf)));
            }
            let start_idx = i;
            let mut count = 0;
            while i < chars.len() && chars[i].1 == ch {
                count += 1;
                i += 1;
            }
            let before = if start_idx == 0 {
                ' '
            } else {
                chars[start_idx - 1].1
            };
            let after = if i >= chars.len() { ' ' } else { chars[i].1 };
            let (can_open, can_close) = flanking_flags(ch, before, after);
            tokens.push(Token::Delim {
                kind: ch,
                count,
                original_count: count,
                can_open,
                can_close,
            });
            continue;
        }

        buf.push(ch);
        i += 1;
    }

    if !buf.is_empty() {
        tokens.push(Token::Text(buf));
    }
    tokens
}

fn is_md_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() || (!ch.is_alphanumeric() && !ch.is_whitespace())
}

fn flanking_flags(kind: char, before: char, after: char) -> (bool, bool) {
    let before_ws = before.is_whitespace();
    let after_ws = after.is_whitespace();
    let before_punct = is_md_punctuation(before);
    let after_punct = is_md_punctuation(after);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    if kind == '*' {
        (left_flanking, right_flanking)
    } else {
        let can_open = left_flanking && (!right_flanking || before_punct);
        let can_close = right_flanking && (!left_flanking || after_punct);
        (can_open, can_close)
    }
}

fn match_delimiters(tokens: &mut [Token]) -> Vec<DelimMatch> {
    let mut matches = Vec::new();
    // CommonMark openers_bottom[kind_idx][closer_orig_count % 3][can_open as usize]
    let mut openers_bottom = [[[0usize; 2]; 3]; 2];

    let mut closer_idx = 0;
    while closer_idx < tokens.len() {
        let (c_kind, c_can_close, c_can_open, c_orig, c_count) = match &tokens[closer_idx] {
            Token::Delim {
                kind,
                can_close,
                can_open,
                original_count,
                count,
            } if *can_close && *count > 0 => (*kind, *can_close, *can_open, *original_count, *count),
            _ => {
                closer_idx += 1;
                continue;
            }
        };
        let _ = (c_can_close, c_count);

        let kind_idx = usize::from(c_kind == '_');
        let bottom = openers_bottom[kind_idx][c_orig % 3][usize::from(c_can_open)];

        let mut found_opener = None;
        let mut search = closer_idx;
        while search > bottom {
            search -= 1;
            if let Token::Delim {
                kind,
                can_open,
                can_close,
                original_count,
                count,
            } = &tokens[search]
            {
                if *kind != c_kind || !*can_open || *count == 0 {
                    continue;
                }
                // CommonMark rule 9/10: intra-word / ambiguous open+close multiple-of-3 check.
                let odd_match = (*can_open && *can_close) || (c_can_open && c_can_close);
                if odd_match
                    && (*original_count + c_orig) % 3 == 0
                    && !(*original_count % 3 == 0 && c_orig % 3 == 0)
                {
                    continue;
                }
                found_opener = Some(search);
                break;
            }
        }

        if let Some(opener_idx) = found_opener {
            let o_count = match &tokens[opener_idx] {
                Token::Delim { count, .. } => *count,
                _ => 0,
            };
            let c_cur_count = match &tokens[closer_idx] {
                Token::Delim { count, .. } => *count,
                _ => 0,
            };
            let use_count = if o_count >= 2 && c_cur_count >= 2 {
                2
            } else {
                1
            };

            if let Token::Delim { count, .. } = &mut tokens[opener_idx] {
                *count -= use_count;
            }
            if let Token::Delim { count, .. } = &mut tokens[closer_idx] {
                *count -= use_count;
            }

            // Deactivate any unmatched delimiters strictly between opener and closer.
            for between in &mut tokens[(opener_idx + 1)..closer_idx] {
                if let Token::Delim { count, .. } = between {
                    *count = 0;
                }
            }

            matches.push(DelimMatch {
                open_token: opener_idx,
                close_token: closer_idx,
                bold: use_count == 2,
            });

            // Keep trying the same closer if it still has remaining delimiters (e.g. ***).
            let remaining = match &tokens[closer_idx] {
                Token::Delim { count, .. } => *count,
                _ => 0,
            };
            if remaining == 0 {
                closer_idx += 1;
            }
        } else {
            openers_bottom[kind_idx][c_orig % 3][usize::from(c_can_open)] = closer_idx;
            closer_idx += 1;
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_styles(p: &ParsedParagraph) -> Vec<(&str, bool, bool)> {
        p.spans
            .iter()
            .map(|s| (&p.text[s.range.clone()], s.style.bold, s.style.italic))
            .collect()
    }

    #[test]
    fn plain_text_has_no_spans() {
        let p = parse_inline("Hello world.");
        assert_eq!(p.text, "Hello world.");
        assert!(p.spans.is_empty());
    }

    #[test]
    fn basic_italic_and_bold() {
        let p = parse_inline("Say *this* and **that** and _also_ __more__.");
        assert_eq!(p.text, "Say this and that and also more.");
        assert_eq!(
            slice_styles(&p),
            vec![
                ("this", false, true),
                ("that", true, false),
                ("also", false, true),
                ("more", true, false),
            ]
        );
    }

    #[test]
    fn bold_and_italic_combined_and_nested() {
        let p = parse_inline("***Crucial*** and **very *important* point** and **_both_**.");
        assert_eq!(p.text, "Crucial and very important point and both.");
        assert_eq!(
            slice_styles(&p),
            vec![
                ("Crucial", true, true),
                ("very ", true, false),
                ("important", true, true),
                (" point", true, false),
                ("both", true, true),
            ]
        );
    }

    #[test]
    fn intraword_underscores_and_spaced_asterisks_stay_literal() {
        let p = parse_inline("snake_case_name and 2 * 3 * 4 and *unclosed");
        assert_eq!(p.text, "snake_case_name and 2 * 3 * 4 and *unclosed");
        assert!(p.spans.is_empty());
    }

    #[test]
    fn escaped_delimiters_stay_literal() {
        let p = parse_inline(r"\*literal\* and **bold \*star\***");
        assert_eq!(p.text, "*literal* and bold *star*");
        assert_eq!(slice_styles(&p), vec![("bold *star*", true, false)]);
    }

    #[test]
    fn unicode_boundaries_are_valid() {
        let p = parse_inline("Café **naïve — *résumé*** 🎉");
        assert_eq!(p.text, "Café naïve — résumé 🎉");
        assert_eq!(
            slice_styles(&p),
            vec![("naïve — ", true, false), ("résumé", true, true)]
        );
    }

    #[test]
    fn splits_crlf_and_blank_lines() {
        let ps = parse_paragraphs("First *line*\r\ncontinued.\r\n   \r\n**Second** block.");
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].text, "First line continued.");
        assert_eq!(slice_styles(&ps[0]), vec![("line", false, true)]);
        assert_eq!(ps[1].text, "Second block.");
        assert_eq!(slice_styles(&ps[1]), vec![("Second", true, false)]);
    }
}

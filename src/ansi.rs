//! Minimal ANSI escape sequence handling.
//!
//! `tmux capture-pane -e` keeps the escape sequences that colour a pane, which
//! is what makes the preview look like the real terminal. The status parsers on
//! the other hand want plain text, so the same tokenizer backs both
//! [`strip`] and [`to_lines`]: whatever one drops, the other renders.
//!
//! Only SGR (`ESC [ … m`) sequences carry style information; every other
//! escape sequence is skipped.

use std::borrow::Cow;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// A piece of an ANSI-encoded stream.
enum Token<'a> {
    /// Printable text (never contains control characters)
    Text(&'a str),
    /// End of a line
    Newline,
    /// Parameter bytes of an `ESC [ … m` sequence (empty means "reset")
    Sgr(&'a str),
}

/// Splits ANSI-encoded text into [`Token`]s without allocating.
struct Tokens<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Tokens<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let bytes = self.input.as_bytes();
        loop {
            if self.pos >= bytes.len() {
                return None;
            }

            match bytes[self.pos] {
                b'\n' => {
                    self.pos += 1;
                    return Some(Token::Newline);
                }
                0x1b => {
                    let (params, next) = parse_escape(self.input, self.pos);
                    self.pos = next;
                    if let Some(params) = params {
                        return Some(Token::Sgr(params));
                    }
                    // Non-SGR escape sequence: carries no style, keep scanning.
                }
                // Carriage returns and other control bytes do not survive a
                // capture in any meaningful way.
                b if b < 0x20 && b != b'\t' => {
                    self.pos += 1;
                }
                _ => {
                    let start = self.pos;
                    while self.pos < bytes.len() {
                        let b = bytes[self.pos];
                        // Multi-byte UTF-8 bytes are all >= 0x80, so this never
                        // splits a character.
                        if b == 0x1b || (b < 0x20 && b != b'\t') {
                            break;
                        }
                        self.pos += 1;
                    }
                    return Some(Token::Text(&self.input[start..self.pos]));
                }
            }
        }
    }
}

/// Parses the escape sequence starting at `start` (which must be an `ESC`).
///
/// Returns the SGR parameter bytes when the sequence is `ESC [ … m`, plus the
/// index just past the sequence.
fn parse_escape(input: &str, start: usize) -> (Option<&str>, usize) {
    let bytes = input.as_bytes();
    let mut i = start + 1;

    match bytes.get(i) {
        // CSI: parameters, intermediates, then a final byte
        Some(b'[') => {
            i += 1;
            let params_start = i;
            while matches!(bytes.get(i), Some(0x30..=0x3f)) {
                i += 1;
            }
            let params_end = i;
            while matches!(bytes.get(i), Some(0x20..=0x2f)) {
                i += 1;
            }
            let final_byte = bytes.get(i).copied();
            if final_byte.is_some() {
                i += 1;
            }
            if final_byte == Some(b'm') {
                (Some(&input[params_start..params_end]), i)
            } else {
                (None, i)
            }
        }
        // String sequences (OSC/DCS/SOS/PM/APC) run until BEL or ST
        Some(b']') | Some(b'P') | Some(b'X') | Some(b'^') | Some(b'_') => {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == 0x07 {
                    i += 1;
                    break;
                }
                if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    break;
                }
                i += 1;
            }
            (None, i)
        }
        // Two-byte sequences (charset selection and friends)
        Some(b'(') | Some(b')') | Some(b'*') | Some(b'+') | Some(b'#') | Some(b'%') => {
            (None, (i + 2).min(bytes.len()))
        }
        Some(_) => (None, i + 1),
        None => (None, i),
    }
}

/// Accumulated SGR attributes.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Sgr {
    fg: Option<Color>,
    bg: Option<Color>,
    modifier: Modifier,
}

impl Sgr {
    fn style(&self) -> Style {
        let mut style = Style::default();
        if let Some(fg) = self.fg {
            style = style.fg(fg);
        }
        if let Some(bg) = self.bg {
            style = style.bg(bg);
        }
        if !self.modifier.is_empty() {
            style = style.add_modifier(self.modifier);
        }
        style
    }

    fn apply(&mut self, params: &str) {
        let mut values = params.split([';', ':']);
        while let Some(raw) = values.next() {
            let code: u32 = if raw.is_empty() {
                0 // `ESC[m` and empty parameters mean reset
            } else {
                match raw.parse() {
                    Ok(code) => code,
                    Err(_) => continue,
                }
            };

            match code {
                0 => *self = Self::default(),
                1 => self.modifier.insert(Modifier::BOLD),
                2 => self.modifier.insert(Modifier::DIM),
                3 => self.modifier.insert(Modifier::ITALIC),
                4 => self.modifier.insert(Modifier::UNDERLINED),
                5 => self.modifier.insert(Modifier::SLOW_BLINK),
                6 => self.modifier.insert(Modifier::RAPID_BLINK),
                7 => self.modifier.insert(Modifier::REVERSED),
                8 => self.modifier.insert(Modifier::HIDDEN),
                9 => self.modifier.insert(Modifier::CROSSED_OUT),
                21 | 22 => self.modifier.remove(Modifier::BOLD | Modifier::DIM),
                23 => self.modifier.remove(Modifier::ITALIC),
                24 => self.modifier.remove(Modifier::UNDERLINED),
                25 => self
                    .modifier
                    .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
                27 => self.modifier.remove(Modifier::REVERSED),
                28 => self.modifier.remove(Modifier::HIDDEN),
                29 => self.modifier.remove(Modifier::CROSSED_OUT),
                30..=37 => self.fg = Some(basic_color(code - 30)),
                38 => {
                    if let Some(color) = extended_color(&mut values) {
                        self.fg = Some(color);
                    }
                }
                39 => self.fg = None,
                40..=47 => self.bg = Some(basic_color(code - 40)),
                48 => {
                    if let Some(color) = extended_color(&mut values) {
                        self.bg = Some(color);
                    }
                }
                49 => self.bg = None,
                90..=97 => self.fg = Some(bright_color(code - 90)),
                100..=107 => self.bg = Some(bright_color(code - 100)),
                _ => {}
            }
        }
    }
}

fn basic_color(index: u32) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn bright_color(index: u32) -> Color {
    match index {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

/// Parses the tail of a `38`/`48` colour selector (`5;n` or `2;r;g;b`).
fn extended_color<'a, I: Iterator<Item = &'a str>>(values: &mut I) -> Option<Color> {
    match values.next()?.parse::<u32>().ok()? {
        5 => {
            let index = values.next()?.parse::<u8>().ok()?;
            Some(Color::Indexed(index))
        }
        2 => {
            let r = values.next()?.parse::<u8>().ok()?;
            let g = values.next()?.parse::<u8>().ok()?;
            let b = values.next()?.parse::<u8>().ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

/// Removes every escape sequence, keeping the printable text.
///
/// Line structure is preserved, so `strip(text).lines().count()` always equals
/// `to_lines(text).len()`.
pub fn strip(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for token in Tokens::new(input) {
        match token {
            Token::Text(text) => out.push_str(text),
            Token::Newline => out.push('\n'),
            Token::Sgr(_) => {}
        }
    }
    out
}

/// Converts ANSI-coloured text into styled lines, borrowing from `input`.
///
/// Attributes are tracked across line breaks, because tmux happily leaves a
/// colour open at the end of a line and closes it on the next one.
pub fn to_lines(input: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut sgr = Sgr::default();
    let mut pending = false;

    for token in Tokens::new(input) {
        match token {
            Token::Sgr(params) => sgr.apply(params),
            Token::Text(text) => {
                if !text.is_empty() {
                    spans.push(Span::styled(text, sgr.style()));
                    pending = true;
                }
            }
            Token::Newline => {
                lines.push(Line::from(std::mem::take(&mut spans)));
                pending = false;
            }
        }
    }

    // Trailing text without a final newline still forms a line; a trailing
    // newline does not add an empty one (matching `str::lines`).
    if pending {
        lines.push(Line::from(spans));
    }

    lines
}

/// Returns true when the line carries no style of its own, meaning the source
/// did not colour it and a fallback highlight may be applied.
pub fn line_is_unstyled(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.style == Style::default())
}

/// Hard-wraps `line` at `width` display columns, the way a terminal does.
///
/// Terminals break mid-word at the screen edge, so word wrapping would not
/// reproduce pane content faithfully. Styles are carried into every row.
pub fn wrap_line<'a>(line: &Line<'a>, width: usize, out: &mut Vec<Line<'a>>) {
    if width == 0 {
        out.push(line.clone());
        return;
    }

    let mut current: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;

    for span in &line.spans {
        let content = &span.content;
        let mut segment_start = 0usize;

        for (offset, ch) in content.char_indices() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + char_width > width && used > 0 {
                if offset > segment_start {
                    current.push(Span::styled(
                        slice_cow(content, segment_start, offset),
                        span.style,
                    ));
                }
                out.push(Line::from(std::mem::take(&mut current)));
                segment_start = offset;
                used = 0;
            }
            used += char_width;
        }

        if segment_start < content.len() {
            current.push(Span::styled(
                slice_cow(content, segment_start, content.len()),
                span.style,
            ));
        }
    }

    out.push(Line::from(current));
}

/// Slices a span's content while keeping a borrow whenever possible.
fn slice_cow<'a>(content: &Cow<'a, str>, start: usize, end: usize) -> Cow<'a, str> {
    match content {
        Cow::Borrowed(text) => Cow::Borrowed(&text[start..end]),
        Cow::Owned(text) => Cow::Owned(text[start..end].to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_and_other_sequences() {
        let input = "\x1b[38;2;0;215;135mdone\x1b[39m \x1b]0;title\x07plain\x1b[2K";
        assert_eq!(strip(input), "done plain");
    }

    #[test]
    fn strip_and_to_lines_agree_on_line_count() {
        let input = "\x1b[31ma\nb\x1b[0m\n\nc\n";
        assert_eq!(strip(input), "a\nb\n\nc\n");
        assert_eq!(strip(input).lines().count(), to_lines(input).len());
        assert_eq!(to_lines(input).len(), 4);
    }

    #[test]
    fn parses_colors_and_modifiers() {
        let lines = to_lines("\x1b[1;31mbold red\x1b[0m normal\n\x1b[38;5;42mindexed");
        assert_eq!(lines.len(), 2);

        let first = &lines[0];
        assert_eq!(first.spans[0].content, "bold red");
        assert_eq!(first.spans[0].style.fg, Some(Color::Red));
        assert!(first.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(first.spans[1].content, " normal");
        assert_eq!(first.spans[1].style, Style::default());

        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Indexed(42)));
    }

    #[test]
    fn carries_style_across_lines() {
        // tmux leaves a colour open at the end of a line and closes it later
        let lines = to_lines("\x1b[32mgreen\nstill green\x1b[39m\n");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn background_and_rgb_colors() {
        let lines = to_lines("\x1b[48;2;10;20;30m \x1b[49mx");
        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(lines[0].spans[1].style.bg, None);
    }

    #[test]
    fn wraps_at_display_width() {
        let lines = to_lines("\x1b[31mabcdef\x1b[39m");
        let mut rows = Vec::new();
        wrap_line(&lines[0], 4, &mut rows);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].spans[0].content, "abcd");
        assert_eq!(rows[1].spans[0].content, "ef");
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn wraps_wide_characters_without_splitting_them() {
        let lines = to_lines("한글abc");
        let mut rows = Vec::new();
        wrap_line(&lines[0], 5, &mut rows);
        // 한글 is 4 columns, so only "a" fits on the first row
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].spans[0].content, "한글a");
        assert_eq!(rows[1].spans[0].content, "bc");
    }

    #[test]
    fn empty_line_is_one_row() {
        let mut rows = Vec::new();
        wrap_line(&Line::from(vec![]), 10, &mut rows);
        assert_eq!(rows.len(), 1);
    }
}

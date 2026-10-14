//! Terminal styling for Mortsom's console reports.
//!
//! Whether colour is welcome, and how deep, is decided per stream at the
//! edge ([`Style::detect`]) and handed in as a [`Style`], so everything else
//! here is pure string building: a redirected report and a test assertion
//! read plain text, a terminal reads colour. Alignment works on visible
//! width, so a coloured cell lines up with a plain one.

use mortsom_common::terminal::{self, ColorEnv, Stream};

/// Report width, wide enough for a failure message on one or two lines
/// and narrow enough to survive an 80-column terminal with little wrap.
pub const REPORT_WIDTH: usize = 96;
/// Left margin of every report line.
pub const INDENT: &str = "  ";

/// A 24-bit colour.
pub type Rgb = (u8, u8, u8);

/// Mostro green, the banner's brand colour.
pub const BRAND: Rgb = (0x9b, 0xce, 0x4b);

/// Paints text for one stream, or leaves it plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    color: bool,
    truecolor: bool,
}

impl Style {
    /// No escape sequences at all.
    pub const fn plain() -> Self {
        Self::new(false, false)
    }

    /// A style that emits escape sequences only when `color` is true, in
    /// 24-bit colour when `truecolor` is, and in the 256-colour cube when not.
    pub const fn new(color: bool, truecolor: bool) -> Self {
        Self { color, truecolor }
    }

    /// The style output headed for `stream` gets in this process.
    pub fn detect(stream: Stream) -> Self {
        let env = ColorEnv::from_process();
        Self::new(env.color(terminal::is_terminal(stream)), env.truecolor)
    }

    /// Whether this style emits escape sequences.
    pub fn is_colored(self) -> bool {
        self.color
    }

    /// `text` in a colour, exact or the nearest the terminal can show.
    pub fn rgb(self, color: Rgb, text: &str) -> String {
        self.paint(&self.rgb_code(color), text)
    }

    /// `text` dimmed, for what supports the line rather than carries it.
    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    /// `text` wrapped in the SGR parameters `code`, when colour is on.
    pub fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_owned()
        }
    }

    /// The SGR parameters for `color`: exact in 24-bit, and the nearest
    /// entry of the 6x6x6 cube otherwise, so an older terminal still shows
    /// the right hue rather than an escape sequence it cannot read.
    pub fn rgb_code(self, (r, g, b): Rgb) -> String {
        if self.truecolor {
            return format!("38;2;{r};{g};{b}");
        }
        let index = 16 + 36 * cube_level(r) + 6 * cube_level(g) + cube_level(b);
        format!("38;5;{index}")
    }
}

/// Which of the cube's six levels a channel is closest to.
fn cube_level(value: u8) -> u16 {
    (u16::from(value) * 5 + 127) / 255
}

/// A horizontal rule that ends at [`REPORT_WIDTH`] once [`INDENT`] is
/// prepended to it.
pub fn rule() -> String {
    "─".repeat(REPORT_WIDTH - INDENT.len())
}

/// Visible width of `text`, skipping ANSI escape sequences.
pub fn visible_len(text: &str) -> usize {
    let mut len = 0;
    let mut escaped = false;
    for ch in text.chars() {
        match (escaped, ch) {
            (true, 'm') => escaped = false,
            (true, _) => {}
            (false, '\u{1b}') => escaped = true,
            (false, _) => len += 1,
        }
    }
    len
}

/// `text` padded on the right to `width` visible columns.
pub fn pad(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(visible_len(text));
    format!("{text}{}", " ".repeat(fill))
}

/// `text` padded on the left to `width` visible columns.
pub fn align_right(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(visible_len(text));
    format!("{}{text}", " ".repeat(fill))
}

/// Greedy word wrap at `width` columns. Line breaks in `text` are kept, so
/// a driver error and its own detail line stay apart; a word longer than
/// `width` stays whole, because an identifier is worth more unbroken.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .flat_map(|line| wrap_line(line, width))
        .collect()
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let fits = current.chars().count() + 1 + word.chars().count() <= width;
        if !current.is_empty() && !fits {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    lines.push(current);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_style_leaves_text_untouched() {
        // Arrange
        let style = Style::plain();

        // Act
        let painted = style.rgb(BRAND, "PASS");

        // Assert
        assert_eq!(painted, "PASS");
        assert_eq!(style.dim("4m 12s"), "4m 12s");
    }

    #[test]
    fn coloured_style_wraps_text_in_a_24_bit_sequence() {
        // Arrange
        let style = Style::new(true, true);

        // Act
        let painted = style.rgb(BRAND, "PASS");

        // Assert
        assert_eq!(painted, "\u{1b}[38;2;155;206;75mPASS\u{1b}[0m");
    }

    #[test]
    fn a_colour_falls_back_to_the_nearest_cube_entry_without_truecolor() {
        // Arrange
        let style = Style::new(true, false);

        // Act / Assert
        assert_eq!(style.rgb_code(BRAND), "38;5;149");
        assert_eq!(style.rgb_code((0, 0, 0)), "38;5;16");
        assert_eq!(style.rgb_code((255, 255, 255)), "38;5;231");
        assert_eq!(style.rgb(BRAND, "PASS"), "\u{1b}[38;5;149mPASS\u{1b}[0m");
    }

    #[test]
    fn a_colour_is_exact_on_a_truecolor_terminal() {
        // Arrange / Act / Assert
        assert_eq!(Style::new(true, true).rgb_code(BRAND), "38;2;155;206;75");
    }

    #[test]
    fn visible_len_skips_escape_sequences() {
        // Arrange
        let painted = Style::new(true, false).rgb(BRAND, "[PASS]");

        // Act / Assert
        assert_eq!(visible_len(&painted), 6);
    }

    #[test]
    fn padding_and_right_alignment_count_visible_columns() {
        // Arrange
        let painted = Style::new(true, true).dim("8.0s");

        // Act
        let padded = pad(&painted, 10);
        let aligned = align_right(&painted, 10);

        // Assert
        assert_eq!(visible_len(&padded), 10);
        assert!(padded.ends_with("      "));
        assert_eq!(visible_len(&aligned), 10);
        assert!(aligned.starts_with("      "));
    }

    #[test]
    fn the_rule_ends_at_the_report_width() {
        // Arrange / Act
        let line = format!("{INDENT}{}", rule());

        // Assert
        assert_eq!(visible_len(&line), REPORT_WIDTH);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        // Arrange
        let text = "`order.add.fab` did not appear within 30s";

        // Act
        let lines = wrap(text, 16);

        // Assert
        assert!(lines.iter().all(|l| l.chars().count() <= 16), "{lines:?}");
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn wrap_keeps_line_breaks_and_drops_blank_lines() {
        // Arrange
        let text = "session deleted\n\n  (Session info: chrome=152)";

        // Act
        let lines = wrap(text, 80);

        // Assert
        assert_eq!(lines, ["session deleted", "(Session info: chrome=152)"]);
    }

    #[test]
    fn wrap_keeps_a_long_word_whole() {
        // Arrange
        let text = "run 20261014T193526Z-e7111efd3bc9";

        // Act
        let lines = wrap(text, 10);

        // Assert
        assert_eq!(lines, ["run", "20261014T193526Z-e7111efd3bc9"]);
    }
}

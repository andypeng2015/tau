//! Styled text types for terminal rendering.
//!
//! Content is represented as sequences of [`Span`]s, each pairing a
//! plain-text string with a [`Style`]. Display width is always
//! computable from the text alone — no ANSI escape codes are stored
//! in the data model.

pub use crossterm::style::Color;
use unicode_width::UnicodeWidthChar;

const EMOJI_VARIATION_SELECTOR: char = '\u{fe0f}';

pub(crate) fn push_cell_with_context(cells: &mut Vec<Cell>, ch: char, style: Style) {
    if ch == EMOJI_VARIATION_SELECTOR {
        if let Some(prev) = cells.last_mut() {
            prev.width = prev.width.max(2);
        }
    }
    cells.push(Cell::new(ch, style));
}

/// Visual attributes for a single character cell.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub underline: bool,
    pub italic: bool,
}

impl Style {
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
}

/// A terminal cell: one character, its visual style, and display width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// Character emitted for this cell.
    pub ch: char,
    /// Visual style applied while emitting this cell.
    pub style: Style,
    /// Display width in terminal columns.
    pub width: usize,
}

impl Cell {
    pub fn new(ch: char, style: Style) -> Self {
        Self {
            ch,
            style,
            width: ch.width().unwrap_or(0),
        }
    }

    pub fn plain(ch: char) -> Self {
        Self {
            ch,
            style: Style::default(),
            width: ch.width().unwrap_or(0),
        }
    }

    /// Returns a copy of this cell with an explicit display width.
    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    /// Display width in terminal columns (1 for ASCII, 2 for wide
    /// chars like emoji/CJK, 0 for zero-width combiners).
    pub fn col_width(&self) -> usize {
        self.width
    }
}

/// A run of text with a uniform style.
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }
}

/// A sequence of styled spans representing rich text.
///
/// Can be constructed from plain `&str` / `String` (unstyled),
/// a single [`Span`], or a `Vec<Span>`.
#[derive(Clone, Debug, Default)]
pub struct StyledText {
    spans: Vec<Span>,
}

impl StyledText {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, span: Span) {
        self.spans.push(span);
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Total display width in terminal columns.
    ///
    /// Wide characters (emoji, CJK) count as 2 columns.
    pub fn char_count(&self) -> usize {
        self.spans
            .iter()
            .flat_map(|s| s.text.chars())
            .map(|ch| ch.width().unwrap_or(0))
            .sum()
    }

    /// Returns `true` if there is no text content.
    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.is_empty())
    }

    /// Converts to a flat sequence of [`Cell`]s (newlines excluded).
    pub fn to_cells(&self) -> Vec<Cell> {
        let mut cells = Vec::new();
        for span in &self.spans {
            for ch in span.text.chars() {
                if ch != '\n' {
                    push_cell_with_context(&mut cells, ch, span.style);
                }
            }
        }
        cells
    }
}

impl From<&str> for StyledText {
    fn from(s: &str) -> Self {
        Self {
            spans: vec![Span::plain(s)],
        }
    }
}

impl From<String> for StyledText {
    fn from(s: String) -> Self {
        Self {
            spans: vec![Span::plain(s)],
        }
    }
}

impl From<Span> for StyledText {
    fn from(span: Span) -> Self {
        Self { spans: vec![span] }
    }
}

impl From<Vec<Span>> for StyledText {
    fn from(spans: Vec<Span>) -> Self {
        Self { spans }
    }
}

/// Opaque numeric identifier for a [`StyledBlock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u64);

/// Horizontal alignment within a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
}

/// A unit of layout: styled content with background, alignment, and margins.
///
/// When rendered, the block's content is word-wrapped to the available
/// width (after subtracting margins), aligned within that space, and
/// the block's background color fills any remaining cells.
#[derive(Clone, Debug)]
pub struct StyledBlock {
    pub content: StyledText,
    pub right_content: StyledText,
    pub bg: Option<Color>,
    pub align: Align,
    pub margin_left: u16,
    pub margin_right: u16,
}

impl StyledBlock {
    pub fn new(content: impl Into<StyledText>) -> Self {
        Self {
            content: content.into(),
            right_content: StyledText::new(),
            bg: None,
            align: Align::Left,
            margin_left: 0,
            margin_right: 0,
        }
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn right_content(mut self, content: impl Into<StyledText>) -> Self {
        self.right_content = content.into();
        self
    }

    pub fn margin_left(mut self, n: u16) -> Self {
        self.margin_left = n;
        self
    }

    pub fn margin_right(mut self, n: u16) -> Self {
        self.margin_right = n;
        self
    }

    pub fn margins(mut self, left: u16, right: u16) -> Self {
        self.margin_left = left;
        self.margin_right = right;
        self
    }
}

impl From<&str> for StyledBlock {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for StyledBlock {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<StyledText> for StyledBlock {
    fn from(text: StyledText) -> Self {
        Self::new(text)
    }
}

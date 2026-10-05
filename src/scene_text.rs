use std::{
    collections::BTreeSet,
    error::Error,
    fmt::{Display, Formatter},
};

use unicode_width::UnicodeWidthChar;

pub const TAB_WIDTH_CELLS: u32 = 8;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_TEXT_CELLS: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextWeight {
    #[default]
    Normal,
    Bold,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub font_alias: Option<String>,
    pub size: Option<f32>,
    pub line_height: Option<f32>,
    pub color: Option<[u8; 4]>,
    pub weight: TextWeight,
    pub align: TextAlign,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub columns: u32,
    pub rows: u32,
    pub width: f64,
    pub height: f64,
    pub cells: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextNode {
    text: String,
    style: TextStyle,
    metrics: TextMetrics,
}

impl TextNode {
    pub fn new(text: String, style: TextStyle, metrics: TextMetrics) -> Self {
        Self {
            text,
            style,
            metrics,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn style(&self) -> &TextStyle {
        &self.style
    }

    pub const fn metrics(&self) -> TextMetrics {
        self.metrics
    }
}

#[derive(Clone, Debug)]
pub struct TextLayout {
    default_size: f32,
    default_cell_width: f32,
    default_line_height: f32,
    font_aliases: BTreeSet<String>,
}

impl Default for TextLayout {
    fn default() -> Self {
        Self::new(16.0, 9.6, 20.0, []).expect("default text metrics are valid")
    }
}

impl TextLayout {
    pub fn new(
        default_size: f32,
        default_cell_width: f32,
        default_line_height: f32,
        font_aliases: impl IntoIterator<Item = String>,
    ) -> Result<Self, TextLayoutError> {
        validate_positive("default font size", default_size)?;
        validate_positive("default cell width", default_cell_width)?;
        validate_positive("default line height", default_line_height)?;
        Ok(Self {
            default_size,
            default_cell_width,
            default_line_height,
            font_aliases: font_aliases.into_iter().collect(),
        })
    }

    pub fn measure(&self, text: &str, style: &TextStyle) -> Result<TextMetrics, TextLayoutError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(TextLayoutError::TextBytesExceeded {
                limit: MAX_TEXT_BYTES,
            });
        }
        self.validate_style(style)?;
        if text.is_empty() {
            return Ok(TextMetrics::default());
        }

        let mut columns = 0_u32;
        let mut current_columns = 0_u32;
        let mut rows = 1_u32;
        let mut cells = 0_usize;
        for character in text.chars() {
            match character {
                '\n' => {
                    columns = columns.max(current_columns);
                    current_columns = 0;
                    rows = rows
                        .checked_add(1)
                        .ok_or(TextLayoutError::GeometryOverflow)?;
                }
                '\t' => {
                    let advance = TAB_WIDTH_CELLS - (current_columns % TAB_WIDTH_CELLS);
                    current_columns = current_columns
                        .checked_add(advance)
                        .ok_or(TextLayoutError::GeometryOverflow)?;
                    cells = cells
                        .checked_add(advance as usize)
                        .ok_or(TextLayoutError::GeometryOverflow)?;
                }
                _ if character.is_control() => {
                    return Err(TextLayoutError::UnsupportedControlCharacter);
                }
                _ => {
                    let width = UnicodeWidthChar::width(character).unwrap_or(0) as u32;
                    current_columns = current_columns
                        .checked_add(width)
                        .ok_or(TextLayoutError::GeometryOverflow)?;
                    cells = cells
                        .checked_add(width as usize)
                        .ok_or(TextLayoutError::GeometryOverflow)?;
                }
            }
            if cells > MAX_TEXT_CELLS {
                return Err(TextLayoutError::TextCellsExceeded {
                    limit: MAX_TEXT_CELLS,
                });
            }
        }
        columns = columns.max(current_columns);

        let scale = style
            .size
            .map(|size| size / self.default_size)
            .unwrap_or(1.0);
        let cell_width = self.default_cell_width * scale;
        let line_height = style
            .line_height
            .unwrap_or(self.default_line_height * scale);
        let width = f64::from(columns) * f64::from(cell_width);
        let height = f64::from(rows) * f64::from(line_height);
        if !width.is_finite() || !height.is_finite() {
            return Err(TextLayoutError::GeometryOverflow);
        }
        Ok(TextMetrics {
            columns,
            rows,
            width,
            height,
            cells,
        })
    }

    fn validate_style(&self, style: &TextStyle) -> Result<(), TextLayoutError> {
        if let Some(alias) = style.font_alias.as_deref() {
            if alias.trim().is_empty() {
                return Err(TextLayoutError::InvalidFontAlias);
            }
            if !self.font_aliases.contains(alias) {
                return Err(TextLayoutError::UnknownFontAlias(alias.to_owned()));
            }
        }
        if let Some(size) = style.size {
            validate_positive("text size", size)?;
        }
        if let Some(line_height) = style.line_height {
            validate_positive("text line height", line_height)?;
        }
        Ok(())
    }
}

pub fn expand_tabs(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut columns = 0_u32;
    for character in text.chars() {
        match character {
            '\n' => {
                expanded.push(character);
                columns = 0;
            }
            '\t' => {
                let advance = TAB_WIDTH_CELLS - (columns % TAB_WIDTH_CELLS);
                expanded.extend(std::iter::repeat_n(' ', advance as usize));
                columns += advance;
            }
            _ => {
                expanded.push(character);
                columns += UnicodeWidthChar::width(character).unwrap_or(0) as u32;
            }
        }
    }
    expanded
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextLayoutError {
    InvalidFontAlias,
    UnknownFontAlias(String),
    InvalidMetric(&'static str),
    TextBytesExceeded { limit: usize },
    TextCellsExceeded { limit: usize },
    UnsupportedControlCharacter,
    GeometryOverflow,
}

impl Display for TextLayoutError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFontAlias => formatter.write_str("a text font alias must not be empty"),
            Self::UnknownFontAlias(alias) => {
                write!(formatter, "the text font alias {alias:?} is not configured")
            }
            Self::InvalidMetric(name) => write!(formatter, "{name} must be finite and positive"),
            Self::TextBytesExceeded { limit } => {
                write!(formatter, "text may not exceed {limit} bytes")
            }
            Self::TextCellsExceeded { limit } => {
                write!(formatter, "text may not occupy more than {limit} cells")
            }
            Self::UnsupportedControlCharacter => formatter
                .write_str("text may contain only printable characters, tabs, and newlines"),
            Self::GeometryOverflow => formatter.write_str("text metrics exceed supported geometry"),
        }
    }
}

impl Error for TextLayoutError {}

fn validate_positive(name: &'static str, value: f32) -> Result<(), TextLayoutError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(TextLayoutError::InvalidMetric(name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TEXT_CELLS, TAB_WIDTH_CELLS, TextAlign, TextLayout, TextLayoutError, TextStyle,
        expand_tabs,
    };

    fn layout() -> TextLayout {
        TextLayout::new(10.0, 6.0, 14.0, ["ui".to_owned()]).unwrap()
    }

    #[test]
    fn measures_terminal_cells_and_explicit_rows() {
        let metrics = layout()
            .measure(" a界e\u{301}\n", &TextStyle::default())
            .unwrap();

        assert_eq!(metrics.columns, 5);
        assert_eq!(metrics.rows, 2);
        assert_eq!(metrics.width, 30.0);
        assert_eq!(metrics.height, 28.0);
        assert_eq!(metrics.cells, 5);
    }

    #[test]
    fn empty_text_has_no_bounds_but_a_newline_creates_rows() {
        assert_eq!(layout().measure("", &TextStyle::default()).unwrap().rows, 0);
        let metrics = layout().measure("\n", &TextStyle::default()).unwrap();

        assert_eq!(metrics.columns, 0);
        assert_eq!(metrics.rows, 2);
        assert_eq!(metrics.width, 0.0);
        assert_eq!(metrics.height, 28.0);
    }

    #[test]
    fn tabs_use_local_eight_cell_stops() {
        let metrics = layout().measure("a\tb", &TextStyle::default()).unwrap();

        assert_eq!(metrics.columns, TAB_WIDTH_CELLS + 1);
        assert_eq!(metrics.cells, (TAB_WIDTH_CELLS + 1) as usize);
        assert_eq!(expand_tabs("a\tb"), "a       b");
    }

    #[test]
    fn scales_default_cell_metrics_and_checks_font_aliases() {
        let style = TextStyle {
            font_alias: Some("ui".to_owned()),
            size: Some(20.0),
            line_height: None,
            color: None,
            weight: Default::default(),
            align: TextAlign::Center,
        };
        let metrics = layout().measure("ok", &style).unwrap();

        assert_eq!(metrics.width, 24.0);
        assert_eq!(metrics.height, 28.0);
        assert_eq!(
            layout().measure(
                "ok",
                &TextStyle {
                    font_alias: Some("missing".to_owned()),
                    ..style
                }
            ),
            Err(TextLayoutError::UnknownFontAlias("missing".to_owned()))
        );
    }

    #[test]
    fn rejects_unsupported_controls_and_cell_exhaustion() {
        assert_eq!(
            layout().measure("a\rb", &TextStyle::default()),
            Err(TextLayoutError::UnsupportedControlCharacter)
        );
        let text = "x".repeat(MAX_TEXT_CELLS + 1);
        assert!(matches!(
            layout().measure(&text, &TextStyle::default()),
            Err(TextLayoutError::TextCellsExceeded { .. })
        ));
    }
}

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;
use vte::{Params, Parser, Perform};

const DEFAULT_SCROLLBACK_CAPACITY: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

const DEFAULT_FOREGROUND: TextColor = TextColor {
    red: 229,
    green: 231,
    blue: 235,
};

impl Default for TextColor {
    fn default() -> Self {
        DEFAULT_FOREGROUND
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextStyle {
    foreground: TextColor,
    bold: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new(TextColor::default())
    }
}

impl TextStyle {
    const fn new(foreground: TextColor) -> Self {
        Self {
            foreground,
            bold: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextSpan {
    pub text: String,
    pub color: TextColor,
    pub bold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    text: String,
    continuation: bool,
    style: TextStyle,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            continuation: false,
            style: TextStyle::default(),
        }
    }
}

impl Cell {
    fn blank(style: TextStyle) -> Self {
        Self {
            text: " ".to_owned(),
            continuation: false,
            style,
        }
    }

    fn character(character: char, style: TextStyle) -> Self {
        Self {
            text: character.to_string(),
            continuation: false,
            style,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cursor {
    column: u16,
    row: u16,
}

#[derive(Clone, Debug)]
struct ScrollbackRow {
    cells: Vec<Cell>,
    soft_wrapped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SelectionPoint {
    row: usize,
    column: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSelection {
    anchor: SelectionPoint,
    focus: SelectionPoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionRange {
    pub row: u16,
    pub start_column: u16,
    pub end_column: u16,
}

#[derive(Debug)]
struct Grid {
    columns: u16,
    rows: u16,
    cells: Vec<Cell>,
    default_style: TextStyle,
    cursor: Cursor,
    saved_cursor: Cursor,
    scroll_top: u16,
    scroll_bottom: u16,
    wrap_pending: bool,
    auto_wrap: bool,
    cursor_visible: bool,
    pen: TextStyle,
    soft_wrapped: Vec<bool>,
    scrollback: VecDeque<ScrollbackRow>,
    scrollback_capacity: usize,
    scrollback_generation: u64,
    scrollback_discarded: u64,
}

impl Grid {
    fn new(
        columns: u16,
        rows: u16,
        scrollback_capacity: usize,
        default_foreground: TextColor,
    ) -> Self {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let default_style = TextStyle::new(default_foreground);

        Self {
            columns,
            rows,
            cells: vec![Cell::blank(default_style); usize::from(columns) * usize::from(rows)],
            default_style,
            cursor: Cursor::default(),
            saved_cursor: Cursor::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            wrap_pending: false,
            auto_wrap: true,
            cursor_visible: true,
            pen: default_style,
            soft_wrapped: vec![false; usize::from(rows)],
            scrollback: VecDeque::new(),
            scrollback_capacity,
            scrollback_generation: 0,
            scrollback_discarded: 0,
        }
    }

    fn index(&self, column: u16, row: u16) -> usize {
        usize::from(row) * usize::from(self.columns) + usize::from(column)
    }

    fn row(&self, row: u16) -> &[Cell] {
        let start = self.index(0, row);
        &self.cells[start..start + usize::from(self.columns)]
    }

    fn row_mut(&mut self, row: u16) -> &mut [Cell] {
        let start = self.index(0, row);
        let columns = usize::from(self.columns);
        &mut self.cells[start..start + columns]
    }

    fn set_cell(&mut self, column: u16, row: u16, cell: Cell) {
        let index = self.index(column, row);
        self.cells[index] = cell;
    }

    fn clear_cell(&mut self, column: u16, row: u16) {
        self.set_cell(column, row, Cell::blank(self.default_style));
    }

    fn clear_row(&mut self, row: u16) {
        let blank = Cell::blank(self.default_style);
        self.row_mut(row).fill(blank);
        self.soft_wrapped[usize::from(row)] = false;
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let mut cells =
            vec![Cell::blank(self.default_style); usize::from(columns) * usize::from(rows)];
        let copy_columns = self.columns.min(columns);
        let copy_rows = self.rows.min(rows);

        for row in 0..copy_rows {
            for column in 0..copy_columns {
                let old_index = self.index(column, row);
                let new_index = usize::from(row) * usize::from(columns) + usize::from(column);
                cells[new_index] = self.cells[old_index].clone();
            }
        }

        self.columns = columns;
        self.rows = rows;
        self.cells = cells;
        self.soft_wrapped.resize(usize::from(rows), false);
        self.cursor.column = self.cursor.column.min(columns - 1);
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.saved_cursor.column = self.saved_cursor.column.min(columns - 1);
        self.saved_cursor.row = self.saved_cursor.row.min(rows - 1);
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.wrap_pending = false;
    }

    fn reset(&mut self) {
        self.cells.fill(Cell::blank(self.default_style));
        self.cursor = Cursor::default();
        self.saved_cursor = Cursor::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.wrap_pending = false;
        self.auto_wrap = true;
        self.cursor_visible = true;
        self.pen = self.default_style;
        self.soft_wrapped.fill(false);
        self.scrollback.clear();
        self.scrollback_generation = 0;
        self.scrollback_discarded = 0;
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = self.cursor;
    }

    fn restore_cursor(&mut self) {
        self.cursor = self.saved_cursor;
        self.wrap_pending = false;
    }

    fn carriage_return(&mut self) {
        self.cursor.column = 0;
        self.wrap_pending = false;
    }

    fn backspace(&mut self) {
        self.wrap_pending = false;
        if self.cursor.column > 0 {
            self.cursor.column -= 1;
            if self.row(self.cursor.row)[usize::from(self.cursor.column)].continuation
                && self.cursor.column > 0
            {
                self.cursor.column -= 1;
            }
        }
    }

    fn tab(&mut self) {
        let next_stop = ((self.cursor.column / 8) + 1) * 8;
        self.cursor.column = next_stop.min(self.columns - 1);
        self.wrap_pending = false;
    }

    fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row < self.rows - 1 {
            self.cursor.row += 1;
        }
    }

    fn soft_wrap(&mut self) {
        self.soft_wrapped[usize::from(self.cursor.row)] = true;
        self.linefeed();
    }

    fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    fn scroll_up(&mut self, count: u16) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);

        for _ in 0..count {
            let removed = ScrollbackRow {
                cells: self.row(self.scroll_top).to_vec(),
                soft_wrapped: self.soft_wrapped[usize::from(self.scroll_top)],
            };
            if self.scroll_top == 0 && self.scroll_bottom == self.rows - 1 {
                self.push_scrollback(removed);
            }

            for row in self.scroll_top..self.scroll_bottom {
                let next = self.row(row + 1).to_vec();
                self.row_mut(row).clone_from_slice(&next);
                self.soft_wrapped[usize::from(row)] = self.soft_wrapped[usize::from(row + 1)];
            }
            self.clear_row(self.scroll_bottom);
        }
    }

    fn scroll_down(&mut self, count: u16) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);

        for _ in 0..count {
            for row in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                let previous = self.row(row - 1).to_vec();
                self.row_mut(row).clone_from_slice(&previous);
                self.soft_wrapped[usize::from(row)] = self.soft_wrapped[usize::from(row - 1)];
            }
            self.clear_row(self.scroll_top);
        }
    }

    fn push_scrollback(&mut self, row: ScrollbackRow) {
        if self.scrollback_capacity == 0 {
            return;
        }

        if self.scrollback.len() == self.scrollback_capacity {
            self.scrollback.pop_front();
            self.scrollback_discarded = self.scrollback_discarded.saturating_add(1);
        }
        self.scrollback.push_back(row);
        self.scrollback_generation = self.scrollback_generation.saturating_add(1);
    }

    fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.scrollback_generation = 0;
        self.scrollback_discarded = 0;
    }

    fn cursor_up(&mut self, count: u16) {
        self.cursor.row = self.cursor.row.saturating_sub(count);
        self.wrap_pending = false;
    }

    fn cursor_down(&mut self, count: u16) {
        self.cursor.row = self.cursor.row.saturating_add(count).min(self.rows - 1);
        self.wrap_pending = false;
    }

    fn cursor_forward(&mut self, count: u16) {
        self.cursor.column = self
            .cursor
            .column
            .saturating_add(count)
            .min(self.columns - 1);
        self.wrap_pending = false;
    }

    fn cursor_back(&mut self, count: u16) {
        self.cursor.column = self.cursor.column.saturating_sub(count);
        self.wrap_pending = false;
    }

    fn cursor_position(&mut self, row: u16, column: u16) {
        self.cursor.row = row.saturating_sub(1).min(self.rows - 1);
        self.cursor.column = column.saturating_sub(1).min(self.columns - 1);
        self.wrap_pending = false;
    }

    fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let top = top.saturating_sub(1).min(self.rows - 1);
        let bottom = bottom.saturating_sub(1).min(self.rows - 1);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.cursor = Cursor::default();
            self.wrap_pending = false;
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                self.erase_in_line(0);
                for row in self.cursor.row + 1..self.rows {
                    self.clear_row(row);
                }
            }
            1 => {
                for row in 0..self.cursor.row {
                    self.clear_row(row);
                }
                self.erase_in_line(1);
            }
            2 => {
                self.cells.fill(Cell::blank(self.default_style));
                self.soft_wrapped.fill(false);
            }
            3 => self.clear_scrollback(),
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let (start, end) = match mode {
            0 => (self.cursor.column, self.columns - 1),
            1 => (0, self.cursor.column),
            2 => (0, self.columns - 1),
            _ => return,
        };

        for column in start..=end {
            self.clear_cell(column, self.cursor.row);
        }
    }

    fn erase_characters(&mut self, count: u16) {
        let end = self
            .cursor
            .column
            .saturating_add(count.saturating_sub(1))
            .min(self.columns - 1);
        for column in self.cursor.column..=end {
            self.clear_cell(column, self.cursor.row);
        }
    }

    fn delete_characters(&mut self, count: u16) {
        let count = count.min(self.columns - self.cursor.column);
        let row = self.cursor.row;
        let source_start = self.cursor.column + count;
        let values = if source_start < self.columns {
            self.row(row)[usize::from(source_start)..].to_vec()
        } else {
            Vec::new()
        };

        for (offset, cell) in values.into_iter().enumerate() {
            self.set_cell(self.cursor.column + offset as u16, row, cell);
        }
        for column in self.columns - count..self.columns {
            self.clear_cell(column, row);
        }
    }

    fn insert_blank_characters(&mut self, count: u16) {
        let count = count.min(self.columns - self.cursor.column);
        let row = self.cursor.row;
        let source_end = self.columns - count;
        let values =
            self.row(row)[usize::from(self.cursor.column)..usize::from(source_end)].to_vec();

        for column in self.cursor.column..self.cursor.column + count {
            self.clear_cell(column, row);
        }
        for (offset, cell) in values.into_iter().enumerate() {
            self.set_cell(self.cursor.column + count + offset as u16, row, cell);
        }
    }

    fn insert_lines(&mut self, count: u16) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }

        let count = count.min(self.scroll_bottom - self.cursor.row + 1);
        for _ in 0..count {
            for row in (self.cursor.row + 1..=self.scroll_bottom).rev() {
                let previous = self.row(row - 1).to_vec();
                self.row_mut(row).clone_from_slice(&previous);
                self.soft_wrapped[usize::from(row)] = self.soft_wrapped[usize::from(row - 1)];
            }
            self.clear_row(self.cursor.row);
        }
    }

    fn delete_lines(&mut self, count: u16) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }

        let count = count.min(self.scroll_bottom - self.cursor.row + 1);
        for _ in 0..count {
            for row in self.cursor.row..self.scroll_bottom {
                let next = self.row(row + 1).to_vec();
                self.row_mut(row).clone_from_slice(&next);
                self.soft_wrapped[usize::from(row)] = self.soft_wrapped[usize::from(row + 1)];
            }
            self.clear_row(self.scroll_bottom);
        }
    }

    fn print(&mut self, character: char) {
        let width = UnicodeWidthChar::width(character).unwrap_or(1).min(2) as u16;
        if width == 0 {
            self.append_combining_character(character);
            return;
        }

        if self.wrap_pending && self.auto_wrap {
            self.cursor.column = 0;
            self.soft_wrap();
        }
        self.wrap_pending = false;

        if width == 2 && self.cursor.column == self.columns - 1 {
            if self.auto_wrap {
                self.cursor.column = 0;
                self.soft_wrap();
            } else {
                self.clear_cell(self.cursor.column, self.cursor.row);
                return;
            }
        }

        self.set_cell(
            self.cursor.column,
            self.cursor.row,
            Cell::character(character, self.pen),
        );
        if width == 2 {
            self.set_cell(
                self.cursor.column + 1,
                self.cursor.row,
                Cell {
                    text: String::new(),
                    continuation: true,
                    style: self.pen,
                },
            );
        }

        let next_column = self.cursor.column.saturating_add(width);
        if next_column >= self.columns {
            self.cursor.column = self.columns - 1;
            self.wrap_pending = true;
        } else {
            self.cursor.column = next_column;
        }
    }

    fn append_combining_character(&mut self, character: char) {
        let mut column = self.cursor.column.saturating_sub(1);
        if self.row(self.cursor.row)[usize::from(column)].continuation && column > 0 {
            column -= 1;
        }
        let index = self.index(column, self.cursor.row);
        if !self.cells[index].continuation {
            self.cells[index].text.push(character);
        }
    }

    fn display_start(&self, viewport_offset: usize) -> usize {
        self.scrollback.len().saturating_sub(viewport_offset)
    }

    fn row_for_global(&self, row: usize) -> Option<&[Cell]> {
        if row < self.scrollback.len() {
            self.scrollback.get(row).map(|row| row.cells.as_slice())
        } else {
            let row = u16::try_from(row - self.scrollback.len()).ok()?;
            (row < self.rows).then(|| self.row(row))
        }
    }

    fn columns_for_cells(&self, cells: &[Cell]) -> u16 {
        u16::try_from(cells.len())
            .unwrap_or(u16::MAX)
            .min(self.columns)
    }

    fn soft_wrapped_for_global(&self, row: usize) -> Option<bool> {
        if row < self.scrollback.len() {
            self.scrollback.get(row).map(|row| row.soft_wrapped)
        } else {
            let row = u16::try_from(row - self.scrollback.len()).ok()?;
            self.soft_wrapped.get(usize::from(row)).copied()
        }
    }

    fn selection_start_column(&self, row: usize, column: u16) -> u16 {
        let Some(cells) = self.row_for_global(row) else {
            return column;
        };
        let columns = self.columns_for_cells(cells);
        let mut column = column.min(columns);
        while column > 0 && column < columns && cells[usize::from(column)].continuation {
            column -= 1;
        }
        column
    }

    fn selection_end_column(&self, row: usize, column: u16) -> u16 {
        let Some(cells) = self.row_for_global(row) else {
            return column;
        };
        let columns = self.columns_for_cells(cells);
        let column = column.min(columns);
        if column < columns && cells[usize::from(column)].continuation {
            (column + 1).min(columns)
        } else {
            column
        }
    }

    fn text_spans(
        &self,
        include_cursor: bool,
        trim_trailing_blanks: bool,
        viewport_offset: usize,
    ) -> Vec<TextSpan> {
        let mut spans = Vec::new();
        let start = self.display_start(viewport_offset);

        for row in 0..self.rows {
            let global_row = start + usize::from(row);
            let Some(cells) = self.row_for_global(global_row) else {
                continue;
            };
            let grid_row = global_row
                .checked_sub(self.scrollback.len())
                .and_then(|row| u16::try_from(row).ok());
            let cursor_visible = include_cursor && viewport_offset == 0 && grid_row.is_some();
            let columns = self.rendered_columns_for_cells(
                cells,
                grid_row,
                cursor_visible,
                trim_trailing_blanks,
            );
            for column in 0..columns {
                let cell = &cells[usize::from(column)];
                if cursor_visible
                    && self.cursor_visible
                    && self.cursor
                        == (Cursor {
                            column,
                            row: grid_row.expect("cursor visibility requires a grid row"),
                        })
                {
                    Self::append_span(&mut spans, "▏", cell.style);
                } else if !cell.continuation {
                    Self::append_span(&mut spans, &cell.text, cell.style);
                }
            }
            if row + 1 < self.rows {
                let style = cells
                    .get(usize::from(columns.saturating_sub(1)))
                    .map(|cell| cell.style)
                    .unwrap_or(self.default_style);
                Self::append_span(&mut spans, "\n", style);
            }
        }

        spans
    }

    fn rendered_columns_for_cells(
        &self,
        cells: &[Cell],
        grid_row: Option<u16>,
        include_cursor: bool,
        trim_trailing_blanks: bool,
    ) -> u16 {
        let columns = self.columns_for_cells(cells);
        if !trim_trailing_blanks {
            return columns;
        }

        (0..columns)
            .rev()
            .find(|&column| {
                let cell = &cells[usize::from(column)];
                (include_cursor
                    && self.cursor_visible
                    && grid_row
                        .map(|row| self.cursor == (Cursor { column, row }))
                        .unwrap_or(false))
                    || (!cell.continuation && cell.text != " ")
            })
            .map_or(0, |column| column + 1)
    }

    fn append_span(spans: &mut Vec<TextSpan>, text: &str, style: TextStyle) {
        if let Some(last) = spans.last_mut()
            && last.color == style.foreground
            && last.bold == style.bold
        {
            last.text.push_str(text);
        } else {
            spans.push(TextSpan {
                text: text.to_owned(),
                color: style.foreground,
                bold: style.bold,
            });
        }
    }

    #[cfg(test)]
    fn text(&self, include_cursor: bool) -> String {
        self.text_spans(include_cursor, false, 0)
            .into_iter()
            .map(|span| span.text)
            .collect()
    }

    fn apply_sgr(&mut self, parameters: &[u16]) {
        if parameters.is_empty() {
            self.pen = self.default_style;
            return;
        }

        let mut index = 0;
        while let Some(&parameter) = parameters.get(index) {
            match parameter {
                0 => self.pen = self.default_style,
                1 => self.pen.bold = true,
                22 => self.pen.bold = false,
                30..=37 => self.pen.foreground = Self::ansi_color(parameter - 30),
                39 => self.pen.foreground = self.default_style.foreground,
                90..=97 => self.pen.foreground = Self::ansi_color(parameter - 90 + 8),
                38 => {
                    if let Some(&5) = parameters.get(index + 1) {
                        if let Some(&color) = parameters.get(index + 2) {
                            self.pen.foreground = Self::indexed_color(color);
                            index += 2;
                        }
                    } else if let Some(&2) = parameters.get(index + 1)
                        && let (Some(&red), Some(&green), Some(&blue)) = (
                            parameters.get(index + 2),
                            parameters.get(index + 3),
                            parameters.get(index + 4),
                        )
                    {
                        self.pen.foreground = TextColor {
                            red: red.min(u16::from(u8::MAX)) as u8,
                            green: green.min(u16::from(u8::MAX)) as u8,
                            blue: blue.min(u16::from(u8::MAX)) as u8,
                        };
                        index += 4;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn ansi_color(index: u16) -> TextColor {
        const COLORS: [TextColor; 16] = [
            TextColor {
                red: 0,
                green: 0,
                blue: 0,
            },
            TextColor {
                red: 205,
                green: 49,
                blue: 49,
            },
            TextColor {
                red: 13,
                green: 188,
                blue: 121,
            },
            TextColor {
                red: 229,
                green: 229,
                blue: 16,
            },
            TextColor {
                red: 36,
                green: 114,
                blue: 200,
            },
            TextColor {
                red: 188,
                green: 63,
                blue: 188,
            },
            TextColor {
                red: 17,
                green: 168,
                blue: 205,
            },
            TextColor {
                red: 229,
                green: 229,
                blue: 229,
            },
            TextColor {
                red: 102,
                green: 102,
                blue: 102,
            },
            TextColor {
                red: 241,
                green: 76,
                blue: 76,
            },
            TextColor {
                red: 35,
                green: 209,
                blue: 139,
            },
            TextColor {
                red: 245,
                green: 245,
                blue: 67,
            },
            TextColor {
                red: 59,
                green: 142,
                blue: 234,
            },
            TextColor {
                red: 214,
                green: 112,
                blue: 214,
            },
            TextColor {
                red: 41,
                green: 184,
                blue: 219,
            },
            TextColor {
                red: 255,
                green: 255,
                blue: 255,
            },
        ];

        COLORS[usize::from(index.min(15))]
    }

    fn indexed_color(index: u16) -> TextColor {
        if index < 16 {
            return Self::ansi_color(index);
        }
        if index < 232 {
            let index = index - 16;
            let red = index / 36;
            let green = (index % 36) / 6;
            let blue = index % 6;
            let component = |value: u16| match value {
                0 => 0,
                1 => 95,
                2 => 135,
                3 => 175,
                4 => 215,
                _ => 255,
            };
            return TextColor {
                red: component(red),
                green: component(green),
                blue: component(blue),
            };
        }

        let shade = 8 + (index.min(255) - 232) as u8 * 10;
        TextColor {
            red: shade,
            green: shade,
            blue: shade,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveScreen {
    Primary,
    Alternate,
}

#[derive(Debug)]
struct TerminalState {
    primary: Grid,
    alternate: Grid,
    active: ActiveScreen,
    primary_cursor_before_alternate: Cursor,
    title: String,
    responses: Vec<Vec<u8>>,
    bracketed_paste: bool,
}

impl TerminalState {
    fn new(columns: u16, rows: u16, foreground: TextColor) -> Self {
        Self {
            primary: Grid::new(columns, rows, DEFAULT_SCROLLBACK_CAPACITY, foreground),
            alternate: Grid::new(columns, rows, 0, foreground),
            active: ActiveScreen::Primary,
            primary_cursor_before_alternate: Cursor::default(),
            title: String::new(),
            responses: Vec::new(),
            bracketed_paste: false,
        }
    }

    fn active_grid(&self) -> &Grid {
        match self.active {
            ActiveScreen::Primary => &self.primary,
            ActiveScreen::Alternate => &self.alternate,
        }
    }

    fn active_grid_mut(&mut self) -> &mut Grid {
        match self.active {
            ActiveScreen::Primary => &mut self.primary,
            ActiveScreen::Alternate => &mut self.alternate,
        }
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        self.primary.resize(columns, rows);
        self.alternate.resize(columns, rows);
    }

    fn reset(&mut self) {
        self.primary.reset();
        self.alternate.reset();
        self.active = ActiveScreen::Primary;
        self.primary_cursor_before_alternate = Cursor::default();
        self.title.clear();
        self.responses.clear();
        self.bracketed_paste = false;
    }

    fn enter_alternate_screen(&mut self) {
        if self.active == ActiveScreen::Primary {
            self.primary_cursor_before_alternate = self.primary.cursor;
            self.alternate.reset();
            self.active = ActiveScreen::Alternate;
        }
    }

    fn leave_alternate_screen(&mut self) {
        if self.active == ActiveScreen::Alternate {
            self.active = ActiveScreen::Primary;
            self.primary.cursor = self.primary_cursor_before_alternate;
            self.primary.wrap_pending = false;
        }
    }

    fn parameter(parameters: &[u16], index: usize, default: u16) -> u16 {
        parameters.get(index).copied().unwrap_or(default)
    }

    fn count(parameters: &[u16], index: usize) -> u16 {
        Self::parameter(parameters, index, 1).max(1)
    }
}

impl Perform for TerminalState {
    fn print(&mut self, character: char) {
        self.active_grid_mut().print(character);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\x08' => self.active_grid_mut().backspace(),
            b'\x09' => self.active_grid_mut().tab(),
            b'\x0a' | b'\x0b' | b'\x0c' => self.active_grid_mut().linefeed(),
            b'\x0d' => self.active_grid_mut().carriage_return(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, parameters: &[&[u8]], _: bool) {
        let Some(command) = parameters.first() else {
            return;
        };
        if *command != b"0" && *command != b"2" {
            return;
        }
        if let Some(title) = parameters.get(1) {
            self.title = String::from_utf8_lossy(title).into_owned();
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }

        let parameters: Vec<u16> = params
            .iter()
            .map(|parameter| parameter.first().copied().unwrap_or(0))
            .collect();
        let private = intermediates == b"?";

        if private {
            match action {
                'h' if parameters.contains(&1049) || parameters.contains(&47) => {
                    self.enter_alternate_screen();
                }
                'l' if parameters.contains(&1049) || parameters.contains(&47) => {
                    self.leave_alternate_screen();
                }
                'h' if parameters.contains(&25) => self.active_grid_mut().cursor_visible = true,
                'l' if parameters.contains(&25) => self.active_grid_mut().cursor_visible = false,
                'h' if parameters.contains(&7) => self.active_grid_mut().auto_wrap = true,
                'l' if parameters.contains(&7) => self.active_grid_mut().auto_wrap = false,
                'h' if parameters.contains(&2004) => self.bracketed_paste = true,
                'l' if parameters.contains(&2004) => self.bracketed_paste = false,
                _ => {}
            }
            return;
        }

        if action == 'n' && parameters.as_slice() == [6] {
            let cursor = self.active_grid().cursor;
            self.responses
                .push(format!("\x1b[{};{}R", cursor.row + 1, cursor.column + 1).into_bytes());
            return;
        }

        let grid = self.active_grid_mut();
        match action {
            'A' => grid.cursor_up(Self::count(&parameters, 0)),
            'B' | 'e' => grid.cursor_down(Self::count(&parameters, 0)),
            'C' | 'a' => grid.cursor_forward(Self::count(&parameters, 0)),
            'D' => grid.cursor_back(Self::count(&parameters, 0)),
            'E' => {
                grid.cursor_down(Self::count(&parameters, 0));
                grid.carriage_return();
            }
            'F' => {
                grid.cursor_up(Self::count(&parameters, 0));
                grid.carriage_return();
            }
            'G' | '`' => grid.cursor_position(grid.cursor.row + 1, Self::count(&parameters, 0)),
            'H' | 'f' => {
                grid.cursor_position(Self::count(&parameters, 0), Self::count(&parameters, 1))
            }
            'd' => grid.cursor_position(Self::count(&parameters, 0), grid.cursor.column + 1),
            'J' => grid.erase_in_display(Self::parameter(&parameters, 0, 0)),
            'K' => grid.erase_in_line(Self::parameter(&parameters, 0, 0)),
            'L' => grid.insert_lines(Self::count(&parameters, 0)),
            'M' => grid.delete_lines(Self::count(&parameters, 0)),
            'P' => grid.delete_characters(Self::count(&parameters, 0)),
            'S' => grid.scroll_up(Self::count(&parameters, 0)),
            'T' => grid.scroll_down(Self::count(&parameters, 0)),
            'X' => grid.erase_characters(Self::count(&parameters, 0)),
            '@' => grid.insert_blank_characters(Self::count(&parameters, 0)),
            'm' => grid.apply_sgr(&parameters),
            'r' => grid.set_scroll_region(
                Self::parameter(&parameters, 0, 1),
                Self::parameter(&parameters, 1, grid.rows),
            ),
            's' => grid.save_cursor(),
            'u' => grid.restore_cursor(),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }

        match byte {
            b'7' => self.active_grid_mut().save_cursor(),
            b'8' => self.active_grid_mut().restore_cursor(),
            b'D' => self.active_grid_mut().linefeed(),
            b'E' => {
                let grid = self.active_grid_mut();
                grid.carriage_return();
                grid.linefeed();
            }
            b'M' => self.active_grid_mut().reverse_index(),
            b'c' => self.reset(),
            _ => {}
        }
    }
}

pub struct Terminal {
    parser: Parser,
    state: TerminalState,
    dirty: bool,
    viewport_offset: usize,
    selection: Option<TerminalSelection>,
    observed_scrollback_generation: u64,
    observed_scrollback_discarded: u64,
}

impl Terminal {
    pub(crate) fn new(columns: u16, rows: u16, foreground: TextColor) -> Self {
        Self {
            parser: Parser::new(),
            state: TerminalState::new(columns, rows, foreground),
            dirty: true,
            viewport_offset: 0,
            selection: None,
            observed_scrollback_generation: 0,
            observed_scrollback_discarded: 0,
        }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
        self.reconcile_scrollback();
        self.dirty = true;
    }

    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.state.resize(columns, rows);
        self.viewport_offset = self
            .viewport_offset
            .min(self.state.primary.scrollback.len());
        self.selection = None;
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn take_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.state.responses)
    }

    pub(crate) fn render_spans(&self) -> Vec<TextSpan> {
        let viewport_offset = matches!(self.state.active, ActiveScreen::Primary)
            .then_some(self.viewport_offset)
            .unwrap_or(0);
        self.state
            .active_grid()
            .text_spans(true, true, viewport_offset)
    }

    pub(crate) fn scroll_view_by(&mut self, lines: i32) -> bool {
        if !matches!(self.state.active, ActiveScreen::Primary) || lines == 0 {
            return false;
        }

        let maximum = self.state.primary.scrollback.len();
        let previous = self.viewport_offset;
        if lines.is_positive() {
            self.viewport_offset = self
                .viewport_offset
                .saturating_add(lines.unsigned_abs() as usize)
                .min(maximum);
        } else {
            self.viewport_offset = self
                .viewport_offset
                .saturating_sub(lines.unsigned_abs() as usize);
        }
        if self.viewport_offset == previous {
            return false;
        }
        self.dirty = true;
        true
    }

    pub(crate) fn scroll_to_bottom(&mut self) -> bool {
        if self.viewport_offset == 0 {
            return false;
        }
        self.viewport_offset = 0;
        self.dirty = true;
        true
    }

    pub(crate) fn begin_selection(&mut self, row: u16, column: u16) -> bool {
        let Some(point) = self.selection_point(row, column) else {
            return false;
        };
        self.selection = Some(TerminalSelection {
            anchor: point,
            focus: point,
        });
        true
    }

    pub(crate) fn extend_selection(&mut self, row: u16, column: u16) -> bool {
        let Some(point) = self.selection_point(row, column) else {
            return false;
        };
        let Some(selection) = self.selection.as_mut() else {
            return false;
        };
        if selection.focus == point {
            return false;
        }
        selection.focus = point;
        true
    }

    pub(crate) fn selection_ranges(&self) -> Vec<SelectionRange> {
        let Some((start, end)) = self.selection_bounds() else {
            return Vec::new();
        };
        let grid = &self.state.primary;
        let display_start = grid.display_start(self.viewport_offset);
        let display_end = display_start + usize::from(grid.rows);
        let first = start.row.max(display_start);
        let last = end.row.min(display_end.saturating_sub(1));
        if first > last {
            return Vec::new();
        }

        (first..=last)
            .filter_map(|global_row| {
                let cells = grid.row_for_global(global_row)?;
                let columns = grid.columns_for_cells(cells);
                let start_column = if global_row == start.row {
                    start.column.min(columns)
                } else {
                    0
                };
                let end_column = if global_row == end.row {
                    end.column.min(columns)
                } else {
                    columns
                };
                (start_column < end_column).then_some(SelectionRange {
                    row: u16::try_from(global_row - display_start).ok()?,
                    start_column,
                    end_column,
                })
            })
            .collect()
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_bounds()?;
        let grid = &self.state.primary;
        let mut text = String::new();
        for row in start.row..=end.row {
            let cells = grid.row_for_global(row)?;
            let columns = grid.columns_for_cells(cells);
            let start_column = if row == start.row {
                start.column.min(columns)
            } else {
                0
            };
            let end_column = if row == end.row {
                end.column.min(columns)
            } else {
                columns
            };
            let mut line = String::new();
            for cell in &cells[usize::from(start_column)..usize::from(end_column)] {
                if !cell.continuation {
                    line.push_str(&cell.text);
                }
            }
            text.push_str(line.trim_end_matches(' '));
            if row < end.row && !grid.soft_wrapped_for_global(row).unwrap_or(false) {
                text.push('\n');
            }
        }
        (!text.is_empty()).then_some(text)
    }

    pub(crate) fn bracketed_paste_enabled(&self) -> bool {
        self.state.bracketed_paste
    }

    fn selection_point(&self, row: u16, column: u16) -> Option<SelectionPoint> {
        if !matches!(self.state.active, ActiveScreen::Primary) || row >= self.state.primary.rows {
            return None;
        }
        Some(SelectionPoint {
            row: self.state.primary.display_start(self.viewport_offset) + usize::from(row),
            column: column.min(self.state.primary.columns),
        })
    }

    fn selection_bounds(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        if !matches!(self.state.active, ActiveScreen::Primary) {
            return None;
        }
        let selection = self.selection?;
        let (mut start, mut end) = if selection.anchor <= selection.focus {
            (selection.anchor, selection.focus)
        } else {
            (selection.focus, selection.anchor)
        };
        let grid = &self.state.primary;
        start.column = grid.selection_start_column(start.row, start.column);
        end.column = grid.selection_end_column(end.row, end.column);
        (start < end).then_some((start, end))
    }

    fn reconcile_scrollback(&mut self) {
        let grid = &self.state.primary;
        if grid.scrollback_generation < self.observed_scrollback_generation
            || grid.scrollback_discarded < self.observed_scrollback_discarded
        {
            self.viewport_offset = 0;
            self.selection = None;
        }
        let pushed = grid
            .scrollback_generation
            .saturating_sub(self.observed_scrollback_generation);
        let discarded = grid
            .scrollback_discarded
            .saturating_sub(self.observed_scrollback_discarded);
        if self.viewport_offset > 0 {
            self.viewport_offset = self
                .viewport_offset
                .saturating_add(usize::try_from(pushed).unwrap_or(usize::MAX))
                .min(grid.scrollback.len());
        }
        if discarded > 0 {
            let discarded = usize::try_from(discarded).unwrap_or(usize::MAX);
            let clear_selection = self.selection.as_ref().is_some_and(|selection| {
                selection.anchor.row < discarded || selection.focus.row < discarded
            });
            if clear_selection {
                self.selection = None;
            } else if let Some(selection) = self.selection.as_mut()
                && selection.anchor.row >= discarded
                && selection.focus.row >= discarded
            {
                selection.anchor.row -= discarded;
                selection.focus.row -= discarded;
            }
        }
        self.observed_scrollback_generation = grid.scrollback_generation;
        self.observed_scrollback_discarded = grid.scrollback_discarded;
        self.viewport_offset = self.viewport_offset.min(grid.scrollback.len());
        if !matches!(self.state.active, ActiveScreen::Primary) {
            self.viewport_offset = 0;
            self.selection = None;
        }
    }

    #[cfg(test)]
    pub fn visible_text(&self) -> String {
        self.state.active_grid().text(false)
    }

    #[cfg(test)]
    pub fn visible_spans(&self) -> Vec<TextSpan> {
        self.state.active_grid().text_spans(false, false, 0)
    }

    pub fn title(&self) -> &str {
        &self.state.title
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;

    fn terminal(columns: u16, rows: u16) -> Terminal {
        Terminal::new(columns, rows, super::TextColor::default())
    }

    fn row(terminal: &Terminal, index: usize) -> String {
        terminal
            .visible_text()
            .lines()
            .nth(index)
            .unwrap()
            .to_owned()
    }

    #[test]
    fn prints_text_and_honors_carriage_return() {
        let mut terminal = terminal(5, 2);
        terminal.advance(b"abc\rXY");

        assert_eq!(row(&terminal, 0), "XYc  ");
    }

    #[test]
    fn interprets_cursor_position_and_erase_line() {
        let mut terminal = terminal(5, 2);
        terminal.advance(b"abcde\x1b[1;3H!\x1b[0K");

        assert_eq!(row(&terminal, 0), "ab!  ");
    }

    #[test]
    fn responds_to_cursor_position_requests() {
        let mut terminal = terminal(5, 3);
        terminal.advance(b"\x1b[2;3H\x1b[6n");

        assert_eq!(terminal.take_responses(), vec![b"\x1b[2;3R".to_vec()]);
    }

    #[test]
    fn scrolls_when_a_line_reaches_the_bottom() {
        let mut terminal = terminal(4, 2);
        terminal.advance(b"one\r\ntwo\r\nend");

        assert_eq!(row(&terminal, 0), "two ");
        assert_eq!(row(&terminal, 1), "end ");
    }

    #[test]
    fn keeps_primary_screen_while_using_alternate_screen() {
        let mut terminal = terminal(5, 2);
        terminal.advance(b"main\x1b[?1049halt\x1b[?1049l");

        assert_eq!(row(&terminal, 0), "main ");
    }

    #[test]
    fn handles_wide_unicode_without_overwriting_the_following_cell() {
        let mut terminal = terminal(5, 1);
        terminal.advance("A界B".as_bytes());

        assert_eq!(row(&terminal, 0), "A界B ");
    }

    #[test]
    fn resize_preserves_the_visible_top_left_cells() {
        let mut terminal = terminal(5, 2);
        terminal.advance(b"hello\r\nworld");
        terminal.resize(3, 3);

        assert_eq!(row(&terminal, 0), "hel");
        assert_eq!(row(&terminal, 1), "wor");
        assert_eq!(row(&terminal, 2), "   ");
    }

    #[test]
    fn applies_ansi_foreground_colors_and_resets_them() {
        let mut terminal = terminal(8, 1);
        terminal.advance(b"\x1b[31mred\x1b[0mplain");
        let spans = terminal.visible_spans();

        assert_eq!(spans[0].text, "red");
        assert_eq!(
            spans[0].color,
            super::TextColor {
                red: 205,
                green: 49,
                blue: 49,
            }
        );
        assert_eq!(spans[1].text, "plain");
    }

    #[test]
    fn uses_the_configured_foreground_for_plain_text_cursor_and_sgr_resets() {
        let foreground = super::TextColor {
            red: 221,
            green: 238,
            blue: 255,
        };
        let mut terminal = Terminal::new(16, 1, foreground);

        assert_eq!(terminal.render_spans()[0].color, foreground);

        terminal.advance(b"\x1b[31mred\x1b[39mplain\x1b[0magain");
        let spans = terminal.visible_spans();

        assert_eq!(spans[0].text, "red");
        assert!(spans[1].text.starts_with("plainagain"));
        assert_eq!(spans[1].color, foreground);
    }

    #[test]
    fn omits_invisible_trailing_cells_from_render_spans() {
        let mut terminal = terminal(8, 2);
        terminal.advance(b"text\x1b[2;4H");
        let rendered: String = terminal
            .render_spans()
            .into_iter()
            .map(|span| span.text)
            .collect();

        assert_eq!(rendered, "text\n   ▏");
    }

    #[test]
    fn selects_across_scrollback_rows() {
        let mut terminal = terminal(4, 2);
        terminal.advance(b"one\r\ntwo\r\nend");

        assert!(terminal.scroll_view_by(1));
        assert!(terminal.begin_selection(0, 0));
        assert!(terminal.extend_selection(1, 3));

        assert_eq!(terminal.selected_text(), Some("one\ntwo".to_owned()));
        assert_eq!(
            terminal.selection_ranges(),
            vec![
                super::SelectionRange {
                    row: 0,
                    start_column: 0,
                    end_column: 4,
                },
                super::SelectionRange {
                    row: 1,
                    start_column: 0,
                    end_column: 3,
                },
            ]
        );
    }

    #[test]
    fn selection_keeps_a_wide_character_intact() {
        let mut terminal = terminal(5, 1);
        terminal.advance("A界B".as_bytes());

        assert!(terminal.begin_selection(0, 1));
        assert!(terminal.extend_selection(0, 2));

        assert_eq!(terminal.selected_text(), Some("界".to_owned()));
        assert_eq!(
            terminal.selection_ranges(),
            vec![super::SelectionRange {
                row: 0,
                start_column: 1,
                end_column: 3,
            }]
        );
    }

    #[test]
    fn selection_keeps_combining_marks_with_their_base_character() {
        let mut terminal = terminal(5, 1);
        terminal.advance("e\u{301}x".as_bytes());

        assert!(terminal.begin_selection(0, 0));
        assert!(terminal.extend_selection(0, 1));

        assert_eq!(terminal.selected_text(), Some("e\u{301}".to_owned()));
    }

    #[test]
    fn selection_does_not_add_a_newline_for_a_soft_wrapped_row() {
        let mut terminal = terminal(3, 2);
        terminal.advance(b"abcdef");

        assert!(terminal.begin_selection(0, 0));
        assert!(terminal.extend_selection(1, 3));

        assert_eq!(terminal.selected_text(), Some("abcdef".to_owned()));
    }

    #[test]
    fn renders_and_selects_narrow_scrollback_after_widening() {
        let mut terminal = terminal(13, 1);
        terminal.advance(b"first\r\nsecond\r\ncurrent");
        terminal.resize(154, 3);
        assert!(terminal.scroll_view_by(2));

        let rendered: String = terminal
            .render_spans()
            .into_iter()
            .map(|span| span.text)
            .collect();
        assert_eq!(rendered, "first\nsecond\ncurrent");

        assert!(terminal.begin_selection(0, 0));
        assert!(terminal.extend_selection(2, 7));
        assert_eq!(
            terminal.selection_ranges(),
            vec![
                super::SelectionRange {
                    row: 0,
                    start_column: 0,
                    end_column: 13,
                },
                super::SelectionRange {
                    row: 1,
                    start_column: 0,
                    end_column: 13,
                },
                super::SelectionRange {
                    row: 2,
                    start_column: 0,
                    end_column: 7,
                },
            ]
        );
        assert_eq!(
            terminal.selected_text(),
            Some("first\nsecond\ncurrent".to_owned())
        );
    }

    #[test]
    fn keeps_the_scrollback_view_stable_when_new_output_arrives() {
        let mut terminal = terminal(4, 2);
        terminal.advance(b"one\r\ntwo\r\nend");
        assert!(terminal.scroll_view_by(1));

        terminal.advance(b"\r\nnext");
        let rendered: String = terminal
            .render_spans()
            .into_iter()
            .map(|span| span.text)
            .collect();

        assert!(rendered.starts_with("one\ntwo"));
    }

    #[test]
    fn clearing_scrollback_invalidates_a_history_selection() {
        let mut terminal = terminal(4, 2);
        terminal.advance(b"one\r\ntwo\r\nend");
        assert!(terminal.scroll_view_by(1));
        assert!(terminal.begin_selection(0, 0));
        assert!(terminal.extend_selection(1, 3));

        terminal.advance(b"\x1b[3J");

        assert_eq!(terminal.selected_text(), None);
        assert!(terminal.selection_ranges().is_empty());
    }

    #[test]
    fn tracks_bracketed_paste_mode() {
        let mut terminal = terminal(4, 1);

        terminal.advance(b"\x1b[?2004h");
        assert!(terminal.bracketed_paste_enabled());

        terminal.advance(b"\x1b[?2004l");
        assert!(!terminal.bracketed_paste_enabled());
    }

    #[test]
    fn clamps_large_csi_counts_without_overflowing() {
        let mut terminal = terminal(5, 2);
        terminal.advance(b"\x1b[4C\x1b[65535C!\x1b[2;5H\x1b[65535B?");

        assert_eq!(row(&terminal, 0), "    !");
        assert_eq!(row(&terminal, 1), "    ?");
    }
}

//! Terminal rendering widget
//!
//! A ratatui Widget that renders a vt100::Screen directly,
//! with support for scrollback.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

/// Widget for rendering a vt100 terminal screen
pub struct TerminalWidget<'a> {
    screen: &'a vt100::Screen,
}

impl<'a> TerminalWidget<'a> {
    pub fn new(screen: &'a vt100::Screen) -> Self {
        Self { screen }
    }
}

impl Widget for TerminalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (rows, cols) = self.screen.size();

        for row in 0..rows.min(area.height) {
            for col in 0..cols.min(area.width) {
                if let Some(cell) = self.screen.cell(row, col) {
                    // Skip wide character continuations
                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let contents = cell.contents();
                    let fg = convert_color(cell.fgcolor());
                    let bg = convert_color(cell.bgcolor());

                    let mut style = Style::default().fg(fg).bg(bg);

                    // Apply text attributes
                    let mut modifiers = Modifier::empty();
                    if cell.bold() {
                        modifiers |= Modifier::BOLD;
                    }
                    if cell.dim() {
                        modifiers |= Modifier::DIM;
                    }
                    if cell.italic() {
                        modifiers |= Modifier::ITALIC;
                    }
                    if cell.underline() {
                        modifiers |= Modifier::UNDERLINED;
                    }
                    if cell.inverse() {
                        modifiers |= Modifier::REVERSED;
                    }
                    style = style.add_modifier(modifiers);

                    let x = area.x + col;
                    let y = area.y + row;

                    if x < area.x + area.width && y < area.y + area.height {
                        let buf_cell = buf.cell_mut((x, y)).unwrap();
                        buf_cell.set_style(style);

                        if contents.is_empty() {
                            buf_cell.set_char(' ');
                        } else {
                            buf_cell.set_symbol(&contents);
                        }
                    }
                }
            }
        }
    }
}

/// Convert vt100 color to ratatui color
fn convert_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => convert_indexed_color(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Convert indexed color (0-255) to ratatui color
fn convert_indexed_color(idx: u8) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        // 16-255: use indexed color directly
        _ => Color::Indexed(idx),
    }
}

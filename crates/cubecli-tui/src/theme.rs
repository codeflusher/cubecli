//! Shared look for cubecli screens.

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Green;
pub const MUTED: Color = Color::DarkGray;
pub const DANGER: Color = Color::Red;
pub const WARN: Color = Color::Yellow;

pub fn selected() -> Style {
    Style::new()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

pub fn title() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn hint() -> Style {
    Style::new().fg(MUTED)
}

pub fn error() -> Style {
    Style::new().fg(DANGER)
}

pub fn value() -> Style {
    Style::new().fg(Color::Cyan)
}

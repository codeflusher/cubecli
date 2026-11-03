use ratatui::style::Style;
use ratatui::text::Span;

/// Braille spinner, advanced by the draw ticker.
#[derive(Debug, Default)]
pub struct Spinner {
    frame: usize,
    running: bool,
}

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Spinner {
    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn tick(&mut self) {
        if self.running {
            self.frame = (self.frame + 1) % FRAMES.len();
        }
    }

    pub fn span(&self, style: Style) -> Span<'static> {
        Span::styled(FRAMES[self.frame].to_string(), style)
    }
}

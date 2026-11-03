//! Terminal events, already filtered to what the app cares about.

use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures_util::StreamExt;

pub struct Events {
    stream: EventStream,
}

impl Events {
    pub fn new() -> Self {
        Self {
            stream: EventStream::new(),
        }
    }

    /// Next relevant event, or None when the stream ends.
    pub async fn next(&mut self) -> Option<Result<KeyEvent, std::io::Error>> {
        loop {
            match self.stream.next().await? {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    return Some(Ok(key));
                }
                Ok(_) => continue, // resize/mouse/release — the draw loop repaints anyway
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

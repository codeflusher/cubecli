use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Single-line text editor. Typed characters are appended; the app decides
/// which widget is focused.
#[derive(Debug, Default, Clone)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Handle a key. Paste events arrive as multiple chars via KeyWithModifiers
    /// on some terminals; bracketed paste is delivered char-by-char here.
    pub fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return; // ctrl combos are shortcuts, not text
                }
                self.insert(c);
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.prev_char_boundary();
                    self.value.replace_range(prev..self.cursor, "");
                    self.cursor = prev;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    let next = self.next_char_boundary();
                    self.value.replace_range(self.cursor..next, "");
                }
            }
            KeyCode::Left => {
                self.cursor = self.prev_char_boundary();
            }
            KeyCode::Right => {
                self.cursor = self.next_char_boundary();
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            _ => {}
        }
    }

    fn prev_char_boundary(&self) -> usize {
        self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_char_boundary(&self) -> usize {
        self.value[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
            .unwrap_or(self.value.len())
    }

    fn insert(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Render with a block cursor at the current position.
    pub fn display(&self, focused: bool) -> String {
        let mut out = self.value.clone();
        if focused {
            if self.cursor >= self.value.len() {
                out.push('▏');
            } else {
                out.insert(self.cursor, '▏');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn typing_and_editing() {
        let mut t = TextInput::default();
        for c in "25565".chars() {
            t.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(t.value(), "25565");
        t.on_key(key(KeyCode::Backspace));
        assert_eq!(t.value(), "2556");
        t.on_key(key(KeyCode::Left));
        t.on_key(key(KeyCode::Char('9')));
        assert_eq!(t.value(), "25596");
    }

    #[test]
    fn unicode_boundaries() {
        let mut t = TextInput::new("héllo");
        t.on_key(key(KeyCode::Home));
        t.on_key(key(KeyCode::Right)); // cursor after 'h'
        t.on_key(key(KeyCode::Backspace)); // removes 'h'
        assert_eq!(t.value(), "éllo");
        t.on_key(key(KeyCode::End));
        t.on_key(key(KeyCode::Char('!')));
        assert_eq!(t.value(), "éllo!");
    }
}

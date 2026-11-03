use crossterm::event::{KeyCode, KeyEvent};

/// A list filtered by typed text. Navigation keys are reserved; every other
/// printable char extends the filter.
#[derive(Debug, Default, Clone)]
pub struct FilterList {
    items: Vec<String>,
    filter: String,
    selected: usize,
}

impl FilterList {
    pub fn new(items: Vec<String>) -> Self {
        Self {
            items,
            filter: String::new(),
            selected: 0,
        }
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.clamp_selection();
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn selected_index(&self) -> Option<usize> {
        let visible = self.visible();
        if visible.is_empty() {
            None
        } else {
            Some(visible[self.selected.min(visible.len() - 1)].0)
        }
    }

    pub fn selected_item(&self) -> Option<&str> {
        self.selected_index().map(|i| self.items[i].as_str())
    }

    /// Indices into the original item list, in order, matching the filter.
    pub fn visible(&self) -> Vec<(usize, &str)> {
        let f = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| f.is_empty() || item.to_lowercase().contains(&f))
            .map(|(i, item)| (i, item.as_str()))
            .collect()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.selected = 0,
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_selection();
            }
            KeyCode::Char(c) => {
                // Typed characters always filter (slugs and versions contain
                // letters like `k`/`j` that must not jump the selection).
                self.filter.push(c);
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let current = self.selected as isize;
        let next = (current + delta).clamp(0, len as isize - 1);
        self.selected = next as usize;
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn list() -> FilterList {
        FilterList::new(vec![
            "1.21.9".into(),
            "1.21.8".into(),
            "1.20.6".into(),
            "1.20.1".into(),
        ])
    }

    #[test]
    fn navigation_and_selection() {
        let mut l = list();
        assert_eq!(l.selected_item(), Some("1.21.9"));
        l.on_key(key(KeyCode::Down));
        assert_eq!(l.selected_item(), Some("1.21.8"));
        l.on_key(key(KeyCode::Down));
        l.on_key(key(KeyCode::Down));
        l.on_key(key(KeyCode::Down)); // clamps at last
        assert_eq!(l.selected_item(), Some("1.20.1"));
        l.on_key(key(KeyCode::Up));
        assert_eq!(l.selected_item(), Some("1.20.6"));
    }

    #[test]
    fn filtering() {
        let mut l = list();
        l.on_key(key(KeyCode::Char('1')));
        l.on_key(key(KeyCode::Char('.')));
        l.on_key(key(KeyCode::Char('2')));
        l.on_key(key(KeyCode::Char('0')));
        assert_eq!(l.visible().len(), 2);
        assert_eq!(l.selected_item(), Some("1.20.6"));
        // "1.201" is a substring of nothing in the list
        l.on_key(key(KeyCode::Char('1')));
        assert_eq!(l.visible().len(), 0);
        l.on_key(key(KeyCode::Backspace));
        assert_eq!(l.visible().len(), 2);
    }

    #[test]
    fn filter_matches_inside_versions() {
        // "1.8" matches "1.21.8" (chars 3..5) and nothing else
        let mut l = list();
        for c in "1.8".chars() {
            l.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(l.visible().len(), 1);
        assert_eq!(l.selected_item(), Some("1.21.8"));
    }

    #[test]
    fn letters_always_filter_never_navigate() {
        // "datapack" style filters contain k/j — they must not move selection.
        let mut l = list();
        l.on_key(key(KeyCode::Char('k')));
        assert_eq!(l.filter(), "k");
        assert!(l.visible().is_empty());
    }
}

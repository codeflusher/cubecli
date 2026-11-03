//! Addon installer screen: search → multi-select → confirm.
//!
//! The input path never awaits a network call — queries are debounced into
//! [`SEARCH_IDLE`], fired as spawned tasks parked in `search_slot`, and
//! drained by the event loop (same pattern as the wizard's fetch slot).

use std::collections::HashSet;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures_util::future::BoxFuture;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use tokio::task::{JoinError, JoinHandle};

use cubecli_core::model::AddonKind;
use cubecli_core::traits::addon_source::AddonSearchHit;

use crate::theme;
use crate::widgets::{Spinner, TextInput};

/// Remote search the screen pulls from — injected so tests can stub it.
pub type SearchFetcher = std::sync::Arc<
    dyn Fn(String, AddonKind) -> BoxFuture<'static, Result<Vec<AddonSearchHit>, String>>
        + Send
        + Sync,
>;

/// Wait after the last keystroke before the query goes to the network.
pub const SEARCH_IDLE: Duration = Duration::from_millis(350);

/// One addon queued for download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonChoice {
    pub slug: String,
    pub title: String,
    pub kind: AddonKind,
}

/// End result of the installer screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerOutcome {
    Selected(Vec<AddonChoice>),
    Aborted,
}

/// What a key press did to the app.
#[derive(Debug)]
pub enum InstallerSignal {
    Continue,
    Done(InstallerOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Search,
    Confirm,
}

/// The single in-flight search, drained by the event loop (and tests).
pub struct SearchHandle {
    /// The (query, kind) this search was issued for — stale responses are dropped.
    pub key: (String, AddonKind),
    pub task: JoinHandle<Result<Vec<AddonSearchHit>, String>>,
}

pub struct InstallerApp {
    kinds: Vec<AddonKind>,
    kind_index: usize,
    /// Where hits come from, for titles ("modrinth" / "curseforge").
    source_label: String,
    query: TextInput,
    results: Vec<AddonSearchHit>,
    highlighted: usize,
    picked: Vec<AddonChoice>,
    picked_slugs: HashSet<String>,
    screen: Screen,
    confirm_cursor: usize,
    loading: bool,
    error: Option<String>,
    spinner: Spinner,
    /// Query+kind waiting for the idle timer to fire.
    pending: Option<(String, AddonKind)>,
    /// The single in-flight search, drained by the event loop.
    pub search_slot: Option<SearchHandle>,
}

impl InstallerApp {
    pub fn new(kinds: Vec<AddonKind>, source_label: impl Into<String>) -> Self {
        assert!(
            !kinds.is_empty(),
            "at least one addon kind must be supported"
        );
        Self {
            kinds,
            kind_index: 0,
            source_label: source_label.into(),
            query: TextInput::default(),
            results: Vec::new(),
            highlighted: 0,
            picked: Vec::new(),
            picked_slugs: HashSet::new(),
            screen: Screen::Search,
            confirm_cursor: 0,
            loading: false,
            error: None,
            spinner: Spinner::default(),
            pending: None,
            search_slot: None,
        }
    }

    pub fn kind(&self) -> AddonKind {
        self.kinds[self.kind_index]
    }

    pub fn results(&self) -> &[AddonSearchHit] {
        &self.results
    }

    pub fn picked(&self) -> &[AddonChoice] {
        &self.picked
    }

    /// Advance the spinner — called from the draw ticker.
    pub fn tick(&mut self) {
        self.spinner.tick();
    }

    fn request_key(&self) -> (String, AddonKind) {
        (self.query.value().trim().to_string(), self.kind())
    }

    /// Queue the current query+kind for a network search.
    fn mark_pending(&mut self) {
        let key = self.request_key();
        self.pending = if key.0.is_empty() { None } else { Some(key) };
    }

    fn cycle_kind(&mut self, delta: isize) {
        let len = self.kinds.len();
        self.kind_index = (self.kind_index as isize + delta).rem_euclid(len as isize) as usize;
        self.mark_pending();
    }

    /// The search to fire when the idle timer expires (none while the query
    /// is empty — empty queries never hit the network).
    pub fn take_pending_search(&mut self) -> Option<(String, AddonKind)> {
        self.pending.take()
    }

    pub fn spawn_search(&mut self, request: (String, AddonKind), fetcher: &SearchFetcher) {
        let key = request.clone();
        self.search_slot = Some(SearchHandle {
            key,
            task: tokio::spawn(fetcher(request.0, request.1)),
        });
        self.loading = true;
        self.spinner.start();
    }

    pub fn apply_search(&mut self, result: Result<Result<Vec<AddonSearchHit>, String>, JoinError>) {
        self.loading = false;
        self.spinner.stop();
        let Some(handle) = self.search_slot.take() else {
            return;
        };
        // A newer keystroke changed the query or kind: this response is stale.
        if handle.key != self.request_key() {
            return;
        }
        match result {
            Ok(Ok(hits)) => {
                self.error = None;
                self.results = hits;
                self.highlighted = 0;
            }
            Ok(Err(err)) => self.error = Some(err),
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn move_highlight(&mut self, delta: isize) {
        let len = self.results.len() as isize;
        if len == 0 {
            return;
        }
        self.highlighted = (self.highlighted as isize + delta).clamp(0, len - 1) as usize;
    }

    fn toggle_pick(&mut self, index: usize) {
        let Some(hit) = self.results.get(index) else {
            return;
        };
        if self.picked_slugs.contains(&hit.slug) {
            self.picked.retain(|c| c.slug != hit.slug);
            self.picked_slugs.remove(&hit.slug);
        } else {
            self.picked_slugs.insert(hit.slug.clone());
            self.picked.push(AddonChoice {
                slug: hit.slug.clone(),
                title: hit.title.clone(),
                kind: self.kind(),
            });
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> InstallerSignal {
        match key.code {
            KeyCode::Esc => return InstallerSignal::Done(InstallerOutcome::Aborted),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return InstallerSignal::Done(InstallerOutcome::Aborted);
            }
            _ => {}
        }
        match self.screen {
            Screen::Search => self.on_key_search(key),
            Screen::Confirm => self.on_key_confirm(key),
        }
    }

    fn on_key_search(&mut self, key: KeyEvent) -> InstallerSignal {
        match key.code {
            KeyCode::Up => self.move_highlight(-1),
            KeyCode::Down => self.move_highlight(1),
            KeyCode::PageUp => self.move_highlight(-10),
            KeyCode::PageDown => self.move_highlight(10),
            KeyCode::Tab => self.cycle_kind(1),
            KeyCode::BackTab => self.cycle_kind(-1),
            KeyCode::Char(' ') => self.toggle_pick(self.highlighted),
            KeyCode::Enter => {
                if !self.picked.is_empty() {
                    self.screen = Screen::Confirm;
                    self.confirm_cursor = 0;
                }
            }
            _ => {
                self.query.on_key(key);
                self.mark_pending();
            }
        }
        InstallerSignal::Continue
    }

    fn on_key_confirm(&mut self, key: KeyEvent) -> InstallerSignal {
        match key.code {
            KeyCode::Up => self.confirm_cursor = self.confirm_cursor.saturating_sub(1),
            KeyCode::Down => {
                self.confirm_cursor =
                    (self.confirm_cursor + 1).min(self.picked.len().saturating_sub(1));
            }
            KeyCode::Char(' ') => {
                if self.confirm_cursor < self.picked.len() {
                    let removed = self.picked.remove(self.confirm_cursor);
                    self.picked_slugs.remove(&removed.slug);
                    self.confirm_cursor =
                        self.confirm_cursor.min(self.picked.len().saturating_sub(1));
                    if self.picked.is_empty() {
                        self.screen = Screen::Search;
                    }
                }
            }
            KeyCode::Backspace => {
                self.screen = Screen::Search;
                self.confirm_cursor = 0;
            }
            KeyCode::Enter => {
                return InstallerSignal::Done(InstallerOutcome::Selected(self.picked.clone()));
            }
            _ => {}
        }
        InstallerSignal::Continue
    }

    /// Full-screen layout: search bar, kind selector, results, selection
    /// summary, hints. The confirm dialog overlays the center.
    pub fn render(&self, f: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(3), // query
            Constraint::Length(1), // kind selector
            Constraint::Min(5),    // results
            Constraint::Length(3), // selection summary
            Constraint::Length(1), // hints
        ])
        .split(f.area());

        let query = Paragraph::new(Line::from(vec![
            Span::styled(" query ", theme::title()),
            Span::raw(self.query.display(true)),
        ]))
        .block(block("search addons"));
        f.render_widget(query, chunks[0]);

        let mut spans = vec![Span::styled(" kind  ", theme::title())];
        for (i, k) in self.kinds.iter().enumerate() {
            let on = i == self.kind_index;
            spans.push(Span::styled(
                if on { "● " } else { "○ " },
                if on {
                    Style::new().fg(theme::ACCENT)
                } else {
                    theme::hint()
                },
            ));
            spans.push(Span::styled(
                format!("{:<16}", k.slug()),
                if on { theme::value() } else { theme::hint() },
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), chunks[1]);

        self.render_results(f, chunks[2]);

        let summary = if self.picked.is_empty() {
            Line::styled(
                " nothing selected yet — space toggles a highlighted result",
                theme::hint(),
            )
        } else {
            Line::from(
                self.picked
                    .iter()
                    .map(|c| {
                        Span::styled(
                            format!(" ● {}:{}", c.kind.slug(), c.slug),
                            Style::new().fg(theme::ACCENT),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        f.render_widget(
            Paragraph::new(summary).block(block(&format!("selected ({})", self.picked.len()))),
            chunks[3],
        );

        let hints = match self.screen {
            Screen::Search => {
                "type to search · ↑/↓ move · space select · tab/⇧tab kind · enter review · esc abort"
            }
            Screen::Confirm => "enter download · space remove · backspace back · esc abort",
        };
        let mut footer = Vec::new();
        if self.spinner.is_running() {
            footer.push(self.spinner.span(theme::title()));
            footer.push(Span::raw(" "));
        }
        footer.push(Span::styled(hints, theme::hint()));
        f.render_widget(Paragraph::new(Line::from(footer)), chunks[4]);

        if self.screen == Screen::Confirm {
            self.render_confirm(f);
        }
    }

    fn render_results(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let title = format!("{} results", self.source_label);
        let inner = block(&title).inner(area);

        if let Some(err) = &self.error {
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled(format!(" {err}"), theme::error()),
                    Line::raw(""),
                    Line::styled(" keep typing to retry", theme::hint()),
                ]),
                inner,
            );
            return;
        }
        if self.results.is_empty() {
            let message = if self.loading {
                Line::from(vec![
                    self.spinner.span(theme::title()),
                    Span::raw(" searching…"),
                ])
            } else if self.query.is_empty() {
                Line::styled(
                    " type to search — e.g. sodium, essentialsx, create",
                    theme::hint(),
                )
            } else {
                Line::styled(
                    format!(
                        " no {} results for \"{}\"",
                        self.kind().slug(),
                        self.query.value().trim()
                    ),
                    theme::hint(),
                )
            };
            f.render_widget(Paragraph::new(vec![message]), inner);
            return;
        }

        let items: Vec<ListItem> = self
            .results
            .iter()
            .map(|hit| {
                let picked = self.picked_slugs.contains(&hit.slug);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if picked { "● " } else { "○ " },
                        if picked {
                            Style::new().fg(theme::ACCENT)
                        } else {
                            theme::hint()
                        },
                    ),
                    Span::styled(
                        format!("{:<24}", truncate(&hit.title, 24)),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {:>6} ", fmt_downloads(hit.downloads)),
                        theme::hint(),
                    ),
                    Span::styled(truncate(&hit.description, 60), theme::hint()),
                ]))
            })
            .collect();
        let mut state = ListState::default();
        if !self.results.is_empty() {
            state.select(Some(self.highlighted.min(self.results.len() - 1)));
        }
        f.render_stateful_widget(
            List::new(items)
                .block(block(&title))
                .highlight_style(theme::selected()),
            area,
            &mut state,
        );
    }

    fn render_confirm(&self, f: &mut Frame) {
        let area = f.area();
        let height = (self.picked.len() as u16 + 5).min(area.height.saturating_sub(2));
        let box_area = ratatui::layout::Rect {
            x: 2,
            y: area.height.saturating_sub(height) / 2,
            width: area.width.saturating_sub(4),
            height,
        };

        let mut lines = vec![Line::styled(
            format!(" download these {} addons?", self.picked.len()),
            theme::title(),
        )];
        for (i, choice) in self.picked.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    if i == self.confirm_cursor {
                        "▸ "
                    } else {
                        "  "
                    },
                    if i == self.confirm_cursor {
                        theme::selected()
                    } else {
                        Style::new()
                    },
                ),
                Span::styled(
                    format!("{}:{:<24}", choice.kind.slug(), choice.slug),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {}", truncate(&choice.title, 40)), theme::hint()),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            " enter — download · space remove · backspace back · esc abort",
            theme::hint(),
        ));

        f.render_widget(Clear, box_area);
        f.render_widget(
            Paragraph::new(lines).block(Block::new().borders(Borders::ALL)),
            box_area,
        );
    }
}

fn block(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .title(Span::styled(format!(" {title} "), theme::title()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// `5_234_000` → `5.2M`, `43_000` → `43k`, `312` → `312`.
fn fmt_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_format() {
        assert_eq!(fmt_downloads(312), "312");
        assert_eq!(fmt_downloads(43_000), "43k");
        assert_eq!(fmt_downloads(4_200_000), "4.2M");
    }

    #[test]
    fn truncate_keeps_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghijk", 5).chars().count(), 5);
    }
}

//! Wizard app state: chrome rendering + the non-blocking fetch slot.
//!
//! The input path never awaits a network call — fetches are spawned tasks
//! parked in `fetch` and drained via `tokio::select!`.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tokio::task::{JoinError, JoinHandle};

use crate::theme;
use crate::widgets::Spinner;
use crate::wizard::{
    FetchPayload, FetchRequest, Fetcher, Transition, WizardFlow, WizardOutcome, steps,
};

/// What a key press did to the app.
#[derive(Debug)]
pub enum FlowSignal {
    Continue,
    Done(WizardOutcome),
}

pub struct App {
    pub flow: WizardFlow,
    spinner: Spinner,
    /// The single in-flight fetch, drained by the event loop.
    pub fetch_slot: Option<JoinHandle<Result<FetchPayload, String>>>,
}

impl App {
    pub fn new(answers: crate::wizard::WizardAnswers) -> Self {
        Self {
            flow: WizardFlow::new(answers),
            spinner: Spinner::default(),
            fetch_slot: None,
        }
    }

    /// Kick off any data the current step needs.
    pub fn enter_current(&mut self, fetcher: &Fetcher) {
        if let Some(request) = self.flow.current().enter_request(&self.flow.answers) {
            self.spawn_fetch(request, fetcher);
        }
    }

    fn spawn_fetch(&mut self, request: FetchRequest, fetcher: &Fetcher) {
        let future = fetcher(request);
        self.fetch_slot = Some(tokio::spawn(async move { future.await }));
        self.flow.current_mut().set_loading(true);
        self.spinner.start();
    }

    pub fn on_key(&mut self, key: KeyEvent, fetcher: &Fetcher) -> FlowSignal {
        // global keys
        match key.code {
            KeyCode::Esc => return FlowSignal::Done(WizardOutcome::Aborted),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return FlowSignal::Done(WizardOutcome::Aborted);
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                return self.retreat(fetcher);
            }
            _ => {}
        }

        let transition = self.flow.handle_key(key);
        match transition {
            Transition::Stay => FlowSignal::Continue,
            Transition::Next => self.advance(1, fetcher),
            Transition::Skip(n) => self.advance(n, fetcher),
            Transition::Back => self.retreat(fetcher),
            Transition::Finish => {
                FlowSignal::Done(WizardOutcome::Answers(self.flow.answers.clone()))
            }
            Transition::Quit => FlowSignal::Done(WizardOutcome::Aborted),
        }
    }

    fn advance(&mut self, n: usize, fetcher: &Fetcher) -> FlowSignal {
        let last = self.flow.steps.len() - 1;
        self.flow.index = (self.flow.index + n).min(last);
        self.enter_current(fetcher);
        FlowSignal::Continue
    }

    fn retreat(&mut self, fetcher: &Fetcher) -> FlowSignal {
        if self.flow.index > 0 {
            self.flow.index -= 1;
            self.enter_current(fetcher);
        }
        FlowSignal::Continue
    }

    pub fn apply_fetch(&mut self, result: Result<Result<FetchPayload, String>, JoinError>) {
        self.spinner.stop();
        self.fetch_slot = None;
        self.flow.current_mut().set_loading(false);
        match result {
            Ok(Ok(payload)) => self.flow.handle_fetch(payload),
            Ok(Err(err)) => self.flow.handle_fetch_failed(&err),
            Err(err) => self.flow.handle_fetch_failed(&err.to_string()),
        }
    }

    /// Full-screen chrome: header, breadcrumbs, step body, footer.
    pub fn render(&self, f: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(f.area());

        let total = self.flow.steps.len();
        let current = self.flow.current();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("cubecli create", theme::title()),
                Span::styled(
                    format!(
                        "  —  step {} of {total}  —  {}",
                        self.flow.index + 1,
                        current.title()
                    ),
                    theme::hint(),
                ),
            ])),
            chunks[0],
        );

        let mut crumbs = Vec::new();
        for (i, name) in steps::breadcrumb_names().iter().enumerate() {
            let style = if i < self.flow.index {
                theme::value()
            } else if i == self.flow.index {
                theme::title()
            } else {
                Style::new().fg(theme::MUTED)
            };
            let mark = if i < self.flow.index {
                "● "
            } else if i == self.flow.index {
                "▸ "
            } else {
                "○ "
            };
            crumbs.push(Span::styled(format!("{mark}{name} "), style));
        }
        f.render_widget(Paragraph::new(Line::from(crumbs)), chunks[1]);

        current.render(f, chunks[2], &self.flow.answers, &self.spinner);

        let mut footer = Vec::new();
        if self.spinner.is_running() {
            footer.push(self.spinner.span(theme::title()));
            footer.push(Span::raw(" "));
        }
        footer.push(Span::styled(current.hints(), theme::hint()));
        f.render_widget(Paragraph::new(Line::from(footer)), chunks[3]);
    }
}

/// Redraw cadence — also drives the spinner.
pub const TICK: Duration = Duration::from_millis(120);

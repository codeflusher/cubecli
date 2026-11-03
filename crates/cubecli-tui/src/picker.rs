//! Restore picker: a single-screen table selection (no async fetches).

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::error::TuiError;
use crate::event::Events;
use crate::theme;

/// Show `items`, return the chosen index (Esc/cancel → None).
pub async fn run_picker(
    title: &str,
    hint: &str,
    items: Vec<String>,
) -> Result<Option<usize>, TuiError> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut terminal = ratatui::init();
    let result = drive(&mut terminal, title, hint, items).await;
    ratatui::restore();
    result
}

async fn drive(
    terminal: &mut Terminal<impl ratatui::backend::Backend>,
    title: &str,
    hint: &str,
    items: Vec<String>,
) -> Result<Option<usize>, TuiError> {
    let mut selected = 0usize;
    let mut events = Events::new();

    let outcome = loop {
        terminal
            .draw(|f| {
                let chunks = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(5),
                    Constraint::Length(1),
                ])
                .split(f.area());

                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(title.to_string(), theme::title()))),
                    chunks[0],
                );

                let list_items: Vec<ListItem> = items
                    .iter()
                    .map(|item| {
                        ListItem::new(Line::from(Span::styled(
                            format!(" {item}"),
                            Style::new().add_modifier(Modifier::empty()),
                        )))
                    })
                    .collect();
                let mut state = ListState::default();
                state.select(Some(selected));
                f.render_stateful_widget(
                    List::new(list_items)
                        .block(Block::new().borders(Borders::ALL))
                        .highlight_style(theme::selected()),
                    chunks[1],
                    &mut state,
                );

                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(hint.to_string(), theme::hint()))),
                    chunks[2],
                );
            })
            .map_err(|e| TuiError::Terminal(e.to_string()))?;

        match events.next().await {
            Some(Ok(key)) => match key.code {
                KeyCode::Esc => break None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break None,
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(items.len() - 1),
                KeyCode::PageUp => selected = selected.saturating_sub(10),
                KeyCode::PageDown => selected = (selected + 10).min(items.len() - 1),
                KeyCode::Enter => break Some(selected),
                _ => {}
            },
            Some(Err(e)) => return Err(TuiError::Terminal(e.to_string())),
            None => break None,
        }
    };
    Ok(outcome)
}

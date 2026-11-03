//! cubecli-tui: full-screen ratatui screens.
//!
//! Contract with callers: pass in a [`wizard::Fetcher`] (async data source)
//! and initial [`wizard::WizardAnswers`]; get back either the completed
//! answers or an abort. All terminal setup/teardown happens here.

pub mod app;
pub mod error;
pub mod event;
pub mod installer;
pub mod picker;
pub mod theme;
pub mod widgets;
pub mod wizard;

use std::time::Duration;

use ratatui::Terminal;

pub use error::TuiError;
pub use installer::{AddonChoice, InstallerApp, InstallerOutcome, SearchFetcher};
pub use wizard::{FetchPayload, FetchRequest, Fetcher, WizardAnswers, WizardOutcome};

use app::{App, FlowSignal};
use event::Events;
use installer::InstallerSignal;

/// Run the create wizard. Restores the terminal on every return path.
pub async fn run_wizard(
    initial: WizardAnswers,
    fetcher: Fetcher,
) -> Result<WizardOutcome, TuiError> {
    let mut terminal = ratatui::init();
    let result = drive_wizard(&mut terminal, initial, fetcher).await;
    ratatui::restore();
    result
}

/// Testable inner loop: same behavior against any terminal backend.
pub async fn drive_wizard(
    terminal: &mut Terminal<impl ratatui::backend::Backend>,
    initial: WizardAnswers,
    fetcher: Fetcher,
) -> Result<WizardOutcome, TuiError> {
    let mut app = App::new(initial);
    app.enter_current(&fetcher);

    let mut events = Events::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let outcome = loop {
        terminal
            .draw(|f| app.render(f))
            .map_err(|e| TuiError::Terminal(e.to_string()))?;

        tokio::select! {
            maybe_key = events.next() => match maybe_key {
                Some(Ok(key)) => match app.on_key(key, &fetcher) {
                    FlowSignal::Continue => {}
                    FlowSignal::Done(outcome) => break outcome,
                },
                Some(Err(e)) => return Err(TuiError::Terminal(e.to_string())),
                None => break WizardOutcome::Aborted,
            },
            _ = ticker.tick() => {}
            fetched = async {
                match &mut app.fetch_slot {
                    Some(handle) => Some(handle.await),
                    None => None,
                }
            }, if app.fetch_slot.is_some() => {
                if let Some(result) = fetched {
                    app.apply_fetch(result);
                }
            }
        }
    };
    Ok(outcome)
}

/// Run the addon installer (search + multi-select + confirm). Restores the
/// terminal on every return path.
pub async fn run_installer(
    kinds: Vec<cubecli_core::model::AddonKind>,
    source_label: impl Into<String>,
    fetcher: SearchFetcher,
) -> Result<InstallerOutcome, TuiError> {
    let mut terminal = ratatui::init();
    let result = drive_installer(
        &mut terminal,
        InstallerApp::new(kinds, source_label),
        fetcher,
    )
    .await;
    ratatui::restore();
    result
}

/// Testable inner loop: debounced searches, key handling, drawing — against
/// any terminal backend.
pub async fn drive_installer(
    terminal: &mut Terminal<impl ratatui::backend::Backend>,
    mut app: InstallerApp,
    fetcher: SearchFetcher,
) -> Result<InstallerOutcome, TuiError> {
    let mut events = Events::new();
    let mut ticker = tokio::time::interval(app::TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut idle = Box::pin(tokio::time::sleep_until(
        tokio::time::Instant::now() + installer::SEARCH_IDLE,
    ));

    let outcome = loop {
        terminal
            .draw(|f| app.render(f))
            .map_err(|e| TuiError::Terminal(e.to_string()))?;

        tokio::select! {
            maybe_key = events.next() => match maybe_key {
                Some(Ok(key)) => {
                    match app.on_key(key) {
                        InstallerSignal::Continue => {}
                        InstallerSignal::Done(outcome) => break outcome,
                    }
                    // Debounce counts from the last keystroke.
                    idle.as_mut()
                        .reset(tokio::time::Instant::now() + installer::SEARCH_IDLE);
                }
                Some(Err(e)) => return Err(TuiError::Terminal(e.to_string())),
                None => break InstallerOutcome::Aborted,
            },
            _ = ticker.tick() => app.tick(),
            _ = idle.as_mut() => {
                if let Some(request) = app.take_pending_search() {
                    app.spawn_search(request, &fetcher);
                }
                idle.as_mut()
                    .reset(tokio::time::Instant::now() + installer::SEARCH_IDLE);
            }
            result = async {
                match app.search_slot.as_mut() {
                    Some(handle) => Some((&mut handle.task).await),
                    None => None,
                }
            }, if app.search_slot.is_some() => {
                if let Some(result) = result {
                    app.apply_search(result);
                }
            }
        }
    };
    Ok(outcome)
}

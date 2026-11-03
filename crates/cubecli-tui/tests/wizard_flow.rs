//! Wizard walkthrough driven by synthetic key events — proves step
//! transitions, fetch draining, and the draw path without a TTY.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use cubecli_core::model::{CoreKind, McVersion};
use cubecli_tui::app::{App, FlowSignal};
use cubecli_tui::{FetchPayload, Fetcher, WizardAnswers, WizardOutcome};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::empty(),
        kind: crossterm::event::KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

fn fetcher() -> Fetcher {
    Arc::new(|_| {
        Box::pin(async {
            // one version only: deterministic selection
            Ok(FetchPayload::McVersions(vec![McVersion {
                id: "1.21.8".into(),
                stable: true,
            }]))
        })
    })
}

/// Wait for the in-flight fetch and feed it into the app.
async fn drain(app: &mut App) {
    if let Some(handle) = app.fetch_slot.take() {
        let result = handle.await.expect("fetch task panicked");
        app.apply_fetch(Ok(result));
    }
}

#[tokio::test]
async fn walkthrough_reaches_answers() {
    let fetcher = fetcher();
    let mut app = App::new(WizardAnswers::defaults("test-server"));
    app.enter_current(&fetcher);
    drain(&mut app).await; // core step: no fetch (defaults), may be none

    // 1. core: move to Fabric (preselected Paper = index 1 → 3)
    app.on_key(key(KeyCode::Down), &fetcher);
    app.on_key(key(KeyCode::Down), &fetcher);
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 2. mc version: fetch spawned on entry — drain, then select
    drain(&mut app).await;
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 3. build step (fabric has one implicit build)
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 4. network (port 25565 default)
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 5. policies (defaults validate)
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 6. addons
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 7. people
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 8. eula: must not pass without agreeing
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));
    assert_eq!(app.flow.index, 7, "eula step must hold until accepted");
    app.on_key(key(KeyCode::Char(' ')), &fetcher);
    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &fetcher),
        FlowSignal::Continue
    ));

    // 9. summary → finish
    let signal = app.on_key(key(KeyCode::Enter), &fetcher);
    match signal {
        FlowSignal::Done(WizardOutcome::Answers(answers)) => {
            assert_eq!(answers.core, Some(CoreKind::Fabric));
            assert_eq!(answers.mc_version.as_deref(), Some("1.21.8"));
            assert_eq!(answers.port, "25565");
            assert!(answers.eula_accepted);
            let meta = answers
                .into_project_meta()
                .expect("answers must build valid meta");
            assert_eq!(meta.server.core, CoreKind::Fabric);
        }
        other => panic!("expected finished answers, got {other:?}"),
    }
}

#[tokio::test]
async fn escape_aborts_from_any_step() {
    let fetcher = fetcher();
    let mut app = App::new(WizardAnswers::defaults("x"));
    app.on_key(key(KeyCode::Down), &fetcher);
    assert!(matches!(
        app.on_key(key(KeyCode::Esc), &fetcher),
        FlowSignal::Done(WizardOutcome::Aborted)
    ));
}

#[test]
fn renders_into_test_backend() {
    let app = App::new(WizardAnswers::defaults("demo"));
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("cubecli create"), "header missing");
    assert!(text.contains("server core"), "step title missing");
    assert!(text.contains("Fabric"), "core list missing");
}

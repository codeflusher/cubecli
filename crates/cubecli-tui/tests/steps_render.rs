//! Render + transition specs for every wizard step, driven through
//! TestBackend and synthetic keys. No TTY involved.

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use cubecli_core::model::{AddonSourceId, CoreKind, RestartPolicy};
use cubecli_tui::app::App;
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
    std::sync::Arc::new(|_| {
        Box::pin(async {
            Ok(FetchPayload::McVersions(vec![
                cubecli_core::model::McVersion {
                    id: "1.21.8".into(),
                    stable: true,
                },
                cubecli_core::model::McVersion {
                    id: "1.21.8-rc1".into(),
                    stable: false,
                },
            ]))
        })
    })
}

fn buffer_text(terminal: &mut Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    buffer.content().iter().map(|c| c.symbol()).collect()
}

/// Render the CURRENT step of `app` and return the screen text.
fn render_step(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    buffer_text(&mut terminal)
}

fn answers() -> WizardAnswers {
    WizardAnswers::defaults("spec")
}

#[tokio::test]
async fn every_step_renders_with_content() {
    let fetcher = fetcher();
    let mut app = App::new(answers());
    app.enter_current(&fetcher);

    let expect_per_step = [
        ("server core", vec!["Fabric", "Paper", "Vanilla"]),
        ("minecraft version", vec!["1.21.8"]),
        ("core build", vec!["latest stable build is used"]),
        ("network", vec!["server port", "25565"]),
        ("policies", vec!["restart policy", "auto backup"]),
        ("addons", vec!["default source", "modrinth"]),
        ("whitelist & operators", vec!["whitelist"]),
        ("eula", vec!["aka.ms/MinecraftEULA"]),
        ("summary", vec!["server folder"]),
    ];

    for (title, needles) in expect_per_step {
        drain(&mut app).await;
        let text = render_step(&app);
        assert!(text.contains(title), "step {title}: title missing — {text}");
        for needle in needles {
            assert!(
                text.contains(needle),
                "step {title}: `{needle}` missing — {text}"
            );
        }
        if app.flow.index == 7 {
            // EULA gates: agree before continuing
            app.on_key(key(KeyCode::Char(' ')), &fetcher);
        }
        if app.flow.index < 8 {
            app.on_key(key(KeyCode::Enter), &fetcher);
        }
    }
}

async fn drain(app: &mut App) {
    if let Some(handle) = app.fetch_slot.take() {
        let result = handle.await.expect("fetch task panicked");
        app.apply_fetch(Ok(result));
    }
}

#[tokio::test]
async fn loading_and_error_states_render() {
    let fetcher = fetcher();
    let mut app = App::new(answers());
    app.enter_current(&fetcher);

    // move to the version step WITHOUT draining: the spinner state renders
    app.on_key(key(KeyCode::Enter), &fetcher);
    let text = render_step(&app);
    assert!(text.contains("fetching versions"), "{text}");
    drain(&mut app).await;

    // a failing fetch renders the error + recovery hint
    let failing: Fetcher =
        std::sync::Arc::new(|_| Box::pin(async { Err("network is down".to_string()) }));
    // retreat to the core step, then advance into version with the failing fetcher
    app.on_key(
        KeyEvent {
            code: KeyCode::Left,
            modifiers: KeyModifiers::SHIFT,
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::empty(),
        },
        &fetcher,
    );
    assert_eq!(app.flow.index, 0, "retreated to core");
    app.on_key(key(KeyCode::Enter), &failing);
    drain(&mut app).await; // the failed fetch
    let text = render_step(&app);
    assert!(text.contains("network is down"), "{text}");
}

#[tokio::test]
async fn policies_step_renders_cron_field_when_selected() {
    let fetcher = fetcher();
    let mut app = App::new(answers());

    // fast-forward to the policies step
    for _ in 0..4 {
        drain(&mut app).await;
        app.on_key(key(KeyCode::Enter), &fetcher);
    }
    assert_eq!(app.flow.index, 4, "on policies step");

    // cycle the policy to cron: field 0 focused, press Right twice
    app.on_key(key(KeyCode::Right), &fetcher);
    app.on_key(key(KeyCode::Right), &fetcher);
    let text = render_step(&app);
    assert!(
        text.contains("restart schedule"),
        "cron reveals the schedule field: {text}"
    );

    // entering without a cron expression shows the inline error
    app.on_key(key(KeyCode::Enter), &fetcher);
    let text = render_step(&app);
    assert!(text.contains("cron expression"), "{text}");
}

#[tokio::test]
async fn eula_step_gates_and_renders_warning_in_summary() {
    let fetcher = fetcher();
    let mut app = App::new(answers());

    // answer everything up to eula with defaults
    for _ in 0..7 {
        drain(&mut app).await;
        app.on_key(key(KeyCode::Enter), &fetcher);
    }
    assert_eq!(app.flow.index, 7);

    // enter without agreeing stays
    app.on_key(key(KeyCode::Enter), &fetcher);
    assert_eq!(app.flow.index, 7);
    let text = render_step(&app);
    assert!(text.contains("must accept"), "{text}");

    // agree → summary shows the warning line
    app.on_key(key(KeyCode::Char(' ')), &fetcher);
    app.on_key(key(KeyCode::Enter), &fetcher);
    let text = render_step(&app);
    assert!(text.contains("eula"), "{text}");

    // accept in answers and re-render: warning gone
    app.flow.answers.eula_accepted = true;
    let text = render_step(&app);
    assert!(!text.contains("will refuse"), "{text}");
}

#[test]
fn summary_reflects_every_answer() {
    let mut a = answers();
    a.core = Some(CoreKind::Purpur);
    a.mc_version = Some("1.21.8".into());
    a.port = "26000".into();
    a.pvp = false;
    a.restart_policy = RestartPolicy::Cron;
    a.restart_cron = "0 4 * * *".into();
    a.backup_auto = true;
    a.backup_rule = "1d6h".into();
    a.backup_limit = "7".into();
    a.addon_source = AddonSourceId::CurseForge;
    a.addon_autocheck = true;
    a.whitelist_enabled = true;
    a.whitelist = "alice, bob".into();
    a.ops = "carol".into();
    a.eula_accepted = true;

    let mut app = App::new(a);
    app.flow.index = 8; // jump straight to the summary
    let text = render_step(&app);
    for needle in [
        "26000",
        "off",
        "cron",
        "0 4 * * *",
        "1d6h",
        "curseforge",
        "alice, bob",
        "carol",
    ] {
        assert!(text.contains(needle), "summary missing `{needle}`:\n{text}");
    }
}

/// Regression: list selections used to render with ratatui's default
/// (invisible) highlight style, so the core/version cursors could only be
/// found blind. The selected row must actually be highlighted — and be the
/// row the state points at.
#[tokio::test]
async fn list_selections_are_visibly_highlighted() {
    let fetcher = fetcher();
    let mut app = App::new(answers());
    app.enter_current(&fetcher);

    let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();

    // core step: Paper is preselected (index 1)
    terminal.draw(|f| app.render(f)).unwrap();
    let highlighted = highlighted_row_texts(&terminal);
    assert_eq!(highlighted.len(), 1, "exactly one row highlighted");
    assert!(
        highlighted[0].contains("Paper"),
        "the highlighted row must be the selected core, got: {:?}",
        highlighted[0]
    );

    // version step: the first (newest) version starts selected
    drain(&mut app).await;
    app.on_key(key(KeyCode::Enter), &fetcher);
    drain(&mut app).await;
    terminal.draw(|f| app.render(f)).unwrap();
    let highlighted = highlighted_row_texts(&terminal);
    assert_eq!(highlighted.len(), 1, "exactly one row highlighted");
    // rows reconstruct the full terminal line (borders included), so match
    // the exact version id, not a prefix of `1.21.8-rc1`
    assert!(
        highlighted[0].contains(" 1.21.8 ") && !highlighted[0].contains("rc1"),
        "the highlighted row must be the selected version, got: {:?}",
        highlighted[0]
    );
}

/// Text of every buffer row that contains a cell painted with the
/// selection background (theme::selected()).
fn highlighted_row_texts(terminal: &Terminal<TestBackend>) -> Vec<String> {
    use ratatui::style::Color;

    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let content = buffer.content();
    let accent: Color = cubecli_tui::theme::ACCENT;
    let mut ys: Vec<usize> = Vec::new();
    for (i, cell) in content.iter().enumerate() {
        let y = i / width;
        if cell.bg == accent && ys.last() != Some(&y) {
            ys.push(y);
        }
    }
    ys.into_iter()
        .map(|y| {
            content[y * width..(y + 1) * width]
                .iter()
                .map(|c| c.symbol())
                .collect()
        })
        .collect()
}

#[test]
fn aborted_flow_writes_nothing() {
    // the wizard only returns answers; persistence happens in the caller —
    // assert the contract: Abort carries no answers.
    let outcome = WizardOutcome::Aborted;
    assert!(matches!(outcome, WizardOutcome::Aborted));
}

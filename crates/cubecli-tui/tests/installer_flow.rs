//! Installer walkthrough driven by synthetic key events — search queuing,
//! stale-response drops, multi-select, and the confirm round-trip. Same
//! approach as the wizard specs: no TTY, no real network.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use cubecli_core::model::AddonKind;
use cubecli_core::traits::addon_source::AddonSearchHit;
use cubecli_tui::AddonChoice;
use cubecli_tui::installer::{InstallerApp, InstallerOutcome, InstallerSignal, SearchFetcher};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::empty(),
        kind: crossterm::event::KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

fn hit(slug: &str) -> AddonSearchHit {
    AddonSearchHit {
        slug: slug.into(),
        title: format!("{slug} title"),
        description: format!("{slug} does things"),
        author: "someone".into(),
        downloads: 4_200_000,
    }
}

/// Echoes the query back as a single hit — proves which search ran.
fn fetcher() -> SearchFetcher {
    Arc::new(|query, kind| {
        Box::pin(async move {
            Ok(vec![AddonSearchHit {
                slug: format!("{query}-hit"),
                title: format!("hit for {query}"),
                description: format!("{kind:?} hit"),
                author: "someone".into(),
                downloads: 4_200_000,
            }])
        })
    })
}

/// Wait for the in-flight search and feed it into the app — awaits without
/// taking the slot, exactly like the event loop does (apply_search owns the
/// stale check and clears the slot itself).
async fn drain(app: &mut InstallerApp) {
    let result = match app.search_slot.as_mut() {
        Some(handle) => Some((&mut handle.task).await),
        None => None,
    };
    if let Some(result) = result {
        app.apply_search(result);
    }
}

fn new_app() -> InstallerApp {
    InstallerApp::new(vec![AddonKind::Mod, AddonKind::Plugin], "modrinth")
}

fn type_query(app: &mut InstallerApp, query: &str) {
    for c in query.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[tokio::test]
async fn search_select_confirm_roundtrip() {
    let fetcher = fetcher();
    let mut app = new_app();

    type_query(&mut app, "sod");
    let request = app.take_pending_search().expect("query queued for search");
    assert_eq!(request, ("sod".into(), AddonKind::Mod));
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    assert_eq!(app.results().len(), 1);
    assert_eq!(app.results()[0].slug, "sod-hit");

    // space toggles the highlighted result, keeping pick order
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.picked().len(), 1);
    assert_eq!(app.picked()[0].slug, "sod-hit");
    assert_eq!(app.picked()[0].kind, AddonKind::Mod);

    // enter → review screen; enter again → the picked list comes back
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Continue
    ));
    match app.on_key(key(KeyCode::Enter)) {
        InstallerSignal::Done(InstallerOutcome::Selected(choices)) => {
            assert_eq!(
                choices,
                vec![AddonChoice {
                    slug: "sod-hit".into(),
                    title: "hit for sod".into(),
                    kind: AddonKind::Mod,
                }]
            );
        }
        other => panic!("expected a confirmed selection, got {other:?}"),
    }
}

#[tokio::test]
async fn toggling_twice_unpicks() {
    let fetcher = fetcher();
    let mut app = new_app();
    type_query(&mut app, "sod");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;

    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Char(' ')));
    assert!(app.picked().is_empty());
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Continue
    ));
    // enter again must NOT submit an empty selection
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Continue
    ));
}

#[tokio::test]
async fn stale_responses_are_dropped() {
    let fetcher = fetcher();
    let mut app = new_app();

    app.on_key(key(KeyCode::Char('a')));
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);

    // the user keeps typing while the "a" search is in flight
    app.on_key(key(KeyCode::Char('b')));
    app.apply_search(Ok(Ok(vec![hit("a-hit")])));
    assert!(
        app.results().is_empty(),
        "a response for an older query must not render"
    );

    // the newer query is still queued and fires afterwards
    assert_eq!(
        app.take_pending_search(),
        Some(("ab".into(), AddonKind::Mod))
    );
}

#[test]
fn empty_query_never_searches() {
    let mut app = new_app();
    type_query(&mut app, "x");
    assert!(app.take_pending_search().is_some());
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(
        app.take_pending_search(),
        None,
        "empty query must not search"
    );
}

#[test]
fn kind_switch_requeues_search() {
    let mut app = new_app();
    type_query(&mut app, "e");
    app.on_key(key(KeyCode::Tab)); // mod → plugin
    assert_eq!(app.kind(), AddonKind::Plugin);
    assert_eq!(
        app.take_pending_search(),
        Some(("e".into(), AddonKind::Plugin)),
        "switching kind must re-run the search"
    );
}

#[tokio::test]
async fn picks_are_tagged_with_the_active_kind() {
    let fetcher = fetcher();
    let mut app = new_app();
    type_query(&mut app, "e");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;

    app.on_key(key(KeyCode::Char(' '))); // pick as mod
    app.on_key(key(KeyCode::Tab)); // switch to plugin
    type_query(&mut app, "x");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    app.on_key(key(KeyCode::Char(' '))); // pick as plugin

    let kinds: Vec<_> = app.picked().iter().map(|c| c.kind).collect();
    assert_eq!(kinds, vec![AddonKind::Mod, AddonKind::Plugin]);
}

#[test]
fn esc_aborts_from_search_screen() {
    let mut app = new_app();
    type_query(&mut app, "sod");
    assert!(matches!(
        app.on_key(key(KeyCode::Esc)),
        InstallerSignal::Done(InstallerOutcome::Aborted)
    ));
}

#[tokio::test]
async fn esc_aborts_from_confirm_screen_and_space_removes() {
    let fetcher = fetcher();
    let mut app = new_app();
    type_query(&mut app, "sod");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Enter)); // → confirm
    assert!(matches!(
        app.on_key(key(KeyCode::Esc)),
        InstallerSignal::Done(InstallerOutcome::Aborted)
    ));

    // back on a fresh app: removing the last pick on the confirm screen
    // returns to search, and enter no longer submits
    let mut app = new_app();
    type_query(&mut app, "sod");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Enter)); // → confirm
    app.on_key(key(KeyCode::Char(' '))); // remove
    assert!(app.picked().is_empty());
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Continue
    ));
}

#[tokio::test]
async fn backspace_on_confirm_returns_to_search() {
    let fetcher = fetcher();
    let mut app = new_app();
    type_query(&mut app, "sod");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Enter)); // → confirm
    app.on_key(key(KeyCode::Backspace)); // → search
    assert_eq!(app.picked().len(), 1, "going back keeps the selection");
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Continue
    ));
    app.on_key(key(KeyCode::Enter)); // → confirm again
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        InstallerSignal::Done(InstallerOutcome::Selected(_))
    ));
}

#[tokio::test]
async fn renders_search_and_confirm_screens() {
    let fetcher = fetcher();
    let mut app = new_app();
    type_query(&mut app, "sod");
    let request = app.take_pending_search().unwrap();
    app.spawn_search(request, &fetcher);
    drain(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("search addons"), "search bar missing");
    assert!(text.contains("modrinth results"), "results pane missing");
    assert!(text.contains("mod"), "kind selector missing");
    assert!(text.contains("sod-hit"), "hit missing");
    assert!(text.contains("selected (1)"), "selection summary missing");
    assert!(text.contains("4.2M"), "download count missing");

    app.on_key(key(KeyCode::Enter)); // → confirm overlay
    terminal.draw(|f| app.render(f)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        text.contains("download these 1 addons?"),
        "confirm dialog missing"
    );
}

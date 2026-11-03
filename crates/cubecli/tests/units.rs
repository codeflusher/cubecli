//! Unit specs for the bin's small modules: exit mapping, output formatting.

use cubecli::exit::UserError;
use cubecli_core::CoreError;

#[test]
fn user_error_renders_message_and_hint() {
    let e = UserError::new("boom").with_hint("try this");
    assert_eq!(e.to_string(), "boom (hint: try this)");
    assert_eq!(UserError::new("boom").to_string(), "boom");
}

#[test]
fn core_error_hints_surface_through_from() {
    let e: UserError = CoreError::NotAProject { searched: vec![] }.into();
    assert!(e.message.contains("not a cubecli project"));
    assert!(e.hint.unwrap().contains("cubecli create"));

    let e: UserError = CoreError::EulaNotAccepted.into();
    assert!(e.hint.unwrap().contains("eula=true"));

    let e: UserError = CoreError::JavaMissing { feature: 21 }.into();
    assert!(e.hint.unwrap().contains("setup java install 21"));
}

#[test]
fn provider_errors_map_with_hints() {
    let e: UserError = cubecli_providers::ProviderError::Auth {
        service: "curseforge",
    }
    .into();
    assert!(e.message.contains("curseforge"));
    assert!(e.hint.unwrap().contains("setup curseforge"));

    let e: UserError = cubecli_providers::ProviderError::RateLimited {
        service: "modrinth",
        retry_after: None,
    }
    .into();
    assert!(e.hint.unwrap().contains("300 requests/min"));
}

#[test]
fn runtime_errors_map() {
    let e: UserError = cubecli_runtime::RuntimeError::Rcon("timeout".into()).into();
    assert!(e.message.contains("rcon"));
}

#[test]
fn exit_codes_are_stable() {
    assert_eq!(cubecli::exit::EXIT_ERROR, 1);
    assert_eq!(cubecli::exit::EXIT_INTERRUPTED, 130);
}

#[test]
fn human_size_ladder() {
    use cubecli::output::human_size;
    assert_eq!(human_size(5), "5 B");
    assert_eq!(human_size(2048), "2.0 KiB");
    assert_eq!(human_size(50 * 1024 * 1024), "50.0 MiB");
    assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
}

#[test]
fn console_progress_reports_without_panicking() {
    use cubecli::output::ConsoleProgress;
    use cubecli_core::traits::progress::ProgressReporter as _;

    let p = ConsoleProgress::new();
    p.started("download.jar", Some(1024));
    p.started("download.jar", Some(1024)); // dedup: no second line, must not panic
    p.progressed(512);
    p.finished("download.jar");
    p.message("step");

    ConsoleProgress::quiet().started("quiet.jar", None);
}

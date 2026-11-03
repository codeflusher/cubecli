//! Create-wizard flow: step trait, shared answers, and the step list.

pub mod steps;

use cubecli_core::model::{AddonKind, AddonSourceId, CoreKind, RestartPolicy};
use cubecli_core::schedule::ScheduleRule;
use cubecli_core::{CoreError, ProjectMeta};

/// Data the wizard needs fetched from providers (resolved async, off the
/// input path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchRequest {
    McVersions(CoreKind),
}

/// What a fetch came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchPayload {
    McVersions(Vec<cubecli_core::model::McVersion>),
}

pub type Fetcher = std::sync::Arc<
    dyn Fn(FetchRequest) -> futures_util::future::BoxFuture<'static, Result<FetchPayload, String>>
        + Send
        + Sync,
>;

/// Movement between steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Stay,
    Next,
    /// Advance n steps (skips that don't apply to the chosen core).
    Skip(usize),
    Back,
    Finish,
    Quit,
}

/// One wizard screen.
pub trait Step {
    fn title(&self) -> &'static str;
    fn hints(&self) -> &'static str {
        "↑/↓ move · ←/→ change · type to edit · enter confirm · ⇧← back · esc abort"
    }
    /// Data to fetch when the step becomes current.
    fn enter_request(&self, answers: &WizardAnswers) -> Option<FetchRequest> {
        let _ = answers;
        None
    }
    fn on_fetch(&mut self, _payload: FetchPayload, _answers: &mut WizardAnswers) {}
    fn on_fetch_failed(&mut self, _err: &str) {}
    /// Spinner state, driven by the app when a fetch spawns/lands.
    fn set_loading(&mut self, _loading: bool) {}
    fn on_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        answers: &mut WizardAnswers,
    ) -> Transition;
    fn render(
        &self,
        f: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        answers: &WizardAnswers,
        spinner: &crate::widgets::Spinner,
    );
}

/// Everything the wizard collects. Inputs stay raw strings until
/// [`WizardAnswers::into_project_meta`] validates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardAnswers {
    pub slug: String,
    pub core: Option<CoreKind>,
    pub mc_version: Option<String>,
    pub port: String,
    pub pvp: bool,
    pub restart_policy: RestartPolicy,
    pub restart_cron: String,
    pub backup_auto: bool,
    pub backup_rule: String,
    pub backup_limit: String,
    pub addon_source: AddonSourceId,
    pub addon_autocheck: bool,
    pub whitelist_enabled: bool,
    /// Comma-separated raw input.
    pub whitelist: String,
    /// Comma-separated raw input.
    pub ops: String,
    pub eula_accepted: bool,
}

impl WizardAnswers {
    /// Sensible defaults; the directory name becomes the slug.
    pub fn defaults(slug: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            core: None,
            mc_version: None,
            port: "25565".into(),
            pvp: true,
            restart_policy: RestartPolicy::ManualOnly,
            restart_cron: String::new(),
            backup_auto: false,
            backup_rule: "1d".into(),
            backup_limit: "10".into(),
            addon_source: AddonSourceId::Modrinth,
            addon_autocheck: false,
            whitelist_enabled: false,
            whitelist: String::new(),
            ops: String::new(),
            eula_accepted: false,
        }
    }

    /// Split a comma separated list.
    pub fn split_names(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Validate everything and produce project metadata.
    pub fn into_project_meta(self) -> Result<ProjectMeta, CoreError> {
        let core = self
            .core
            .ok_or_else(|| CoreError::invalid_config("core", "no server core selected"))?;
        let mc_version = self.mc_version.ok_or_else(|| {
            CoreError::invalid_config("mc-version", "no Minecraft version selected")
        })?;
        let port: u16 = self.port.trim().parse().map_err(|_| {
            CoreError::invalid_config("port", format!("`{}` is not a valid port", self.port))
        })?;
        if port == 0 {
            return Err(CoreError::invalid_config(
                "port",
                "must be between 1 and 65535",
            ));
        }
        let backup_rule = ScheduleRule::parse(&self.backup_rule)
            .map_err(|e| CoreError::invalid_config("auto-backup-rule", e.to_string()))?
            .normalize();
        let backup_limit: u32 =
            self.backup_limit.trim().parse().map_err(|_| {
                CoreError::invalid_config("auto-backup-limit", "must be a number ≥ 1")
            })?;
        if backup_limit == 0 {
            return Err(CoreError::invalid_config(
                "auto-backup-limit",
                "must be at least 1",
            ));
        }
        let restart_cron = if self.restart_policy == RestartPolicy::Cron {
            let rule = ScheduleRule::parse(&self.restart_cron)
                .map_err(|e| CoreError::invalid_config("restart-cron", e.to_string()))?;
            match rule {
                ScheduleRule::Cron(expr) => expr,
                ScheduleRule::Fixed(_) => {
                    return Err(CoreError::invalid_config(
                        "restart-cron",
                        "restart schedule must be a cron expression",
                    ));
                }
            }
        } else {
            String::new()
        };

        // Installer cores pick their build at provisioning time.
        let core_build = if core.uses_installer() {
            String::new()
        } else {
            "latest".to_string()
        };

        let mut meta = ProjectMeta::default();
        meta.project.slug = self.slug;
        meta.project.created_at = Some(chrono::Utc::now());
        meta.server.core = core;
        meta.server.mc_version = mc_version;
        meta.server.core_build = core_build;
        meta.server.eula_accepted = self.eula_accepted;
        meta.server.whitelist = if self.whitelist_enabled {
            Self::split_names(&self.whitelist)
        } else {
            Vec::new()
        };
        meta.server.ops = Self::split_names(&self.ops);
        meta.runtime.port = port;
        meta.runtime.pvp = self.pvp;
        meta.runtime.restart_policy = self.restart_policy;
        meta.runtime.restart_cron = restart_cron;
        meta.backup.auto = self.backup_auto;
        meta.backup.rule = backup_rule;
        meta.backup.limit = backup_limit;
        meta.addons.default_source = self.addon_source;
        meta.addons.autocheck = self.addon_autocheck;
        meta.validate()?;
        Ok(meta)
    }

    /// Addon kinds that make sense to mention in the summary for this core.
    pub fn compatible_kinds(core: CoreKind) -> Vec<AddonKind> {
        AddonKind::ALL
            .into_iter()
            .filter(|k| core.supports_addon(*k))
            .collect()
    }
}

pub struct WizardFlow {
    pub steps: Vec<Box<dyn Step>>,
    pub index: usize,
    pub answers: WizardAnswers,
    pub error: Option<String>,
}

impl WizardFlow {
    pub fn new(answers: WizardAnswers) -> Self {
        Self {
            steps: steps::all(),
            index: 0,
            answers,
            error: None,
        }
    }

    /// Forward a key to the current step (disjoint borrows of steps/answers).
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Transition {
        let answers = &mut self.answers;
        self.steps[self.index].on_key(key, answers)
    }

    pub fn handle_fetch(&mut self, payload: FetchPayload) {
        let answers = &mut self.answers;
        self.steps[self.index].on_fetch(payload, answers);
    }

    pub fn handle_fetch_failed(&mut self, err: &str) {
        self.steps[self.index].on_fetch_failed(err);
    }

    pub fn current(&self) -> &dyn Step {
        self.steps[self.index].as_ref()
    }

    pub fn current_mut(&mut self) -> &mut dyn Step {
        self.steps[self.index].as_mut()
    }

    pub fn is_last(&self) -> bool {
        self.index == self.steps.len() - 1
    }
}

/// End result of the wizard.
#[derive(Debug, Clone, PartialEq)]
pub enum WizardOutcome {
    Answers(WizardAnswers),
    Aborted,
}

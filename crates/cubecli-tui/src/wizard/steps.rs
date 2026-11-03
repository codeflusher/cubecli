//! The individual wizard screens.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use cubecli_core::model::{AddonSourceId, CoreKind, RestartPolicy};
use cubecli_core::schedule::ScheduleRule;

use super::{FetchPayload, FetchRequest, Step, Transition, WizardAnswers};
use crate::theme;
use crate::widgets::{FilterList, Spinner, TextInput};

// ------------------------------------------------------------------------------------------------
// shared helpers

fn block(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .title(Span::styled(format!(" {title} "), theme::title()))
}

fn focus_style(focused: bool) -> Style {
    if focused {
        theme::selected()
    } else {
        Style::new()
    }
}

fn row(label: &str, value: Span<'static>, focused: bool, enabled: bool) -> Line<'static> {
    let label_style = if focused {
        theme::selected()
    } else if enabled {
        Style::new()
    } else {
        theme::hint()
    };
    Line::from(vec![
        Span::styled(format!(" {label:<18}"), label_style),
        Span::raw("  "),
        value,
    ])
}

fn text_span(s: &str, focused: bool) -> Span<'static> {
    Span::styled(
        s.to_string(),
        if focused {
            theme::value()
        } else {
            Style::new()
        },
    )
}

fn toggle(value: bool) -> Span<'static> {
    let (symbol, style) = if value {
        ("● on", Style::new().fg(theme::ACCENT))
    } else {
        ("○ off", theme::hint())
    };
    Span::styled(symbol, style)
}

fn choice(value: bool) -> Span<'static> {
    if value {
        Span::styled("● ", theme::value())
    } else {
        Span::styled("○ ", theme::hint())
    }
}

fn error_line(err: &str) -> Line<'static> {
    Line::styled(format!(" {err}"), theme::error())
}

/// Down/Tab move forward through form fields, Up/BackTab move back.
fn field_delta(key: &KeyEvent) -> Option<isize> {
    match key.code {
        KeyCode::Down | KeyCode::Tab => Some(1),
        KeyCode::Up | KeyCode::BackTab => Some(-1),
        _ => None,
    }
}

/// Left/Right direction for cycling choices; 0 when the key isn't one of them.
fn cycle(key: &KeyEvent) -> isize {
    match key.code {
        KeyCode::Right => 1,
        KeyCode::Left => -1,
        _ => 0,
    }
}

/// Move a field cursor within `len` fields.
fn move_field(field: usize, delta: isize, len: usize) -> usize {
    (field as isize + delta).rem_euclid(len as isize) as usize
}

// ------------------------------------------------------------------------------------------------
// 1. core kind

pub struct CoreKindStep {
    selected: usize,
}

impl Default for CoreKindStep {
    fn default() -> Self {
        Self { selected: 1 } // Paper preselected: the pragmatic default
    }
}

impl Step for CoreKindStep {
    fn title(&self) -> &'static str {
        "server core"
    }

    fn hints(&self) -> &'static str {
        "↑/↓ move · enter select · esc abort"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(CoreKind::ALL.len() - 1),
            KeyCode::Enter => {
                answers.core = Some(CoreKind::ALL[self.selected]);
                return Transition::Next;
            }
            _ => {}
        }
        Transition::Stay
    }

    fn render(&self, f: &mut Frame, area: Rect, _answers: &WizardAnswers, _spinner: &Spinner) {
        let items: Vec<ListItem> = CoreKind::ALL
            .iter()
            .map(|k| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {:<9}", k.display_name()),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(k.description(), theme::hint()),
                ]))
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.selected));
        f.render_stateful_widget(
            List::new(items)
                .block(block("which server core?"))
                .highlight_style(theme::selected()),
            area,
            &mut state,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// 2. mc version

pub struct McVersionStep {
    list: FilterList,
    loading: bool,
    error: Option<String>,
}

impl Default for McVersionStep {
    fn default() -> Self {
        Self {
            list: FilterList::default(),
            loading: false,
            error: None,
        }
    }
}

impl Step for McVersionStep {
    fn title(&self) -> &'static str {
        "minecraft version"
    }

    fn hints(&self) -> &'static str {
        "type to filter · ↑/↓ move · enter select · ⇧← back · esc abort"
    }

    fn enter_request(&self, answers: &WizardAnswers) -> Option<FetchRequest> {
        answers.core.map(FetchRequest::McVersions)
    }

    fn on_fetch(&mut self, payload: FetchPayload, _answers: &mut WizardAnswers) {
        self.loading = false;
        match payload {
            FetchPayload::McVersions(versions) => {
                self.list
                    .set_items(versions.into_iter().map(|v| v.id).collect());
            }
        }
    }

    fn on_fetch_failed(&mut self, err: &str) {
        self.loading = false;
        self.error = Some(err.to_string());
    }

    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        match key.code {
            KeyCode::Enter => {
                if self.loading {
                    return Transition::Stay;
                }
                if let Some(version) = self.list.selected_item().map(str::to_string) {
                    answers.mc_version = Some(version);
                    // Vanilla/fabric have one implicit build — skip the build screen.
                    return match answers.core {
                        Some(c)
                            if matches!(
                                c,
                                CoreKind::Paper
                                    | CoreKind::Purpur
                                    | CoreKind::NeoForge
                                    | CoreKind::Forge
                            ) =>
                        {
                            Transition::Next
                        }
                        _ => Transition::Skip(1),
                    };
                }
                Transition::Stay
            }
            _ => {
                self.list.on_key(key);
                Transition::Stay
            }
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, spinner: &Spinner) {
        let title = format!(
            "minecraft version for {}",
            answers.core.map(|c| c.display_name()).unwrap_or("?")
        );
        let inner = block(&title).inner(area);

        if self.loading {
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    spinner.span(theme::title()),
                    Span::raw(" fetching versions…"),
                ])),
                inner,
            );
            return;
        }
        if let Some(err) = &self.error {
            f.render_widget(
                Paragraph::new(vec![
                    error_line(err),
                    Line::raw(""),
                    Line::styled("⇧← go back and pick a different core", theme::hint()),
                ]),
                inner,
            );
            return;
        }

        let mut top = 0u16;
        if !self.list.filter().is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(
                    format!(" filter: {}", self.list.filter()),
                    theme::value(),
                )),
                inner,
            );
            top = 1;
        }
        let list_area = Rect::new(
            inner.x,
            inner.y + top,
            inner.width,
            inner.height.saturating_sub(top),
        );
        let items: Vec<ListItem> = self
            .list
            .visible()
            .into_iter()
            .map(|(_, id)| ListItem::new(Line::from(format!(" {id}"))))
            .collect();
        let mut state = ListState::default();
        state.select(self.list.selected_index());
        f.render_stateful_widget(
            List::new(items)
                .block(block("pick a version (newest first)"))
                .highlight_style(theme::selected()),
            list_area,
            &mut state,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// 3. core build (resolved at provision time for installer cores)

pub struct CoreBuildStep;

impl Default for CoreBuildStep {
    fn default() -> Self {
        Self
    }
}

impl Step for CoreBuildStep {
    fn title(&self) -> &'static str {
        "core build"
    }

    fn hints(&self) -> &'static str {
        "enter continue · ⇧← back · esc abort"
    }

    fn on_key(&mut self, key: KeyEvent, _answers: &mut WizardAnswers) -> Transition {
        if key.code == KeyCode::Enter {
            Transition::Next
        } else {
            Transition::Stay
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let label = match answers.core {
            Some(c) if c.uses_installer() => format!(
                "the latest {} build for {} is resolved during install",
                c.display_name(),
                answers.mc_version.as_deref().unwrap_or("?")
            ),
            _ => "the latest stable build is used".to_string(),
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(format!(" {label}")),
                Line::raw(""),
                Line::styled(" enter — continue", theme::hint()),
            ])
            .block(block("core build")),
            area,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// 4. network (port, pvp)

const NETWORK_FIELDS: usize = 2;

pub struct NetworkStep {
    field: usize,
    port: TextInput,
    error: Option<String>,
}

impl Default for NetworkStep {
    fn default() -> Self {
        Self {
            field: 0,
            port: TextInput::new("25565"),
            error: None,
        }
    }
}

impl Step for NetworkStep {
    fn title(&self) -> &'static str {
        "network"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        if let Some(delta) = field_delta(&key) {
            self.field = move_field(self.field, delta, NETWORK_FIELDS);
            self.error = None;
            return Transition::Stay;
        }
        match key.code {
            KeyCode::Enter => match self.port.value().trim().parse::<u16>() {
                Ok(p) if p > 0 => {
                    answers.port = self.port.value().trim().to_string();
                    Transition::Next
                }
                _ => {
                    self.error = Some("port must be a number between 1 and 65535".into());
                    Transition::Stay
                }
            },
            _ => {
                match self.field {
                    0 => self.port.on_key(key),
                    1 => {
                        if key.code == KeyCode::Char(' ') || cycle(&key) != 0 {
                            answers.pvp = !answers.pvp;
                        }
                    }
                    _ => {}
                }
                Transition::Stay
            }
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let mut lines = vec![
            row(
                "server port",
                text_span(&self.port.display(self.field == 0), self.field == 0),
                self.field == 0,
                true,
            ),
            row("pvp", toggle(answers.pvp), self.field == 1, true),
            Line::raw(""),
            Line::styled(
                " ↑/↓ field · space — toggle · enter — continue",
                theme::hint(),
            ),
        ];
        if let Some(err) = &self.error {
            lines.push(error_line(err));
        }
        f.render_widget(Paragraph::new(lines).block(block("network")), area);
    }
}

// ------------------------------------------------------------------------------------------------
// 5. policies (restart + backup)

const POLICY_CHOICES: [RestartPolicy; 3] = [
    RestartPolicy::ManualOnly,
    RestartPolicy::UnlessStopped,
    RestartPolicy::Cron,
];

#[derive(Clone, Copy, PartialEq)]
enum PolicyField {
    Restart,
    RestartCron,
    BackupAuto,
    BackupRule,
    BackupLimit,
}

pub struct PoliciesStep {
    field: usize,
    restart_cron: TextInput,
    backup_rule: TextInput,
    backup_limit: TextInput,
    error: Option<String>,
}

impl Default for PoliciesStep {
    fn default() -> Self {
        Self {
            field: 0,
            restart_cron: TextInput::default(),
            backup_rule: TextInput::new("1d"),
            backup_limit: TextInput::new("10"),
            error: None,
        }
    }
}

impl PoliciesStep {
    fn fields(&self, answers: &WizardAnswers) -> Vec<PolicyField> {
        let mut v = vec![PolicyField::Restart];
        if answers.restart_policy == RestartPolicy::Cron {
            v.push(PolicyField::RestartCron);
        }
        v.extend([
            PolicyField::BackupAuto,
            PolicyField::BackupRule,
            PolicyField::BackupLimit,
        ]);
        v
    }

    fn focus_input_mut(&mut self, field: PolicyField) -> Option<&mut TextInput> {
        match field {
            PolicyField::RestartCron => Some(&mut self.restart_cron),
            PolicyField::BackupRule => Some(&mut self.backup_rule),
            PolicyField::BackupLimit => Some(&mut self.backup_limit),
            _ => None,
        }
    }
}

impl Step for PoliciesStep {
    fn title(&self) -> &'static str {
        "policies"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        let fields = self.fields(answers);
        let current = fields[self.field.min(fields.len() - 1)];

        if let Some(delta) = field_delta(&key) {
            self.field = move_field(self.field, delta, fields.len());
            self.error = None;
            return Transition::Stay;
        }

        match current {
            PolicyField::Restart => {
                let d = cycle(&key);
                if d != 0 {
                    let selected = POLICY_CHOICES
                        .iter()
                        .position(|p| *p == answers.restart_policy)
                        .unwrap_or(0);
                    answers.restart_policy =
                        POLICY_CHOICES[move_field(selected, d, POLICY_CHOICES.len())];
                    // switching to/from cron reveals/hides the schedule field
                }
            }
            PolicyField::BackupAuto => {
                if key.code == KeyCode::Char(' ') || cycle(&key) != 0 {
                    answers.backup_auto = !answers.backup_auto;
                }
            }
            text_field => {
                if let Some(input) = self.focus_input_mut(text_field) {
                    input.on_key(key);
                }
            }
        }

        if key.code == KeyCode::Enter {
            if answers.restart_policy == RestartPolicy::Cron {
                match ScheduleRule::parse(self.restart_cron.value()) {
                    Ok(ScheduleRule::Cron(expr)) => answers.restart_cron = expr,
                    _ => {
                        self.error = Some(
                            "restart schedule must be a cron expression like `0 4 * * *`".into(),
                        );
                        return Transition::Stay;
                    }
                }
            }
            match ScheduleRule::parse(self.backup_rule.value()) {
                Ok(rule) => answers.backup_rule = rule.normalize(),
                Err(_) => {
                    self.error =
                        Some("backup rule must be `6h`, `1d6h`, or a cron expression".into());
                    return Transition::Stay;
                }
            }
            match self.backup_limit.value().trim().parse::<u32>() {
                Ok(limit) if limit > 0 => answers.backup_limit = limit.to_string(),
                _ => {
                    self.error = Some("backup limit must be a number ≥ 1".into());
                    return Transition::Stay;
                }
            }
            return Transition::Next;
        }
        Transition::Stay
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let fields = self.fields(answers);
        let focused = |field: PolicyField| fields.get(self.field) == Some(&field);

        let mut rows = vec![Line::from(Span::styled(
            " restart policy",
            focus_style(focused(PolicyField::Restart)),
        ))];
        let mut choices = vec![Span::raw(" ".repeat(21))];
        for p in POLICY_CHOICES {
            let on = answers.restart_policy == p;
            choices.push(choice(on));
            choices.push(Span::styled(
                format!("{:<16}", p.slug()),
                if on { theme::value() } else { theme::hint() },
            ));
        }
        rows.push(Line::from(choices));

        if fields.contains(&PolicyField::RestartCron) {
            rows.push(row(
                "restart schedule",
                text_span(
                    &self.restart_cron.display(focused(PolicyField::RestartCron)),
                    focused(PolicyField::RestartCron),
                ),
                focused(PolicyField::RestartCron),
                true,
            ));
        }
        rows.push(row(
            "auto backup",
            toggle(answers.backup_auto),
            focused(PolicyField::BackupAuto),
            true,
        ));
        rows.push(row(
            "backup rule",
            text_span(
                &self.backup_rule.display(focused(PolicyField::BackupRule)),
                focused(PolicyField::BackupRule),
            ),
            focused(PolicyField::BackupRule),
            true,
        ));
        rows.push(row(
            "backup limit",
            text_span(
                &self.backup_limit.display(focused(PolicyField::BackupLimit)),
                focused(PolicyField::BackupLimit),
            ),
            focused(PolicyField::BackupLimit),
            true,
        ));
        rows.push(Line::raw(""));
        rows.push(Line::styled(
            " ←/→ cycle policy · space — toggle · enter — continue",
            theme::hint(),
        ));
        if let Some(err) = &self.error {
            rows.push(error_line(err));
        }
        f.render_widget(
            Paragraph::new(rows).block(block("restart & backup policies")),
            area,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// 6. addons

const ADDON_FIELDS: usize = 2;

pub struct AddonsStep {
    field: usize,
}

impl Default for AddonsStep {
    fn default() -> Self {
        Self { field: 0 }
    }
}

impl Step for AddonsStep {
    fn title(&self) -> &'static str {
        "addons"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        if let Some(delta) = field_delta(&key) {
            self.field = move_field(self.field, delta, ADDON_FIELDS);
            return Transition::Stay;
        }
        match self.field {
            0 => {
                let d = cycle(&key);
                if d != 0 {
                    let cur = AddonSourceId::ALL
                        .iter()
                        .position(|s| *s == answers.addon_source)
                        .unwrap_or(0);
                    answers.addon_source =
                        AddonSourceId::ALL[move_field(cur, d, AddonSourceId::ALL.len())];
                }
            }
            1 => {
                if key.code == KeyCode::Char(' ') || cycle(&key) != 0 {
                    answers.addon_autocheck = !answers.addon_autocheck;
                }
            }
            _ => {}
        }
        if key.code == KeyCode::Enter {
            Transition::Next
        } else {
            Transition::Stay
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let mut choices = vec![Span::raw(" ".repeat(21))];
        for s in AddonSourceId::ALL {
            choices.push(choice(answers.addon_source == s));
            choices.push(Span::styled(
                format!("{:<12}", s.slug()),
                if answers.addon_source == s {
                    theme::value()
                } else {
                    theme::hint()
                },
            ));
        }
        let lines = vec![
            Line::from(vec![Span::styled(
                format!(" {:<18}", "default source"),
                focus_style(self.field == 0),
            )]),
            Line::from(choices),
            row(
                "check for updates",
                toggle(answers.addon_autocheck),
                self.field == 1,
                true,
            ),
            Line::raw(""),
            Line::styled(
                " install addons later with `cubecli project install mod:<slug>`",
                theme::hint(),
            ),
            Line::styled(
                " ←/→ cycle · space — toggle · enter — continue",
                theme::hint(),
            ),
        ];
        f.render_widget(Paragraph::new(lines).block(block("addons")), area);
    }
}

// ------------------------------------------------------------------------------------------------
// 7. whitelist & ops

const PEOPLE_FIELDS: usize = 3;

pub struct PeopleStep {
    field: usize,
    whitelist: TextInput,
    ops: TextInput,
}

impl Default for PeopleStep {
    fn default() -> Self {
        Self {
            field: 0,
            whitelist: TextInput::default(),
            ops: TextInput::default(),
        }
    }
}

impl Step for PeopleStep {
    fn title(&self) -> &'static str {
        "whitelist & ops"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        if let Some(delta) = field_delta(&key) {
            self.field = move_field(self.field, delta, PEOPLE_FIELDS);
            return Transition::Stay;
        }
        match self.field {
            0 => {
                if key.code == KeyCode::Char(' ') || cycle(&key) != 0 {
                    answers.whitelist_enabled = !answers.whitelist_enabled;
                }
            }
            1 => self.whitelist.on_key(key),
            2 => self.ops.on_key(key),
            _ => {}
        }
        if key.code == KeyCode::Enter {
            answers.whitelist = self.whitelist.value().to_string();
            answers.ops = self.ops.value().to_string();
            Transition::Next
        } else {
            Transition::Stay
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let lines = vec![
            row(
                "enable whitelist",
                toggle(answers.whitelist_enabled),
                self.field == 0,
                true,
            ),
            row(
                "players",
                text_span(&self.whitelist.display(self.field == 1), self.field == 1),
                self.field == 1,
                answers.whitelist_enabled,
            ),
            row(
                "operators (ops)",
                text_span(&self.ops.display(self.field == 2), self.field == 2),
                self.field == 2,
                true,
            ),
            Line::raw(""),
            Line::styled(
                " comma-separated player names · enter — continue",
                theme::hint(),
            ),
        ];
        f.render_widget(
            Paragraph::new(lines).block(block("whitelist & operators")),
            area,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// 8. eula

pub struct EulaStep {
    agreed: bool,
    error: Option<String>,
}

impl Default for EulaStep {
    fn default() -> Self {
        Self {
            agreed: false,
            error: None,
        }
    }
}

impl Step for EulaStep {
    fn title(&self) -> &'static str {
        "eula"
    }

    fn on_key(&mut self, key: KeyEvent, answers: &mut WizardAnswers) -> Transition {
        match key.code {
            KeyCode::Char(' ') => {
                self.agreed = !self.agreed;
                self.error = None;
            }
            KeyCode::Enter => {
                if self.agreed {
                    answers.eula_accepted = true;
                    return Transition::Next;
                }
                self.error = Some("you must accept the Minecraft EULA to run a server".into());
            }
            _ => {}
        }
        Transition::Stay
    }

    fn render(&self, f: &mut Frame, area: Rect, _answers: &WizardAnswers, _spinner: &Spinner) {
        let mut lines = vec![
            Line::from(" Running a Minecraft server requires accepting Mojang's EULA:"),
            Line::styled(" https://aka.ms/MinecraftEULA", theme::value()),
            Line::raw(""),
            Line::from(vec![
                Span::styled(
                    if self.agreed { "[x]" } else { "[ ]" },
                    Style::new().fg(if self.agreed {
                        theme::ACCENT
                    } else {
                        theme::MUTED
                    }),
                ),
                Span::raw(" I accept the Minecraft EULA"),
            ]),
            Line::raw(""),
            Line::styled(" space — toggle · enter — continue", theme::hint()),
        ];
        if let Some(err) = &self.error {
            lines.push(error_line(err));
        }
        f.render_widget(Paragraph::new(lines).block(block("minecraft eula")), area);
    }
}

// ------------------------------------------------------------------------------------------------
// 9. summary

pub struct SummaryStep;

impl Default for SummaryStep {
    fn default() -> Self {
        Self
    }
}

impl Step for SummaryStep {
    fn title(&self) -> &'static str {
        "summary"
    }

    fn hints(&self) -> &'static str {
        "enter — create the project · ⇧← back · esc abort"
    }

    fn on_key(&mut self, key: KeyEvent, _answers: &mut WizardAnswers) -> Transition {
        if key.code == KeyCode::Enter {
            Transition::Finish
        } else {
            Transition::Stay
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, answers: &WizardAnswers, _spinner: &Spinner) {
        let mut rows = vec![
            kv("server folder", &answers.slug),
            kv(
                "core",
                answers
                    .core
                    .map(|c| c.display_name().to_string())
                    .unwrap_or_else(|| "—".into()),
            ),
            kv(
                "mc version",
                answers.mc_version.clone().unwrap_or_else(|| "—".into()),
            ),
            kv("port", &answers.port),
            kv("pvp", on_off(answers.pvp)),
            kv("restart policy", answers.restart_policy.slug()),
        ];
        if answers.restart_policy == RestartPolicy::Cron {
            rows.push(kv("restart schedule", &answers.restart_cron));
        }
        rows.push(kv(
            "auto backup",
            format!(
                "{} (every {}, keep {})",
                on_off(answers.backup_auto),
                answers.backup_rule,
                answers.backup_limit
            ),
        ));
        rows.push(kv("addon source", answers.addon_source.slug()));
        if answers.addon_autocheck {
            rows.push(kv("update checks", "on"));
        }
        if answers.whitelist_enabled && !WizardAnswers::split_names(&answers.whitelist).is_empty() {
            rows.push(kv("whitelist", &answers.whitelist));
        }
        if !WizardAnswers::split_names(&answers.ops).is_empty() {
            rows.push(kv("operators", &answers.ops));
        }
        rows.push(kv("eula", on_off(answers.eula_accepted)));
        if !answers.eula_accepted {
            rows.push(error_line(
                "EULA not accepted — `project start` will refuse until it is",
            ));
        }
        rows.push(Line::raw(""));
        rows.push(Line::styled(
            " enter — create the project (core download happens on first start)",
            theme::hint(),
        ));
        f.render_widget(Paragraph::new(rows).block(block("review & confirm")), area);
    }
}

fn kv(key: &str, value: impl std::fmt::Display) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {key:<18}"), theme::hint()),
        Span::raw("  "),
        Span::styled(value.to_string(), theme::value()),
    ])
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

/// Step names for the breadcrumb bar.
pub fn breadcrumb_names() -> &'static [&'static str] {
    &[
        "core", "version", "build", "network", "policies", "addons", "people", "eula", "summary",
    ]
}

pub fn all() -> Vec<Box<dyn Step>> {
    vec![
        Box::new(CoreKindStep::default()),
        Box::new(McVersionStep::default()),
        Box::new(CoreBuildStep::default()),
        Box::new(NetworkStep::default()),
        Box::new(PoliciesStep::default()),
        Box::new(AddonsStep::default()),
        Box::new(PeopleStep::default()),
        Box::new(EulaStep::default()),
        Box::new(SummaryStep::default()),
    ]
}

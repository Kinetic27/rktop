use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::config::{
    AppConfig, ConfigOrigin, LoadedConfig, ServerConfig, config_to_toml, default_config_path,
    enabled_servers, load_config, load_config_file, write_example_config,
};
use crate::model::{
    CpuMetrics, Freshness, HostMetrics, HostSource, HostStatus, NetworkMetrics, RamMetrics,
    StorageMetrics,
};
use crate::render::{self, Dashboard};
use crate::theme::Status;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, TimeZone, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

const HISTORY_LEN: usize = 96;
const CONFIG_HEALTH_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const OPTIONAL_REFRESH_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Mock,
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandMode {
    Run,
    Init { force: bool, print: bool },
    ConfigManager,
    Edit,
    Doctor,
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub command: CommandMode,
    pub mode: Mode,
    pub snapshot: bool,
    pub once: bool,
    pub config_path: Option<PathBuf>,
}

impl Cli {
    pub fn parse<I>(args: I) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        let mut command = CommandMode::Run;
        let mut mode = Mode::Live;
        let mut snapshot = false;
        let mut once = false;
        let mut config_path = None;
        let mut init_force = false;
        let mut init_print = false;
        let mut command_seen = false;

        let mut args = args.into_iter().map(Into::into).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "init" => {
                    if command_seen {
                        bail!("only one command can be used at a time");
                    }
                    command_seen = true;
                    command = CommandMode::Init {
                        force: false,
                        print: false,
                    }
                }
                "setup" | "config" => {
                    if command_seen {
                        bail!("only one command can be used at a time");
                    }
                    command_seen = true;
                    command = CommandMode::ConfigManager;
                }
                "edit" => {
                    if command_seen {
                        bail!("only one command can be used at a time");
                    }
                    command_seen = true;
                    command = CommandMode::Edit;
                }
                "doctor" => {
                    if command_seen {
                        bail!("only one command can be used at a time");
                    }
                    command_seen = true;
                    command = CommandMode::Doctor;
                }
                "--mock" => mode = Mode::Mock,
                "--live" => mode = Mode::Live,
                "--snapshot" => snapshot = true,
                "--once" => once = true,
                "--config" => {
                    let path = args
                        .next()
                        .ok_or_else(|| anyhow!("--config requires a path"))?;
                    config_path = Some(PathBuf::from(path));
                }
                "--force" => init_force = true,
                "--print" => init_print = true,
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => bail!("unknown argument: {other}"),
            }
        }

        if !matches!(command, CommandMode::Init { .. }) && (init_force || init_print) {
            bail!("--force/--print are only valid with `init`");
        }
        if let CommandMode::Init { .. } = command {
            command = CommandMode::Init {
                force: init_force,
                print: init_print,
            };
        }

        Ok(Self {
            command,
            mode,
            snapshot,
            once,
            config_path,
        })
    }
}

fn print_help() {
    println!(
        "rktop\n\nUSAGE:\n  rktop [--mock|--live] [--snapshot] [--once] [--config PATH]\n  rktop init [--config PATH] [--force|--print]\n  rktop setup [--config PATH]   # full-screen config manager\n  rktop config [--config PATH]\n  rktop edit [--config PATH]    # raw $EDITOR fallback\n  rktop doctor [--config PATH]\n\nDefaults to the live TUI, like htop/btop.\nConfig lookup: --config, $RKTOP_CONFIG, ./config.toml beside the executable, ~/.config/rktop/config.toml, /etc/rktop/config.toml\n\nFIRST RUN:\n  rktop config    # create config and open the full-screen setup manager\n  rktop doctor\n  rktop\n\nCONFIG:\n  rktop config  open the full-screen Ratatui config manager\n  rktop setup   alias for config\n  rktop init    create an empty user config without opening the TUI\n  rktop edit    open raw config in $VISUAL/$EDITOR\n  rktop doctor  validate config and SSH key auth\n\nKEYS:\n  +/- or =  adjust refresh by 100ms\n  q / ctrl-c / esc exits"
    );
}

#[derive(Debug, Clone)]
pub struct HostSnapshot {
    pub id: String,
    pub name: String,
    pub group: String,
    pub role: String,
    pub status: Status,
    pub cpu_percent: u16,
    pub ram_percent: u16,
    pub storage_percent: u16,
    pub cpu_history: Vec<u16>,
    pub ram_history: Vec<u16>,
    pub storage_history: Vec<u16>,
    pub net_history: Vec<u16>,
    pub net_rx_bytes_per_sec: Option<f64>,
    pub net_tx_bytes_per_sec: Option<f64>,
    pub net_rx_total_bytes: Option<u64>,
    pub net_tx_total_bytes: Option<u64>,
    pub last_seen: DateTime<Utc>,
    pub hostname: Option<String>,
    pub kernel: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub cpu_cores: Option<u16>,
    pub cpu_temperature_celsius: Option<f32>,
    pub load_1m: Option<f32>,
    pub load_5m: Option<f32>,
    pub load_15m: Option<f32>,
    pub ram_used_kib: Option<u64>,
    pub ram_total_kib: Option<u64>,
    pub storage_used_kib: Option<u64>,
    pub storage_total_kib: Option<u64>,
    pub disks: Vec<DiskSnapshot>,
}

#[derive(Debug, Clone)]
pub struct DiskSnapshot {
    pub mount: String,
    pub used_kib: u64,
    pub total_kib: u64,
    pub percent: u16,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub title: String,
    pub generated_at: DateTime<Utc>,
    pub mode: Mode,
    pub refresh_interval_ms: u64,
    pub hosts: Vec<HostSnapshot>,
}

#[derive(Debug, Clone)]
struct TuiState {
    dashboard: AppState,
    refresh_interval: Duration,
    last_refresh: Instant,
    force_refresh: bool,
    refresh_in_flight: bool,
    optional_retry_after: HashMap<String, Instant>,
}

impl TuiState {
    fn new(dashboard: AppState, refresh_interval: Duration) -> Self {
        // Contract: let mut force_refresh = false;
        let force_refresh = false;
        Self {
            dashboard,
            refresh_interval,
            last_refresh: Instant::now(),
            force_refresh,
            refresh_in_flight: false,
            optional_retry_after: HashMap::new(),
        }
    }

    fn refresh_poll_timeout(&self, mode: Mode) -> Duration {
        refresh_poll_timeout(
            mode,
            self.force_refresh,
            self.refresh_in_flight,
            self.last_refresh,
            self.refresh_interval,
        )
    }

    fn should_refresh(&self, mode: Mode) -> bool {
        mode == Mode::Live
            && !self.refresh_in_flight
            && (self.force_refresh || self.last_refresh.elapsed() >= self.refresh_interval)
    }

    fn replace_dashboard_after_refresh(&mut self, dashboard: AppState) {
        self.dashboard = dashboard;
        self.force_refresh = false;
        self.refresh_in_flight = false;
    }

    fn skipped_optional_ids(&mut self) -> HashSet<String> {
        let now = Instant::now();
        self.optional_retry_after
            .retain(|_, retry_at| *retry_at > now);
        self.optional_retry_after.keys().cloned().collect()
    }

    fn note_optional_failures(&mut self, ids: Vec<String>) {
        let retry_at = Instant::now() + OPTIONAL_REFRESH_RETRY_AFTER;
        for id in ids {
            self.optional_retry_after.insert(id, retry_at);
        }
    }
}

#[derive(Debug)]
struct DashboardRefresh {
    result: Result<DashboardBuild>,
}

#[derive(Debug)]
struct DashboardBuild {
    dashboard: AppState,
    optional_failures: Vec<String>,
}

struct LiveCollection {
    hosts: Vec<HostSnapshot>,
    optional_failures: Vec<String>,
}

struct EnabledCollection {
    collected: Vec<(crate::config::ServerConfig, HostMetrics)>,
    optional_failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiAction {
    Exit,
    AdjustRefresh(i64),
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiControl {
    Continue,
    Exit,
}

fn action_from_key(key: KeyEvent) -> TuiAction {
    if key.kind != KeyEventKind::Press {
        return TuiAction::Ignore;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => TuiAction::Exit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => TuiAction::Exit,
        KeyCode::Char('+') | KeyCode::Char('=') => TuiAction::AdjustRefresh(100),
        KeyCode::Char('-') => TuiAction::AdjustRefresh(-100),
        _ => TuiAction::Ignore,
    }
}

fn reduce_tui_state(state: &mut TuiState, action: TuiAction) -> TuiControl {
    match action {
        TuiAction::Exit => TuiControl::Exit,
        TuiAction::AdjustRefresh(delta_ms) => {
            state.refresh_interval = adjust_refresh_interval(state.refresh_interval, delta_ms);
            state.dashboard.refresh_interval_ms = state.refresh_interval.as_millis() as u64;
            state.force_refresh = true;
            TuiControl::Continue
        }
        TuiAction::Ignore => TuiControl::Continue,
    }
}

pub fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        CommandMode::Run => {
            let loaded = load_config(cli.config_path.as_deref())?;
            run_dashboard(cli, loaded.config)
        }
        CommandMode::Init { force, print } => {
            init_config(cli.config_path.as_deref(), *force, *print)
        }
        CommandMode::ConfigManager => run_config_manager(cli.config_path.as_deref()),
        CommandMode::Edit => edit_config(cli.config_path.as_deref()),
        CommandMode::Doctor => doctor(cli.config_path.as_deref()),
    }
}

fn run_dashboard(cli: Cli, config: AppConfig) -> Result<()> {
    if cli.snapshot || (cli.once && !is_terminal()) {
        let state = build_snapshot_state(cli.mode, &config)?;
        println!("{}", render::snapshot_text(&state));
        return Ok(());
    }

    run_tui(cli.mode, cli.once, config)
}

fn init_config(path: Option<&Path>, force: bool, print: bool) -> Result<()> {
    if print {
        print!("{}", crate::config::default_config_toml());
        return Ok(());
    }

    let path = config_path_or_default(path)?;
    write_example_config(&path, force)?;
    println!("created config: {}", path.display());
    println!("next:");
    println!("  1. run: rktop config");
    println!("  2. run: rktop doctor");
    println!("  3. run: rktop");
    Ok(())
}

fn run_config_manager(path: Option<&Path>) -> Result<()> {
    let path = config_path_or_default(path)?;
    if !path.exists() {
        write_example_config(&path, false)?;
    }

    let config = load_config_file(&path)?;
    if !is_terminal() {
        fs_write_config(&path, &config)?;
        println!(
            "config manager requires an interactive terminal; validated config: {}",
            path.display()
        );
        println!(
            "config ok: {} enabled / {} total servers, refresh {}ms",
            enabled_servers(&config).count(),
            config.servers.len(),
            config.refresh_interval_ms
        );
        print_ssh_setup_guidance(&config);
        return Ok(());
    }

    run_config_tui(&path, config)
}

fn fs_write_config(path: &Path, config: &AppConfig) -> Result<()> {
    std::fs::write(path, config_to_toml(config))
        .with_context(|| format!("failed to write config file {}", path.display()))
}

fn run_config_tui(path: &Path, config: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut state = ConfigManagerState::new(config, path.to_path_buf());

    let result = run_config_tui_loop(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_config_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut ConfigManagerState,
) -> Result<()> {
    let mut last_health_check = Instant::now();
    let mut health_rx = None;
    start_config_health_check(state, &mut health_rx);
    loop {
        poll_config_health_check(state, &mut health_rx);
        terminal.draw(|frame| draw_config_manager(frame, state))?;
        if matches!(state.control, ConfigManagerControl::Exit) {
            return Ok(());
        }

        if last_health_check.elapsed() >= CONFIG_HEALTH_REFRESH_INTERVAL {
            start_config_health_check(state, &mut health_rx);
            last_health_check = Instant::now();
            continue;
        }

        let timeout = CONFIG_HEALTH_REFRESH_INTERVAL.saturating_sub(last_health_check.elapsed());
        if !event::poll(timeout.min(Duration::from_millis(250)))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let control = handle_config_key(state, key)?;
            match control {
                ConfigManagerControl::Continue => {
                    state.control = ConfigManagerControl::Continue;
                }
                ConfigManagerControl::Exit => {
                    state.control = ConfigManagerControl::Exit;
                }
            }
            if state.health_refresh_requested {
                start_config_health_check(state, &mut health_rx);
                state.health_refresh_requested = false;
                last_health_check = Instant::now();
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigManagerControl {
    Continue,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingConfirmation {
    Delete,
    GenerateSshKey,
    ExitUnsaved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigEditField {
    Id,
    Name,
    Source,
    Host,
    Group,
    Role,
    Enabled,
    Optional,
    DiskMaxRows,
    DiskAliases,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigHealthStatus {
    Ok(String),
    Failed(String),
    Pending(String),
}

impl ConfigHealthStatus {
    fn label(&self) -> String {
        match self {
            Self::Ok(label) => format!("✓ {label}"),
            Self::Failed(label) => format!("✗ {label}"),
            Self::Pending(label) => format!("… {label}"),
        }
    }

    fn style(&self) -> Style {
        match self {
            Self::Ok(_) => Style::default().fg(Color::Green),
            Self::Failed(_) => Style::default().fg(Color::Red),
            Self::Pending(_) => Style::default().fg(Color::Yellow),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SshConfigEntry {
    alias: String,
    hostname: Option<String>,
    user: Option<String>,
}

impl SshConfigEntry {
    fn detail(&self) -> String {
        match (&self.user, &self.hostname) {
            (Some(user), Some(hostname)) => format!("{user}@{hostname}"),
            (None, Some(hostname)) => hostname.clone(),
            _ => "ssh config host".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddServerState {
    hosts: Vec<SshConfigEntry>,
    selected: usize,
    direct_input: bool,
    input: String,
}

impl AddServerState {
    fn option_count(&self) -> usize {
        self.hosts.len() + self.host_option_offset()
    }

    fn local_option_available(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn host_option_offset(&self) -> usize {
        if self.local_option_available() { 2 } else { 1 }
    }

    fn selected_host(&self) -> Option<&SshConfigEntry> {
        self.selected
            .checked_sub(self.host_option_offset())
            .and_then(|idx| self.hosts.get(idx))
    }
}

const CONFIG_EDIT_FIELDS: [ConfigEditField; 10] = [
    ConfigEditField::Id,
    ConfigEditField::Name,
    ConfigEditField::Source,
    ConfigEditField::Host,
    ConfigEditField::Group,
    ConfigEditField::Role,
    ConfigEditField::Enabled,
    ConfigEditField::Optional,
    ConfigEditField::DiskMaxRows,
    ConfigEditField::DiskAliases,
];

impl ConfigEditField {
    fn label(self) -> &'static str {
        match self {
            ConfigEditField::Id => "id",
            ConfigEditField::Name => "name",
            ConfigEditField::Source => "source",
            ConfigEditField::Host => "host",
            ConfigEditField::Group => "group",
            ConfigEditField::Role => "role",
            ConfigEditField::Enabled => "enabled",
            ConfigEditField::Optional => "optional",
            ConfigEditField::DiskMaxRows => "disk_max_rows",
            ConfigEditField::DiskAliases => "disk_aliases",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            ConfigEditField::Id => "stable config id",
            ConfigEditField::Name => "display name",
            ConfigEditField::Source => "ssh/local/proxmox/truenas-scale",
            ConfigEditField::Host => "SSH/API target; blank not allowed for ssh-like sources",
            ConfigEditField::Group => "optional group label; blank clears",
            ConfigEditField::Role => "optional role label; blank clears",
            ConfigEditField::Enabled => "space toggles",
            ConfigEditField::Optional => "space toggles; unreachable optional hosts are hidden",
            ConfigEditField::DiskMaxRows => "blank clears; positive integer limits disk rows",
            ConfigEditField::DiskAliases => "/path:label,/path2:label2",
        }
    }

    fn is_toggle(self) -> bool {
        matches!(self, ConfigEditField::Enabled | ConfigEditField::Optional)
    }

    fn current_value(self, server: &ServerConfig) -> String {
        match self {
            ConfigEditField::Id => server.id.clone(),
            ConfigEditField::Name => server.name.clone(),
            ConfigEditField::Source => server.source.kind().to_string(),
            ConfigEditField::Host => server.source.endpoint().unwrap_or("").to_string(),
            ConfigEditField::Group => server.group.clone().unwrap_or_default(),
            ConfigEditField::Role => server.role.clone().unwrap_or_default(),
            ConfigEditField::Enabled => server.enabled.to_string(),
            ConfigEditField::Optional => server.optional.to_string(),
            ConfigEditField::DiskMaxRows => server
                .disk_max_rows
                .map(|rows| rows.to_string())
                .unwrap_or_default(),
            ConfigEditField::DiskAliases => server
                .disk_aliases
                .iter()
                .map(|(mount, alias)| format!("{mount}:{alias}"))
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

#[derive(Debug, Clone)]
struct ConfigManagerState {
    config: AppConfig,
    saved_config: AppConfig,
    path: PathBuf,
    selected: usize,
    input: String,
    adding: Option<AddServerState>,
    editing: bool,
    edit_selected: usize,
    edit_value_field: Option<ConfigEditField>,
    health: HashMap<String, ConfigHealthStatus>,
    health_issues: HashMap<String, String>,
    health_copy_id_hosts: HashMap<String, String>,
    health_refresh_requested: bool,
    pending_confirmation: Option<PendingConfirmation>,
    control: ConfigManagerControl,
    message: String,
    health_message: String,
}

impl ConfigManagerState {
    fn new(config: AppConfig, path: PathBuf) -> Self {
        Self {
            saved_config: config.clone(),
            config,
            path,
            selected: 0,
            input: String::new(),
            adding: None,
            editing: false,
            edit_selected: 0,
            edit_value_field: None,
            health: HashMap::new(),
            health_issues: HashMap::new(),
            health_copy_id_hosts: HashMap::new(),
            health_refresh_requested: false,
            pending_confirmation: None,
            control: ConfigManagerControl::Continue,
            message: String::new(),
            health_message: String::new(),
        }
    }

    fn selected_server(&self) -> Option<&ServerConfig> {
        self.config.servers.get(self.selected)
    }

    fn selected_server_mut(&mut self) -> Option<&mut ServerConfig> {
        self.config.servers.get_mut(self.selected)
    }

    fn clamp_selection(&mut self) {
        if self.config.servers.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.config.servers.len() - 1);
        }
    }

    fn is_dirty(&self) -> bool {
        self.config != self.saved_config
    }
}

fn handle_config_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
) -> Result<ConfigManagerControl> {
    if state.adding.is_some() {
        return handle_add_server_key(state, key);
    }
    if state.editing {
        return handle_config_input_key(state, key);
    }
    if let Some(confirmation) = state.pending_confirmation.clone() {
        return handle_config_confirmation_key(state, key, confirmation);
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => request_config_exit(state),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            request_config_exit(state)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            select_next_server(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            select_previous_server(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('a') => {
            enter_add_server_wizard(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('d') => {
            state.pending_confirmation = Some(PendingConfirmation::Delete);
            state.message = "Delete selected server?".to_string();
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char(' ') => {
            toggle_selected_enabled(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('o') => {
            toggle_selected_optional(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('<') => {
            move_selected_server_up(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('>') => {
            move_selected_server_down(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('e') | KeyCode::Char('i') | KeyCode::Enter => {
            enter_config_field_editor(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('s') => {
            save_config_manager_state(state)?;
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('r') => {
            run_selected_ssh_probe(state);
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('h') => {
            state.health_refresh_requested = true;
            state.message = "health check queued".to_string();
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('g') => {
            state.pending_confirmation = Some(PendingConfirmation::GenerateSshKey);
            state.message = "Show ssh-keygen command? No command is run yet.".to_string();
            Ok(ConfigManagerControl::Continue)
        }
        KeyCode::Char('c') => {
            show_selected_copy_id_command(state);
            Ok(ConfigManagerControl::Continue)
        }
        _ => Ok(ConfigManagerControl::Continue),
    }
}

fn handle_add_server_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
) -> Result<ConfigManagerControl> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return request_config_exit(state);
    }
    let Some(add_state) = state.adding.as_mut() else {
        return Ok(ConfigManagerControl::Continue);
    };

    if add_state.direct_input {
        match key.code {
            KeyCode::Esc => {
                add_state.direct_input = false;
                add_state.input.clear();
                state.message =
                    "add server: choose an SSH config host, direct target, or local".to_string();
            }
            KeyCode::Enter => {
                let target = add_state.input.trim().to_string();
                if target.is_empty() {
                    state.message = "direct SSH target is empty".to_string();
                    return Ok(ConfigManagerControl::Continue);
                }
                if let Err(error) = crate::collectors::ssh::validate_ssh_host(&target) {
                    state.message = format!("invalid SSH target: {error}");
                    return Ok(ConfigManagerControl::Continue);
                }
                state.adding = None;
                add_ssh_target_server(state, &target, &target);
            }
            KeyCode::Backspace => {
                add_state.input.pop();
            }
            KeyCode::Char(ch) => {
                add_state.input.push(ch);
            }
            _ => {}
        }
        return Ok(ConfigManagerControl::Continue);
    }

    match key.code {
        KeyCode::Esc => {
            state.adding = None;
            state.message = "add server cancelled".to_string();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            add_state.selected = (add_state.selected + 1).min(add_state.option_count() - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            add_state.selected = add_state.selected.saturating_sub(1);
        }
        KeyCode::Enter => match add_state.selected {
            0 => {
                add_state.direct_input = true;
                add_state.input.clear();
                state.message = "type SSH target, e.g. user@example.com, then Enter".to_string();
            }
            1 if add_state.local_option_available() => {
                state.adding = None;
                add_local_server(state);
            }
            _ => {
                if let Some(host) = add_state.selected_host().cloned() {
                    state.adding = None;
                    add_ssh_target_server(state, &host.alias, &host.alias);
                }
            }
        },
        _ => {}
    }

    Ok(ConfigManagerControl::Continue)
}

fn handle_config_input_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
) -> Result<ConfigManagerControl> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return request_config_exit(state);
    }
    if let Some(field) = state.edit_value_field {
        return handle_config_value_input_key(state, key, field);
    }

    handle_config_field_select_key(state, key)
}

fn handle_config_field_select_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
) -> Result<ConfigManagerControl> {
    match key.code {
        KeyCode::Esc => {
            state.editing = false;
            state.edit_value_field = None;
            state.input.clear();
            state.message.clear();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.edit_selected = (state.edit_selected + 1).min(CONFIG_EDIT_FIELDS.len() - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.edit_selected = state.edit_selected.saturating_sub(1);
        }
        KeyCode::Char(' ') => edit_selected_field_or_toggle(state),
        KeyCode::Char('e') | KeyCode::Char('i') | KeyCode::Enter => {
            edit_selected_field_or_toggle(state);
        }
        KeyCode::Char('s') => {
            save_config_manager_state(state)?;
        }
        _ => {}
    }
    Ok(ConfigManagerControl::Continue)
}

fn handle_config_value_input_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
    field: ConfigEditField,
) -> Result<ConfigManagerControl> {
    match key.code {
        KeyCode::Esc => {
            state.edit_value_field = None;
            state.input.clear();
            state.message.clear();
        }
        KeyCode::Enter => {
            let assignment = format!("{}={}", field.label(), state.input.trim());
            match apply_server_assignment(state, &assignment) {
                Ok(()) => {
                    state.edit_value_field = None;
                    state.input.clear();
                }
                Err(error) => {
                    state.message = format!("edit error: {error}");
                }
            }
        }
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::Char(ch) => {
            state.input.push(ch);
        }
        _ => {}
    }
    Ok(ConfigManagerControl::Continue)
}

fn enter_config_field_editor(state: &mut ConfigManagerState) {
    if state.config.servers.is_empty() {
        state.message = "add a server first".to_string();
        return;
    }
    state.editing = true;
    state.edit_value_field = None;
    state.input.clear();
    state.message = "editing server fields".to_string();
}

fn selected_edit_field(state: &ConfigManagerState) -> ConfigEditField {
    CONFIG_EDIT_FIELDS[state.edit_selected.min(CONFIG_EDIT_FIELDS.len() - 1)]
}

fn edit_selected_field_or_toggle(state: &mut ConfigManagerState) {
    let field = selected_edit_field(state);
    if field.is_toggle() {
        let assignment = match field {
            ConfigEditField::Enabled => state
                .selected_server()
                .map(|server| format!("enabled={}", !server.enabled)),
            ConfigEditField::Optional => state
                .selected_server()
                .map(|server| format!("optional={}", !server.optional)),
            _ => None,
        };
        if let Some(assignment) = assignment
            && let Err(error) = apply_server_assignment(state, &assignment)
        {
            state.message = format!("edit error: {error}");
        }
        return;
    }

    let Some(server) = state.selected_server() else {
        state.message = "no server selected".to_string();
        return;
    };
    state.input = field.current_value(server);
    state.edit_value_field = Some(field);
    state.message = format!(
        "editing {}: type value, Enter apply, Esc back ({})",
        field.label(),
        field.hint()
    );
}

fn handle_config_confirmation_key(
    state: &mut ConfigManagerState,
    key: KeyEvent,
    confirmation: PendingConfirmation,
) -> Result<ConfigManagerControl> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            state.pending_confirmation = None;
            match confirmation {
                PendingConfirmation::Delete => delete_selected_server(state),
                PendingConfirmation::GenerateSshKey => {
                    state.message = "Command: ssh-keygen -t ed25519".to_string();
                }
                PendingConfirmation::ExitUnsaved => {
                    save_config_manager_state(state)?;
                    return Ok(ConfigManagerControl::Exit);
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') => match confirmation {
            PendingConfirmation::ExitUnsaved => {
                state.pending_confirmation = None;
                state.message = "discarded unsaved changes".to_string();
                return Ok(ConfigManagerControl::Exit);
            }
            _ => {
                state.pending_confirmation = None;
                state.message = "confirmation cancelled".to_string();
            }
        },
        KeyCode::Esc => {
            state.pending_confirmation = None;
            state.message = "confirmation cancelled".to_string();
        }
        _ => {}
    }
    Ok(ConfigManagerControl::Continue)
}

fn request_config_exit(state: &mut ConfigManagerState) -> Result<ConfigManagerControl> {
    if state.is_dirty() {
        state.adding = None;
        state.editing = false;
        state.edit_value_field = None;
        state.input.clear();
        state.pending_confirmation = Some(PendingConfirmation::ExitUnsaved);
        state.message = "Unsaved changes. Save before quit?".to_string();
        Ok(ConfigManagerControl::Continue)
    } else {
        Ok(ConfigManagerControl::Exit)
    }
}

fn select_next_server(state: &mut ConfigManagerState) {
    if !state.config.servers.is_empty() {
        state.selected = (state.selected + 1).min(state.config.servers.len() - 1);
    }
}

fn select_previous_server(state: &mut ConfigManagerState) {
    state.selected = state.selected.saturating_sub(1);
}

fn enter_add_server_wizard(state: &mut ConfigManagerState) {
    let hosts = load_ssh_config_hosts();
    let count = hosts.len();
    state.adding = Some(AddServerState {
        hosts,
        selected: 0,
        direct_input: false,
        input: String::new(),
    });
    state.editing = false;
    state.edit_value_field = None;
    state.pending_confirmation = None;
    state.message = if count == 0 && cfg!(target_os = "linux") {
        "add server: no ~/.ssh/config hosts found; use direct SSH target or local".to_string()
    } else if count == 0 {
        "add server: no SSH config hosts found; use direct SSH target".to_string()
    } else {
        format!("add server: found {count} SSH config host(s)")
    };
}

fn add_ssh_target_server(state: &mut ConfigManagerState, name_hint: &str, host: &str) {
    let id = unique_server_id_from_base(&state.config, &server_id_from_target(name_hint));
    let name = display_name_from_target(name_hint);
    let server = ServerConfig::ssh(id, name, host);
    state.config.servers.push(server);
    state.selected = state.config.servers.len() - 1;
    state.health_refresh_requested = true;
    state.message = format!("added SSH server: {host}. Press r to test, c to show ssh-copy-id.");
}

fn add_local_server(state: &mut ConfigManagerState) {
    let id = unique_server_id_from_base(&state.config, "local");
    let name = if id == "local" {
        "Local".to_string()
    } else {
        format!("Local {}", state.config.servers.len() + 1)
    };
    state.config.servers.push(ServerConfig::local(id, name));
    state.selected = state.config.servers.len() - 1;
    state.health_refresh_requested = true;
    state.message = "added local server".to_string();
}

fn unique_server_id_from_base(config: &AppConfig, base: &str) -> String {
    let base = safe_server_id(base);
    if !config.servers.iter().any(|server| server.id == base) {
        return base;
    }

    let mut index = 2;
    loop {
        let candidate = format!("{base}-{index}");
        if !config.servers.iter().any(|server| server.id == candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn safe_server_id(value: &str) -> String {
    let host_part = value.rsplit('@').next().unwrap_or(value);
    let mut id = String::new();
    let mut last_was_dash = false;
    for ch in host_part.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            id.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            id.push('-');
            last_was_dash = true;
        }
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "server".to_string()
    } else {
        id
    }
}

fn server_id_from_target(target: &str) -> String {
    safe_server_id(target)
}

fn display_name_from_target(target: &str) -> String {
    let target = target.trim();
    if target.is_empty() {
        return "Server".to_string();
    }
    target
        .rsplit('@')
        .next()
        .unwrap_or(target)
        .split('.')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(target)
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn load_ssh_config_hosts() -> Vec<SshConfigEntry> {
    ssh_config_paths()
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .map(|input| parse_ssh_config_hosts(&input))
        .unwrap_or_default()
}

fn ssh_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for key in ["HOME", "USERPROFILE"] {
        let Some(home) = env::var_os(key).filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = PathBuf::from(home).join(".ssh").join("config");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn parse_ssh_config_hosts(input: &str) -> Vec<SshConfigEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut aliases: Vec<String> = Vec::new();
    let mut hostname: Option<String> = None;
    let mut user: Option<String> = None;

    fn flush(
        entries: &mut Vec<SshConfigEntry>,
        seen: &mut HashSet<String>,
        aliases: &mut Vec<String>,
        hostname: &mut Option<String>,
        user: &mut Option<String>,
    ) {
        for alias in aliases.drain(..) {
            if seen.insert(alias.clone()) {
                entries.push(SshConfigEntry {
                    alias,
                    hostname: hostname.clone(),
                    user: user.clone(),
                });
            }
        }
        *hostname = None;
        *user = None;
    }

    for raw_line in input.lines() {
        let line = raw_line
            .split_once('#')
            .map(|(before, _)| before)
            .unwrap_or(raw_line)
            .trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        if key.eq_ignore_ascii_case("Host") {
            flush(
                &mut entries,
                &mut seen,
                &mut aliases,
                &mut hostname,
                &mut user,
            );
            aliases = parts
                .filter(|alias| is_selectable_ssh_host_alias(alias))
                .map(ToString::to_string)
                .collect();
        } else if key.eq_ignore_ascii_case("HostName") {
            hostname = parts.next().map(ToString::to_string);
        } else if key.eq_ignore_ascii_case("User") {
            user = parts.next().map(ToString::to_string);
        }
    }

    flush(
        &mut entries,
        &mut seen,
        &mut aliases,
        &mut hostname,
        &mut user,
    );
    entries
}

fn is_selectable_ssh_host_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('!')
        && !alias.contains('*')
        && !alias.contains('?')
        && !alias.contains('%')
}

fn delete_selected_server(state: &mut ConfigManagerState) {
    if state.config.servers.is_empty() {
        state.message = "nothing to delete".to_string();
        return;
    }
    let removed = state.config.servers.remove(state.selected);
    state.clamp_selection();
    state.health_refresh_requested = true;
    state.message = format!("deleted server {} ({})", removed.name, removed.id);
}

fn toggle_selected_enabled(state: &mut ConfigManagerState) {
    let Some(server) = state.selected_server_mut() else {
        return;
    };
    server.enabled = !server.enabled;
    let name = server.name.clone();
    let enabled = server.enabled;
    state.health_refresh_requested = true;
    state.message = format!("{name}: enabled = {enabled}");
}

fn toggle_selected_optional(state: &mut ConfigManagerState) {
    let Some(server) = state.selected_server_mut() else {
        return;
    };
    server.optional = !server.optional;
    let name = server.name.clone();
    let optional = server.optional;
    state.health_refresh_requested = true;
    state.message = format!("{name}: optional = {optional}");
}

fn move_selected_server_up(state: &mut ConfigManagerState) {
    if state.selected > 0 {
        state
            .config
            .servers
            .swap(state.selected, state.selected - 1);
        state.selected -= 1;
        state.health_refresh_requested = true;
        state.message = "moved selected server up".to_string();
    }
}

fn move_selected_server_down(state: &mut ConfigManagerState) {
    if state.selected + 1 < state.config.servers.len() {
        state
            .config
            .servers
            .swap(state.selected, state.selected + 1);
        state.selected += 1;
        state.health_refresh_requested = true;
        state.message = "moved selected server down".to_string();
    }
}

fn apply_server_assignment(state: &mut ConfigManagerState, assignment: &str) -> Result<()> {
    let (field, value) = assignment
        .split_once('=')
        .ok_or_else(|| anyhow!("expected field=value"))?;
    let field = field.trim();
    let value = value.trim();
    let Some(server) = state.selected_server_mut() else {
        bail!("no server selected");
    };

    match field {
        "id" => server.id = value.to_string(),
        "name" => server.name = value.to_string(),
        "source" => set_server_source(server, value)?,
        "host" => set_server_host(server, value)?,
        "group" => server.group = optional_string(value),
        "role" => server.role = optional_string(value),
        "enabled" => server.enabled = parse_bool_field(value)?,
        "optional" => server.optional = parse_bool_field(value)?,
        "disk_max_rows" => {
            server.disk_max_rows = if value.is_empty() {
                None
            } else {
                Some(
                    value
                        .parse()
                        .context("disk_max_rows must be a positive integer")?,
                )
            };
        }
        "disk_aliases" => server.disk_aliases = parse_disk_aliases(value)?,
        _ => bail!(
            "unknown field `{field}`; supported: id/name/source/host/group/role/enabled/optional/disk_max_rows/disk_aliases"
        ),
    }
    state.health_refresh_requested = true;
    state.message = format!("updated {field}");
    Ok(())
}

fn optional_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_bool_field(value: &str) -> Result<bool> {
    match value {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        _ => bail!("expected boolean true/false"),
    }
}

fn current_host_or_placeholder(server: &ServerConfig) -> String {
    server
        .source
        .endpoint()
        .filter(|host| !host.is_empty())
        .unwrap_or("host-alias")
        .to_string()
}

fn set_server_source(server: &mut ServerConfig, source: &str) -> Result<()> {
    let host = current_host_or_placeholder(server);
    server.source = match source {
        "local" => HostSource::local(),
        "ssh" => HostSource::ssh(host),
        "proxmox" => HostSource::proxmox(host),
        "truenas" | "truenas-scale" => HostSource::truenas_scale(host),
        _ => bail!("source must be local, ssh, proxmox, truenas, or truenas-scale"),
    };
    Ok(())
}

fn set_server_host(server: &mut ServerConfig, host: &str) -> Result<()> {
    server.source = match &server.source {
        HostSource::Local => {
            bail!("local source does not use host; set source=ssh/proxmox/truenas-scale first")
        }
        HostSource::Ssh { .. } => HostSource::ssh(host),
        HostSource::Proxmox { .. } => HostSource::proxmox(host),
        HostSource::TrueNasScale { .. } => HostSource::truenas_scale(host),
    };
    Ok(())
}

fn parse_disk_aliases(value: &str) -> Result<BTreeMap<String, String>> {
    let mut aliases = BTreeMap::new();
    if value.trim().is_empty() {
        return Ok(aliases);
    }

    for pair in value.split(',') {
        let (mount, alias) = pair
            .split_once(':')
            .ok_or_else(|| anyhow!("disk_aliases entries must look like /mount:alias"))?;
        aliases.insert(mount.trim().to_string(), alias.trim().to_string());
    }
    Ok(aliases)
}

fn save_config_manager_state(state: &mut ConfigManagerState) -> Result<()> {
    fs_write_config(&state.path, &state.config)?;
    state.saved_config = state.config.clone();
    state.message = format!("saved canonical config: {}", state.path.display());
    Ok(())
}

fn run_selected_ssh_probe(state: &mut ConfigManagerState) {
    let Some(server) = state.selected_server().cloned() else {
        return;
    };
    match &server.source {
        HostSource::Ssh { host } => {
            let host = host.clone();
            if let Err(error) = crate::collectors::ssh::validate_ssh_host(&host) {
                state.health.insert(
                    server.id.clone(),
                    ConfigHealthStatus::Failed("invalid host".to_string()),
                );
                state.message = format!("ssh probe skipped: {error}");
                return;
            }
            match crate::collectors::ssh::ssh_probe_command(&host).output() {
                Ok(output) if output.status.success() => {
                    state.health.insert(
                        server.id.clone(),
                        ConfigHealthStatus::Ok("ssh ok".to_string()),
                    );
                    state.message = format!("ssh probe ok for {host}");
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    state.health.insert(
                        server.id.clone(),
                        ConfigHealthStatus::Failed("ssh failed".to_string()),
                    );
                    state.message = format!(
                        "ssh failed for {host}. Press c to show ssh-copy-id. ({})",
                        stderr.trim()
                    );
                }
                Err(error) => {
                    state.health.insert(
                        server.id.clone(),
                        ConfigHealthStatus::Failed("probe error".to_string()),
                    );
                    state.message = format!("ssh probe could not run for {host}: {error}")
                }
            }
        }
        source => {
            state.health.insert(
                server.id.clone(),
                ConfigHealthStatus::Ok(source.kind().to_string()),
            );
            state.message = format!("ssh probe applies only to source=ssh; selected {source}");
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct ConfigHealthReport {
    health: HashMap<String, ConfigHealthStatus>,
    issues: HashMap<String, String>,
    copy_id_hosts: HashMap<String, String>,
    message: String,
}

#[derive(Debug, Clone)]
struct ConfigHealthUpdate {
    id: String,
    status: ConfigHealthStatus,
    issue: Option<String>,
    copy_id_host: Option<String>,
}

fn start_config_health_check(
    state: &mut ConfigManagerState,
    health_rx: &mut Option<Receiver<ConfigHealthUpdate>>,
) {
    if health_rx.is_some() {
        state.message = "health check already running".to_string();
        return;
    }
    mark_config_health_pending(state);
    let servers = state
        .config
        .servers
        .iter()
        .filter(|server| server.enabled)
        .cloned()
        .collect::<Vec<_>>();
    let (tx, rx) = mpsc::channel();
    for server in servers {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(check_config_server_health(&server));
        });
    }
    drop(tx);
    *health_rx = Some(rx);
}

fn poll_config_health_check(
    state: &mut ConfigManagerState,
    health_rx: &mut Option<Receiver<ConfigHealthUpdate>>,
) {
    let Some(rx) = health_rx else {
        return;
    };

    let mut changed = false;
    loop {
        match rx.try_recv() {
            Ok(update) => {
                apply_config_health_update(state, update);
                changed = true;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                *health_rx = None;
                changed = true;
                break;
            }
        }
    }

    if changed {
        state.health_message = config_health_message(
            &state.config,
            &state.health,
            &state.health_issues,
            &state.health_copy_id_hosts,
        );
    }
}

fn apply_config_health_update(state: &mut ConfigManagerState, update: ConfigHealthUpdate) {
    state.health.insert(update.id.clone(), update.status);
    if let Some(issue) = update.issue {
        state.health_issues.insert(update.id.clone(), issue);
    } else {
        state.health_issues.remove(&update.id);
    }
    if let Some(host) = update.copy_id_host {
        state.health_copy_id_hosts.insert(update.id, host);
    } else {
        state.health_copy_id_hosts.remove(&update.id);
    }
}

fn mark_config_health_pending(state: &mut ConfigManagerState) {
    state.health.clear();
    state.health_issues.clear();
    state.health_copy_id_hosts.clear();
    for server in &state.config.servers {
        let status = if !server.enabled {
            ConfigHealthStatus::Pending("disabled".to_string())
        } else {
            match server.source {
                HostSource::Local
                | HostSource::Ssh { .. }
                | HostSource::Proxmox { .. }
                | HostSource::TrueNasScale { .. } => {
                    ConfigHealthStatus::Pending("checking".to_string())
                }
            }
        };
        state.health.insert(server.id.clone(), status);
    }
    state.health_message = "health: checking...".to_string();
}

#[cfg(test)]
fn run_config_health_check(state: &mut ConfigManagerState) {
    let report = compute_config_health(&state.config);
    state.health = report.health;
    state.health_issues = report.issues;
    state.health_copy_id_hosts = report.copy_id_hosts;
    state.health_message = report.message;
}

#[cfg(test)]
fn compute_config_health(config: &AppConfig) -> ConfigHealthReport {
    let mut health = HashMap::new();
    let mut issues = HashMap::new();
    let mut copy_id_hosts = HashMap::new();

    for server in config.servers.iter().filter(|server| server.enabled) {
        let update = check_config_server_health(server);
        health.insert(update.id.clone(), update.status);
        if let Some(issue) = update.issue {
            issues.insert(update.id.clone(), issue);
        }
        if let Some(host) = update.copy_id_host {
            copy_id_hosts.insert(update.id, host);
        }
    }
    for server in config.servers.iter().filter(|server| !server.enabled) {
        health.insert(
            server.id.clone(),
            ConfigHealthStatus::Pending("disabled".to_string()),
        );
    }

    let message = config_health_message(config, &health, &issues, &copy_id_hosts);

    ConfigHealthReport {
        health,
        issues,
        copy_id_hosts,
        message,
    }
}

fn check_config_server_health(server: &ServerConfig) -> ConfigHealthUpdate {
    let (status, issue, copy_id_host) = match &server.source {
        HostSource::Local => (ConfigHealthStatus::Ok("local".to_string()), None, None),
        HostSource::Ssh { host } => {
            if let Err(error) = crate::collectors::ssh::validate_ssh_host(host) {
                (
                    ConfigHealthStatus::Failed("invalid host".to_string()),
                    Some(format!("{}: {error}", server.name)),
                    None,
                )
            } else {
                match crate::collectors::ssh::ssh_probe_command(host).output() {
                    Ok(output) if output.status.success() => {
                        (ConfigHealthStatus::Ok("ssh ok".to_string()), None, None)
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        (
                            ConfigHealthStatus::Failed("ssh failed".to_string()),
                            Some(format!("{}: ssh failed ({})", server.name, stderr.trim())),
                            Some(host.clone()),
                        )
                    }
                    Err(error) => (
                        ConfigHealthStatus::Failed("probe error".to_string()),
                        Some(format!("{}: ssh probe error ({error})", server.name)),
                        None,
                    ),
                }
            }
        }
        HostSource::Proxmox { .. } | HostSource::TrueNasScale { .. } => (
            ConfigHealthStatus::Ok(server.source.kind().to_string()),
            None,
            None,
        ),
    };

    ConfigHealthUpdate {
        id: server.id.clone(),
        status,
        issue,
        copy_id_host,
    }
}

fn config_health_message(
    config: &AppConfig,
    health: &HashMap<String, ConfigHealthStatus>,
    issues_by_id: &HashMap<String, String>,
    copy_id_hosts: &HashMap<String, String>,
) -> String {
    let enabled_servers = config
        .servers
        .iter()
        .filter(|server| server.enabled)
        .collect::<Vec<_>>();
    let checked = enabled_servers.len();
    let ok = enabled_servers
        .iter()
        .filter(|server| matches!(health.get(&server.id), Some(ConfigHealthStatus::Ok(_))))
        .count();
    let issues = enabled_servers
        .iter()
        .filter(|server| matches!(health.get(&server.id), Some(ConfigHealthStatus::Failed(_))))
        .count();
    let checking = checked.saturating_sub(ok + issues);
    let first_issue = enabled_servers
        .iter()
        .find_map(|server| issues_by_id.get(&server.id).cloned());
    let first_copy_id_host = enabled_servers
        .iter()
        .find_map(|server| copy_id_hosts.get(&server.id).cloned());

    let mut message = format!("health: {ok}/{checked} ok");
    if issues > 0 {
        message.push_str(&format!(", {issues} issue"));
    }
    if checking > 0 {
        message.push_str(&format!(", {checking} checking"));
    }
    if let Some(issue) = first_issue {
        message.push_str(&format!(". {issue}"));
        if let Some(host) = first_copy_id_host {
            message.push_str(&format!(
                ". select it and press c to show ssh-copy-id {host}"
            ));
        }
    }
    message
}

fn show_selected_copy_id_command(state: &mut ConfigManagerState) {
    let Some(source) = state.selected_server().map(|server| server.source.clone()) else {
        return;
    };
    match source {
        HostSource::Ssh { host } if crate::collectors::ssh::validate_ssh_host(&host).is_ok() => {
            state.message = format!("Command: ssh-copy-id {host}");
        }
        HostSource::Ssh { host } => {
            state.message = format!("ssh-copy-id skipped: invalid ssh host `{host}`");
        }
        _ => {
            state.message =
                "ssh-copy-id applies only to SSH servers; edit source to ssh first".to_string();
        }
    }
}

fn draw_config_manager(frame: &mut ratatui::Frame<'_>, state: &ConfigManagerState) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(9),
        ])
        .split(area);

    let dirty = if state.is_dirty() { " — unsaved" } else { "" };
    let title = format!(
        " rktop setup/config TUI — {} servers{} — {} ",
        state.config.servers.len(),
        dirty,
        state.path.display()
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Full-screen config manager",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            key_span("s"),
            Span::raw(" save  "),
            key_span("q"),
            Span::raw(" quit  "),
            key_span("e"),
            Span::raw(" edit fields  "),
            key_span("r"),
            Span::raw(" ssh probe  "),
            key_span("c"),
            Span::raw(" show ssh-copy-id"),
        ]))
        .block(
            Block::default()
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        rows[0],
    );

    let (list_title, list_lines) = if let Some(add_state) = &state.adding {
        (" Add server ", add_server_lines(add_state))
    } else if state.editing {
        (
            " Edit selected server ",
            edit_field_lines(state.selected_server(), state),
        )
    } else if state.config.servers.is_empty() {
        (
            " Servers ",
            vec![Line::from(vec![
                Span::styled("No servers configured.", Style::default().fg(Color::Yellow)),
                Span::raw(" Press "),
                key_span("a"),
                Span::raw(" to add from SSH config, direct SSH target, or local. Then "),
                key_span("e"),
                Span::raw(" to edit fields."),
            ])],
        )
    } else {
        let mut server_lines = vec![server_list_header_line()];
        server_lines.extend(
            state
                .config
                .servers
                .iter()
                .enumerate()
                .map(|(idx, server)| {
                    server_list_line(
                        idx,
                        server,
                        state.health.get(&server.id),
                        idx == state.selected,
                    )
                }),
        );
        (" Servers ", server_lines)
    };
    frame.render_widget(
        Paragraph::new(list_lines).block(
            Block::default()
                .title(Span::styled(
                    list_title,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        rows[1],
    );

    draw_config_manager_detail(frame, rows[2], state);
}

fn server_list_header_line() -> Line<'static> {
    let left = server_list_columns(ServerListColumns {
        marker: " ",
        number: "#",
        id: "ID",
        name: "NAME",
        source: "SOURCE",
        enabled: "STATUS",
        required: "REQUIRED",
        host: "HOST",
    });
    Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "HEALTH",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

struct ServerListColumns<'a> {
    marker: &'a str,
    number: &'a str,
    id: &'a str,
    name: &'a str,
    source: &'a str,
    enabled: &'a str,
    required: &'a str,
    host: &'a str,
}

fn server_list_columns(columns: ServerListColumns<'_>) -> String {
    format!(
        "{} {:>2} │ {:<16} │ {:<24} │ {:<11} │ {:<8} │ {:<8} │ {:<32} │",
        columns.marker,
        columns.number,
        columns.id,
        columns.name,
        columns.source,
        columns.enabled,
        columns.required,
        columns.host,
    )
}

fn server_list_line(
    index: usize,
    server: &ServerConfig,
    health: Option<&ConfigHealthStatus>,
    selected: bool,
) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let enabled = if server.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let required = if server.optional {
        "optional"
    } else {
        "required"
    };
    let id = fit_text(&server.id, 16);
    let name = fit_text(&server.name, 24);
    let source = fit_text(server.source.kind(), 11);
    let host = fit_text(server.source.endpoint().unwrap_or("-"), 32);
    let number = format!("{:02}", index + 1);
    let left = server_list_columns(ServerListColumns {
        marker,
        number: &number,
        id: &id,
        name: &name,
        source: &source,
        enabled,
        required,
        host: &host,
    });
    let health_label = health
        .map(ConfigHealthStatus::label)
        .unwrap_or_else(|| "health -".to_string());
    if selected {
        Line::from(vec![
            Span::styled(
                left,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                health_label,
                health
                    .map(ConfigHealthStatus::style)
                    .unwrap_or_else(|| Style::default().fg(Color::DarkGray)),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(left, Style::default().fg(Color::Gray)),
            Span::raw(" "),
            Span::styled(
                health_label,
                health
                    .map(ConfigHealthStatus::style)
                    .unwrap_or_else(|| Style::default().fg(Color::DarkGray)),
            ),
        ])
    }
}

fn fit_text(value: &str, width: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    value.chars().take(width - 1).collect::<String>() + "…"
}

fn key_span(text: &'static str) -> Span<'static> {
    Span::styled(
        text,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

fn add_server_lines(add_state: &AddServerState) -> Vec<Line<'static>> {
    if add_state.direct_input {
        return vec![
            Line::from(vec![
                Span::styled(
                    "Direct SSH target",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                key_span("Enter"),
                Span::raw(" add · "),
                key_span("Esc"),
                Span::raw(" back"),
            ]),
            Line::from(vec![
                Span::styled("ssh> ", Style::default().fg(Color::Yellow)),
                Span::raw(add_state.input.clone()),
                Span::styled("▌", Style::default().fg(Color::Yellow)),
            ]),
            Line::from(Span::styled(
                "Examples: user@example.com, server-1, root@192.0.2.10",
                Style::default().fg(Color::DarkGray),
            )),
        ];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "Choose how to add a server",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        key_span("↑/↓"),
        Span::raw(" select · "),
        key_span("Enter"),
        Span::raw(" add · "),
        key_span("Esc"),
        Span::raw(" cancel"),
    ])];

    lines.push(add_option_line(
        add_state.selected == 0,
        "Manual SSH target",
        "type user@host or SSH alias",
    ));
    if add_state.local_option_available() {
        lines.push(add_option_line(
            add_state.selected == 1,
            "Local machine",
            "collect this machine directly",
        ));
    }

    if add_state.hosts.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No ~/.ssh/config Host entries found.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  SSH config hosts",
            Style::default().fg(Color::DarkGray),
        )));
        for (index, host) in add_state.hosts.iter().enumerate() {
            lines.push(add_option_line(
                add_state.selected == index + add_state.host_option_offset(),
                &host.alias,
                &host.detail(),
            ));
        }
    }

    lines
}

fn add_option_line(selected: bool, label: &str, detail: &str) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {:<24}", label), style),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.to_string(), style),
    ])
}

fn edit_field_lines(
    server: Option<&ServerConfig>,
    state: &ConfigManagerState,
) -> Vec<Line<'static>> {
    let Some(server) = server else {
        return vec![Line::from(Span::styled(
            "No server selected. Press Esc, then a to add a server.",
            Style::default().fg(Color::Yellow),
        ))];
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ({})", server.name, server.id),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        key_span("↑/↓"),
        Span::raw(" field · "),
        key_span("Enter"),
        Span::raw(" edit · "),
        key_span("space"),
        Span::raw(" toggle · "),
        key_span("Esc"),
        Span::raw(" back"),
    ])];

    for (index, field) in CONFIG_EDIT_FIELDS.iter().enumerate() {
        let selected = index == state.edit_selected.min(CONFIG_EDIT_FIELDS.len() - 1);
        let marker = if selected { ">" } else { " " };
        let value = field.current_value(server);
        let active_input = state.edit_value_field == Some(*field);
        let value = if active_input {
            format!("{}▌", state.input)
        } else if value.is_empty() {
            "∅".to_string()
        } else {
            value
        };
        let line = format!(
            "{marker} {:<14} │ {:<38} │ {}",
            field.label(),
            value,
            field.hint()
        );
        let style = if active_input {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(Span::styled(line, style)));
    }

    lines
}

fn config_status_line(message: &str) -> Line<'static> {
    if let Some(command) = message.strip_prefix("Command: ") {
        return Line::from(vec![
            Span::styled(
                "Command ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                command.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    }

    let color = if message.contains("error")
        || message.contains("failed")
        || message.contains("skipped")
        || message.contains("could not")
    {
        Color::Red
    } else if message.contains("ok")
        || message.contains("saved")
        || message.contains("updated")
        || message.contains("enabled")
    {
        Color::Green
    } else if message.contains("delete") || message.contains("show") || message.contains("editing")
    {
        Color::Yellow
    } else {
        Color::Cyan
    };

    Line::from(Span::styled(
        message.to_string(),
        Style::default().fg(color),
    ))
}

fn draw_config_manager_detail(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &ConfigManagerState,
) {
    let mut lines = if state.adding.is_some() {
        vec![Line::from(vec![
            Span::styled(
                "Add ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            key_span("↑/↓"),
            Span::raw(" select · "),
            key_span("Enter"),
            Span::raw(" choose/add · "),
            key_span("Esc"),
            Span::raw(" cancel/back"),
        ])]
    } else if state.editing {
        vec![Line::from(vec![
            Span::styled(
                "Edit ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            key_span("↑/↓"),
            Span::raw(" field · "),
            key_span("Enter"),
            Span::raw(" edit · "),
            key_span("space"),
            Span::raw(" toggle · "),
            key_span("s"),
            Span::raw(" save · "),
            key_span("Esc"),
            Span::raw(" back"),
        ])]
    } else {
        vec![
            Line::from(vec![
                Span::styled(
                    "Keys ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                key_span("↑/↓"),
                Span::raw(" select · "),
                key_span("a"),
                Span::raw(" add · "),
                key_span("e"),
                Span::raw(" edit · "),
                key_span("space"),
                Span::raw(" enable · "),
                key_span("d"),
                Span::raw(" delete · "),
                key_span("</>"),
                Span::raw(" move · "),
                key_span("s"),
                Span::raw(" save · "),
                key_span("q"),
                Span::raw(" quit"),
            ]),
            Line::from(vec![
                Span::styled(
                    "SSH tools ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                key_span("h"),
                Span::raw(" health check · "),
                key_span("r"),
                Span::raw(" test connection · "),
                key_span("g"),
                Span::raw(" show keygen · "),
                key_span("c"),
                Span::raw(" show ssh-copy-id"),
            ]),
        ]
    };
    if let Some(field) = state.edit_value_field {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}> ", field.label()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(state.input.clone()),
        ]));
    } else if let Some(confirmation) = &state.pending_confirmation {
        let confirm_line = match confirmation {
            PendingConfirmation::ExitUnsaved => vec![
                Span::styled(
                    "Unsaved ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                key_span("y"),
                Span::raw(" save & quit · "),
                key_span("n"),
                Span::raw(" discard · "),
                key_span("Esc"),
                Span::raw(" cancel"),
            ],
            _ => vec![
                Span::styled(
                    "Confirm ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                key_span("y"),
                Span::raw(" yes · "),
                key_span("n"),
                Span::raw(" no · "),
                key_span("Esc"),
                Span::raw(" cancel"),
            ],
        };
        lines.push(Line::from(confirm_line));
    }
    if !state.message.is_empty() {
        lines.push(config_status_line(&state.message));
    }
    if !state.health_message.is_empty() {
        lines.push(config_status_line(&state.health_message));
    }

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Span::styled(
                    " Help / status ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn edit_config(path: Option<&Path>) -> Result<()> {
    let path = config_path_or_default(path)?;
    if !path.exists() {
        write_example_config(&path, false)?;
        println!("created config: {}", path.display());
    }

    let editor = editor_command();
    println!("editing config: {}", path.display());
    println!("editor: {}", editor);
    let status = run_editor(&editor, &path)?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }

    let config = load_config_file(&path)?;
    let enabled_count = enabled_servers(&config).count();
    println!(
        "config ok: {} enabled / {} total servers, refresh {}ms",
        enabled_count,
        config.servers.len(),
        config.refresh_interval_ms
    );
    print_ssh_setup_guidance(&config);
    println!("next: rktop doctor");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SshSetupCommand {
    GenerateKey,
    CopyKey { host: String },
}

impl SshSetupCommand {
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn display_command(&self) -> String {
        match self {
            Self::GenerateKey => "ssh-keygen -t ed25519".to_string(),
            Self::CopyKey { host } => format!("ssh-copy-id {host}"),
        }
    }
}

fn ssh_setup_commands_for_server(server: &crate::config::ServerConfig) -> Vec<SshSetupCommand> {
    match &server.source {
        HostSource::Ssh { host } if crate::collectors::ssh::validate_ssh_host(host).is_ok() => {
            vec![
                SshSetupCommand::GenerateKey,
                SshSetupCommand::CopyKey { host: host.clone() },
            ]
        }
        _ => Vec::new(),
    }
}

fn print_ssh_setup_guidance(config: &AppConfig) {
    let ssh_servers = config
        .servers
        .iter()
        .filter(|server| !ssh_setup_commands_for_server(server).is_empty())
        .collect::<Vec<_>>();
    if ssh_servers.is_empty() {
        return;
    }

    println!("ssh setup: manual only after confirming each source = \"ssh\" target");
    println!("ssh setup: this command does not run ssh-keygen or ssh-copy-id for you");
    for server in ssh_servers {
        let commands = ssh_setup_commands_for_server(server)
            .into_iter()
            .filter(|command| command.requires_confirmation())
            .map(|command| command.display_command())
            .collect::<Vec<_>>()
            .join("; ");
        println!("ssh setup: {} ({}) -> {commands}", server.name, server.id);
    }
}

fn editor_command() -> String {
    env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| {
            if command_exists("nano") {
                "nano".to_string()
            } else if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        })
}

fn command_exists(command: &str) -> bool {
    if cfg!(windows) {
        return Command::new("where")
            .arg(command)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    }
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {command} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_editor(editor: &str, path: &Path) -> Result<std::process::ExitStatus> {
    if editor.split_whitespace().count() > 1 {
        if cfg!(windows) {
            return Command::new("cmd")
                .arg("/C")
                .arg(format!(r#"{editor} "{}""#, path.display()))
                .status()
                .with_context(|| format!("failed to launch editor `{editor}`"));
        }
        Command::new("sh")
            .arg("-c")
            .arg(r#"$0 "$1""#)
            .arg(editor)
            .arg(path)
            .status()
            .with_context(|| format!("failed to launch editor `{editor}`"))
    } else {
        Command::new(editor)
            .arg(path)
            .status()
            .with_context(|| format!("failed to launch editor `{editor}`"))
    }
}

fn config_path_or_default(path: Option<&Path>) -> Result<PathBuf> {
    path.map(Path::to_path_buf)
        .or_else(default_config_path)
        .ok_or_else(|| anyhow!("could not determine config path; pass --config PATH"))
}

fn doctor(path: Option<&Path>) -> Result<()> {
    let loaded = load_config(path)?;
    print_config_origin(&loaded);

    let enabled = enabled_servers(&loaded.config).cloned().collect::<Vec<_>>();
    println!(
        "servers: {} enabled / {} total",
        enabled.len(),
        loaded.config.servers.len()
    );
    println!("refresh: {}ms", loaded.config.refresh_interval_ms);

    if enabled.is_empty() {
        bail!("doctor failed: no enabled servers in config");
    }

    let mut required_failures = 0usize;
    for server in enabled {
        let required = if server.optional {
            "optional"
        } else {
            "required"
        };
        match doctor_server(&server) {
            Ok(message) => println!("[ok]   {:<10} {:<8} {}", server.name, required, message),
            Err(error) if server.optional => {
                println!("[warn] {:<10} {:<8} {}", server.name, required, error);
            }
            Err(error) => {
                required_failures += 1;
                println!("[fail] {:<10} {:<8} {}", server.name, required, error);
            }
        }
    }

    if required_failures > 0 {
        bail!("doctor failed: {required_failures} required server(s) unavailable");
    }

    println!("doctor: ok");
    Ok(())
}

fn print_config_origin(loaded: &LoadedConfig) {
    match &loaded.origin {
        ConfigOrigin::File(path) => println!("config: {}", path.display()),
        ConfigOrigin::BuiltIn { default_path } => {
            println!("config: built-in defaults");
            if let Some(path) = default_path {
                println!(
                    "hint: run `rktop config` to create and set up {}",
                    path.display()
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SetupEffect {
    LocalCollect,
    SshKeyProbe { host: String },
    UnsupportedSource(String),
}

fn setup_effect_for_source(server: &crate::config::ServerConfig) -> SetupEffect {
    match &server.source {
        HostSource::Local => SetupEffect::LocalCollect,
        HostSource::Ssh { host } => SetupEffect::SshKeyProbe { host: host.clone() },
        source => SetupEffect::UnsupportedSource(source.to_string()),
    }
}

fn doctor_server(server: &crate::config::ServerConfig) -> Result<String> {
    match setup_effect_for_source(server) {
        SetupEffect::LocalCollect => {
            let metrics = crate::collectors::collect_host(server)?;
            Ok(format!(
                "local collector ok ({})",
                metrics.hostname.unwrap_or_else(|| "localhost".to_string())
            ))
        }
        SetupEffect::SshKeyProbe { host } => {
            crate::collectors::ssh::validate_ssh_host(&host)?;
            let ssh_probe = crate::collectors::ssh::ssh_probe_command(&host)
                .output()
                .with_context(|| format!("failed to run ssh probe for {host}"))?;
            if !ssh_probe.status.success() {
                let stderr = String::from_utf8_lossy(&ssh_probe.stderr)
                    .trim()
                    .to_string();
                bail!("ssh key check failed for {host}: {stderr}");
            }
            let metrics = crate::collectors::collect_host(server)?;
            Ok(format!(
                "ssh {} ok ({})",
                host,
                metrics.hostname.unwrap_or_else(|| host.to_string())
            ))
        }
        SetupEffect::UnsupportedSource(source) => {
            bail!("{source} source is configured but not implemented in the live collector yet")
        }
    }
}

fn is_terminal() -> bool {
    std::io::IsTerminal::is_terminal(&io::stdout())
}

fn build_snapshot_state(mode: Mode, config: &AppConfig) -> Result<AppState> {
    let refresh_interval = Duration::from_millis(config.refresh_interval_ms);
    let first = build_state_with_previous(mode, None, refresh_interval, config)?;
    if mode == Mode::Live {
        std::thread::sleep(refresh_interval);
        build_state_with_previous(mode, Some(&first), refresh_interval, config)
    } else {
        Ok(first)
    }
}

fn build_state_with_previous(
    mode: Mode,
    previous: Option<&AppState>,
    refresh_interval: Duration,
    config: &AppConfig,
) -> Result<AppState> {
    Ok(
        build_dashboard_with_previous(mode, previous, refresh_interval, config, &HashSet::new())?
            .dashboard,
    )
}

fn build_dashboard_with_previous(
    mode: Mode,
    previous: Option<&AppState>,
    refresh_interval: Duration,
    config: &AppConfig,
    skipped_optional_ids: &HashSet<String>,
) -> Result<DashboardBuild> {
    let generated_at = match mode {
        Mode::Mock => Utc
            .with_ymd_and_hms(2026, 7, 7, 6, 45, 0)
            .single()
            .ok_or_else(|| anyhow!("invalid deterministic timestamp"))?,
        Mode::Live => Utc::now(),
    };

    let (hosts, optional_failures) = match mode {
        Mode::Mock => (
            crate::fixtures::fixture_hosts_for_config(config)
                .into_iter()
                .enumerate()
                .map(|(idx, host)| {
                    let server = config.servers.iter().find(|server| server.id == host.id);
                    view_from_metrics(host, server, idx, generated_at, false, None, None)
                })
                .collect(),
            Vec::new(),
        ),
        Mode::Live => {
            let collection =
                collect_live_hosts(generated_at, previous, config, skipped_optional_ids);
            (collection.hosts, collection.optional_failures)
        }
    };

    Ok(DashboardBuild {
        dashboard: AppState {
            title: "Server TUI Monitor".to_string(),
            generated_at,
            mode,
            refresh_interval_ms: refresh_interval.as_millis() as u64,
            hosts,
        },
        optional_failures,
    })
}

fn collect_live_hosts(
    generated_at: DateTime<Utc>,
    previous: Option<&AppState>,
    config: &AppConfig,
    skipped_optional_ids: &HashSet<String>,
) -> LiveCollection {
    let previous_hosts = previous
        .map(|state| {
            state
                .hosts
                .iter()
                .map(|host| (host.id.as_str(), host))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let elapsed_secs = previous
        .and_then(|state| {
            generated_at
                .signed_duration_since(state.generated_at)
                .to_std()
                .ok()
        })
        .map(|duration| duration.as_secs_f64())
        .filter(|seconds| *seconds > 0.0);

    let collection = collect_enabled_servers_with_skip(config, skipped_optional_ids);

    let hosts = collection
        .collected
        .into_iter()
        .enumerate()
        .filter_map(|(idx, (server, metrics))| {
            let previous_host = previous_hosts.get(server.id.as_str()).copied();
            let host = view_from_metrics(
                metrics,
                Some(&server),
                idx,
                generated_at,
                true,
                previous_host,
                elapsed_secs,
            );
            if server.optional && matches!(host.status, Status::Unavailable) {
                None
            } else {
                Some(host)
            }
        })
        .collect();

    LiveCollection {
        hosts,
        optional_failures: collection.optional_failures,
    }
}

#[cfg(test)]
fn collect_enabled_servers(config: &crate::config::AppConfig) -> EnabledCollection {
    collect_enabled_servers_with_skip(config, &HashSet::new())
}

fn collect_enabled_servers_with_skip(
    config: &crate::config::AppConfig,
    skipped_optional_ids: &HashSet<String>,
) -> EnabledCollection {
    let servers = enabled_servers(config)
        .filter(|server| !(server.optional && skipped_optional_ids.contains(&server.id)))
        .cloned()
        .collect::<Vec<_>>();

    std::thread::scope(|scope| {
        let handles = servers
            .into_iter()
            .map(|server| {
                scope.spawn(move || match crate::collectors::collect_host(&server) {
                    Ok(metrics) => Some(CollectionOutcome::Collected(server, Box::new(metrics))),
                    Err(_) if server.optional => Some(CollectionOutcome::OptionalFailed(server.id)),
                    Err(error) => {
                        let metrics = unavailable_metrics(&server, error.to_string());
                        Some(CollectionOutcome::Collected(server, Box::new(metrics)))
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut collected = Vec::new();
        let mut optional_failures = Vec::new();
        for result in handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap_or(None))
        {
            match result {
                CollectionOutcome::Collected(server, metrics) => collected.push((server, *metrics)),
                CollectionOutcome::OptionalFailed(id) => optional_failures.push(id),
            }
        }

        EnabledCollection {
            collected,
            optional_failures,
        }
    })
}

enum CollectionOutcome {
    Collected(crate::config::ServerConfig, Box<HostMetrics>),
    OptionalFailed(String),
}

fn unavailable_metrics(
    server: &crate::config::ServerConfig,
    error: impl Into<String>,
) -> HostMetrics {
    HostMetrics {
        id: server.id.clone(),
        name: server.name.clone(),
        hostname: server.source.endpoint().map(str::to_string),
        kernel: None,
        uptime_seconds: None,
        group: server.group.clone(),
        role: server.role.clone(),
        source: server.source.clone(),
        status: HostStatus::Offline,
        error: Some(error.into()),
        freshness: Freshness::now(crate::collectors::DEFAULT_STALE_AFTER),
        cpu: CpuMetrics::empty(),
        ram: RamMetrics::empty(),
        network: NetworkMetrics::empty(),
        storage: StorageMetrics::empty(),
    }
}

fn view_from_metrics(
    host: HostMetrics,
    server: Option<&ServerConfig>,
    idx: usize,
    generated_at: DateTime<Utc>,
    prefer_hostname: bool,
    previous: Option<&HostSnapshot>,
    elapsed_secs: Option<f64>,
) -> HostSnapshot {
    let unavailable = matches!(host.status, HostStatus::Offline | HostStatus::Disabled);
    let cpu = if unavailable {
        0.0
    } else {
        host.cpu
            .usage_percent
            .or(host.cpu.load_1m.map(|load| (load * 16.0).min(99.0)))
            .unwrap_or_else(|| deterministic_percent(&host.name, idx, 12, 72))
    };
    let ram = if unavailable {
        0.0
    } else {
        host.ram
            .usage_percent()
            .unwrap_or_else(|| deterministic_percent(&host.name, idx + 3, 18, 70))
    };
    let storage = if unavailable {
        0.0
    } else {
        host.storage
            .usage_percent()
            .or_else(|| max_disk_usage_percent(&host.storage))
            .unwrap_or_else(|| deterministic_percent(&host.name, idx + 7, 24, 68))
    };
    let (rx_bytes_per_sec, tx_bytes_per_sec) = if unavailable {
        (None, None)
    } else if prefer_hostname {
        (
            network_rate(
                host.network.rx_bytes_total,
                previous.and_then(|host| host.net_rx_total_bytes),
                elapsed_secs,
            ),
            network_rate(
                host.network.tx_bytes_total,
                previous.and_then(|host| host.net_tx_total_bytes),
                elapsed_secs,
            ),
        )
    } else {
        (
            Some(f64::from(deterministic_net(&host.name, 40, 220)) * 1024.0),
            Some(f64::from(deterministic_net(&host.name, 18, 180)) * 1024.0),
        )
    };

    let status = if prefer_hostname {
        status_from_live_metrics(&host, cpu, ram, storage)
    } else {
        status_from_metrics(&host, cpu, ram, storage)
    };
    let display_name = host.name.clone();
    let role = live_role(&host);
    let hostname = visible_hostname(&host);
    let hide_metrics = matches!(status, Status::Unavailable);
    let cpu_percent = if hide_metrics { 0 } else { clamp_percent(cpu) };
    let ram_percent = if hide_metrics { 0 } else { clamp_percent(ram) };
    let storage_percent = if hide_metrics {
        0
    } else {
        clamp_percent(storage)
    };
    let net_percent = if hide_metrics {
        0
    } else {
        network_activity_percent(rx_bytes_per_sec, tx_bytes_per_sec)
    };

    HostSnapshot {
        id: host.id.clone(),
        name: display_name,
        group: host.group.unwrap_or_else(|| "Ungrouped".to_string()),
        role,
        status,
        cpu_percent,
        ram_percent,
        storage_percent,
        cpu_history: if hide_metrics {
            blank_history()
        } else {
            percent_history(
                previous.map(|host| host.cpu_history.as_slice()),
                cpu_percent,
                &host.id,
                3,
            )
        },
        ram_history: if hide_metrics {
            blank_history()
        } else {
            percent_history(
                previous.map(|host| host.ram_history.as_slice()),
                ram_percent,
                &host.id,
                7,
            )
        },
        storage_history: if hide_metrics {
            blank_history()
        } else {
            percent_history(
                previous.map(|host| host.storage_history.as_slice()),
                storage_percent,
                &host.id,
                11,
            )
        },
        net_history: if hide_metrics {
            blank_history()
        } else {
            percent_history(
                previous.map(|host| host.net_history.as_slice()),
                net_percent,
                &host.id,
                13,
            )
        },
        net_rx_bytes_per_sec: if hide_metrics { None } else { rx_bytes_per_sec },
        net_tx_bytes_per_sec: if hide_metrics { None } else { tx_bytes_per_sec },
        net_rx_total_bytes: if hide_metrics {
            None
        } else {
            host.network.rx_bytes_total
        },
        net_tx_total_bytes: if hide_metrics {
            None
        } else {
            host.network.tx_bytes_total
        },
        last_seen: generated_at - chrono::Duration::seconds((idx as i64) * 31),
        hostname,
        kernel: host.kernel.clone(),
        uptime_seconds: if hide_metrics {
            None
        } else {
            host.uptime_seconds
        },
        cpu_cores: if hide_metrics { None } else { host.cpu.cores },
        cpu_temperature_celsius: if hide_metrics {
            None
        } else {
            host.cpu.temperature_celsius
        },
        load_1m: if hide_metrics { None } else { host.cpu.load_1m },
        load_5m: if hide_metrics { None } else { host.cpu.load_5m },
        load_15m: if hide_metrics {
            None
        } else {
            host.cpu.load_15m
        },
        ram_used_kib: if hide_metrics {
            None
        } else {
            host.ram.used_kib
        },
        ram_total_kib: if hide_metrics {
            None
        } else {
            host.ram.total_kib
        },
        storage_used_kib: if hide_metrics {
            None
        } else {
            host.storage.root_used_kib
        },
        storage_total_kib: if hide_metrics {
            None
        } else {
            host.storage.root_total_kib
        },
        disks: if hide_metrics {
            Vec::new()
        } else {
            disk_snapshots(&host.storage, server)
        },
    }
}

fn max_disk_usage_percent(storage: &StorageMetrics) -> Option<f32> {
    storage
        .disks
        .iter()
        .filter(|disk| disk.total_kib > 0)
        .map(|disk| (disk.used_kib as f32 / disk.total_kib as f32) * 100.0)
        .max_by(|a, b| a.total_cmp(b))
}

fn disk_snapshots(storage: &StorageMetrics, server: Option<&ServerConfig>) -> Vec<DiskSnapshot> {
    let mut disks = storage
        .disks
        .iter()
        .filter(|disk| meaningful_disk_mount(&disk.mount, disk.total_kib))
        .map(|disk| DiskSnapshot {
            mount: disk.mount.clone(),
            used_kib: disk.used_kib,
            total_kib: disk.total_kib,
            percent: clamp_percent((disk.used_kib as f32 / disk.total_kib as f32) * 100.0),
        })
        .collect::<Vec<_>>();

    disks = dedupe_disk_mounts(disks);
    disks = collapse_child_mounts(disks);

    if disks.is_empty()
        && let (Some(total), Some(used)) = (storage.root_total_kib, storage.root_used_kib)
        && total > 0
    {
        disks.push(DiskSnapshot {
            mount: "/".to_string(),
            used_kib: used,
            total_kib: total,
            percent: clamp_percent((used as f32 / total as f32) * 100.0),
        });
    }

    sort_disk_snapshots(&mut disks);
    apply_disk_aliases(&mut disks, server);
    let max_rows = server
        .and_then(|server| server.disk_max_rows)
        .unwrap_or(crate::config::DEFAULT_DISK_MAX_ROWS)
        .min(crate::config::DEFAULT_DISK_MAX_ROWS);
    disks.truncate(max_rows);
    disks
}

fn apply_disk_aliases(disks: &mut [DiskSnapshot], server: Option<&ServerConfig>) {
    let Some(server) = server else {
        return;
    };
    for disk in disks {
        if let Some(alias) = server.disk_aliases.get(&disk.mount) {
            disk.mount = alias.clone();
        }
    }
}

fn dedupe_disk_mounts(disks: Vec<DiskSnapshot>) -> Vec<DiskSnapshot> {
    let mut deduped: Vec<DiskSnapshot> = Vec::new();
    for disk in disks {
        if let Some(existing) = deduped
            .iter_mut()
            .find(|existing| existing.mount == disk.mount)
        {
            if disk.used_kib > existing.used_kib
                || (disk.used_kib == existing.used_kib && disk.total_kib > existing.total_kib)
            {
                *existing = disk;
            }
        } else {
            deduped.push(disk);
        }
    }
    deduped
}

fn collapse_child_mounts(mut disks: Vec<DiskSnapshot>) -> Vec<DiskSnapshot> {
    disks.sort_by(|a, b| {
        mount_depth(&a.mount)
            .cmp(&mount_depth(&b.mount))
            .then(a.mount.cmp(&b.mount))
    });
    let mut collapsed: Vec<DiskSnapshot> = Vec::new();
    for disk in disks {
        let covered_by_parent = collapsed.iter().any(|parent| {
            parent.used_kib >= 1024 * 1024 && is_parent_mount(&parent.mount, &disk.mount)
        });
        if !covered_by_parent {
            collapsed.push(disk);
        }
    }
    collapsed
}

fn sort_disk_snapshots(disks: &mut [DiskSnapshot]) {
    disks.sort_by(|a, b| {
        if a.mount == "/" && b.mount != "/" {
            std::cmp::Ordering::Less
        } else if b.mount == "/" && a.mount != "/" {
            std::cmp::Ordering::Greater
        } else {
            b.total_kib.cmp(&a.total_kib).then(a.mount.cmp(&b.mount))
        }
    });
}

fn mount_depth(mount: &str) -> usize {
    mount.split('/').filter(|part| !part.is_empty()).count()
}

fn is_parent_mount(parent: &str, child: &str) -> bool {
    if parent == "/" || parent == child {
        return false;
    }
    child
        .strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn meaningful_disk_mount(mount: &str, total_kib: u64) -> bool {
    const ONE_GIB_KIB: u64 = 1024 * 1024;
    if mount == "/" {
        return true;
    }
    if total_kib < ONE_GIB_KIB {
        return false;
    }
    if mount.starts_with("/mnt/") {
        return visible_mnt_mount(mount);
    }
    mount.starts_with("/media/") || mount.starts_with("/srv/") || mount.starts_with("/home/")
}

fn visible_mnt_mount(mount: &str) -> bool {
    mount
        .strip_prefix("/mnt/")
        .and_then(|suffix| suffix.split('/').next())
        .is_some_and(|first| !first.is_empty() && !first.starts_with('.'))
}

fn percent_history(previous: Option<&[u16]>, current: u16, id: &str, salt: usize) -> Vec<u16> {
    if let Some(previous) = previous {
        let mut history = previous.to_vec();
        history.push(current);
        let excess = history.len().saturating_sub(HISTORY_LEN);
        if excess > 0 {
            history.drain(0..excess);
        }
        history
    } else {
        pseudo_history(current, id, salt, HISTORY_LEN)
    }
}

fn blank_history() -> Vec<u16> {
    vec![0; HISTORY_LEN]
}

fn pseudo_history(current: u16, id: &str, salt: usize, len: usize) -> Vec<u16> {
    let mut seed = id.bytes().map(u32::from).sum::<u32>() + salt as u32 * 97;
    let target = i16::try_from(current).unwrap_or(0);
    let mut value = target;

    (0..len)
        .map(|idx| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let jitter = ((seed >> 16) % 9) as i16 - 4;
            let drift = (((idx as i16 + salt as i16) % 18) - 9) / 3;
            value += jitter + drift;
            value += (target - value) / 5;
            value.clamp(0, 99) as u16
        })
        .collect()
}

fn network_activity_percent(rx_bytes_per_sec: Option<f64>, tx_bytes_per_sec: Option<f64>) -> u16 {
    let total_kib = (rx_bytes_per_sec.unwrap_or(0.0) + tx_bytes_per_sec.unwrap_or(0.0)) / 1024.0;
    if total_kib <= 0.0 {
        0
    } else {
        clamp_percent((total_kib.log10() as f32 * 24.0).clamp(1.0, 99.0))
    }
}

fn network_rate(
    current_total_bytes: Option<u64>,
    previous_total_bytes: Option<u64>,
    elapsed_secs: Option<f64>,
) -> Option<f64> {
    let current = current_total_bytes?;
    let previous = previous_total_bytes?;
    let elapsed = elapsed_secs?;
    if current < previous || elapsed <= 0.0 {
        return None;
    }
    Some((current - previous) as f64 / elapsed)
}

fn live_role(host: &HostMetrics) -> String {
    let base = host.role.as_deref().unwrap_or_else(|| host.source.kind());

    match &host.error {
        Some(error) => format!("{base}: {error}"),
        None => base.to_string(),
    }
}

fn visible_hostname(host: &HostMetrics) -> Option<String> {
    match host.id.as_str() {
        "cloud" => Some("cloud".to_string()),
        "local" => Some(
            host.hostname
                .as_deref()
                .filter(|hostname| !hostname.eq_ignore_ascii_case("local"))
                .unwrap_or("local")
                .to_string(),
        ),
        _ => host
            .hostname
            .clone()
            .or_else(|| Some(host.name.to_ascii_lowercase())),
    }
}

fn deterministic_percent(name: &str, salt: usize, base: u16, spread: u16) -> f32 {
    let score: u16 = name.bytes().map(u16::from).sum();
    (base + ((score + salt as u16 * 13) % spread)) as f32
}

fn deterministic_net(name: &str, base: u32, spread: u32) -> u32 {
    base + (name.bytes().map(u32::from).sum::<u32>() % spread)
}

fn status_from_metrics(host: &HostMetrics, cpu: f32, ram: f32, storage: f32) -> Status {
    match host.status {
        HostStatus::Offline | HostStatus::Disabled => Status::Unavailable,
        HostStatus::Degraded => Status::Warn,
        HostStatus::Unknown => Status::Stale,
        HostStatus::Online => {
            status_from_mock_values(&host.name, cpu, ram, storage, host.cpu.temperature_celsius)
        }
    }
}

fn status_from_live_metrics(host: &HostMetrics, cpu: f32, ram: f32, storage: f32) -> Status {
    match host.status {
        HostStatus::Offline | HostStatus::Disabled => Status::Unavailable,
        HostStatus::Degraded => Status::Warn,
        HostStatus::Unknown => Status::Stale,
        HostStatus::Online => {
            status_from_thresholds(cpu, ram, storage, host.cpu.temperature_celsius)
        }
    }
}

fn status_from_mock_values(
    name: &str,
    cpu: f32,
    ram: f32,
    storage: f32,
    temperature_celsius: Option<f32>,
) -> Status {
    match name.to_ascii_lowercase().as_str() {
        n if n.contains("nas") => Status::Stale,
        n if n.contains("cloud") => Status::Unavailable,
        _ => status_from_thresholds(cpu, ram, storage, temperature_celsius),
    }
}

fn status_from_thresholds(
    cpu: f32,
    ram: f32,
    storage: f32,
    temperature_celsius: Option<f32>,
) -> Status {
    let temp = temperature_celsius.unwrap_or(0.0);
    if cpu > 92.0 || ram > 92.0 || storage > 94.0 || temp >= 85.0 {
        Status::Critical
    } else if cpu > 80.0 || ram > 82.0 || storage > 88.0 || temp >= 70.0 {
        Status::Warn
    } else {
        Status::Healthy
    }
}

fn clamp_percent(percent: f32) -> u16 {
    percent.clamp(0.0, 99.0).round() as u16
}

fn run_tui(mode: Mode, once: bool, config: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, mode, once, config);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mode: Mode,
    once: bool,
    config: AppConfig,
) -> Result<()> {
    let refresh_interval = Duration::from_millis(config.refresh_interval_ms);
    let initial =
        build_dashboard_with_previous(mode, None, refresh_interval, &config, &HashSet::new())?;
    let mut state = TuiState::new(initial.dashboard, refresh_interval);
    state.note_optional_failures(initial.optional_failures);
    let mut refresh_rx = None::<Receiver<DashboardRefresh>>;

    loop {
        poll_dashboard_refresh(&mut state, &mut refresh_rx)?;
        terminal.draw(|frame| {
            let dashboard = Dashboard {
                state: &state.dashboard,
            };
            render::draw(frame, &dashboard);
        })?;

        if once {
            return Ok(());
        }

        if event::poll(state.refresh_poll_timeout(mode))? {
            loop {
                if let Event::Key(key) = event::read()?
                    && reduce_tui_state(&mut state, action_from_key(key)) == TuiControl::Exit
                {
                    return Ok(());
                }

                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
        }

        if state.should_refresh(mode) {
            start_dashboard_refresh(&mut state, &mut refresh_rx, mode, config.clone());
        }
    }
}

fn start_dashboard_refresh(
    state: &mut TuiState,
    refresh_rx: &mut Option<Receiver<DashboardRefresh>>,
    mode: Mode,
    config: AppConfig,
) {
    if refresh_rx.is_some() || mode != Mode::Live {
        return;
    }
    state.refresh_in_flight = true;
    state.force_refresh = false;
    state.last_refresh = Instant::now();
    let previous = state.dashboard.clone();
    let refresh_interval = state.refresh_interval;
    let skipped_optional_ids = state.skipped_optional_ids();
    let thread_previous = previous.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = build_dashboard_with_previous(
            mode,
            Some(&thread_previous),
            refresh_interval,
            &config,
            &skipped_optional_ids,
        );
        let _ = tx.send(DashboardRefresh { result });
    });
    *refresh_rx = Some(rx);
}

fn poll_dashboard_refresh(
    state: &mut TuiState,
    refresh_rx: &mut Option<Receiver<DashboardRefresh>>,
) -> Result<()> {
    let Some(rx) = refresh_rx else {
        return Ok(());
    };
    match rx.try_recv() {
        Ok(refresh) => {
            let mut build = refresh.result?;
            build.dashboard.refresh_interval_ms = state.refresh_interval.as_millis() as u64;
            state.note_optional_failures(build.optional_failures);
            state.replace_dashboard_after_refresh(build.dashboard);
            *refresh_rx = None;
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            state.force_refresh = false;
            state.refresh_in_flight = false;
            *refresh_rx = None;
        }
    }
    Ok(())
}

fn refresh_poll_timeout(
    mode: Mode,
    force_refresh: bool,
    refresh_in_flight: bool,
    last_refresh: Instant,
    refresh_interval: Duration,
) -> Duration {
    if mode != Mode::Live {
        return Duration::from_millis(50);
    }
    if force_refresh && !refresh_in_flight {
        return Duration::from_millis(0);
    }
    if refresh_in_flight {
        return Duration::from_millis(50);
    }
    refresh_interval
        .saturating_sub(last_refresh.elapsed())
        .min(Duration::from_millis(50))
}

fn adjust_refresh_interval(current: Duration, delta_ms: i64) -> Duration {
    let current_ms = i64::try_from(current.as_millis()).unwrap_or(1_000);
    let next_ms = (current_ms + delta_ms).clamp(100, 60_000);
    Duration::from_millis(next_ms as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zfs_pool_mount_collapses_child_dataset_mounts() {
        let gib = 1024 * 1024;
        let storage = StorageMetrics {
            root_total_kib: None,
            root_used_kib: None,
            root_available_kib: None,
            disks: vec![
                crate::model::DiskMetrics {
                    mount: "/mnt/tank".to_string(),
                    total_kib: 12 * 1024 * gib,
                    used_kib: 2 * 1024 * gib,
                    available_kib: 10 * 1024 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/tank/shared/media".to_string(),
                    total_kib: 10 * 1024 * gib,
                    used_kib: 1400 * gib,
                    available_kib: 8 * 1024 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/tank/shared/app-data/user-data".to_string(),
                    total_kib: 10 * 1024 * gib,
                    used_kib: 1700 * gib,
                    available_kib: 8 * 1024 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/fast".to_string(),
                    total_kib: 300 * gib,
                    used_kib: 50 * gib,
                    available_kib: 250 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/fast/apps/media-service".to_string(),
                    total_kib: 300 * gib,
                    used_kib: 10 * gib,
                    available_kib: 250 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/.ix-apps/docker".to_string(),
                    total_kib: 300 * gib,
                    used_kib: 50 * gib,
                    available_kib: 250 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/var/lib/incus/storage-pools/tank/containers/backup".to_string(),
                    total_kib: 10 * 1024 * gib,
                    used_kib: 700 * gib,
                    available_kib: 8 * 1024 * gib,
                },
            ],
        };

        let disks = disk_snapshots(&storage, None);
        let mounts = disks
            .iter()
            .map(|disk| disk.mount.as_str())
            .collect::<Vec<_>>();
        assert_eq!(mounts, vec!["/mnt/tank", "/mnt/fast"]);
        assert_eq!(disks[0].used_kib, 2 * 1024 * gib);
    }

    #[test]
    fn low_usage_parent_mount_does_not_hide_real_child_usage() {
        let gib = 1024 * 1024;
        let storage = StorageMetrics {
            root_total_kib: None,
            root_used_kib: None,
            root_available_kib: None,
            disks: vec![
                crate::model::DiskMetrics {
                    mount: "/mnt/tank".to_string(),
                    total_kib: 10 * 1024 * gib,
                    used_kib: 128,
                    available_kib: 10 * 1024 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/tank/shared/media".to_string(),
                    total_kib: 10 * 1024 * gib,
                    used_kib: 1400 * gib,
                    available_kib: 8 * 1024 * gib,
                },
            ],
        };

        let mounts = disk_snapshots(&storage, None)
            .iter()
            .map(|disk| disk.mount.clone())
            .collect::<Vec<_>>();
        assert!(mounts.contains(&"/mnt/tank".to_string()));
        assert!(mounts.contains(&"/mnt/tank/shared/media".to_string()));
    }

    #[test]
    fn disk_aliases_and_max_rows_apply_after_mount_cleanup() {
        let gib = 1024 * 1024;
        let storage = StorageMetrics {
            root_total_kib: None,
            root_used_kib: None,
            root_available_kib: None,
            disks: vec![
                crate::model::DiskMetrics {
                    mount: "/mnt/tank".to_string(),
                    total_kib: 12 * 1024 * gib,
                    used_kib: 4 * 1024 * gib,
                    available_kib: 8 * 1024 * gib,
                },
                crate::model::DiskMetrics {
                    mount: "/mnt/fast".to_string(),
                    total_kib: 472 * gib,
                    used_kib: 97 * gib,
                    available_kib: 375 * gib,
                },
            ],
        };
        let server = ServerConfig::ssh("storage", "Storage", "storage-host")
            .with_disk_max_rows(1)
            .with_disk_alias("/mnt/tank", "tank")
            .with_disk_alias("/mnt/fast", "fast");

        let disks = disk_snapshots(&storage, Some(&server));

        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].mount, "tank");
    }

    #[test]
    fn refresh_interval_adjustment_uses_100ms_steps() {
        assert_eq!(
            adjust_refresh_interval(Duration::from_millis(1_000), -100),
            Duration::from_millis(900)
        );
        assert_eq!(
            adjust_refresh_interval(Duration::from_millis(100), -100),
            Duration::from_millis(100)
        );
        assert_eq!(
            adjust_refresh_interval(Duration::from_millis(60_000), 100),
            Duration::from_millis(60_000)
        );
    }

    #[test]
    fn forced_refresh_poll_does_not_wait_for_old_cadence() {
        assert_eq!(
            refresh_poll_timeout(
                Mode::Live,
                true,
                false,
                Instant::now(),
                Duration::from_millis(1_000),
            ),
            Duration::from_millis(0)
        );
    }

    #[test]
    fn tui_reducer_updates_refresh_interval_and_requests_refresh() {
        let dashboard = AppState {
            title: "test".to_string(),
            generated_at: Utc::now(),
            mode: Mode::Live,
            refresh_interval_ms: 1_000,
            hosts: Vec::new(),
        };
        let mut state = TuiState::new(dashboard, Duration::from_millis(1_000));

        assert_eq!(
            reduce_tui_state(&mut state, TuiAction::AdjustRefresh(-100)),
            TuiControl::Continue
        );

        assert_eq!(state.refresh_interval, Duration::from_millis(900));
        assert_eq!(state.dashboard.refresh_interval_ms, 900);
        assert!(state.force_refresh);
        assert_eq!(
            state.refresh_poll_timeout(Mode::Live),
            Duration::from_millis(0)
        );
    }

    #[test]
    fn refresh_does_not_start_while_collection_is_in_flight() {
        let dashboard = AppState {
            title: "test".to_string(),
            generated_at: Utc::now(),
            mode: Mode::Live,
            refresh_interval_ms: 500,
            hosts: Vec::new(),
        };
        let mut state = TuiState::new(dashboard, Duration::from_millis(500));
        state.last_refresh = Instant::now() - Duration::from_secs(1);
        assert!(state.should_refresh(Mode::Live));

        state.refresh_in_flight = true;
        assert!(
            !state.should_refresh(Mode::Live),
            "UI loop should not start overlapping SSH collection jobs"
        );
    }

    #[test]
    fn tui_reducer_reports_exit_without_mutating_refresh() {
        let dashboard = AppState {
            title: "test".to_string(),
            generated_at: Utc::now(),
            mode: Mode::Live,
            refresh_interval_ms: 1_000,
            hosts: Vec::new(),
        };
        let mut state = TuiState::new(dashboard, Duration::from_millis(1_000));

        assert_eq!(
            reduce_tui_state(&mut state, TuiAction::Exit),
            TuiControl::Exit
        );

        assert_eq!(state.refresh_interval, Duration::from_millis(1_000));
        assert_eq!(state.dashboard.refresh_interval_ms, 1_000);
        assert!(!state.force_refresh);
    }

    #[test]
    fn setup_effects_are_source_aware_and_ssh_only() {
        assert_eq!(
            setup_effect_for_source(&ServerConfig::local("local", "Local")),
            SetupEffect::LocalCollect
        );
        assert_eq!(
            setup_effect_for_source(&ServerConfig::ssh("ssh", "SSH", "ssh-host")),
            SetupEffect::SshKeyProbe {
                host: "ssh-host".to_string()
            }
        );

        let mut proxmox = ServerConfig::local("pve", "PVE");
        proxmox.source = HostSource::proxmox("pve-host");
        assert_eq!(
            setup_effect_for_source(&proxmox),
            SetupEffect::UnsupportedSource("proxmox:pve-host".to_string())
        );
    }

    #[test]
    fn ssh_key_setup_commands_are_source_ssh_only_and_confirmation_gated() {
        assert!(ssh_setup_commands_for_server(&ServerConfig::local("local", "Local")).is_empty());

        let mut proxmox = ServerConfig::local("pve", "PVE");
        proxmox.source = HostSource::proxmox("pve-host");
        assert!(ssh_setup_commands_for_server(&proxmox).is_empty());

        let commands = ssh_setup_commands_for_server(&ServerConfig::ssh("ssh", "SSH", "ssh-host"));
        assert_eq!(
            commands,
            vec![
                SshSetupCommand::GenerateKey,
                SshSetupCommand::CopyKey {
                    host: "ssh-host".to_string()
                }
            ]
        );
        assert!(commands.iter().all(SshSetupCommand::requires_confirmation));
        assert_eq!(commands[0].display_command(), "ssh-keygen -t ed25519");
        assert_eq!(commands[1].display_command(), "ssh-copy-id ssh-host");
    }

    #[test]
    fn config_manager_adds_direct_ssh_and_edits_source_variants() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: Vec::new(),
            },
            PathBuf::from("config.toml"),
        );

        add_ssh_target_server(&mut state, "user@example.com", "user@example.com");

        assert!(matches!(
            state.config.servers[0].source,
            HostSource::Ssh { .. }
        ));
        apply_server_assignment(&mut state, "source=local").unwrap();
        assert_eq!(state.config.servers[0].source, HostSource::Local);
        apply_server_assignment(&mut state, "source=proxmox").unwrap();
        assert!(matches!(
            state.config.servers[0].source,
            HostSource::Proxmox { .. }
        ));
        apply_server_assignment(&mut state, "source=truenas-scale").unwrap();
        assert!(matches!(
            state.config.servers[0].source,
            HostSource::TrueNasScale { .. }
        ));
    }

    #[test]
    fn config_manager_uses_add_wizard_edit_and_space_toggle_shortcuts() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: Vec::new(),
            },
            PathBuf::from("config.toml"),
        );

        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.adding.is_some(), "a should open add wizard");
        assert_eq!(state.config.servers.len(), 0);

        // Enter on the first option switches to manual SSH target input.
        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.adding.as_ref().unwrap().direct_input);
        for ch in "user@example.com".chars() {
            handle_config_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            )
            .unwrap();
        }
        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.adding.is_none());
        assert_eq!(state.config.servers.len(), 1);
        assert_eq!(
            state.config.servers[0].source.endpoint(),
            Some("user@example.com")
        );

        for legacy_source_shortcut in ['l', 'p', 't'] {
            handle_config_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(legacy_source_shortcut), KeyModifiers::NONE),
            )
            .unwrap();
        }
        assert_eq!(
            state.config.servers.len(),
            1,
            "source-specific shortcuts should not add servers"
        );

        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.editing, "e should enter field=value edit mode");
        handle_config_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();

        assert!(state.config.servers[0].enabled);
        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(!state.config.servers[0].enabled);
    }

    #[test]
    fn ssh_config_hosts_are_parsed_for_add_wizard() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host *
  User ignored

Host server-1 server-2
  HostName 192.0.2.10
  User admin

Host !blocked *.wildcard literal?
  HostName ignored

Host storage
  HostName storage.example.com
"#,
        );

        assert_eq!(
            hosts,
            vec![
                SshConfigEntry {
                    alias: "server-1".to_string(),
                    hostname: Some("192.0.2.10".to_string()),
                    user: Some("admin".to_string()),
                },
                SshConfigEntry {
                    alias: "server-2".to_string(),
                    hostname: Some("192.0.2.10".to_string()),
                    user: Some("admin".to_string()),
                },
                SshConfigEntry {
                    alias: "storage".to_string(),
                    hostname: Some("storage.example.com".to_string()),
                    user: None,
                },
            ]
        );
    }

    #[test]
    fn config_manager_plain_c_shows_copy_id_and_ctrl_c_exits() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: vec![ServerConfig::ssh("one", "One", "one-host")],
            },
            PathBuf::from("config.toml"),
        );

        let control = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(control, ConfigManagerControl::Continue);
        assert_eq!(state.message, "Command: ssh-copy-id one-host");
        assert!(state.pending_confirmation.is_none());

        let control = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert_eq!(control, ConfigManagerControl::Exit);
    }

    #[test]
    fn config_manager_prompts_to_save_dirty_config_on_quit() {
        let dir = std::env::temp_dir().join(format!("rktop-dirty-exit-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: Vec::new(),
            },
            path.clone(),
        );

        add_local_server(&mut state);
        assert!(state.is_dirty());
        let control = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(control, ConfigManagerControl::Continue);
        assert_eq!(
            state.pending_confirmation,
            Some(PendingConfirmation::ExitUnsaved)
        );
        assert!(state.message.contains("Unsaved changes"));

        let control =
            handle_config_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert_eq!(control, ConfigManagerControl::Continue);
        assert!(state.pending_confirmation.is_none());
        assert!(state.is_dirty());

        let _ = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        )
        .unwrap();
        let control = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(control, ConfigManagerControl::Exit);
        assert!(!state.is_dirty());
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_manager_can_discard_dirty_config_on_quit() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: Vec::new(),
            },
            PathBuf::from("config.toml"),
        );
        add_local_server(&mut state);

        let _ = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        )
        .unwrap();
        let control = handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        )
        .unwrap();

        assert_eq!(control, ConfigManagerControl::Exit);
        assert_eq!(state.message, "discarded unsaved changes");
    }

    #[test]
    fn config_manager_health_check_reports_local_ok() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: vec![ServerConfig::local("local", "Local")],
            },
            PathBuf::from("config.toml"),
        );

        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(state.message, "health check queued");
        assert!(state.health_refresh_requested);
        run_config_health_check(&mut state);
        assert_eq!(state.message, "health check queued");
        assert_eq!(state.health_message, "health: 1/1 ok");
        assert!(state.pending_confirmation.is_none());
    }

    #[test]
    fn config_manager_server_list_truncates_long_fields_with_ellipsis() {
        let server = ServerConfig::ssh(
            "example-server-prod-long-id",
            "Example Server Very Long Display Name",
            "example-server-with-a-very-long-domain.example.com",
        );
        let line = server_list_line(0, &server, None, true);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains('…'), "rendered line: {rendered}");
        assert!(
            !rendered.contains("Example Server Very Long Display Name"),
            "long name should not push later columns: {rendered}"
        );
        assert!(
            rendered.contains("│"),
            "list should keep visible column separators: {rendered}"
        );
        assert_eq!(fit_text("Example Server", 24), "Example Server");
        assert_eq!(fit_text("Example Server", 10), "Example S…");
    }

    #[test]
    fn optional_unavailable_servers_are_hidden_from_live_collection() {
        let optional = ServerConfig::ssh("optional", "Optional", "bad host");
        let required = ServerConfig::ssh("required", "Required", "bad host").optional(false);

        let optional_config = AppConfig {
            refresh_interval_ms: 1_000,
            servers: vec![optional],
        };
        let optional_hosts = collect_enabled_servers(&optional_config);
        assert!(
            optional_hosts.collected.is_empty(),
            "optional SSH failures should be hidden instead of rendering UNAVAILABLE"
        );
        assert_eq!(optional_hosts.optional_failures, vec!["optional"]);

        let required_config = AppConfig {
            refresh_interval_ms: 1_000,
            servers: vec![required],
        };
        let required_hosts = collect_enabled_servers(&required_config);
        assert_eq!(required_hosts.collected.len(), 1);
        assert!(matches!(
            required_hosts.collected[0].1.status,
            HostStatus::Offline
        ));
    }

    #[test]
    fn config_manager_edit_errors_stay_inside_tui() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: vec![ServerConfig::ssh("one", "One", "one-host")],
            },
            PathBuf::from("config.toml"),
        );

        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.editing, "e should enter field editor");
        assert!(state.edit_value_field.is_none());

        for _ in 0..2 {
            handle_config_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                .unwrap();
        }
        assert_eq!(selected_edit_field(&state), ConfigEditField::Source);

        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(state.edit_value_field, Some(ConfigEditField::Source));

        state.input.clear();
        for ch in "not-a-source".chars() {
            handle_config_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            )
            .unwrap();
        }
        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.editing);
        assert!(state.message.contains("edit error"));

        state.input.clear();
        for ch in "local".chars() {
            handle_config_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            )
            .unwrap();
        }
        handle_config_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();
        assert!(state.editing);
        assert!(state.edit_value_field.is_none());
        assert_eq!(state.config.servers[0].source, HostSource::Local);
    }

    #[test]
    fn config_manager_edits_fields_toggles_reorders_and_deletes() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: vec![
                    ServerConfig::ssh("one", "One", "one-host"),
                    ServerConfig::local("two", "Two"),
                ],
            },
            PathBuf::from("config.toml"),
        );

        apply_server_assignment(&mut state, "name=Primary").unwrap();
        apply_server_assignment(&mut state, "host=primary-host").unwrap();
        apply_server_assignment(&mut state, "disk_max_rows=3").unwrap();
        apply_server_assignment(&mut state, "disk_aliases=/mnt/tank:tank,/mnt/fast:fast").unwrap();
        toggle_selected_enabled(&mut state);
        toggle_selected_optional(&mut state);

        let selected = state.selected_server().unwrap();
        assert_eq!(selected.name, "Primary");
        assert_eq!(selected.source.endpoint(), Some("primary-host"));
        assert_eq!(selected.disk_max_rows, Some(3));
        assert_eq!(
            selected.disk_aliases.get("/mnt/tank").map(String::as_str),
            Some("tank")
        );
        assert!(!selected.enabled);
        assert!(
            !selected.optional,
            "SSH entries default to optional=true, so the optional toggle should turn it off"
        );

        move_selected_server_down(&mut state);
        assert_eq!(state.config.servers[1].id, "one");
        delete_selected_server(&mut state);
        assert_eq!(state.config.servers.len(), 1);
        assert_eq!(state.config.servers[0].id, "two");
    }

    #[test]
    fn config_manager_source_changes_preserve_or_reject_host_by_source() {
        let mut state = ConfigManagerState::new(
            AppConfig {
                refresh_interval_ms: 1_000,
                servers: vec![ServerConfig::local("box", "Box")],
            },
            PathBuf::from("config.toml"),
        );

        apply_server_assignment(&mut state, "source=ssh").unwrap();
        apply_server_assignment(&mut state, "host=box-host").unwrap();
        assert_eq!(state.config.servers[0].source.endpoint(), Some("box-host"));

        apply_server_assignment(&mut state, "source=local").unwrap();
        assert!(apply_server_assignment(&mut state, "host=ignored").is_err());
    }
}

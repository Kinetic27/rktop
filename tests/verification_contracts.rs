use std::{fs, process::Command};

const CONFIG_RS: &str = include_str!("../src/config.rs");
const APP_RS: &str = include_str!("../src/app.rs");
const FIXTURES_RS: &str = include_str!("../src/fixtures.rs");
const RENDER_RS: &str = include_str!("../src/render/mod.rs");
const SSH_RS: &str = include_str!("../src/collectors/ssh.rs");
const LOCAL_RS: &str = include_str!("../src/collectors/local.rs");
const TRUENAS_RS: &str = include_str!("../src/collectors/truenas.rs");
const EXAMPLE_CONFIG: &str = include_str!("../config/server-tui-monitor.example.toml");
const README_MD: &str = include_str!("../README.md");
const BRANCHING_MD: &str = include_str!("../docs/branching.md");
const VERIFICATION_MD: &str = include_str!("../docs/verification.md");
const CI_YML: &str = include_str!("../.github/workflows/ci.yml");

#[derive(Debug)]
struct TomlServer<'a> {
    id: &'a str,
    name: &'a str,
    group: &'a str,
    enabled: bool,
    optional: bool,
}

fn parse_example_servers() -> Vec<TomlServer<'static>> {
    let mut servers = Vec::new();
    let mut current: Option<TomlServer<'static>> = None;

    for raw_line in EXAMPLE_CONFIG.lines() {
        let line = raw_line.trim();
        if line == "[[servers]]" {
            if let Some(server) = current.take() {
                servers.push(server);
            }
            current = Some(TomlServer {
                id: "",
                name: "",
                group: "",
                enabled: true,
                optional: false,
            });
            continue;
        }

        let Some(server) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "id" => server.id = trim_toml_string(value),
            "name" => server.name = trim_toml_string(value),
            "group" => server.group = trim_toml_string(value),
            "enabled" => server.enabled = parse_toml_bool(value),
            "optional" => server.optional = parse_toml_bool(value),
            _ => {}
        }
    }

    if let Some(server) = current.take() {
        servers.push(server);
    }

    servers
}

fn trim_toml_string(value: &'static str) -> &'static str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn parse_toml_bool(value: &str) -> bool {
    match value {
        "true" => true,
        "false" => false,
        other => panic!("example config has non-boolean value: {other}"),
    }
}

fn assert_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        haystack.contains(needle),
        "missing {context}: expected to find `{needle}`"
    );
}

fn assert_not_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        !haystack.contains(needle),
        "forbidden {context}: found `{needle}`"
    );
}

#[test]
fn default_refresh_interval_is_one_second() {
    assert_contains(
        CONFIG_RS,
        "pub const DEFAULT_REFRESH_INTERVAL_MS: u64 = 1_000;",
        "default one-second refresh interval constant",
    );
    assert_contains(
        EXAMPLE_CONFIG,
        "refresh_interval_ms = 1000",
        "example config one-second refresh interval",
    );
}

#[test]
fn main_branch_docs_and_ci_contract_are_consistent() {
    assert_contains(
        README_MD,
        "`main` is stable/release-ready.",
        "README stable branch guidance",
    );
    assert_contains(
        README_MD,
        "merge-back to `main`",
        "README develop-to-main release guidance",
    );
    assert_contains(
        BRANCHING_MD,
        "| `main` | Stable, release-ready branch.",
        "branching docs main release branch role",
    );
    assert_contains(
        BRANCHING_MD,
        "git switch main",
        "release flow should switch to main",
    );
    assert_contains(
        BRANCHING_MD,
        "git push origin main vX.Y.Z",
        "release flow should push main tags",
    );
    assert_contains(
        BRANCHING_MD,
        "branch protection rules",
        "branch-protection deferral note",
    );
    assert_contains(
        CI_YML,
        "branches: [main, develop]",
        "CI push branches should cover stable main and integration develop",
    );
    assert_not_contains(
        CI_YML,
        "branches: [main, master]",
        "CI should not keep the legacy master trigger",
    );
}

#[test]
fn default_config_is_empty_and_first_run_does_not_embed_private_servers() {
    assert_contains(
        CONFIG_RS,
        "servers: Vec::new()",
        "default config should start empty for first-run setup",
    );
    assert!(
        parse_example_servers().is_empty(),
        "checked-in first-run config should not pre-populate any servers"
    );
    assert_contains(
        EXAMPLE_CONFIG,
        "Example server entries",
        "example file should keep optional commented examples without pre-populating them",
    );
    assert_contains(
        APP_RS,
        "Err(_) if server.optional => None",
        "optional SSH failures should hide that host instead of rendering an unavailable card",
    );
}

#[test]
fn fixture_data_is_config_driven_and_populates_required_metric_fields() {
    assert_contains(
        FIXTURES_RS,
        "default_servers()",
        "fixtures must derive host cards from default config servers",
    );
    assert_contains(
        FIXTURES_RS,
        ".filter(|server| server.enabled)",
        "fixture host list must follow enabled default hosts",
    );
    assert_contains(
        FIXTURES_RS,
        "fixture_hosts_with_disabled",
        "fixture helper that can include disabled optional hosts",
    );
    assert_contains(
        FIXTURES_RS,
        "disabled_optional_fixture",
        "explicit disabled optional fixture",
    );

    for field in [
        "usage_percent: Some",
        "load_1m: Some",
        "uptime_seconds: Some",
        "cores: Some",
        "total_kib: Some",
        "available_kib: Some",
        "used_kib: Some",
        "rx_bytes_total: Some",
        "tx_bytes_total: Some",
        "root_total_kib: Some",
        "root_used_kib: Some",
        "root_available_kib: Some",
        "disks:",
    ] {
        assert_contains(FIXTURES_RS, field, "fixture metric population");
    }
    assert_contains(FIXTURES_RS, "fixture_disks", "fixture disk list helper");
    assert_contains(
        FIXTURES_RS,
        "/mnt/tank",
        "storage fixture should expose multiple disks",
    );
}

#[test]
fn collector_exports_multiple_disk_mounts_and_runtime_refresh_is_adjustable() {
    assert_contains(
        LOCAL_RS,
        "disk=%s",
        "local/SSH fixed collector must emit disk mount rows",
    );
    assert_contains(
        LOCAL_RS,
        "/proc/uptime",
        "local/SSH fixed collector must emit uptime seconds",
    );
    assert_contains(
        LOCAL_RS,
        "printf 'uptime_seconds=%s\\n'",
        "collector must print parsed uptime seconds",
    );
    assert_contains(
        LOCAL_RS,
        "cpu_temp_millicelsius",
        "collector should emit CPU temperature when Linux hwmon exposes it",
    );
    assert_contains(
        LOCAL_RS,
        "coretemp|k10temp",
        "collector should limit CPU temperature reads to common CPU hwmon sensors",
    );
    assert_contains(
        RENDER_RS,
        "temperature_text",
        "CPU header should format collected CPU temperature",
    );
    assert_contains(
        RENDER_RS,
        "temperature_color",
        "CPU header should colorize warning/critical temperature values",
    );
    assert_contains(
        APP_RS,
        "temp >= 85.0",
        "critical host status should include high CPU temperature",
    );
    assert_contains(
        RENDER_RS,
        "cpu_temperature_celsius",
        "CPU temperature should be passed into the rendered host snapshot",
    );
    assert_contains(
        RENDER_RS,
        "fn format_uptime",
        "host header should render btop-style uptime instead of a bare clock",
    );
    assert_contains(
        RENDER_RS,
        "up {days}d {hours}:{minutes:02}",
        "uptime should use btop-style day/hour/minute formatting",
    );
    assert_not_contains(
        RENDER_RS,
        r#"host.last_seen.format("%H:%M")"#,
        "host card should not show an ambiguous bare HH:MM timestamp",
    );
    assert_contains(
        RENDER_RS,
        "with_timezone(&chrono::Local)",
        "snapshot timestamps should use the local timezone instead of UTC-only Z output",
    );
    assert_not_contains(
        RENDER_RS,
        r#"generated_at.format("%Y-%m-%dT%H:%M:%SZ")"#,
        "snapshot generated timestamp should not be forced to UTC Z",
    );
    assert_contains(
        LOCAL_RS,
        "zpool list -Hp -o name,size,alloc,free",
        "disk collection should prefer ZFS pool summaries when zpool is available",
    );
    assert_contains(
        LOCAL_RS,
        "df -kP -x tmpfs",
        "disk collection must scan non-pseudo mounts as a fallback",
    );
    assert_contains(
        APP_RS,
        "collapse_child_mounts",
        "ZFS child dataset mounts should collapse under parent pool rows",
    );
    assert_contains(
        APP_RS,
        "visible_mnt_mount",
        "hidden TrueNAS app/system mounts under /mnt should be filtered",
    );
    assert_contains(
        APP_RS,
        "KeyCode::Char('+') | KeyCode::Char('=')",
        "plus/equal key refresh interval adjustment",
    );
    assert_contains(
        APP_RS,
        "KeyCode::Char('-')",
        "minus key refresh interval adjustment",
    );
    assert_contains(
        APP_RS,
        "KeyModifiers::CONTROL",
        "ctrl-c should exit the interactive TUI",
    );
    for key in ["'1'", "'2'", "'3'", "'4'"] {
        assert_not_contains(
            APP_RS,
            &format!("KeyCode::Char({key})"),
            "removed section fold/expand key",
        );
    }
    assert_contains(
        APP_RS,
        ".clamp(100, 60_000)",
        "100ms refresh adjustment floor",
    );
    assert_contains(
        APP_RS,
        "let mut force_refresh = false;",
        "changing refresh interval should force the next collection instead of waiting for the old cadence",
    );
    assert_contains(
        APP_RS,
        "force_refresh = true;",
        "plus/minus refresh changes should trigger an actual refresh",
    );
    assert_contains(
        APP_RS,
        "event::poll(Duration::from_millis(0))",
        "pending key events should be drained before forced collection",
    );
    assert_contains(
        APP_RS,
        "fn refresh_poll_timeout",
        "refresh polling should wake immediately when a refresh is due",
    );
    assert_contains(
        APP_RS,
        "start_dashboard_refresh",
        "live dashboard collection should run outside the TUI input/render loop",
    );
    assert_contains(
        APP_RS,
        "refresh_in_flight",
        "live dashboard should avoid overlapping SSH collection jobs",
    );
    assert_contains(
        APP_RS,
        "try_recv()",
        "live dashboard should poll background collection results without blocking",
    );
    assert_contains(
        APP_RS,
        "std::thread::scope",
        "live host collection should run hosts concurrently so short refresh intervals are not serialized by SSH",
    );
    assert_contains(
        RENDER_RS,
        "Terminal size too small:",
        "small terminal warning should match btop-style wording",
    );
    assert_contains(
        RENDER_RS,
        "Needed for current config:",
        "small terminal warning should show required size",
    );
    assert_contains(
        RENDER_RS,
        "const MIN_TERMINAL_WIDTH: u16 = 80;",
        "minimum terminal width should follow the common btop-style floor",
    );
    assert_contains(
        RENDER_RS,
        "const MIN_TERMINAL_HEIGHT: u16 = 24;",
        "minimum terminal height should follow the common btop-style floor",
    );
    assert_contains(
        RENDER_RS,
        "vertical_braille_graph",
        "CPU/RAM/NET should use multi-row btop-like braille history graphs",
    );
    assert_contains(
        RENDER_RS,
        "progress_bar",
        "disk should use high-contrast block progress rendering",
    );
    assert_not_contains(
        RENDER_RS,
        "DSK ",
        "disk labels should use the same lowercase section style as other metrics",
    );
    assert_not_contains(RENDER_RS, "↳ ", "mount rows should not use indented arrows");
    assert_contains(
        RENDER_RS,
        "disk_mount_line",
        "disk mount rows should have their own aligned column layout",
    );
    assert_contains(
        RENDER_RS,
        "disk_detail_text",
        "disk mount percent and size columns should be fixed-width formatted",
    );
    assert_contains(
        RENDER_RS,
        "disk_detail_widths",
        "disk size columns should be aligned from per-card visible disk widths",
    );
    assert_contains(
        APP_RS,
        "apply_disk_aliases",
        "server config should be able to rename verbose disk mounts before rendering",
    );
    assert_contains(
        CONFIG_RS,
        "disk_max_rows",
        "server config should expose per-host disk row limits",
    );
    assert_contains(
        EXAMPLE_CONFIG,
        "disk_aliases",
        "example config should document disk mount aliases",
    );
    for source in ["local", "ssh", "proxmox", "truenas-scale"] {
        assert_contains(
            CONFIG_RS,
            source,
            "config parser should preserve all documented source variants",
        );
        assert_contains(
            EXAMPLE_CONFIG,
            source,
            "example config should document all source variants",
        );
    }
    assert_contains(
        RENDER_RS,
        "total_width = widths.total",
        "disk total size should be right-aligned to the widest total in the card",
    );
    assert_contains(
        RENDER_RS,
        "net_graph_rows",
        "network throughput should render as a graph, not text only",
    );
    assert!(
        RENDER_RS.find("lines.push(net_line").unwrap()
            < RENDER_RS.find("disk_section_line(host").unwrap(),
        "network graph block should render before disk so disk stays at the bottom"
    );
    assert_contains(
        RENDER_RS,
        "resampled_history_points",
        "history graphs should resample to fill the card width",
    );
    assert_contains(
        RENDER_RS,
        "│",
        "disk mount rows should use vertical column separators",
    );
    assert_not_contains(
        RENDER_RS,
        "mount{}",
        "disk section header should stay minimal; mount counts belong in rows/details, not the header",
    );
    assert_contains(RENDER_RS, "+/- 100ms", "refresh adjustment hint");
    assert_contains(
        RENDER_RS,
        "header_detail_text",
        "overview should use a focused minimal header detail formatter",
    );
    assert_not_contains(
        RENDER_RS,
        "{mode} MODE",
        "overview should not show constant LIVE/MOCK mode text",
    );
    assert_not_contains(
        RENDER_RS,
        "{} attention",
        "overview should not spend space on low-value attention counts",
    );
    assert_not_contains(
        RENDER_RS,
        "{} ok",
        "overview should not spend space on low-value ok counts",
    );
    assert_contains(
        RENDER_RS,
        r#"format!("{ms}ms")"#,
        "refresh must always render in ms",
    );
    assert_contains(RENDER_RS, "q/ctrl-c/esc exits", "ctrl-c exit hint");
    assert_contains(
        RENDER_RS,
        "section_line",
        "btop-like horizontal section dividers",
    );
    assert_contains(
        RENDER_RS,
        "let title = label.to_string();",
        "section labels should be followed directly by rules instead of padded with spaces",
    );
    assert_not_contains(
        RENDER_RS,
        r#"format!("{label} ")"#,
        "section labels must not include a trailing space",
    );
    assert_contains(
        RENDER_RS,
        r#"Span::styled("─""#,
        "section details should use rule separators between values",
    );
    assert_contains(RENDER_RS, "cpu_detail", "CPU detail formatter");
    assert_not_contains(
        RENDER_RS,
        "1m {}",
        "CPU header should stay minimal instead of showing low-value load fields",
    );
    assert_not_contains(
        RENDER_RS,
        "core {c:02}",
        "CPU header should not spend space on core count",
    );
    assert_contains(
        RENDER_RS,
        "column_disk_mount_widths",
        "disk mount label width should be computed per grid column",
    );
    assert_contains(
        RENDER_RS,
        "MAX_DISK_MOUNT_LABEL_WIDTH",
        "adaptive disk mount labels should still have a sane upper bound",
    );
    assert_contains(
        RENDER_RS,
        "size_pair_text",
        "compact rule-separated capacity formatter",
    );
    assert_not_contains(
        RENDER_RS,
        "fn draw_footer",
        "dashboard footer/status panel should stay removed",
    );
}

#[test]
fn first_run_config_flow_is_wired_into_cli() {
    assert_contains(CONFIG_RS, "pub fn load_config", "runtime config loader");
    assert_contains(
        CONFIG_RS,
        "XDG_CONFIG_HOME",
        "default config path should follow XDG_CONFIG_HOME when present",
    );
    assert_contains(
        CONFIG_RS,
        "server-tui-monitor",
        "default app config directory",
    );
    assert_contains(APP_RS, "CommandMode::Init", "stm init command mode");
    assert_contains(
        APP_RS,
        "write_example_config",
        "stm init should write the canonical config",
    );
    assert_contains(APP_RS, "CommandMode::Edit", "rktop raw edit command mode");
    assert_contains(
        APP_RS,
        "CommandMode::ConfigManager",
        "rktop setup/config full-screen config manager mode",
    );
    assert_contains(APP_RS, r#""setup" | "config""#, "setup/config TUI aliases");
    assert_contains(
        APP_RS,
        r#""edit""#,
        "edit should stay a distinct raw editor command",
    );
    assert_contains(
        APP_RS,
        "server-tui-monitor setup [--config PATH]",
        "help output should list setup alias",
    );
    assert_contains(
        APP_RS,
        "full-screen config manager",
        "first-run help should describe setup/config TUI",
    );
    assert_contains(
        APP_RS,
        "fn run_config_manager",
        "rktop setup/config implementation",
    );
    assert_contains(
        APP_RS,
        "fn run_config_tui",
        "setup/config should use a Ratatui alternate-screen TUI",
    );
    assert_contains(APP_RS, "ConfigManagerState", "config manager reducer state");
    assert_contains(APP_RS, "draw_config_manager", "config manager UI shell");
    assert_contains(
        APP_RS,
        "apply_server_assignment",
        "field=value edit reducer",
    );
    assert_contains(
        APP_RS,
        "enter_add_server_wizard",
        "interactive add wizard action",
    );
    assert_contains(
        APP_RS,
        "parse_ssh_config_hosts",
        "add wizard should read SSH config host aliases",
    );
    assert_contains(
        APP_RS,
        "add_ssh_target_server",
        "add wizard should support direct user@host targets",
    );
    assert_contains(
        APP_RS,
        "add_local_server",
        "add wizard should support this-machine local collection",
    );
    assert_contains(
        APP_RS,
        "delete_selected_server",
        "delete action with confirmation path",
    );
    assert_contains(
        APP_RS,
        "move_selected_server_up",
        "server reorder up action",
    );
    assert_contains(
        APP_RS,
        "move_selected_server_down",
        "server reorder down action",
    );
    assert_contains(APP_RS, "toggle_selected_enabled", "enabled toggle action");
    assert_contains(APP_RS, "toggle_selected_optional", "optional toggle action");
    assert_contains(
        APP_RS,
        "save_config_manager_state",
        "canonical config save action",
    );
    assert_contains(
        APP_RS,
        "config_to_toml",
        "setup/config should save canonical config",
    );
    assert_contains(
        APP_RS,
        "EnterAlternateScreen",
        "config manager should use alternate screen",
    );
    assert_contains(
        APP_RS,
        "LeaveAlternateScreen",
        "config manager should restore terminal",
    );
    assert_contains(
        APP_RS,
        "fn edit_config",
        "rktop edit raw editor implementation",
    );
    assert_contains(
        APP_RS,
        "editor_command",
        "rktop edit should respect editor env/fallbacks",
    );
    assert_contains(APP_RS, "VISUAL", "rktop edit should prefer VISUAL");
    assert_contains(APP_RS, "EDITOR", "rktop edit should support EDITOR");
    assert_contains(
        APP_RS,
        "load_config_file(&path)",
        "stm edit should validate config after editor exits",
    );
    assert_contains(APP_RS, "CommandMode::Doctor", "stm doctor command mode");
    assert_contains(
        APP_RS,
        "ssh_probe_command(&host)",
        "doctor should check SSH key auth through the shared non-interactive SSH probe",
    );
    assert_contains(
        APP_RS,
        "ssh_setup_commands_for_server",
        "setup SSH key guidance should be source-aware",
    );
    assert_contains(
        APP_RS,
        "requires_confirmation",
        "ssh-copy-id/keygen guidance must be confirmation-gated",
    );
    assert_contains(
        APP_RS,
        "HostSource::Ssh",
        "ssh-copy-id/keygen guidance must be limited to source=ssh",
    );
    assert_contains(
        SSH_RS,
        "BatchMode=yes",
        "shared SSH builder should keep batch-mode key auth checks non-interactive",
    );
    assert_contains(
        APP_RS,
        "load_config(cli.config_path.as_deref())",
        "normal runs should load config from --config/default path",
    );
    assert_contains(
        EXAMPLE_CONFIG,
        "source = \"ssh\"",
        "example/canonical config should use beginner-friendly compact source syntax",
    );
    assert_contains(
        EXAMPLE_CONFIG,
        "host = \"server-1\"",
        "example config should document editable generic SSH host aliases",
    );
}

#[test]
fn setup_config_and_edit_commands_create_config_and_validate_with_noop_editor() {
    let dir = std::env::temp_dir().join(format!(
        "stm-edit-test-{}-{}",
        std::process::id(),
        unique_test_suffix()
    ));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    for command in ["setup", "config"] {
        let command_config = dir.join(format!("{command}.toml"));
        let output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
            .arg(command)
            .arg("--config")
            .arg(&command_config)
            .env("EDITOR", "true")
            .env_remove("VISUAL")
            .output()
            .unwrap_or_else(|error| panic!("failed to run server-tui-monitor {command}: {error}"));

        assert!(
            output.status.success(),
            "{command} command failed: status={:?}\nstdout={}\nstderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            command_config.exists(),
            "{command} command should create missing config file"
        );
        let written_config =
            fs::read_to_string(&command_config).expect("config should be readable");
        assert_eq!(
            written_config.parse::<toml::Value>().unwrap(),
            EXAMPLE_CONFIG.parse::<toml::Value>().unwrap(),
            "{command} should write a semantically equivalent example config"
        );

        let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
        assert!(
            stdout.contains("config manager requires an interactive terminal"),
            "setup/config should use TUI path with non-terminal fallback. stdout: {stdout}"
        );
        assert!(stdout.contains("config ok:"), "stdout: {stdout}");
    }

    let edit_config = dir.join("edit.toml");
    let edit_output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
        .arg("edit")
        .arg("--config")
        .arg(&edit_config)
        .env("EDITOR", "true")
        .env_remove("VISUAL")
        .output()
        .expect("failed to run server-tui-monitor edit");
    assert!(
        edit_output.status.success(),
        "edit command failed: status={:?}\nstdout={}\nstderr={}",
        edit_output.status.code(),
        String::from_utf8_lossy(&edit_output.stdout),
        String::from_utf8_lossy(&edit_output.stderr)
    );
    assert!(
        edit_config.exists(),
        "edit should create missing config file"
    );
    let edit_stdout = String::from_utf8(edit_output.stdout).expect("stdout must be UTF-8");
    assert!(
        edit_stdout.contains("editing config:"),
        "stdout: {edit_stdout}"
    );
    assert!(
        edit_stdout.contains("editor: true"),
        "stdout: {edit_stdout}"
    );
    assert!(edit_stdout.contains("config ok:"), "stdout: {edit_stdout}");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn setup_ssh_key_guidance_is_source_ssh_only_and_confirmation_gated() {
    let dir = std::env::temp_dir().join(format!(
        "stm-setup-ssh-guidance-test-{}-{}",
        std::process::id(),
        unique_test_suffix()
    ));
    fs::create_dir_all(&dir).expect("failed to create temp dir");

    let local_config = dir.join("local.toml");
    fs::write(
        &local_config,
        r#"refresh_interval_ms = 1000

[[servers]]
id = "local"
name = "Local"
source = "local"
"#,
    )
    .expect("failed to write local config");
    let local_output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
        .args(["setup", "--config"])
        .arg(&local_config)
        .env("EDITOR", "true")
        .env_remove("VISUAL")
        .output()
        .expect("failed to run setup for local config");
    assert!(
        local_output.status.success(),
        "local setup failed: status={:?}\nstdout={}\nstderr={}",
        local_output.status.code(),
        String::from_utf8_lossy(&local_output.stdout),
        String::from_utf8_lossy(&local_output.stderr)
    );
    let local_stdout = String::from_utf8(local_output.stdout).expect("stdout must be UTF-8");
    assert!(
        !local_stdout.contains("ssh-keygen") && !local_stdout.contains("ssh-copy-id"),
        "local-only setup must not print SSH key commands. stdout: {local_stdout}"
    );

    let ssh_config = dir.join("ssh.toml");
    fs::write(
        &ssh_config,
        r#"refresh_interval_ms = 1000

[[servers]]
id = "ssh"
name = "SSH"
source = "ssh"
host = "ssh-host"
"#,
    )
    .expect("failed to write ssh config");
    let ssh_output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
        .args(["setup", "--config"])
        .arg(&ssh_config)
        .env("EDITOR", "true")
        .env_remove("VISUAL")
        .output()
        .expect("failed to run setup for ssh config");
    assert!(
        ssh_output.status.success(),
        "ssh setup failed: status={:?}\nstdout={}\nstderr={}",
        ssh_output.status.code(),
        String::from_utf8_lossy(&ssh_output.stdout),
        String::from_utf8_lossy(&ssh_output.stderr)
    );
    let ssh_stdout = String::from_utf8(ssh_output.stdout).expect("stdout must be UTF-8");
    let _ = fs::remove_dir_all(dir);
    assert!(
        ssh_stdout.contains("after confirming each source = \"ssh\" target"),
        "ssh setup should require confirmation. stdout: {ssh_stdout}"
    );
    assert!(
        ssh_stdout.contains("does not run ssh-keygen or ssh-copy-id for you"),
        "ssh setup must be guidance only, not execution. stdout: {ssh_stdout}"
    );
    assert!(
        ssh_stdout.contains("ssh-keygen -t ed25519") && ssh_stdout.contains("ssh-copy-id ssh-host"),
        "ssh setup should print SSH-only manual commands. stdout: {ssh_stdout}"
    );
}

fn unique_test_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

#[test]
fn doctor_ssh_probe_is_runtime_noninteractive_and_does_not_setup_keys() {
    let dir = std::env::temp_dir().join(format!(
        "stm-doctor-ssh-test-{}-{}",
        std::process::id(),
        unique_test_suffix()
    ));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create fake bin dir");
    let log_path = dir.join("ssh-argv.log");
    let fake_ssh = bin_dir.join("ssh");
    fs::write(
        &fake_ssh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case " $* " in
  *' ssh-copy-id '*|*' ssh-keygen '*|*' authorized_keys '*|*' sudo '*|*' apt-get '*|*' systemctl '*)
    echo forbidden setup command >&2
    exit 97
    ;;
esac
case " $* " in
  *' true') exit 0 ;;
esac
cat <<'METRICS'
hostname=fake-ssh-host
kernel=Linux fake
uptime_seconds=42
loadavg=0.01 0.02 0.03 1/1 1
cpu_cores=2
mem_total_kib=1024
mem_available_kib=512
net_rx_bytes=10
net_tx_bytes=20
root_total_kib=2048
root_used_kib=1024
root_available_kib=1024
METRICS
"#,
            log_path.display()
        ),
    )
    .expect("failed to write fake ssh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&fake_ssh)
            .expect("failed to stat fake ssh")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_ssh, permissions).expect("failed to chmod fake ssh");
    }

    let config = dir.join("config.toml");
    fs::write(
        &config,
        r#"refresh_interval_ms = 1000

[[servers]]
id = "server-1"
name = "Server 1"
source = "ssh"
host = "fake-host"
"#,
    )
    .expect("failed to write temp config");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    let output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
        .args(["doctor", "--config"])
        .arg(&config)
        .env("PATH", path)
        .output()
        .expect("failed to run server-tui-monitor doctor");

    assert!(
        output.status.success(),
        "doctor command failed: status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(&log_path).expect("fake ssh should record argv");
    let _ = fs::remove_dir_all(dir);
    assert!(log.contains("BatchMode=yes"), "ssh argv log: {log}");
    assert!(
        log.contains("PasswordAuthentication=no"),
        "ssh argv log: {log}"
    );
    assert!(
        log.contains("KbdInteractiveAuthentication=no"),
        "ssh argv log: {log}"
    );
    assert!(
        log.contains("NumberOfPasswordPrompts=0"),
        "ssh argv log: {log}"
    );
    assert!(log.contains("ConnectionAttempts=1"), "ssh argv log: {log}");
    for forbidden in [
        "ssh-copy-id",
        "ssh-keygen",
        "authorized_keys",
        "sudo",
        "apt-get",
        "systemctl",
    ] {
        assert!(
            !log.contains(forbidden),
            "doctor/live SSH must not run setup command `{forbidden}`. ssh argv log: {log}"
        );
    }
}

#[test]
fn mock_snapshot_cli_includes_enabled_host_names_and_metric_labels() {
    let dir = std::env::temp_dir().join(format!(
        "stm-snapshot-test-{}-{}",
        std::process::id(),
        unique_test_suffix()
    ));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    let config = dir.join("config.toml");
    fs::write(
        &config,
        r#"refresh_interval_ms = 1000

[[servers]]
id = "server-1"
name = "Server 1"
group = "Compute"
role = "Example Linux host"
source = "ssh"
host = "server-1"

[[servers]]
id = "local"
name = "Local"
source = "local"

[[servers]]
id = "storage"
name = "Storage"
group = "Storage"
role = "Example storage host"
source = "ssh"
host = "storage"
"#,
    )
    .expect("failed to write temp snapshot config");

    let output = Command::new(env!("CARGO_BIN_EXE_server-tui-monitor"))
        .args(["--mock", "--snapshot", "--config"])
        .arg(&config)
        .output()
        .expect("failed to run server-tui-monitor --mock --snapshot");

    assert!(
        output.status.success(),
        "mock snapshot command failed: status={:?}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("snapshot stdout must be UTF-8");
    let _ = fs::remove_dir_all(dir);
    for host in ["Server 1", "Local", "Storage"] {
        assert!(
            stdout.contains(host),
            "mock snapshot missing enabled host `{host}`. Snapshot output:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("server-1\n"),
        "mock snapshot should use display labels, not raw SSH aliases. Snapshot output:\n{stdout}"
    );

    for label in [
        "cpu=", "temp=", "load=", "cores=", "ram=", "storage=", "disks=", "net=", "host=",
        "kernel=", "uptime=",
    ] {
        assert!(
            stdout.contains(label),
            "mock snapshot missing required metric label `{label}`. Snapshot output:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("/s"),
        "network values should be current down/up speed, not cumulative traffic totals. Snapshot output:\n{stdout}"
    );
}

#[test]
fn truenas_scaffold_is_json_rpc_websocket_read_only_and_not_deprecated_rest() {
    let truenas = TRUENAS_RS.to_ascii_lowercase();
    assert!(
        truenas.contains("json-rpc") || truenas.contains("json_rpc") || truenas.contains("jsonrpc"),
        "TrueNAS scaffold must name JSON-RPC as the MVP API contract"
    );
    assert!(
        truenas.contains("websocket") || truenas.contains("web socket"),
        "TrueNAS scaffold must name WebSocket as the MVP transport"
    );

    for forbidden in [
        "/api/v2.0",
        "/api/v1.0",
        "rest/v1",
        "rest/v2",
        "reqwest",
        "curl",
        "http://",
        "https://",
        "post /api",
        "get /api",
    ] {
        assert!(
            !truenas.contains(forbidden),
            "TrueNAS MVP adapter must not use or reference deprecated REST path `{forbidden}`"
        );
    }

    for forbidden_method in [
        ".create",
        ".update",
        ".delete",
        ".set",
        ".restart",
        ".shutdown",
        ".reboot",
        ".start",
        ".stop",
    ] {
        assert!(
            !truenas.contains(forbidden_method),
            "TrueNAS scaffold must expose only read/query-style methods; found forbidden method pattern `{forbidden_method}`"
        );
    }
}

#[test]
fn live_ssh_monitoring_contract_is_read_only_and_noninteractive() {
    assert_contains(SSH_RS, "pub fn ssh_command", "public SSH command builder");
    assert_contains(
        SSH_RS,
        "SSH_OPTIONS",
        "shared non-interactive SSH option table",
    );
    assert_contains(
        SSH_RS,
        "pub fn ssh_probe_command",
        "shared SSH probe command builder for setup/doctor",
    );
    assert_contains(
        APP_RS,
        "ssh_probe_command(&host)",
        "doctor must use the shared SSH probe builder",
    );
    assert_contains(
        APP_RS,
        "SetupEffect::SshKeyProbe",
        "source-aware setup effects should isolate SSH-only checks",
    );
    assert_not_contains(
        APP_RS,
        "Command::new(\"ssh\")",
        "doctor must not duplicate SSH command construction inline",
    );
    assert_contains(SSH_RS, "BatchMode=yes", "SSH batch-mode safety option");
    assert_contains(
        SSH_RS,
        "PasswordAuthentication=no",
        "SSH password-auth disabled option",
    );
    assert_contains(
        SSH_RS,
        "KbdInteractiveAuthentication=no",
        "SSH keyboard-interactive auth disabled option",
    );
    assert_contains(SSH_RS, "ConnectTimeout=5", "SSH bounded connect timeout");
    assert_contains(
        SSH_RS,
        "crate::collectors::local::FIXED_COLLECT_COMMAND",
        "SSH collector must use the fixed local metrics command",
    );
    assert_contains(
        LOCAL_RS,
        "pub const FIXED_COLLECT_COMMAND",
        "fixed local metrics command constant",
    );

    for safety_option in [
        "-n",
        "BatchMode=yes",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "ConnectionAttempts=1",
        "NumberOfPasswordPrompts=0",
    ] {
        assert_contains(
            SSH_RS,
            safety_option,
            "shared SSH probe/collector builder should use non-interactive safety options",
        );
    }

    for forbidden in [
        "ssh-copy-id",
        "ssh-keygen",
        "authorized_keys",
        "apt ",
        "apt-get",
        "dnf ",
        "yum ",
        "sudo ",
        "systemctl",
        "reboot",
        "shutdown",
        "rm -rf",
    ] {
        assert!(
            !SSH_RS.contains(forbidden) && !LOCAL_RS.contains(forbidden),
            "live collectors must remain read-only/noninteractive; found forbidden pattern `{forbidden}`"
        );
    }

    for expected_doc in [
        "No remote writes, installs, or credential prompts are performed.",
        "Tests do not open live SSH connections or require remote credentials.",
        "Live SSH/manual checks are intentionally outside automated tests",
    ] {
        assert!(
            README_MD.contains(expected_doc) || VERIFICATION_MD.contains(expected_doc),
            "missing read-only SSH verification documentation: `{expected_doc}`"
        );
    }

    for forbidden in [
        "server.command",
        "config.command",
        "remote_command",
        "custom_command",
        "CommandConfig",
    ] {
        assert!(
            !SSH_RS.contains(forbidden) && !CONFIG_RS.contains(forbidden),
            "SSH collector/config must not accept arbitrary commands from config; found `{forbidden}`"
        );
    }
}

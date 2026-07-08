use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::model::HostSource;

pub const DEFAULT_REFRESH_INTERVAL_MS: u64 = 1_000;
pub const MIN_REFRESH_INTERVAL_MS: u64 = 100;
pub const MAX_REFRESH_INTERVAL_MS: u64 = 60_000;
pub const DEFAULT_DISK_MAX_ROWS: usize = 8;
pub const APP_CONFIG_DIR: &str = "rktop";
pub const LEGACY_APP_CONFIG_DIR: &str = "server-tui-monitor";
pub const APP_CONFIG_FILE: &str = "config.toml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub refresh_interval_ms: u64,
    pub servers: Vec<ServerConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub source: HostSource,
    pub group: Option<String>,
    pub role: Option<String>,
    pub enabled: bool,
    pub optional: bool,
    pub disk_max_rows: Option<usize>,
    pub disk_aliases: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigOrigin {
    BuiltIn { default_path: Option<PathBuf> },
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub origin: ConfigOrigin,
}

impl ServerConfig {
    pub fn local(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            source: HostSource::local(),
            group: None,
            role: None,
            enabled: true,
            optional: false,
            disk_max_rows: None,
            disk_aliases: BTreeMap::new(),
        }
    }

    pub fn ssh(id: impl Into<String>, name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            source: HostSource::ssh(host),
            group: None,
            role: None,
            enabled: true,
            optional: true,
            disk_max_rows: None,
            disk_aliases: BTreeMap::new(),
        }
    }

    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = optional;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_disk_max_rows(mut self, rows: usize) -> Self {
        self.disk_max_rows = Some(rows);
        self
    }

    pub fn with_disk_alias(mut self, mount: impl Into<String>, alias: impl Into<String>) -> Self {
        self.disk_aliases.insert(mount.into(), alias.into());
        self
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        default_config()
    }
}

pub fn default_config() -> AppConfig {
    AppConfig {
        refresh_interval_ms: DEFAULT_REFRESH_INTERVAL_MS,
        servers: Vec::new(),
    }
}

pub fn default_servers() -> Vec<ServerConfig> {
    default_config().servers
}

pub fn enabled_servers(config: &AppConfig) -> impl Iterator<Item = &ServerConfig> {
    config.servers.iter().filter(|server| server.enabled)
}

pub fn default_config_path() -> Option<PathBuf> {
    default_user_config_path()
}

pub fn default_user_config_path() -> Option<PathBuf> {
    let config_home = default_config_home()?;
    Some(config_home.join(APP_CONFIG_DIR).join(APP_CONFIG_FILE))
}

pub fn system_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    Some(
        PathBuf::from("/etc")
            .join(APP_CONFIG_DIR)
            .join(APP_CONFIG_FILE),
    )
}

fn legacy_user_config_path() -> Option<PathBuf> {
    let config_home = default_config_home()?;
    Some(
        config_home
            .join(LEGACY_APP_CONFIG_DIR)
            .join(APP_CONFIG_FILE),
    )
}

pub fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [
        env_config_path(),
        default_user_config_path(),
        system_config_path(),
        legacy_user_config_path(),
    ]
    .into_iter()
    .flatten()
    {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn env_config_path() -> Option<PathBuf> {
    env::var_os("RKTOP_CONFIG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("APPDATA")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .or_else(|| {
            env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
}

pub fn load_config(explicit_path: Option<&Path>) -> Result<LoadedConfig> {
    match explicit_path {
        Some(path) => load_config_file(path).map(|config| LoadedConfig {
            config,
            origin: ConfigOrigin::File(path.to_path_buf()),
        }),
        None => {
            for path in config_search_paths() {
                if path.exists() {
                    return load_config_file(&path).map(|config| LoadedConfig {
                        config,
                        origin: ConfigOrigin::File(path),
                    });
                }
            }
            Ok(LoadedConfig {
                config: default_config(),
                origin: ConfigOrigin::BuiltIn {
                    default_path: default_user_config_path(),
                },
            })
        }
    }
}

pub fn load_config_file(path: &Path) -> Result<AppConfig> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    parse_config_toml(&input)
        .with_context(|| format!("failed to parse config file {}", path.display()))
}

pub fn write_example_config(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "config already exists: {}\nrerun with --force to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    fs::write(path, default_config_toml())
        .with_context(|| format!("failed to write config file {}", path.display()))
}

pub fn parse_config_toml(input: &str) -> Result<AppConfig> {
    let file: FileConfig = toml::from_str(input)?;
    file.into_app_config()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    refresh_interval_ms: Option<u64>,
    refresh_interval_secs: Option<u64>,
    #[serde(default)]
    servers: Vec<FileServer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileServer {
    id: String,
    name: String,
    group: Option<String>,
    role: Option<String>,
    enabled: Option<bool>,
    optional: Option<bool>,
    disk_max_rows: Option<usize>,
    disk_aliases: Option<BTreeMap<String, String>>,
    source: SourceDef,
    host: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SourceDef {
    Kind(String),
    Table {
        #[serde(rename = "type")]
        kind: String,
        host: Option<String>,
    },
}

impl FileConfig {
    fn into_app_config(self) -> Result<AppConfig> {
        let refresh_interval_ms = match (self.refresh_interval_ms, self.refresh_interval_secs) {
            (Some(ms), None) => ms,
            (None, Some(secs)) => secs.saturating_mul(1_000),
            (None, None) => DEFAULT_REFRESH_INTERVAL_MS,
            (Some(_), Some(_)) => {
                bail!("use only one of refresh_interval_ms or refresh_interval_secs, not both")
            }
        }
        .clamp(MIN_REFRESH_INTERVAL_MS, MAX_REFRESH_INTERVAL_MS);

        let servers = self
            .servers
            .into_iter()
            .map(FileServer::into_server_config)
            .collect::<Result<Vec<_>>>()?;

        Ok(AppConfig {
            refresh_interval_ms,
            servers,
        })
    }
}

impl FileServer {
    fn into_server_config(self) -> Result<ServerConfig> {
        require_safe_id(&self.id)?;
        if self.name.trim().is_empty() {
            bail!("server `{}` has an empty name", self.id);
        }

        let (kind, table_host) = match self.source {
            SourceDef::Kind(kind) => (kind, None),
            SourceDef::Table { kind, host } => (kind, host),
        };
        let host = self.host.or(table_host);
        let source = match kind.as_str() {
            "local" => {
                if host.is_some() {
                    bail!("local server `{}` must not set host", self.id);
                }
                HostSource::local()
            }
            "ssh" => HostSource::ssh(required_host(&self.id, host)?),
            "proxmox" => HostSource::proxmox(required_host(&self.id, host)?),
            "truenas-scale" | "truenas" => {
                HostSource::truenas_scale(required_host(&self.id, host)?)
            }
            other => bail!("server `{}` uses unsupported source `{other}`", self.id),
        };

        let default_optional = matches!(
            source,
            HostSource::Ssh { .. } | HostSource::Proxmox { .. } | HostSource::TrueNasScale { .. }
        );

        Ok(ServerConfig {
            id: self.id,
            name: self.name,
            source,
            group: self.group,
            role: self.role,
            enabled: self.enabled.unwrap_or(true),
            optional: self.optional.unwrap_or(default_optional),
            disk_max_rows: self.disk_max_rows,
            disk_aliases: self.disk_aliases.unwrap_or_default(),
        })
    }
}

fn required_host(id: &str, host: Option<String>) -> Result<String> {
    let host = host.ok_or_else(|| anyhow!("server `{id}` source requires host"))?;
    if host.trim().is_empty() {
        bail!("server `{id}` has an empty host");
    }
    Ok(host)
}

fn require_safe_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if valid {
        Ok(())
    } else {
        bail!("server id must use only ASCII letters, numbers, '-' or '_': {id:?}")
    }
}

pub fn default_config_toml() -> String {
    canonical_config_toml(&default_config())
}

pub fn canonical_config_toml(config: &AppConfig) -> String {
    let mut output = String::from(
        "# rktop config\n\
# User config: ~/.config/rktop/config.toml\n\
# System config fallback: /etc/rktop/config.toml\n\
#\n\
# source can be:\n\
#   \"local\"          collect this machine directly\n\
#   \"ssh\"            collect via non-interactive SSH key auth\n\
#\n\
# SSH hosts should be aliases from ~/.ssh/config or user@host values that work with:\n\
#   ssh -o BatchMode=yes <host> true\n\
#\n\
# Optional per-server display tuning:\n\
#   disk_max_rows = 6\n\
#   disk_aliases = { \"/mnt/fast\" = \"fast\", \"/mnt/tank\" = \"tank\" }\n\n",
    );
    output.push_str(&format!(
        "refresh_interval_ms = {}\n",
        config.refresh_interval_ms
    ));

    for server in &config.servers {
        output.push_str("\n[[servers]]\n");
        push_toml_string(&mut output, "id", &server.id);
        push_toml_string(&mut output, "name", &server.name);
        if let Some(group) = &server.group {
            push_toml_string(&mut output, "group", group);
        }
        if let Some(role) = &server.role {
            push_toml_string(&mut output, "role", role);
        }
        output.push_str(&format!("enabled = {}\n", server.enabled));
        output.push_str(&format!("optional = {}\n", server.optional));
        if let Some(rows) = server.disk_max_rows {
            output.push_str(&format!("disk_max_rows = {rows}\n"));
        }
        if !server.disk_aliases.is_empty() {
            output.push_str("disk_aliases = { ");
            for (index, (mount, alias)) in server.disk_aliases.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!(
                    "\"{}\" = \"{}\"",
                    escape_toml_string(mount),
                    escape_toml_string(alias)
                ));
            }
            output.push_str(" }\n");
        }
        push_toml_string(&mut output, "source", server.source.kind());
        if let Some(host) = server.source.endpoint() {
            push_toml_string(&mut output, "host", host);
        }
    }

    output
}

pub fn config_to_toml(config: &AppConfig) -> String {
    canonical_config_toml(config)
}

pub fn example_config_toml() -> String {
    default_config_toml()
}

fn push_toml_string(output: &mut String, key: &str, value: &str) {
    output.push_str(&format!("{key} = \"{}\"\n", escape_toml_string(value)));
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn temp_config_home(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "rktop-config-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn with_config_env<T>(xdg_config_home: &Path, action: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        let old_xdg = env::var_os("XDG_CONFIG_HOME");
        let old_appdata = env::var_os("APPDATA");
        let old_home = env::var_os("HOME");
        let old_userprofile = env::var_os("USERPROFILE");
        let old_rktop = env::var_os("RKTOP_CONFIG");

        unsafe {
            env::set_var("XDG_CONFIG_HOME", xdg_config_home);
            env::remove_var("APPDATA");
            env::remove_var("HOME");
            env::remove_var("USERPROFILE");
            env::remove_var("RKTOP_CONFIG");
        }

        let result = action();

        unsafe {
            restore_env("XDG_CONFIG_HOME", old_xdg);
            restore_env("APPDATA", old_appdata);
            restore_env("HOME", old_home);
            restore_env("USERPROFILE", old_userprofile);
            restore_env("RKTOP_CONFIG", old_rktop);
        }

        result
    }

    unsafe fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => unsafe { env::set_var(key, value) },
            None => unsafe { env::remove_var(key) },
        }
    }

    #[test]
    fn parses_example_config() {
        let config = parse_config_toml(&example_config_toml()).unwrap();
        assert_eq!(config.refresh_interval_ms, 1_000);
        assert!(
            config.servers.is_empty(),
            "first-run default config must not embed any private or environment-specific servers"
        );
    }

    #[test]
    fn canonical_default_config_matches_checked_in_example() {
        assert_eq!(
            parse_config_toml(&config_to_toml(&default_config())).unwrap(),
            parse_config_toml(&example_config_toml()).unwrap()
        );
    }

    #[test]
    fn default_user_config_path_uses_rktop_directory() {
        let home = temp_config_home("default-path");
        with_config_env(&home, || {
            assert_eq!(
                default_user_config_path().unwrap(),
                home.join("rktop").join("config.toml")
            );
            assert_eq!(
                default_config_path().unwrap(),
                default_user_config_path().unwrap()
            );
            assert!(config_search_paths().contains(&home.join("rktop").join("config.toml")));
            assert!(
                config_search_paths()
                    .contains(&home.join("server-tui-monitor").join("config.toml"))
            );
        });
    }

    #[test]
    fn load_config_prefers_new_user_path_over_legacy_path() {
        let home = temp_config_home("new-wins");
        with_config_env(&home, || {
            let new_path = home.join("rktop").join("config.toml");
            let legacy_path = home.join("server-tui-monitor").join("config.toml");
            fs::create_dir_all(new_path.parent().unwrap()).unwrap();
            fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
            fs::write(
                &new_path,
                r#"refresh_interval_ms = 1500

[[servers]]
id = "new"
name = "New"
source = "ssh"
host = "new-host"
"#,
            )
            .unwrap();
            fs::write(
                &legacy_path,
                r#"refresh_interval_ms = 2500

[[servers]]
id = "legacy"
name = "Legacy"
source = "ssh"
host = "legacy-host"
"#,
            )
            .unwrap();

            let loaded = load_config(None).unwrap();
            assert_eq!(loaded.origin, ConfigOrigin::File(new_path));
            assert_eq!(loaded.config.refresh_interval_ms, 1500);
            assert_eq!(loaded.config.servers[0].id, "new");
        });
    }

    #[test]
    fn load_config_reads_legacy_path_when_new_path_is_missing() {
        let home = temp_config_home("legacy");
        with_config_env(&home, || {
            let legacy_path = home.join("server-tui-monitor").join("config.toml");
            fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
            fs::write(
                &legacy_path,
                r#"refresh_interval_ms = 2500

[[servers]]
id = "legacy"
name = "Legacy"
source = "ssh"
host = "legacy-host"
"#,
            )
            .unwrap();

            let loaded = load_config(None).unwrap();
            assert_eq!(loaded.origin, ConfigOrigin::File(legacy_path));
            assert_eq!(loaded.config.refresh_interval_ms, 2500);
            assert_eq!(loaded.config.servers[0].id, "legacy");
        });
    }

    #[test]
    fn remote_servers_default_to_optional_and_local_servers_do_not() {
        let config = parse_config_toml(
            r#"refresh_interval_ms = 1000

[[servers]]
id = "local"
name = "Local"
source = "local"

[[servers]]
id = "remote"
name = "Remote"
source = "ssh"
host = "remote-host"

[[servers]]
id = "required"
name = "Required"
source = "ssh"
host = "required-host"
optional = false
"#,
        )
        .unwrap();

        assert!(!config.servers[0].optional);
        assert!(
            config.servers[1].optional,
            "SSH servers should hide by default when powered off or unreachable"
        );
        assert!(!config.servers[2].optional);
        assert!(ServerConfig::ssh("ssh", "SSH", "ssh-host").optional);
        assert!(!ServerConfig::local("local", "Local").optional);
    }

    #[test]
    fn canonical_config_writer_escapes_strings_and_round_trips() {
        let config = AppConfig {
            refresh_interval_ms: 250,
            servers: vec![
                ServerConfig::ssh("box", "Box \"A\"", "host\\alias")
                    .with_group("Rack")
                    .with_disk_alias("/mnt/quote\"", "quote\""),
            ],
        };

        let toml = canonical_config_toml(&config);
        assert!(toml.contains("name = \"Box \\\"A\\\"\""));
        assert!(toml.contains("host = \"host\\\\alias\""));
        assert_eq!(parse_config_toml(&toml).unwrap(), config);
    }

    #[test]
    fn parses_compact_source_syntax() {
        let config = parse_config_toml(
            r#"
refresh_interval_ms = 500

[[servers]]
id = "box"
name = "Box"
source = "ssh"
host = "Box"
disk_max_rows = 4
disk_aliases = { "/mnt/tank" = "tank" }
"#,
        )
        .unwrap();
        assert_eq!(config.refresh_interval_ms, 500);
        assert_eq!(config.servers[0].source, HostSource::ssh("Box"));
        assert_eq!(config.servers[0].disk_max_rows, Some(4));
        assert_eq!(
            config.servers[0]
                .disk_aliases
                .get("/mnt/tank")
                .map(String::as_str),
            Some("tank")
        );
    }

    #[test]
    fn preserves_local_ssh_proxmox_and_truenas_source_variants() {
        let config = parse_config_toml(
            r#"
refresh_interval_ms = 1000

[[servers]]
id = "local"
name = "Local"
source = "local"

[[servers]]
id = "ssh"
name = "SSH"
source = "ssh"
host = "ssh-host"

[[servers]]
id = "pve"
name = "Proxmox"
source = { type = "proxmox", host = "https://pve.example.invalid:8006" }

[[servers]]
id = "truenas"
name = "TrueNAS"
source = { type = "truenas", host = "https://truenas.example.invalid" }

[[servers]]
id = "truenas-scale"
name = "TrueNAS SCALE"
source = "truenas-scale"
host = "https://truenas-scale.example.invalid"
"#,
        )
        .unwrap();

        assert_eq!(config.servers[0].source, HostSource::Local);
        assert_eq!(config.servers[1].source, HostSource::ssh("ssh-host"));
        assert_eq!(
            config.servers[2].source,
            HostSource::proxmox("https://pve.example.invalid:8006")
        );
        assert_eq!(
            config.servers[3].source,
            HostSource::truenas_scale("https://truenas.example.invalid")
        );
        assert_eq!(
            config.servers[4].source,
            HostSource::truenas_scale("https://truenas-scale.example.invalid")
        );
    }

    #[test]
    fn clamps_refresh_interval_from_config() {
        let config = parse_config_toml(
            r#"
refresh_interval_ms = 1

[[servers]]
id = "local"
name = "Local"
source = "local"
"#,
        )
        .unwrap();
        assert_eq!(config.refresh_interval_ms, MIN_REFRESH_INTERVAL_MS);
    }

    #[test]
    fn rejects_config_with_both_refresh_units() {
        let error = parse_config_toml(
            r#"
refresh_interval_ms = 1000
refresh_interval_secs = 1

[[servers]]
id = "local"
name = "Local"
source = "local"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("use only one"));
    }
}

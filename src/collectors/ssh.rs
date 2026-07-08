use std::process::Command;

use crate::collectors::{CollectorError, metrics_from_key_values};
use crate::config::ServerConfig;
use crate::model::{HostMetrics, HostSource};

const SSH_OPTIONS: &[(&str, &str)] = &[
    ("BatchMode", "yes"),
    ("PasswordAuthentication", "no"),
    ("KbdInteractiveAuthentication", "no"),
    ("ConnectTimeout", "5"),
    ("ConnectionAttempts", "1"),
    ("NumberOfPasswordPrompts", "0"),
];

const SSH_MULTIPLEX_OPTIONS: &[(&str, &str)] = &[
    ("ControlMaster", "auto"),
    ("ControlPersist", "10m"),
    ("ControlPath", "~/.ssh/rktop-%C"),
];

pub fn collect(server: &ServerConfig, host: &str) -> Result<HostMetrics, CollectorError> {
    validate_ssh_host(host)?;
    let output = ssh_command(host).output()?;
    if !output.status.success() {
        return Err(CollectorError::CommandFailed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(metrics_from_key_values(
        server,
        HostSource::ssh(host.to_string()),
        &stdout,
    ))
}

pub fn ssh_command(host: &str) -> Command {
    let mut command = base_ssh_command(host);
    command
        .arg("sh")
        .arg("-c")
        .arg(crate::collectors::local::FIXED_COLLECT_COMMAND);
    command
}

pub fn ssh_probe_command(host: &str) -> Command {
    let mut command = base_ssh_command(host);
    command.arg("true");
    command
}

fn base_ssh_command(host: &str) -> Command {
    let mut command = Command::new("ssh");
    command.arg("-n");
    for (key, value) in SSH_OPTIONS {
        command.arg("-o").arg(format!("{key}={value}"));
    }
    if !cfg!(windows) {
        for (key, value) in SSH_MULTIPLEX_OPTIONS {
            command.arg("-o").arg(format!("{key}={value}"));
        }
    }
    command.arg(host);
    command
}

pub fn validate_ssh_host(host: &str) -> Result<(), CollectorError> {
    let valid = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '@'));

    if valid {
        Ok(())
    } else {
        Err(CollectorError::UnsupportedSource(format!(
            "invalid SSH host alias: {host:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ssh_collector_uses_non_interactive_fixed_command() {
        super::validate_ssh_host("ExampleHost").unwrap();
        let debug = format!("{:?}", super::ssh_command("ExampleHost"));
        assert!(debug.contains("BatchMode=yes"));
        assert!(debug.contains("PasswordAuthentication=no"));
        assert!(debug.contains("KbdInteractiveAuthentication=no"));
        assert!(debug.contains("ConnectTimeout=5"));
        assert!(debug.contains("ConnectionAttempts=1"));
        assert!(debug.contains("NumberOfPasswordPrompts=0"));
        assert!(debug.contains("ExampleHost"));
        assert!(debug.contains("/proc/loadavg"));
    }

    #[test]
    fn ssh_probe_uses_shared_non_interactive_options_without_collection_script() {
        super::validate_ssh_host("ExampleHost").unwrap();
        let debug = format!("{:?}", super::ssh_probe_command("ExampleHost"));
        assert!(debug.contains("BatchMode=yes"));
        assert!(debug.contains("PasswordAuthentication=no"));
        assert!(debug.contains("KbdInteractiveAuthentication=no"));
        assert!(debug.contains("ConnectTimeout=5"));
        assert!(debug.contains("ConnectionAttempts=1"));
        assert!(debug.contains("NumberOfPasswordPrompts=0"));
        assert!(debug.contains("ExampleHost"));
        assert!(debug.contains("true"));
        assert!(!debug.contains("/proc/loadavg"));
    }

    #[test]
    fn ssh_multiplexing_is_disabled_on_windows() {
        let debug = format!("{:?}", super::ssh_command("ExampleHost"));
        if cfg!(windows) {
            assert!(!debug.contains("ControlMaster=auto"));
            assert!(!debug.contains("ControlPersist=10m"));
            assert!(!debug.contains("ControlPath=~/.ssh/rktop-%C"));
        } else {
            assert!(debug.contains("ControlMaster=auto"));
            assert!(debug.contains("ControlPersist=10m"));
            assert!(debug.contains("ControlPath=~/.ssh/rktop-%C"));
        }
    }

    #[test]
    fn rejects_option_like_hosts() {
        assert!(super::validate_ssh_host("-oProxyCommand=x").is_err());
        assert!(super::validate_ssh_host("host name").is_err());
    }
}

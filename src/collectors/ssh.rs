use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use crate::collectors::{CollectorError, metrics_from_key_values};
use crate::config::ServerConfig;
use crate::model::{HostMetrics, HostSource};

const SSH_TIMEOUT: Duration = Duration::from_secs(10);
const SSH_POLL_INTERVAL: Duration = Duration::from_millis(25);

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
    let output = run_ssh_script(host, crate::collectors::local::FIXED_COLLECT_COMMAND)?;
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
    let mut command = base_ssh_command(false);
    command.arg(host).arg("sh").arg("-s");
    command
}

pub fn ssh_probe_command(host: &str) -> Command {
    let mut command = base_ssh_command(true);
    command.arg(host).arg("true");
    command
}

fn run_ssh_script(host: &str, script: &str) -> Result<Output, CollectorError> {
    let mut command = ssh_command(host);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_command_with_input(command, Some(script.as_bytes()), SSH_TIMEOUT)
}

pub fn run_ssh_probe(host: &str) -> Result<Output, CollectorError> {
    let mut command = ssh_probe_command(host);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    run_command_with_input(command, None, SSH_TIMEOUT)
}

fn run_command_with_input(
    mut command: Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, CollectorError> {
    let mut child = command.spawn()?;

    if let Some(input) = stdin
        && let Some(mut child_stdin) = child.stdin.take()
    {
        match child_stdin.write_all(input) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
            Err(error) => return Err(error.into()),
        }
    }

    let stdout_reader = read_pipe_in_background(child.stdout.take());
    let stderr_reader = read_pipe_in_background(child.stderr.take());

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait()?;
            let _stdout = join_pipe_reader(stdout_reader)?;
            let stderr = join_pipe_reader(stderr_reader)?;
            let mut message = format!("SSH command timed out after {}s", timeout.as_secs());
            let detail = String::from_utf8_lossy(&stderr).trim().to_string();
            if !detail.is_empty() {
                message.push_str(": ");
                message.push_str(&detail);
            }
            return Err(CollectorError::CommandFailed {
                code: status.code(),
                stderr: message,
            });
        }
        sleep(SSH_POLL_INTERVAL);
    };

    Ok(Output {
        status,
        stdout: join_pipe_reader(stdout_reader)?,
        stderr: join_pipe_reader(stderr_reader)?,
    })
}

fn read_pipe_in_background<T>(pipe: Option<T>) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    T: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut data = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut data)?;
        }
        Ok(data)
    })
}

fn join_pipe_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("SSH pipe reader thread panicked")))
}

fn base_ssh_command(detach_stdin: bool) -> Command {
    let mut command = Command::new("ssh");
    if detach_stdin {
        command.arg("-n");
    }
    for (key, value) in SSH_OPTIONS {
        command.arg("-o").arg(format!("{key}={value}"));
    }
    if !cfg!(windows) {
        for (key, value) in SSH_MULTIPLEX_OPTIONS {
            command.arg("-o").arg(format!("{key}={value}"));
        }
    }
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
    fn ssh_collector_uses_non_interactive_stdin_script_command() {
        super::validate_ssh_host("ExampleHost").unwrap();
        let debug = format!("{:?}", super::ssh_command("ExampleHost"));
        assert!(debug.contains("BatchMode=yes"));
        assert!(debug.contains("PasswordAuthentication=no"));
        assert!(debug.contains("KbdInteractiveAuthentication=no"));
        assert!(debug.contains("ConnectTimeout=5"));
        assert!(debug.contains("ConnectionAttempts=1"));
        assert!(debug.contains("NumberOfPasswordPrompts=0"));
        assert!(debug.contains("ExampleHost"));
        assert!(debug.contains("sh"));
        assert!(debug.contains("-s"));
        assert!(!debug.contains("/proc/loadavg"));
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

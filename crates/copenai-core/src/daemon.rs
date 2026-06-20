use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{CoreError, Result};
use crate::paths::DataPaths;

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub pid: u32,
    pub bind: String,
}

pub fn read_pid(paths: &DataPaths) -> Result<Option<u32>> {
    let path = paths.pid_file();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?.trim().to_string();
    raw.parse()
        .map(Some)
        .map_err(|_| CoreError::Other("invalid pid file".into()))
}

pub fn write_pid(paths: &DataPaths, pid: u32) -> Result<()> {
    paths.ensure_layout()?;
    fs::write(paths.pid_file(), pid.to_string())?;
    Ok(())
}

pub fn remove_pid(paths: &DataPaths) -> Result<()> {
    let path = paths.pid_file();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Avoid shell `kill` stderr noise on stale pids.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// PIDs listening on `port` (best-effort via `lsof` on Unix).
pub fn find_pids_on_port(port: u16) -> Vec<u32> {
    #[cfg(unix)]
    {
        use std::process::Command;
        let output = Command::new("lsof")
            .args(["-ti", &format!(":{port}")])
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = port;
        Vec::new()
    }
}

pub fn stop_process(pid: u32) -> Result<()> {
    if !is_process_alive(pid) {
        return Err(CoreError::DaemonNotRunning);
    }

    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        for _ in 0..30 {
            if !is_process_alive(pid) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(CoreError::Other(
            "stop not supported on this platform".into(),
        ))
    }
}

/// Stop daemon tracked by pidfile, or any process still listening on `port`.
pub fn stop_daemon(paths: &DataPaths, port: u16) -> Result<StopOutcome> {
    if let Some(pid) = read_pid(paths)? {
        if is_process_alive(pid) {
            stop_process(pid)?;
            remove_pid(paths)?;
            return Ok(StopOutcome::StoppedPid(pid));
        }
        remove_pid(paths)?;
    }

    let listeners = find_pids_on_port(port);
    if listeners.is_empty() {
        return Ok(StopOutcome::NotRunning);
    }

    let mut stopped = Vec::new();
    for pid in listeners {
        if is_process_alive(pid) {
            let _ = stop_process(pid);
            if !is_process_alive(pid) {
                stopped.push(pid);
            }
        }
    }

    if stopped.is_empty() {
        Ok(StopOutcome::NotRunning)
    } else {
        Ok(StopOutcome::StoppedListeners(stopped))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    StoppedPid(u32),
    StoppedListeners(Vec<u32>),
    NotRunning,
}

pub fn tail_log(path: &Path, follow: bool, lines: usize) -> Result<()> {
    if !path.exists() {
        return Err(CoreError::Other(format!(
            "log not found: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new("tail");
        if follow {
            cmd.arg("-f");
        }
        cmd.arg("-n").arg(lines.to_string()).arg(path);
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(CoreError::Other("tail failed".into()))
        }
    }
    #[cfg(not(unix))]
    {
        let content = fs::read_to_string(path)?;
        let slice: Vec<&str> = content.lines().collect();
        let start = slice.len().saturating_sub(lines);
        for line in &slice[start..] {
            println!("{line}");
        }
        let _ = follow;
        Ok(())
    }
}

pub fn parse_bind_port(bind: &str) -> Result<u16> {
    let port = bind
        .rsplit(':')
        .next()
        .ok_or_else(|| CoreError::Config(format!("invalid bind address: {bind}")))?;
    port.parse()
        .map_err(|_| CoreError::Config(format!("invalid bind port in: {bind}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_from_bind() {
        assert_eq!(parse_bind_port("0.0.0.0:9241").unwrap(), 9241);
        assert_eq!(parse_bind_port("127.0.0.1:3000").unwrap(), 3000);
    }
}

//! Process, system-load, port and shell-command inspection.
//!
//! Everything here is read-only except [`kill_process`] and [`run_command`],
//! which the frontend gates behind an explicit confirmation card.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// `System` is expensive to construct and CPU usage is a *delta* between two
/// refreshes, so we keep one instance alive for the process of the app.
pub struct ProcState {
    sys: Mutex<System>,
}

impl Default for ProcState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcState {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.refresh_cpu_all();
        Self {
            sys: Mutex::new(sys),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    /// Full command line, joined with spaces. Empty when not readable.
    pub cmd: String,
    pub exe: Option<String>,
    /// Percent of one core, so this can exceed 100 on multi-threaded procs.
    pub cpu: f32,
    pub memory_mb: f64,
    /// Seconds the process has been alive.
    pub run_time: u64,
    pub status: String,
}

#[derive(Deserialize, Default)]
pub struct ProcQuery {
    /// Case-insensitive substring matched against name, exe and cmd.
    pub filter: Option<String>,
    /// `"cpu"` (default), `"memory"`, `"name"`, or `"pid"`.
    pub sort_by: Option<String>,
    pub limit: Option<usize>,
}

/// Snapshot of running processes.
///
/// CPU percentages need two samples taken at least
/// `MINIMUM_CPU_UPDATE_INTERVAL` apart. The state is long-lived, so on the
/// second and later calls the previous refresh serves as the first sample;
/// only the very first call has to sleep.
#[tauri::command]
pub fn list_processes(
    query: Option<ProcQuery>,
    state: tauri::State<'_, ProcState>,
) -> Result<Vec<ProcInfo>, String> {
    let query = query.unwrap_or_default();
    let mut sys = state.sys.lock().map_err(|e| e.to_string())?;

    let kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet);

    let first_run = sys.processes().is_empty();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    if first_run {
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }

    let needle = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let mut out: Vec<ProcInfo> = sys
        .processes()
        .values()
        .map(|p| {
            let cmd = p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            ProcInfo {
                pid: p.pid().as_u32(),
                parent_pid: p.parent().map(|p| p.as_u32()),
                name: p.name().to_string_lossy().to_string(),
                exe: p.exe().map(|e| e.to_string_lossy().to_string()),
                cmd,
                cpu: (p.cpu_usage() * 10.0).round() / 10.0,
                memory_mb: ((p.memory() as f64 / 1_048_576.0) * 10.0).round() / 10.0,
                run_time: p.run_time(),
                status: p.status().to_string(),
            }
        })
        .filter(|p| match &needle {
            None => true,
            Some(n) => {
                p.name.to_lowercase().contains(n)
                    || p.cmd.to_lowercase().contains(n)
                    || p.exe
                        .as_deref()
                        .map(|e| e.to_lowercase().contains(n))
                        .unwrap_or(false)
            }
        })
        .collect();

    match query.sort_by.as_deref().unwrap_or("cpu") {
        "memory" => out.sort_by(|a, b| {
            b.memory_mb
                .partial_cmp(&a.memory_mb)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "name" => out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        "pid" => out.sort_by_key(|p| p.pid),
        _ => out.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    out.truncate(query.limit.unwrap_or(60).clamp(1, 500));
    Ok(out)
}

#[derive(Serialize)]
pub struct SystemStats {
    pub os: String,
    pub os_version: String,
    pub kernel: String,
    pub hostname: String,
    pub arch: String,
    pub cpu_brand: String,
    pub cpu_count: usize,
    pub cpu_usage: f32,
    pub mem_total_mb: f64,
    pub mem_used_mb: f64,
    pub mem_percent: f32,
    pub swap_total_mb: f64,
    pub swap_used_mb: f64,
    pub uptime_secs: u64,
    pub process_count: usize,
}

#[tauri::command]
pub fn system_stats(state: tauri::State<'_, ProcState>) -> Result<SystemStats, String> {
    let mut sys = state.sys.lock().map_err(|e| e.to_string())?;
    sys.refresh_memory();
    sys.refresh_cpu_usage();

    let mb = |bytes: u64| ((bytes as f64 / 1_048_576.0) * 10.0).round() / 10.0;
    let total = sys.total_memory();
    let used = sys.used_memory();

    Ok(SystemStats {
        os: System::name().unwrap_or_else(|| std::env::consts::OS.to_string()),
        os_version: System::os_version().unwrap_or_default(),
        kernel: System::kernel_version().unwrap_or_default(),
        hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
        arch: System::cpu_arch(),
        cpu_brand: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
        cpu_count: sys.cpus().len(),
        cpu_usage: (sys.global_cpu_usage() * 10.0).round() / 10.0,
        mem_total_mb: mb(total),
        mem_used_mb: mb(used),
        mem_percent: if total == 0 {
            0.0
        } else {
            ((used as f64 / total as f64 * 1000.0).round() / 10.0) as f32
        },
        swap_total_mb: mb(sys.total_swap()),
        swap_used_mb: mb(sys.used_swap()),
        uptime_secs: System::uptime(),
        process_count: sys.processes().len(),
    })
}

/// SIGKILL-equivalent. Refuses PID 0 and the app's own PID so a
/// mistranscribed command can't take Theta down with it.
#[tauri::command]
pub fn kill_process(pid: u32, state: tauri::State<'_, ProcState>) -> Result<String, String> {
    if pid == 0 {
        return Err("Refusing to kill PID 0.".into());
    }
    if pid == std::process::id() {
        return Err("Refusing to kill Theta itself.".into());
    }

    let mut sys = state.sys.lock().map_err(|e| e.to_string())?;
    let target = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        ProcessRefreshKind::nothing(),
    );

    let proc = sys
        .process(target)
        .ok_or_else(|| format!("No process with PID {pid} is running."))?;
    let name = proc.name().to_string_lossy().to_string();

    if proc.kill() {
        Ok(format!("Killed {name} (PID {pid})."))
    } else {
        Err(format!(
            "Could not kill {name} (PID {pid}) — it may need elevated permissions."
        ))
    }
}

#[derive(Serialize)]
pub struct PortInfo {
    pub port: u16,
    pub proto: String,
    pub address: String,
    pub pid: u32,
    pub process: String,
}

/// TCP/UDP listeners, resolved to owning process names.
///
/// Parses `netstat -ano` on Windows and `ss -tulpn` elsewhere rather than
/// pulling in a raw-socket dependency — the output is stable enough and this
/// keeps the build free of extra native linkage.
#[tauri::command]
pub fn listening_ports(state: tauri::State<'_, ProcState>) -> Result<Vec<PortInfo>, String> {
    let mut rows = if cfg!(target_os = "windows") {
        parse_netstat(&run_capture("netstat", &["-ano"])?)
    } else {
        parse_ss(&run_capture("ss", &["-tulpn"])?)
    };

    // Resolve PIDs to names in one pass.
    if let Ok(mut sys) = state.sys.lock() {
        let pids: Vec<Pid> = rows
            .iter()
            .filter(|r| r.pid != 0)
            .map(|r| Pid::from_u32(r.pid))
            .collect();
        if !pids.is_empty() {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                false,
                ProcessRefreshKind::nothing(),
            );
        }
        for row in &mut rows {
            if row.process.is_empty() {
                row.process = sys
                    .process(Pid::from_u32(row.pid))
                    .map(|p| p.name().to_string_lossy().to_string())
                    .unwrap_or_else(|| "?".into());
            }
        }
    }

    rows.sort_by_key(|r| r.port);
    rows.dedup_by(|a, b| a.port == b.port && a.proto == b.proto && a.pid == b.pid);
    Ok(rows)
}

fn run_capture(program: &str, args: &[&str]) -> Result<String, String> {
    let out = new_command(program)
        .args(args)
        .output()
        .map_err(|e| format!("`{program}` failed to run: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `Proto  Local Address  Foreign Address  State  PID`
fn parse_netstat(text: &str) -> Vec<PortInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let proto = f[0].to_uppercase();
        let is_tcp = proto == "TCP";
        if !is_tcp && proto != "UDP" {
            continue;
        }
        // TCP rows carry a state column; UDP rows don't.
        if is_tcp && f.len() >= 4 && f[3] != "LISTENING" {
            continue;
        }
        let (addr, pid_str) = (f[1], f[f.len() - 1]);
        let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
            continue;
        };
        out.push(PortInfo {
            port,
            proto,
            address: addr.to_string(),
            pid: pid_str.parse().unwrap_or(0),
            process: String::new(),
        });
    }
    out
}

/// `Netid State Recv-Q Send-Q Local:Port Peer:Port users:(("name",pid=123,..))`
fn parse_ss(text: &str) -> Vec<PortInfo> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let addr = f[4];
        let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
            continue;
        };
        let (mut pid, mut process) = (0u32, String::new());
        if let Some(users) = f.get(6).or_else(|| f.get(5)) {
            if let Some(rest) = users.split("pid=").nth(1) {
                pid = rest
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(name) = users.split('"').nth(1) {
                process = name.to_string();
            }
        }
        out.push(PortInfo {
            port,
            proto: f[0].to_uppercase(),
            address: addr.to_string(),
            pid,
            process,
        });
    }
    out
}

#[derive(Serialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

const MAX_CAPTURE_CHARS: usize = 20_000;

fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_CAPTURE_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX_CAPTURE_CHARS).collect();
    format!("{head}\n[...output truncated...]")
}

/// Builds a `Command` that does not flash a console window on Windows.
fn new_command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs a shell command and captures its output.
///
/// The frontend requires user confirmation before this is ever invoked — see
/// `REQUIRES_CONFIRMATION` in `src/agent/tools.ts`. Killed after `timeout_secs`
/// (default 20, max 120) so a hung command can't wedge the assistant.
#[tauri::command]
pub async fn run_command(
    command: String,
    cwd: Option<String>,
    timeout_secs: Option<u64>,
) -> Result<CommandOutput, String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("Empty command.".into());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(20).clamp(1, 120));

    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = new_command("powershell.exe");
            c.args(["-NoProfile", "-NonInteractive", "-Command", &command]);
            c
        } else {
            let mut c = new_command("sh");
            c.args(["-c", &command]);
            c
        };
        if let Some(dir) = cwd.as_deref().filter(|d| !d.trim().is_empty()) {
            cmd.current_dir(dir);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start command: {e}"))?;

        // Poll rather than block so we can enforce the timeout without
        // needing an async process crate.
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(CommandOutput {
                            stdout: String::new(),
                            stderr: format!("Command timed out after {}s.", timeout.as_secs()),
                            exit_code: -1,
                            timed_out: true,
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(format!("Failed while waiting on command: {e}")),
            }
        }

        let out = child
            .wait_with_output()
            .map_err(|e| format!("Failed to read command output: {e}"))?;
        Ok(CommandOutput {
            stdout: clip(&String::from_utf8_lossy(&out.stdout)),
            stderr: clip(&String::from_utf8_lossy(&out.stderr)),
            exit_code: out.status.code().unwrap_or(-1),
            timed_out: false,
        })
    })
    .await
    .map_err(|e| format!("Command task failed: {e}"))?
}

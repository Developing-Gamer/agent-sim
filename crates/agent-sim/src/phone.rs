//! A phone as the CLI drives it: its screen geometry, its processes, x86Sim's own command line, and agent-sim's
//! scripts.

use crate::device::Device;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The phone's settings from its sim.env (screen size, scale, name...).
pub fn sim_env(dev: &Device) -> Result<HashMap<String, String>> {
    let text = std::fs::read_to_string(dev.dir.join("sim.env"))
        .with_context(|| format!("reading {}/sim.env", dev.dir.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// The screen in points (what apps and the accessibility tree use).
#[derive(Debug, Clone, Copy)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
}

impl Screen {
    pub fn of(dev: &Device) -> Result<Self> {
        let env = sim_env(dev)?;
        let num = |k: &str| -> Result<f64> {
            env.get(k)
                .with_context(|| format!("sim.env has no {k}"))?
                .parse::<f64>()
                .with_context(|| format!("sim.env's {k} is not a number"))
        };
        let scale = num("SIMULATOR_MAINSCREEN_SCALE")?.max(1.0);
        Ok(Self {
            width: num("SIMULATOR_MAINSCREEN_WIDTH")? / scale,
            height: num("SIMULATOR_MAINSCREEN_HEIGHT")? / scale,
        })
    }

    /// A point in screen points as simhid's 0..1 coordinates.
    pub fn unit(&self, x: f64, y: f64) -> (f64, f64) {
        (x / self.width, y / self.height)
    }
}

/// The device's name as its processes see it (SIMULATOR_DEVICE_NAME).
pub fn device_name(dev: &Device) -> Result<String> {
    sim_env(dev)?
        .remove("SIMULATOR_DEVICE_NAME")
        .context("sim.env has no SIMULATOR_DEVICE_NAME")
}

/// The device's data directory as the host sees it.
pub fn data_dir(dev: &Device) -> PathBuf {
    dev.dir.join("data")
}

/// Linux pids of this phone's processes for which `pick` holds, given each process's Darling pid (the pid the
/// phone itself uses) and the basename of its executable.
pub fn processes(dev: &Device, pick: impl Fn(u32, &str) -> bool) -> Result<Vec<u32>> {
    Ok(process_ids(dev, pick)?
        .into_iter()
        .map(|(l, _)| l)
        .collect())
}

/// Like processes, with each one's Darling pid: (Linux pid, Darling pid).
pub fn process_ids(dev: &Device, pick: impl Fn(u32, &str) -> bool) -> Result<Vec<(u32, u32)>> {
    let name = device_name(dev)?;
    let want = format!("SIMULATOR_DEVICE_NAME={name}");
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let base = entry.path();
        let Ok(status) = std::fs::read_to_string(base.join("status")) else {
            continue;
        };
        // NSpid: the Linux pid, then the pid inside Darling's namespace
        let Some(inner) = status
            .lines()
            .find_map(|l| l.strip_prefix("NSpid:"))
            .and_then(|l| l.split_whitespace().last())
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(base.join("cmdline")) else {
            continue;
        };
        let argv0 = cmdline.split(|b| *b == 0).next().unwrap_or_default();
        let argv0 = String::from_utf8_lossy(argv0);
        let exe = argv0.rsplit('/').next().unwrap_or_default();
        if !pick(inner, exe) {
            continue;
        }
        let Ok(environ) = std::fs::read(base.join("environ")) else {
            continue;
        };
        if environ.split(|b| *b == 0).any(|v| v == want.as_bytes()) {
            out.push((pid, inner));
        }
    }
    Ok(out)
}

/// Asks a phone process to read a request from agent-sim's hook (SIGUSR2). `inner` is its pid inside the phone. Only
/// once the hook has set up its handler, which it marks with `hooked-<inner>` in `dir`: until then SIGUSR2 would
/// end the process (an app or WebContent process still starting).
pub fn ask_hook(dir: &Path, linux: u32, inner: u32) -> Result<()> {
    if !dir.join(format!("hooked-{inner}")).exists() {
        bail!("process {inner} is still starting");
    }
    signal(linux, libc::SIGUSR2)
}

pub fn signal(pid: u32, sig: i32) -> Result<()> {
    // SAFETY: kill(2) with a pid we just read from /proc and a valid signal number; no memory is shared.
    #[allow(unsafe_code)]
    let r = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if r != 0 {
        bail!("signalling pid {pid}: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

/// agent-sim's home: `$AGENT_SIM_HOME`, else `~/.local/share/agent-sim`.
pub fn home() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("AGENT_SIM_HOME") {
        return Ok(PathBuf::from(d));
    }
    let h = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(h).join(".local/share/agent-sim"))
}

/// x86Sim's launcher (the unpacked AppImage's AppRun).
pub fn x86sim() -> Result<PathBuf> {
    let ver = std::env::var("X86SIM_VERSION").unwrap_or_else(|_| "0.5.2".into());
    let p = home()?.join(format!("x86sim-{ver}/AppRun"));
    if !p.is_file() {
        bail!(
            "x86Sim is not installed at {} (run scripts/setup-vm.sh)",
            p.display()
        );
    }
    Ok(p)
}

/// Runs `x86sim ARGS...` against this phone.
pub fn x86sim_run(dev: &Device, args: &[&str]) -> Result<Output> {
    let ring = dev.ring_path().ok();
    let mut cmd = Command::new(x86sim()?);
    cmd.arg("x86sim")
        .args(args)
        .env("X86SIM_DEVICE", &dev.dir)
        .current_dir(crate::device::data_dir()?);
    if let Some(r) = ring {
        cmd.env("SIMFB_RING_LINUX", r);
    }
    cmd.output().context("running x86sim")
}

/// Like x86sim_run, but fails with x86Sim's own message when it fails.
pub fn x86sim_ok(dev: &Device, args: &[&str]) -> Result<String> {
    x86sim_check(args, x86sim_run(dev, args)?)
}

/// Like x86sim_ok, piping `input` on stdin (used for JSON payloads without inheriting MCP stdio).
pub fn x86sim_ok_stdin(dev: &Device, args: &[&str], input: &[u8]) -> Result<String> {
    let ring = dev.ring_path().ok();
    let mut cmd = Command::new(x86sim()?);
    cmd.arg("x86sim")
        .args(args)
        .env("X86SIM_DEVICE", &dev.dir)
        .current_dir(crate::device::data_dir()?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(r) = ring {
        cmd.env("SIMFB_RING_LINUX", r);
    }
    let mut child = cmd.spawn().context("running x86sim")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).context("writing to x86sim stdin")?;
    }
    let out = child.wait_with_output().context("waiting for x86sim")?;
    x86sim_check(args, out)
}

fn x86sim_check(args: &[&str], out: Output) -> Result<String> {
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err
            .lines()
            .chain(text.lines())
            .rfind(|l| !l.trim().is_empty());
        bail!("x86sim {}: {}", args.join(" "), msg.unwrap_or("failed"));
    }
    Ok(text)
}

/// agent-sim's scripts directory: `$AGENT_SIM_SCRIPTS`, next to the binary in a checkout (target/release/..), or
/// installed under agent-sim's home.
pub fn scripts() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("AGENT_SIM_SCRIPTS") {
        return Ok(PathBuf::from(d));
    }
    let exe = std::env::current_exe()?;
    for up in exe.ancestors().skip(1).take(4) {
        if up.join("scripts/phone.sh").is_file() {
            return Ok(up.join("scripts"));
        }
    }
    let installed = home()?.join("src/scripts");
    if installed.join("phone.sh").is_file() {
        return Ok(installed);
    }
    bail!("agent-sim's scripts are not found; set AGENT_SIM_SCRIPTS")
}

/// Runs a script from agent-sim's scripts directory, passing its output through.
pub fn script(name: &str, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    let path = scripts()?.join(name);
    let status = Command::new(&path)
        .args(args)
        .envs(env.iter().copied())
        .status()
        .with_context(|| format!("running {}", path.display()))?;
    if !status.success() {
        bail!("{name} {} failed", args.join(" "));
    }
    Ok(())
}

/// The device name to use: the given one, else `$AGENT_SIM_DEVICE`, else `p` (setup-vm.sh's phone).
pub fn pick_device(name: Option<&str>) -> String {
    name.map(str::to_string)
        .or_else(|| std::env::var("AGENT_SIM_DEVICE").ok())
        .unwrap_or_else(|| "p".into())
}

/// Whether a phone is up, down, or down without having been stopped.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Health {
    Running,
    Stopped,
    /// Darling's server exited while the phone ran: every process left freezes or goes, and no frame is drawn
    Crashed,
}

/// The device's short name (`p` for `device-a64-p`), as phone.sh and x86Sim's prefix name it.
fn short_name(dev: &Device) -> String {
    let n = dev.dir.file_name().unwrap_or_default().to_string_lossy();
    n.strip_prefix("device-a64-").unwrap_or(&n).to_string()
}

/// Whether the device's Darling server runs (`darlingserver <prefix> ...`).
fn server_running(dev: &Device) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let prefix = Path::new(&home).join(format!(".cache/x86sim/a64prefix-{}", short_name(dev)));
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        let Ok(cmd) = std::fs::read(e.path().join("cmdline")) else {
            return false;
        };
        let mut args = cmd.split(|&b| b == 0);
        let exe = args.next().unwrap_or_default();
        exe.ends_with(b"darlingserver")
            && args.next() == Some(prefix.as_os_str().as_encoded_bytes())
    })
}

pub fn health(dev: &Device) -> Health {
    let springboard = processes(dev, |_, exe| exe == "SpringBoard")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let server = server_running(dev);
    if springboard && server {
        return Health::Running;
    }
    let unit_active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet"])
        .arg(format!("agent-sim-phone-{}", short_name(dev)))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if (springboard || unit_active) && !server {
        Health::Crashed
    } else {
        Health::Stopped
    }
}

pub fn is_running(dev: &Device) -> bool {
    health(dev) == Health::Running
}

/// Why the phone cannot answer: stopped, or crashed (and how to get it back).
pub fn not_running(dev: &Device) -> anyhow::Error {
    match health(dev) {
        Health::Crashed => anyhow::anyhow!(
            "the phone stopped: its Darling server exited (a known x86Sim/Darling issue, docs/KNOWN_ISSUES.md); \
             `agent-sim up` restarts it"
        ),
        _ => anyhow::anyhow!("the phone is not running (`agent-sim up` starts it)"),
    }
}

/// Where agent-sim keeps per-phone state on the host (element references from the last snapshot).
pub fn state_dir(dev: &Device) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))
        .context("HOME is not set")?;
    let d = base
        .join("agent-sim")
        .join(dev.dir.file_name().unwrap_or_default());
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

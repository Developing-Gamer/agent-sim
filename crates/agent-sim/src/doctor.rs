//! `agent-sim doctor`: what a phone needs on this machine, and what is missing, so a failed setup or boot can be
//! reported with something to go on.

use crate::{device, phone};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

pub struct Check {
    pub level: Level,
    pub name: &'static str,
    pub detail: String,
}

fn check(level: Level, name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        level,
        name,
        detail: detail.into(),
    }
}

/// One line per check: `ok`, `warn` or `FAIL`, the check, and what it found or what to do.
pub fn format(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let tag = match c.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        out.push_str(&format!("{tag}  {}: {}\n", c.name, c.detail));
    }
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    out.push_str(&match (fails, warns) {
        (0, 0) => "everything a phone needs is here".to_string(),
        (0, w) => format!("no blocking problem; {w} warning(s)"),
        (f, _) => format!("{f} problem(s) to fix first"),
    });
    out
}

/// Total memory in GiB from `/proc/meminfo`'s text.
pub fn mem_total_gib(meminfo: &str) -> Option<f64> {
    let kb: f64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb / 1024.0 / 1024.0)
}

/// Whether `/proc/version` is Windows Subsystem for Linux's kernel.
pub fn is_wsl(proc_version: &str) -> bool {
    let v = proc_version.to_ascii_lowercase();
    v.contains("microsoft") || v.contains("wsl")
}

fn on_path(bin: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

fn succeeds(cmd: &str, args: &[&str]) -> Option<bool> {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|s| s.success())
}

fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn vulkan_icd() -> Option<PathBuf> {
    if let Some(list) = std::env::var_os("VK_ICD_FILENAMES") {
        if let Some(p) = std::env::split_paths(&list).find(|p| p.is_file()) {
            return Some(p);
        }
    }
    ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
}

fn installed_runtime(data: &Path) -> Option<String> {
    std::fs::read_dir(data.join("runtimes"))
        .ok()?
        .flatten()
        .find(|e| e.path().join("RuntimeRoot").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
}

/// Runs every check for the phone `name`.
pub fn run(name: &str) -> Vec<Check> {
    let mut v = Vec::new();

    // the machine
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        v.push(check(Level::Ok, "machine", "Linux x86-64"));
    } else {
        v.push(check(
            Level::Fail,
            "machine",
            "x86Sim needs a Linux x86-64 machine (for a Mac or an ARM host, use a Linux VM)",
        ));
    }
    if std::fs::read_to_string("/proc/version").is_ok_and(|s| is_wsl(&s)) {
        v.push(check(
            Level::Warn,
            "WSL",
            "Windows Subsystem for Linux is not a tested setup; Darling needs user namespaces and overlay mounts",
        ));
    }
    match std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| mem_total_gib(&s))
    {
        Some(g) if g < 7.0 => v.push(check(
            Level::Warn,
            "memory",
            format!("{g:.1} GiB; a phone wants 8 GiB or more"),
        )),
        Some(g) => v.push(check(Level::Ok, "memory", format!("{g:.1} GiB"))),
        None => {}
    }
    match succeeds("unshare", &["-U", "true"]) {
        Some(true) => v.push(check(Level::Ok, "user namespaces", "allowed")),
        Some(false) => v.push(check(
            Level::Warn,
            "user namespaces",
            "blocked (Ubuntu's AppArmor default); x86Sim then needs its root-owned Darling install, which `agent-sim setup` makes with sudo",
        )),
        None => {}
    }
    match output("systemctl", &["--user", "is-system-running"]) {
        Some(s) if s == "running" || s == "degraded" => {
            v.push(check(Level::Ok, "systemd user session", s))
        }
        Some(s) => v.push(check(
            Level::Fail,
            "systemd user session",
            format!(
                "{} (phones run as user services; `loginctl enable-linger $(id -un)` keeps a session on a server)",
                if s.is_empty() { "not available" } else { &s }
            ),
        )),
        None => v.push(check(
            Level::Fail,
            "systemd user session",
            "systemctl is not available; phones run as systemd user services",
        )),
    }

    // programs the commands call
    for (bin, why) in [
        ("curl", "downloads (install from a URL, the runtime)"),
        ("tar", "install from archives"),
        ("python3", "x86Sim's tools, install from .ipa and .zip"),
        ("ffmpeg", "record"),
    ] {
        match on_path(bin) {
            Some(p) => v.push(check(Level::Ok, bin, p.display().to_string())),
            None => v.push(check(
                Level::Fail,
                bin,
                format!("not on PATH (needed for: {why}); `agent-sim setup` installs it"),
            )),
        }
    }
    match vulkan_icd() {
        Some(p) => v.push(check(Level::Ok, "Vulkan driver", p.display().to_string())),
        None => v.push(check(
            Level::Fail,
            "Vulkan driver",
            "no Vulkan ICD found; on a machine without a GPU install Mesa's lavapipe (mesa-vulkan-drivers)",
        )),
    }

    // agent-sim's own pieces
    match phone::x86sim() {
        Ok(p) => v.push(check(Level::Ok, "x86Sim", p.display().to_string())),
        Err(e) => v.push(check(Level::Fail, "x86Sim", e.to_string())),
    }
    match phone::scripts() {
        Ok(s) => {
            let hook = s.join("../hooks/prebuilt/libagentsim-ax.dylib");
            if hook.is_file() {
                v.push(check(
                    Level::Ok,
                    "hooks",
                    "built (snapshot can read the screen)",
                ));
            } else {
                v.push(check(
                    Level::Fail,
                    "hooks",
                    format!("{} is missing; snapshot would be empty", hook.display()),
                ));
            }
        }
        Err(e) => v.push(check(Level::Fail, "scripts", e.to_string())),
    }
    match device::data_dir() {
        Ok(data) => match installed_runtime(&data) {
            Some(r) => v.push(check(Level::Ok, "iOS runtime", r)),
            None => v.push(check(
                Level::Fail,
                "iOS runtime",
                format!("none unpacked in {} (run agent-sim setup)", data.display()),
            )),
        },
        Err(e) => v.push(check(Level::Fail, "iOS runtime", e.to_string())),
    }

    // the phone
    match device::Device::find(name) {
        Ok(dev) => {
            let h = phone::health(&dev);
            let (level, detail) = match h {
                phone::Health::Running => (Level::Ok, "running".to_string()),
                phone::Health::Stopped => (
                    Level::Warn,
                    "stopped (`agent-sim up` starts it)".to_string(),
                ),
                phone::Health::Crashed => (
                    Level::Fail,
                    "its Darling server exited (`agent-sim up` restarts it; docs/KNOWN_ISSUES.md)"
                        .to_string(),
                ),
            };
            v.push(check(level, "phone", format!("{name}: {detail}")));
            if h == phone::Health::Running {
                match phone::springboard_answers(&dev) {
                    Some(true) => v.push(check(Level::Ok, "SpringBoard", "answers")),
                    Some(false) => v.push(check(
                        Level::Fail,
                        "SpringBoard",
                        "does not answer: the phone is hung (`agent-sim up` restarts it)",
                    )),
                    None => v.push(check(
                        Level::Warn,
                        "SpringBoard",
                        "hooks not loaded yet (a phone started with AGENT_SIM_HOOKS=0, or still booting)",
                    )),
                }
            }
        }
        Err(_) => v.push(check(
            Level::Warn,
            "phone",
            format!("no phone named {name} (agent-sim setup makes one)"),
        )),
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_read_from_meminfo() {
        let m = "MemTotal:       16384000 kB\nMemFree:  100 kB\n";
        let g = mem_total_gib(m).unwrap();
        assert!((g - 15.625).abs() < 0.01, "{g}");
        assert_eq!(mem_total_gib("nothing here"), None);
    }

    #[test]
    fn wsl_is_told_from_a_normal_kernel() {
        assert!(is_wsl(
            "Linux version 6.18.58.1-microsoft-standard-WSL2 (root@x) #1 SMP"
        ));
        assert!(!is_wsl(
            "Linux version 6.8.0-45-generic (buildd@lcy02) #45-Ubuntu"
        ));
    }

    #[test]
    fn the_report_counts_what_blocks() {
        let all_ok = [check(Level::Ok, "a", "fine")];
        assert!(format(&all_ok).ends_with("everything a phone needs is here"));
        let warn = [
            check(Level::Ok, "a", "fine"),
            check(Level::Warn, "b", "meh"),
        ];
        assert!(format(&warn).ends_with("no blocking problem; 1 warning(s)"));
        let fail = [
            check(Level::Fail, "b", "gone"),
            check(Level::Warn, "c", "meh"),
        ];
        let r = format(&fail);
        assert!(r.contains("FAIL  b: gone"), "{r}");
        assert!(r.ends_with("1 problem(s) to fix first"), "{r}");
    }
}

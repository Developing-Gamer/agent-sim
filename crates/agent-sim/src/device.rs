//! Where an x86Sim phone keeps its frame ring and input socket.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// One x86Sim device, found by name under x86Sim's data directory.
#[derive(Debug, Clone)]
pub struct Device {
    pub dir: PathBuf,
}

/// x86Sim's data directory: `$X86SIM_DATA`, else `$XDG_DATA_HOME/x86sim`, else `~/.local/share/x86sim`.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("X86SIM_DATA") {
        return Ok(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(d).join("x86sim"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/x86sim"))
}

/// x86Sim's cache directory (Darling prefixes, logs): `$X86SIM_CACHE`, else `$XDG_CACHE_HOME/x86sim`, else
/// `~/.cache/x86sim`, as x86Sim's AppRun picks it.
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("X86SIM_CACHE") {
        return Ok(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(d).join("x86sim"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".cache/x86sim"))
}

impl Device {
    /// `name` is a device name (`a` for `var/device-a64-a`, empty for x86Sim's default `var/device-a64`) or a path
    /// to a device directory.
    pub fn find(name: &str) -> Result<Self> {
        let given = Path::new(name);
        let dir = if given.is_absolute() || name.contains('/') {
            given.to_path_buf()
        } else {
            let var = data_dir()?.join("var");
            if name.is_empty() {
                var.join("device-a64")
            } else {
                var.join(format!("device-a64-{name}"))
            }
        };
        if !dir.join("sim.env").is_file() {
            bail!(
                "no x86Sim device at {} (create one with `x86sim device create`)",
                dir.display()
            );
        }
        Ok(Self { dir })
    }

    /// The frame ring simfb writes, as the host sees it (sim.env's SIMFB_RING without Darling's
    /// `/Volumes/SystemRoot` prefix).
    pub fn ring_path(&self) -> Result<PathBuf> {
        let env = std::fs::read_to_string(self.dir.join("sim.env"))
            .with_context(|| format!("reading {}/sim.env", self.dir.display()))?;
        let ring = env
            .lines()
            .find_map(|l| l.strip_prefix("SIMFB_RING="))
            .context("sim.env has no SIMFB_RING: boot the phone once first")?;
        Ok(PathBuf::from(
            ring.strip_prefix("/Volumes/SystemRoot").unwrap_or(ring),
        ))
    }

    /// simhid's input socket (a symlink when the device path is too long for a Unix socket).
    pub fn hid_socket(&self) -> PathBuf {
        let link = self.dir.join("data/simhid.sock");
        std::fs::canonicalize(&link).unwrap_or(link)
    }
}

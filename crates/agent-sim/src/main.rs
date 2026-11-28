mod ax;
mod cmd;
mod device;
mod doctor;
mod encode_bench;
mod files;
mod input;
mod ipa;
mod mcp;
mod phone;
mod ring;
mod serve;
mod video;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// iOS simulators for AI agents, on Linux.
///
/// A typical loop: `agent-sim up`, `agent-sim install MyApp.ipa`, `agent-sim launch com.me.app`, then
/// `agent-sim snapshot` to read the screen and `agent-sim tap @e3` / `type` / `scroll` to act on it.
#[derive(Parser)]
#[command(name = "agent-sim", version, about, long_about)]
struct Cli {
    /// The phone: its name (setup-vm.sh makes `p`; pool.sh `pool1`...), or $AGENT_SIM_DEVICE.
    #[arg(long, short = 'd', global = true)]
    device: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Set this machine up: x86Sim, Apple's iOS runtime (about 8 GB, from Apple) and a phone. Re-runnable; with
    /// --device, adds another phone.
    Setup {
        /// fast: drawn at 2x with Reduce Transparency and Reduce Motion (scrolls faster). high: the model's own 3x
        /// and every visual effect, as Apple's simulator shows it.
        #[arg(long, default_value = "fast", value_parser = ["fast", "high"])]
        fidelity: String,
        /// Device model, e.g. "iPhone 17 Pro" (`x86sim device models` lists them).
        #[arg(long)]
        model: Option<String>,
    },
    /// Visual effects (blur, transparency, motion): full, as on a device, or reduced, which draws faster. Restarts
    /// the phone if it runs.
    Effects {
        #[arg(value_parser = ["full", "reduced"])]
        mode: String,
    },
    /// Check what a phone needs on this machine (systemd, namespaces, Vulkan, the runtime, the hooks, the phone) and
    /// say what is missing. Exits 1 when something blocks.
    Doctor,
    /// Boot the phone and wait for its home screen.
    Up,
    /// Shut the phone down.
    Down,
    /// List the phones on this machine and whether they run.
    Status,
    /// Read the screen: one line per element, with references (@e3) that tap accepts.
    Snapshot {
        /// Everything as JSON (frames, ids, hints, states).
        #[arg(long)]
        json: bool,
    },
    /// Save the screen as a PNG.
    Screenshot {
        /// Output file.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Tap an element (@e3 from the last snapshot), a point (x y, in screen points), or the first element whose
    /// text matches.
    Tap {
        #[arg(required = true, num_args = 1..)]
        target: Vec<String>,
        /// Hold for this long (a long press).
        #[arg(long, default_value_t = 80)]
        hold_ms: u64,
    },
    /// Type text into the focused field (\n is Return).
    Type {
        text: String,
        /// Send key presses instead of inserting the text (for apps that watch keystrokes).
        #[arg(long)]
        keys: bool,
    },
    /// Scroll the content: down shows what is below.
    Scroll {
        #[arg(value_parser = ["up", "down", "left", "right"])]
        direction: String,
        /// How far, in screen heights times 0.35 (1 by default).
        #[arg(long, default_value_t = 1.0)]
        amount: f64,
    },
    /// Drag from one point to another (screen points).
    Swipe {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[arg(long, default_value_t = 300)]
        ms: u64,
    },
    /// Two-finger pinch centered at (cx, cy) in screen points: finger half-spacing goes from r1 to r2 (r2 > r1 zooms in).
    Pinch {
        cx: f64,
        cy: f64,
        r1: f64,
        r2: f64,
        #[arg(long, default_value_t = 500)]
        ms: u64,
    },
    /// Print or set the device orientation (portrait, portraitUpsideDown, landscapeLeft, landscapeRight, faceUp, faceDown).
    Orientation {
        #[arg(value_parser = ["portrait", "portraitUpsideDown", "landscapeLeft", "landscapeRight", "faceUp", "faceDown"])]
        name: Option<String>,
    },
    /// Rotate the phone 90 degrees left or right.
    Rotate {
        #[arg(value_parser = ["left", "right"])]
        direction: String,
    },
    /// Press a button: home, lock, volup, voldown, siri, action...
    Press { button: String },
    /// Install an app: an .ipa (device builds are converted for the simulator), a simulator .app, an archive holding
    /// one (.tar.gz, .zip, as EAS Build and CI artifacts come), or an http(s) URL of any of these.
    Install { path: PathBuf },
    /// Remove an installed app.
    Uninstall { bundle_id: String },
    /// Start an app.
    Launch {
        bundle_id: String,
        /// Capture its os_log messages (read them with `agent-sim logs`); restarts it if it runs.
        #[arg(long)]
        logs: bool,
        /// Environment variable for the app, as KEY=VALUE (repeatable).
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Arguments passed to the app (after `--`).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// An app's os_log messages and the report of its last run (a crash's cause and backtrace).
    Logs {
        bundle_id: String,
        #[arg(long, default_value_t = 100)]
        lines: usize,
        /// Only lines containing this text.
        #[arg(long)]
        grep: Option<String>,
    },
    /// Stop an app.
    Terminate { bundle_id: String },
    /// List installed apps (yours; --all for the system's too).
    Apps {
        #[arg(long)]
        all: bool,
    },
    /// Light or dark mode.
    Appearance {
        #[arg(value_parser = ["light", "dark"])]
        mode: String,
    },
    /// Print the phone's clipboard (x86Sim 0.6 or later), or set it to TEXT.
    Clipboard { text: Option<String> },
    /// Override the status bar for clean screenshots (time, battery), or --clear.
    StatusBar {
        /// Shown time, e.g. 9:41.
        #[arg(long)]
        time: Option<String>,
        /// Battery percentage.
        #[arg(long)]
        battery: Option<u8>,
        #[arg(long)]
        charging: bool,
        #[arg(long, conflicts_with_all = ["time", "battery", "charging"])]
        clear: bool,
    },
    /// Simulate the phone's GPS location (`location set LAT,LON` or `location clear`).
    Location {
        #[command(subcommand)]
        action: LocationAction,
    },
    /// Add photos or videos to the Photos library (as `simctl addmedia`).
    Addmedia {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Put files in the Files app's On My iPhone, list them and remove them.
    Files {
        #[command(subcommand)]
        action: FilesAction,
    },
    /// Grant, revoke or reset app permissions without a prompt (as `simctl privacy`).
    Privacy {
        #[arg(value_parser = ["grant", "revoke", "reset"])]
        action: String,
        /// Service: all, location, location-always, calendar, contacts, contacts-limited, photos,
        /// photos-add, media-library, microphone, motion, reminders, siri, camera, homekit.
        service: String,
        /// App bundle id (required for grant/revoke, and for reset location/location-always).
        bundle_id: Option<String>,
    },
    /// Send a simulated push notification (`push [BUNDLE_ID] PAYLOAD.json|-`).
    Push {
        #[arg(required = true, num_args = 1..=2)]
        args: Vec<String>,
    },
    /// Record the screen to an MP4.
    Record {
        #[arg(short, long, default_value = "recording.mp4")]
        out: PathBuf,
        /// Seconds.
        #[arg(long, default_value_t = 10.0)]
        secs: f64,
        #[arg(long, default_value_t = 30)]
        fps: u32,
    },
    /// Start the phone over: apps, data and settings gone (as `simctl erase`).
    Erase,
    /// Open a URL (web pages in Safari, or an app's deep link).
    Open { url: String },
    /// Wait until text is on screen (or, with --gone, no longer is).
    Wait {
        text: String,
        /// Seconds.
        #[arg(long, default_value_t = 20.0)]
        timeout: f64,
        #[arg(long)]
        gone: bool,
    },
    /// Keep phones booted for agents: `pool up 3`, `pool take`, `pool release NAME`, `pool status`, `pool down`.
    Pool {
        #[arg(required = true, num_args = 1..=2)]
        args: Vec<String>,
    },
    /// Serve agent-sim's tools over MCP (stdio), for Claude Code, Cursor and other MCP clients.
    Mcp,
    /// Stream a running phone to a browser and take input back.
    Serve {
        /// Address to listen on.
        #[arg(long, default_value = "127.0.0.1:7979")]
        listen: SocketAddr,
        /// Access token in the URL path; random when not given.
        #[arg(long, env = "AGENT_SIM_TOKEN")]
        token: Option<String>,
        /// Stream at 1/N of the device's pixel size in each direction; 0 picks the largest size up to 900 pixels
        /// wide.
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=4))]
        scale: u8,
        /// Video bitrate in kbit/s.
        #[arg(long, default_value_t = 4000)]
        bitrate: u32,
    },
    /// Time the streaming path on synthetic frames (for tuning).
    #[command(hide = true)]
    EncodeBench {
        #[arg(long, default_value_t = 804)]
        width: usize,
        #[arg(long, default_value_t = 1748)]
        height: usize,
        #[arg(long, default_value_t = 1)]
        scale: usize,
        #[arg(long, default_value_t = 120)]
        frames: usize,
        #[arg(long, default_value_t = 4000)]
        bitrate: u32,
    },
}

#[derive(Subcommand)]
enum LocationAction {
    /// Fix the simulated location to LAT,LON (e.g. `51.5074,-0.1278`).
    Set {
        #[arg(allow_hyphen_values = true)]
        coords: String,
    },
    /// Stop simulating a location.
    Clear,
}

#[derive(Subcommand)]
enum FilesAction {
    /// Copy files or folders into On My iPhone (`files push book.pdf Scores/ --to Library`).
    Push {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Folder inside On My iPhone to copy into, created if missing.
        #[arg(long)]
        to: Option<String>,
    },
    /// List a folder of On My iPhone (its top when none is given).
    Ls { dir: Option<String> },
    /// Remove a file or folder from On My iPhone.
    Rm { path: String },
}

fn random_token() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .context("reading /dev/urandom")?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn say(r: Result<String>) -> Result<()> {
    println!("{}", r?);
    Ok(())
}

fn main() -> Result<()> {
    // piped into `head` and the like: stop quietly when the reader goes away, as command-line tools do
    // SAFETY: setting a signal's disposition to its default before any other thread exists
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let name = phone::pick_device(cli.device.as_deref());
    let dev = || device::Device::find(&name);
    match cli.command {
        Command::Setup { fidelity, model } => {
            let mut env = vec![("DEVICE", name.as_str()), ("FIDELITY", fidelity.as_str())];
            if let Some(m) = &model {
                env.push(("MODEL", m.as_str()));
            }
            phone::script("setup-vm.sh", &[], &env)
        }
        Command::Doctor => {
            let checks = doctor::run(&name);
            println!("{}", doctor::format(&checks));
            if checks.iter().any(|c| c.level == doctor::Level::Fail) {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Effects { mode } => say(cmd::effects(&name, &mode)),
        Command::Pool { args } => {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            phone::script("pool.sh", &a, &[])
        }
        Command::Mcp => mcp::run(&name),
        Command::Up => cmd::up(&name),
        Command::Erase => cmd::erase(&name),
        Command::Appearance { mode } => say(cmd::appearance(&dev()?, &mode)),
        Command::StatusBar {
            time,
            battery,
            charging,
            clear,
        } => say(cmd::status_bar(
            &dev()?,
            time.as_deref(),
            battery,
            charging,
            clear,
        )),
        Command::Location { action } => match action {
            LocationAction::Set { coords } => {
                let (lat, lon) = cmd::parse_lat_lon(&coords)?;
                say(cmd::location_set(&dev()?, lat, lon))
            }
            LocationAction::Clear => say(cmd::location_clear(&dev()?)),
        },
        Command::Addmedia { paths } => say(cmd::addmedia(&dev()?, &paths)),
        Command::Files { action } => match action {
            FilesAction::Push { paths, to } => say(files::push(&dev()?, &paths, to.as_deref())),
            FilesAction::Ls { dir } => say(files::ls(&dev()?, dir.as_deref())),
            FilesAction::Rm { path } => say(files::rm(&dev()?, &path)),
        },
        Command::Privacy {
            action,
            service,
            bundle_id,
        } => say(cmd::privacy(
            &dev()?,
            &action,
            &service,
            bundle_id.as_deref(),
        )),
        Command::Push { args } => say(cmd::push_cli(&dev()?, &args)),
        Command::Clipboard { text } => say(cmd::clipboard(&dev()?, text.as_deref())),
        Command::Record { out, secs, fps } => say(cmd::record(
            &dev()?,
            &out,
            secs.clamp(0.1, 3600.0),
            fps.clamp(1, 60),
        )),
        Command::Down => cmd::down(&name),
        Command::Status => cmd::status(),
        Command::Snapshot { json } => say(cmd::snapshot(&dev()?, json)),
        Command::Screenshot { out } => say(cmd::screenshot(&dev()?, out)),
        Command::Tap { target, hold_ms } => say(cmd::tap(&dev()?, &target, hold_ms)),
        Command::Type { text, keys } => {
            say(cmd::type_text(&dev()?, &text.replace("\\n", "\n"), keys))
        }
        Command::Scroll { direction, amount } => say(cmd::scroll(&dev()?, &direction, amount)),
        Command::Swipe { x1, y1, x2, y2, ms } => say(cmd::swipe(&dev()?, (x1, y1), (x2, y2), ms)),
        Command::Pinch { cx, cy, r1, r2, ms } => say(cmd::pinch(&dev()?, cx, cy, r1, r2, ms)),
        Command::Orientation { name } => say(cmd::set_orientation(&dev()?, name.as_deref())),
        Command::Rotate { direction } => say(cmd::rotate_device(&dev()?, &direction)),
        Command::Press { button } => say(cmd::press(&dev()?, &button)),
        Command::Install { path } => say(cmd::install(&dev()?, &path)),
        Command::Uninstall { bundle_id } => say(cmd::uninstall(&dev()?, &bundle_id)),
        Command::Launch {
            bundle_id,
            logs,
            env,
            args,
        } => say(cmd::launch(&dev()?, &bundle_id, logs, &env, &args)),
        Command::Logs {
            bundle_id,
            lines,
            grep,
        } => say(cmd::logs(&dev()?, &bundle_id, lines, grep.as_deref())),
        Command::Terminate { bundle_id } => say(cmd::terminate(&dev()?, &bundle_id)),
        Command::Apps { all } => say(cmd::apps(&dev()?, all)),
        Command::Open { url } => say(cmd::open_url(&dev()?, &url)),
        Command::Wait {
            text,
            timeout,
            gone,
        } => say(cmd::wait(
            &dev()?,
            &text,
            Duration::from_secs_f64(timeout.max(0.0)),
            gone,
        )),
        Command::EncodeBench {
            width,
            height,
            scale,
            frames,
            bitrate,
        } => encode_bench::run(
            width,
            height,
            scale,
            frames.max(1),
            bitrate.saturating_mul(1000),
        ),
        Command::Serve {
            listen,
            token,
            scale,
            bitrate,
        } => {
            let token = match token {
                Some(t) if t.len() >= 16 && t.chars().all(|c| c.is_ascii_alphanumeric()) => t,
                Some(_) => anyhow::bail!("--token must be at least 16 letters or digits"),
                None => random_token()?,
            };
            let opts = serve::Options {
                device: dev()?,
                listen,
                token,
                scale: scale as usize,
                bitrate_bps: bitrate.saturating_mul(1000),
            };
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(serve::run(opts))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn launch_parses_env_and_trailing_args() {
        let cli = Cli::parse_from([
            "agent-sim",
            "launch",
            "com.example.app",
            "--logs",
            "--env",
            "FOO=bar",
            "--",
            "-mode",
            "demo",
        ]);
        match cli.command {
            Command::Launch {
                bundle_id,
                logs,
                env,
                args,
            } => {
                assert_eq!(bundle_id, "com.example.app");
                assert!(logs);
                assert_eq!(env, vec!["FOO=bar"]);
                assert_eq!(args, vec!["-mode", "demo"]);
            }
            _ => panic!("expected Launch"),
        }
    }

    #[test]
    fn tap_parses_hold_ms_before_and_after_target() {
        for argv in [
            vec!["agent-sim", "tap", "Settings", "--hold-ms", "900"],
            vec!["agent-sim", "tap", "--hold-ms", "900", "Settings"],
        ] {
            match Cli::parse_from(argv).command {
                Command::Tap { target, hold_ms } => {
                    assert_eq!(target, vec!["Settings"]);
                    assert_eq!(hold_ms, 900);
                }
                _ => panic!("expected Tap"),
            }
        }
        match Cli::parse_from(["agent-sim", "tap", "200", "400", "--hold-ms", "650"]).command {
            Command::Tap { target, hold_ms } => {
                assert_eq!(target, vec!["200", "400"]);
                assert_eq!(hold_ms, 650);
            }
            _ => panic!("expected Tap"),
        }
        match Cli::parse_from(["agent-sim", "tap", "--", "-5%"]).command {
            Command::Tap { target, hold_ms } => {
                assert_eq!(target, vec!["-5%"]);
                assert_eq!(hold_ms, 80);
            }
            _ => panic!("expected Tap"),
        }
    }
}

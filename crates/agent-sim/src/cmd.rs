//! The commands an agent uses: boot, look, touch, type, apps.

use crate::ax;
use crate::device::Device;
use crate::input::Hid;
use crate::phone::{self, Screen};
use crate::ring::Ring;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn up(name: &str) -> Result<()> {
    phone::script("phone.sh", &["start", name], &[])?;
    phone::script("phone.sh", &["wait", name], &[])
}

pub fn down(name: &str) -> Result<()> {
    phone::script("phone.sh", &["stop", name], &[])
}

pub fn status() -> Result<()> {
    let var = crate::device::data_dir()?.join("var");
    let mut found = false;
    for e in std::fs::read_dir(&var)
        .with_context(|| format!("reading {}", var.display()))?
        .flatten()
    {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(name) = n.strip_prefix("device-a64-") else {
            continue;
        };
        let Ok(dev) = Device::find(name) else {
            continue;
        };
        found = true;
        let model = phone::sim_env(&dev)
            .ok()
            .and_then(|m| m.get("SIMULATOR_MODEL_IDENTIFIER").cloned())
            .unwrap_or_default();
        let state = match phone::health(&dev) {
            phone::Health::Running => match phone::springboard_answers(&dev) {
                Some(false) => "hung (SpringBoard does not answer; `agent-sim up` restarts it)",
                _ => "running",
            },
            phone::Health::Stopped => "stopped",
            phone::Health::Crashed => {
                "crashed (Darling's server exited; `agent-sim up` restarts it)"
            }
        };
        println!("{name}\t{state}\t{model}");
    }
    if !found {
        println!("no phones yet: run scripts/setup-vm.sh");
    }
    Ok(())
}

/// Valid device orientations accepted by `x86sim orientation` / `simhid`.
pub const ORIENTATIONS: &[&str] = &[
    "portrait",
    "portraitUpsideDown",
    "landscapeLeft",
    "landscapeRight",
    "faceUp",
    "faceDown",
];

/// How the phone is turned (x86Sim's orientation): portrait, landscapeLeft, landscapeRight, portraitUpsideDown.
fn current_orientation(dev: &Device) -> String {
    phone::x86sim_ok(dev, &["orientation"])
        .map(|o| o.trim().to_string())
        .unwrap_or_else(|_| "portrait".into())
}

/// Gets or sets the phone's orientation (`portrait`, `portraitUpsideDown`, `landscapeLeft`,
/// `landscapeRight`, `faceUp`, `faceDown`).
pub fn set_orientation(dev: &Device, name: Option<&str>) -> Result<String> {
    let out = match name {
        Some(n) => {
            if !ORIENTATIONS.contains(&n) {
                bail!(
                    "unknown orientation {n:?} (expected one of: {})",
                    ORIENTATIONS.join(", ")
                );
            }
            phone::x86sim_ok(dev, &["orientation", n])?
        }
        None => phone::x86sim_ok(dev, &["orientation"])?,
    };
    // a phone nobody has turned yet reports `unknown`; it is upright, as `screenshot` treats it
    Ok(match out.trim() {
        "unknown" | "" => "portrait".to_string(),
        o => o.to_string(),
    })
}

/// Turns the phone 90 degrees left (counterclockwise) or right (clockwise) and returns the new orientation.
pub fn rotate_device(dev: &Device, direction: &str) -> Result<String> {
    if !matches!(direction, "left" | "right") {
        bail!("rotate direction must be left or right (got {direction:?})");
    }
    phone::x86sim_ok(dev, &["rotate", direction])?;
    let now = phone::x86sim_ok(dev, &["orientation"])
        .map(|o| o.trim().to_string())
        .unwrap_or_default();
    if now.is_empty() {
        Ok(format!("rotated {direction}"))
    } else {
        Ok(format!("rotated {direction} ({now})"))
    }
}

/// Maps upright screen points `(x, y)` into physical panel normalized `(0..1, 0..1)` coordinates
/// matching `rotate()` and `x86sim`'s `unrotate()`.
fn upright_to_panel(scr: Screen, x: f64, y: f64, orientation: &str) -> (f64, f64) {
    match orientation {
        "landscapeLeft" => {
            let (u, v) = (x / scr.height, y / scr.width);
            (1.0 - v, u)
        }
        "landscapeRight" => {
            let (u, v) = (x / scr.height, y / scr.width);
            (v, 1.0 - u)
        }
        "portraitUpsideDown" => {
            let (u, v) = (x / scr.width, y / scr.height);
            (1.0 - u, 1.0 - v)
        }
        _ => scr.unit(x, y),
    }
}

fn upright_bounds(scr: Screen, orientation: &str) -> (f64, f64) {
    match orientation {
        "landscapeLeft" | "landscapeRight" => (scr.height, scr.width),
        _ => (scr.width, scr.height),
    }
}

/// The screen as a PNG, upright as the user sees it: turned with the phone, as Apple's Simulator turns its window.
pub fn screenshot_png(dev: &Device) -> Result<Vec<u8>> {
    let mut ring = Ring::new(&dev.ring_path()?);
    let frame = ring
        .latest()
        .context("no frame yet: is the phone running?")?;
    let (w, h) = (frame.width, frame.height);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for px in frame.bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let (rgb, w, h) = rotate(rgb, w, h, &current_orientation(dev));
    let mut png_bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut png_bytes, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(png_bytes)
}

/// Turns an RGB image so a landscape or upside-down screen reads upright.
fn rotate(src: Vec<u8>, w: usize, h: usize, orientation: &str) -> (Vec<u8>, usize, usize) {
    let px = |x: usize, y: usize| &src[(y * w + x) * 3..(y * w + x) * 3 + 3];
    match orientation {
        // the content's top is at the framebuffer's right edge: turn it a quarter to the left
        "landscapeLeft" => {
            let mut out = Vec::with_capacity(src.len());
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(w - 1 - y, x));
                }
            }
            (out, h, w)
        }
        "landscapeRight" => {
            let mut out = Vec::with_capacity(src.len());
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(y, h - 1 - x));
                }
            }
            (out, h, w)
        }
        "portraitUpsideDown" => {
            let mut out = Vec::with_capacity(src.len());
            for y in 0..h {
                for x in 0..w {
                    out.extend_from_slice(px(w - 1 - x, h - 1 - y));
                }
            }
            (out, w, h)
        }
        _ => (src, w, h),
    }
}

pub fn screenshot(dev: &Device, out: Option<PathBuf>) -> Result<String> {
    let out = out.unwrap_or_else(|| PathBuf::from("screenshot.png"));
    std::fs::write(&out, screenshot_png(dev)?)
        .with_context(|| format!("writing {}", out.display()))?;
    Ok(out.display().to_string())
}

pub fn snapshot(dev: &Device, json: bool) -> Result<String> {
    let s = ax::snapshot(dev)?;
    Ok(if json {
        serde_json::to_string_pretty(&s)?
    } else {
        ax::format(&s).trim_end().to_string()
    })
}

/// Where a tap target is: `@e3`, `x y` in screen points, or text an element's label, value or id contains.
/// Like point, with the element's role (empty for a bare point).
fn point_role(dev: &Device, target: &[String]) -> Result<(f64, f64, String, String)> {
    match target {
        [x, y] if x.parse::<f64>().is_ok() && y.parse::<f64>().is_ok() => {
            Ok((x.parse()?, y.parse()?, format!("({x},{y})"), String::new()))
        }
        [r] if r.starts_with('@') => {
            let e = ax::resolve(dev, r)?;
            let (x, y) = e.center();
            Ok((x, y, describe(&e), e.role.clone()))
        }
        [] => bail!("tap what? an element (@e3), a point (x y) or text"),
        words => {
            // the screen may still be changing (an app opening, a page sliding in): look again for a moment
            let text = words.join(" ");
            let start = Instant::now();
            loop {
                let s = ax::snapshot(dev)?;
                if let Some(e) = ax::find(&s, &text) {
                    let (x, y) = e.center();
                    return Ok((x, y, describe(e), e.role.clone()));
                }
                if start.elapsed() > Duration::from_secs(3) {
                    bail!("nothing on screen matches \"{text}\"");
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        }
    }
}

fn describe(e: &ax::Element) -> String {
    match &e.label {
        Some(l) => format!("@{} {} \"{l}\"", e.reference, e.role),
        None => format!("@{} {}", e.reference, e.role),
    }
}

fn hid(dev: &Device) -> Hid {
    Hid::new(dev.hid_socket())
}

pub fn tap(dev: &Device, target: &[String], hold_ms: u64) -> Result<String> {
    let (x, y, what, role) = point_role(dev, target)?;
    let scr = Screen::of(dev)?;
    let (u, v) = scr.unit(x, y);
    let mut h = hid(dev);
    h.touch("down", u, v)?;
    std::thread::sleep(Duration::from_millis(hold_ms.max(80)));
    h.touch("up", u, v)?;
    if role == "textfield" {
        // focus reaches a field (a web field above all) a moment after the tap; typing before it lands elsewhere
        std::thread::sleep(Duration::from_millis(1200));
    }
    Ok(format!("tapped {what}"))
}

fn drag_progress(t: f64, ease_out: bool) -> f64 {
    if ease_out { 1.0 - (1.0 - t).powi(3) } else { t }
}

fn drag(dev: &Device, from: (f64, f64), to: (f64, f64), ms: u64, hold_ms: u64) -> Result<()> {
    let scr = Screen::of(dev)?;
    let mut h = hid(dev);
    let steps = (ms / 16).clamp(4, 120);
    let ease_out = hold_ms > 0;
    let (a, b) = scr.unit(from.0, from.1);
    h.touch("down", a, b)?;
    for i in 1..=steps {
        let t = drag_progress(i as f64 / steps as f64, ease_out);
        let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let (u, v) = scr.unit(x, y);
        std::thread::sleep(Duration::from_millis(ms / steps));
        h.touch("move", u, v)?;
    }
    let (u, v) = scr.unit(to.0, to.1);
    if hold_ms > 0 {
        // Hold stationary at the endpoint while continuing to send move samples so UIKit and Flutter
        // velocity trackers see near-zero release velocity and do not start inertial deceleration.
        let until = Instant::now() + Duration::from_millis(hold_ms);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(16));
            h.touch("move", u, v)?;
        }
    }
    h.touch("up", u, v)?;
    Ok(())
}

pub fn swipe(dev: &Device, from: (f64, f64), to: (f64, f64), ms: u64) -> Result<String> {
    drag(dev, from, to, ms, 0)?;
    Ok(format!(
        "swiped ({:.0},{:.0}) to ({:.0},{:.0})",
        from.0, from.1, to.0, to.1
    ))
}

fn validate_pinch(cx: f64, cy: f64, r1: f64, r2: f64, bounds: (f64, f64)) -> Result<()> {
    if ![cx, cy, r1, r2].iter().all(|v| v.is_finite()) {
        bail!("pinch coordinates and radii must be finite numbers");
    }
    if r1 < 0.0 || r2 < 0.0 {
        bail!("pinch radii must be non-negative (got r1={r1}, r2={r2})");
    }
    let (w, h) = bounds;
    let rmax = r1.max(r2);
    if cy < 0.0 || cy > h || cx - rmax < 0.0 || cx + rmax > w {
        bail!(
            "pinch endpoints ({:.0}±{:.0}, {:.0}) exceed screen bounds ({:.0}x{:.0})",
            cx,
            rmax,
            cy,
            w,
            h
        );
    }
    Ok(())
}

/// Two-finger pinch centered at `(cx, cy)` in upright screen points: finger half-spacing moves from `r1` to `r2`
/// (`r2 > r1` zooms in, `r2 < r1` zooms out).
pub fn pinch(dev: &Device, cx: f64, cy: f64, r1: f64, r2: f64, ms: u64) -> Result<String> {
    let scr = Screen::of(dev)?;
    let orient = current_orientation(dev);
    validate_pinch(cx, cy, r1, r2, upright_bounds(scr, &orient))?;
    let mut h = hid(dev);
    let steps = (ms / 16).clamp(4, 120);
    let fingers = |r: f64| {
        (
            upright_to_panel(scr, cx - r, cy, &orient),
            upright_to_panel(scr, cx + r, cy, &orient),
        )
    };
    let (p1, p2) = fingers(r1);
    h.touch2("down", p1, p2)?;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let r = r1 + (r2 - r1) * t;
        let (p1, p2) = fingers(r);
        std::thread::sleep(Duration::from_millis(ms / steps));
        h.touch2("move", p1, p2)?;
    }
    std::thread::sleep(Duration::from_millis(50));
    let (p1, p2) = fingers(r2);
    h.touch2("up", p1, p2)?;
    Ok(format!(
        "pinched at ({cx:.0},{cy:.0}) radius {r1:.0} to {r2:.0}"
    ))
}

/// Scrolls the content: `down` shows what is below (the finger moves up), as on a phone.
/// Slows into the endpoint and holds briefly before lifting so the next tap is not eaten by scroll deceleration.
pub fn scroll(dev: &Device, dir: &str, amount: f64) -> Result<String> {
    let scr = Screen::of(dev)?;
    let (cx, cy) = (scr.width / 2.0, scr.height / 2.0);
    let d = scr.height * 0.35 * amount.clamp(0.1, 2.0);
    let (from, to) = match dir {
        "down" => ((cx, cy + d / 2.0), (cx, cy - d / 2.0)),
        "up" => ((cx, cy - d / 2.0), (cx, cy + d / 2.0)),
        "left" => ((cx - d / 2.0, cy), (cx + d / 2.0, cy)),
        "right" => ((cx + d / 2.0, cy), (cx - d / 2.0, cy)),
        _ => bail!("scroll up, down, left or right"),
    };
    drag(dev, from, to, 300, 120)?;
    Ok(format!("scrolled {dir}"))
}

#[derive(Debug, PartialEq, Eq)]
enum TypeStep<'a> {
    Text(&'a str),
    Return,
}

/// Splits text on newlines so single-line iOS fields (which turn `\n` passed to `insertText:` into a space instead of
/// submitting) receive Return as an actual HID key press.
fn type_steps(text: &str) -> Vec<TypeStep<'_>> {
    let mut steps = Vec::new();
    for (i, part) in text.split('\n').enumerate() {
        if i > 0 {
            steps.push(TypeStep::Return);
        }
        if !part.is_empty() {
            steps.push(TypeStep::Text(part));
        }
    }
    steps
}

fn type_keys(dev: &Device, text: &str) -> Result<()> {
    if text.chars().all(|c| crate::input::key_usage(c).is_some()) {
        hid(dev).text(text)?;
    } else {
        // characters without a key of their own go through x86Sim's typing
        phone::x86sim_ok(dev, &["type", text])?;
    }
    Ok(())
}

/// Types into the field that has focus. The app inserts the text as the software keyboard does, which every field
/// takes (web fields lost most hardware key events), and sends `\n` as a HID Return key so single-line fields submit;
/// with no field in focus, or `keys`, it goes as key presses.
pub fn type_text(dev: &Device, text: &str, keys: bool) -> Result<String> {
    if !keys {
        let steps = type_steps(text);
        if steps.iter().any(|s| matches!(s, TypeStep::Text(_))) {
            let mut sent_any = false;
            let mut fallback = false;
            for step in &steps {
                match step {
                    TypeStep::Text(part) => {
                        if insert_text(dev, part)? {
                            sent_any = true;
                        } else if !sent_any {
                            fallback = true;
                            break;
                        } else {
                            type_keys(dev, part)?;
                        }
                    }
                    TypeStep::Return => {
                        hid(dev).text("\n")?;
                        sent_any = true;
                    }
                }
            }
            if !fallback {
                return Ok(format!("typed {} characters", text.chars().count()));
            }
        }
    }
    type_keys(dev, text)?;
    Ok(format!(
        "typed {} characters as key presses",
        text.chars().count()
    ))
}

/// Asks the app in front to insert `text` into its focused field; false when it has none (or does not answer).
fn insert_text(dev: &Device, text: &str) -> Result<bool> {
    springboard(dev, "front")?;
    let dir = phone::data_dir(dev).join("tmp/agent-sim");
    let front: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("front.json"))?)?;
    if front
        .get("locked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let Some(pid) = front
        .get("hosted_pid")
        .and_then(|p| p.as_u64())
        .filter(|p| *p > 0)
        .or_else(|| front.get("pid").and_then(|p| p.as_u64()).filter(|p| *p > 0))
    else {
        return Ok(false);
    };
    let pid = pid as u32;
    let Some(&linux) = phone::processes(dev, |inner, _| inner == pid)?.first() else {
        return Ok(false);
    };
    let answer = dir.join(format!("answer-{pid}.txt"));
    let _ = std::fs::remove_file(&answer);
    std::fs::write(
        dir.join(format!("cmd-{pid}.txt")),
        format!("insert\n{text}"),
    )?;
    if phone::ask_hook(&dir, linux, pid).is_err() {
        return Ok(false);
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok(a) = std::fs::read_to_string(&answer) {
            return Ok(a == "ok");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(dir.join(format!("cmd-{pid}.txt")));
    Ok(false)
}

pub fn press(dev: &Device, button: &str) -> Result<String> {
    match button {
        "home" | "lock" | "volup" | "voldown" => {
            let mut h = hid(dev);
            h.button(button, true)?;
            std::thread::sleep(Duration::from_millis(100));
            h.button(button, false)?;
        }
        _ => {
            phone::x86sim_ok(dev, &["button", button])?;
        }
    }
    Ok(format!("pressed {button}"))
}

fn launch_args(
    bundle: &str,
    logs: bool,
    env: &[String],
    app_args: &[String],
) -> Result<Vec<String>> {
    let mut out = vec!["launch".to_string()];
    if logs {
        out.push("--oslog".into());
    }
    for entry in env {
        let Some((key, _)) = entry.split_once('=') else {
            bail!("--env expects KEY=VALUE, got {entry:?}");
        };
        if key.is_empty() {
            bail!("--env expects a non-empty key, got {entry:?}");
        }
        out.push("--env".into());
        out.push(entry.clone());
    }
    out.push(bundle.to_string());
    out.extend(app_args.iter().cloned());
    Ok(out)
}

pub fn launch(
    dev: &Device,
    bundle: &str,
    logs: bool,
    env: &[String],
    app_args: &[String],
) -> Result<String> {
    let owned = launch_args(bundle, logs, env, app_args)?;
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    // os_log capture starts with the process, so a running app is stopped first: here, where the exit is awaited and
    // the settle time below applies, rather than by x86Sim's own stop-and-start inside one command (issue #6)
    if logs && phone::x86sim_run(dev, &["pidof", bundle])?.status.success() {
        terminate(dev, bundle)?;
    }
    settle_after_terminate(dev, bundle);
    // an app started right after it was stopped sometimes dies during startup: start it again once
    for attempt in 0..2 {
        if attempt == 1 {
            std::thread::sleep(RELAUNCH_SETTLE);
        }
        let out = phone::x86sim_ok(dev, &args).map_err(|e| {
            if format!("{e:#}").contains("returned nil for") {
                anyhow::anyhow!("{bundle} is not installed (`agent-sim apps --all` lists what is)")
            } else {
                e
            }
        })?;
        let pid = out.split_whitespace().last().unwrap_or("?").to_string();
        if pid == "-1" {
            bail!("{bundle} did not start (is it installed? `agent-sim apps`)");
        }
        std::thread::sleep(Duration::from_millis(1500));
        let alive = phone::x86sim_run(dev, &["pidof", bundle])?.status.success();
        if alive || attempt == 1 {
            return Ok(format!("launched {bundle} (pid {pid})"));
        }
    }
    unreachable!()
}

/// How long after an app was stopped before it is started again. RunningBoard and FrontBoard are still tearing the
/// old process down, and a launch inside that window dies during startup, and after a few in a row can leave
/// SpringBoard unresponsive (issue #6).
const RELAUNCH_SETTLE: Duration = Duration::from_millis(1500);

/// The file whose modification time says when `terminate` last finished with `bundle`.
fn terminated_marker(dev: &Device, bundle: &str) -> Result<PathBuf> {
    let safe: String = bundle
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(phone::state_dir(dev)?.join(format!("terminated-{safe}")))
}

/// What is left of the settle time for an app stopped `age` ago, if anything.
fn settle_left(age: Duration) -> Option<Duration> {
    RELAUNCH_SETTLE.checked_sub(age).filter(|d| !d.is_zero())
}

/// Waits out what is left of the settle time when `bundle` was stopped a moment ago (no wait otherwise).
fn settle_after_terminate(dev: &Device, bundle: &str) {
    let age = terminated_marker(dev, bundle)
        .ok()
        .and_then(|m| std::fs::metadata(m).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok());
    if let Some(left) = age.and_then(settle_left) {
        std::thread::sleep(left);
    }
}

pub fn terminate(dev: &Device, bundle: &str) -> Result<String> {
    if let Err(e) = phone::x86sim_ok(dev, &["terminate", bundle]) {
        if format!("{e:#}").contains("not running") {
            return Ok(format!("{bundle} was not running"));
        }
        return Err(e);
    }
    // x86Sim answers before the process has exited; a launch right away would race the old process
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        let out = phone::x86sim_run(dev, &["pidof", bundle])?;
        if !out.status.success() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if let Ok(marker) = terminated_marker(dev, bundle) {
        let _ = std::fs::write(marker, b"");
    }
    Ok(format!("terminated {bundle}"))
}

pub fn open_url(dev: &Device, url: &str) -> Result<String> {
    const SAFARI: &str = "com.apple.mobilesafari";
    let web = url.starts_with("http://") || url.starts_with("https://");
    for attempt in 0..2 {
        phone::x86sim_ok(dev, &["openurl", url])?;
        if !web {
            break;
        }
        // a web link starts Safari when it is not running; make sure it did (and still runs), else ask again
        let start = Instant::now();
        let mut up = false;
        while start.elapsed() < Duration::from_secs(10) {
            if phone::x86sim_run(dev, &["pidof", SAFARI])?.status.success() {
                up = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if up {
            std::thread::sleep(Duration::from_millis(1000));
            if phone::x86sim_run(dev, &["pidof", SAFARI])?.status.success() {
                break;
            }
        }
        if attempt == 1 {
            bail!("Safari did not start for {url}");
        }
    }
    Ok(format!("opened {url}"))
}

pub fn apps(dev: &Device, all: bool) -> Result<String> {
    let out = phone::x86sim_ok(dev, &["listapps", "--json"])?;
    let v: serde_json::Value = serde_json::from_str(&out).context("reading x86sim listapps")?;
    let map = v
        .as_object()
        .context("x86sim listapps: unexpected output")?;
    let mut rows: Vec<(String, String, String)> = map
        .iter()
        .filter_map(|(id, a)| {
            let kind = a.get("ApplicationType")?.as_str()?.to_string();
            let name = a
                .get("CFBundleDisplayName")
                .or_else(|| a.get("CFBundleName"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            (all || kind == "User").then(|| (id.clone(), name, kind))
        })
        .collect();
    rows.sort();
    if rows.is_empty() {
        return Ok(
            "no apps installed yet (`agent-sim install App.app|App.ipa`; --all lists the system's)"
                .into(),
        );
    }
    Ok(rows
        .into_iter()
        .map(|(id, name, kind)| format!("{id}\t{name}\t{}", kind.to_lowercase()))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn install(dev: &Device, path: &Path) -> Result<String> {
    let app = crate::ipa::prepare(path)?;
    phone::x86sim_ok(dev, &["install", &app.path.to_string_lossy()])?;
    Ok(format!("installed {}", app.bundle_id))
}

pub fn uninstall(dev: &Device, bundle: &str) -> Result<String> {
    phone::x86sim_ok(dev, &["uninstall", bundle])?;
    Ok(format!("uninstalled {bundle}"))
}

/// Waits until an element whose label, value or id contains `text` is on screen (or, with `gone`, is not).
pub fn wait(dev: &Device, text: &str, timeout: Duration, gone: bool) -> Result<String> {
    let start = Instant::now();
    loop {
        let s = ax::snapshot(dev)?;
        let hit = ax::find(&s, text);
        match (hit, gone) {
            (Some(e), false) => return Ok(format!("found {}", describe(e))),
            (None, true) => return Ok(format!("\"{text}\" is gone")),
            _ => {}
        }
        if start.elapsed() > timeout {
            bail!(
                "{text:?} {} after {:.0} s",
                if gone {
                    "is still on screen"
                } else {
                    "did not appear"
                },
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// The executable name of an installed app.
fn executable(dev: &Device, bundle: &str) -> Result<String> {
    let out = phone::x86sim_ok(dev, &["listapps", "--json"])?;
    let v: serde_json::Value = serde_json::from_str(&out).context("reading x86sim listapps")?;
    v.get(bundle)
        .and_then(|a| a.get("CFBundleExecutable"))
        .and_then(|e| e.as_str())
        .map(str::to_string)
        .with_context(|| format!("{bundle} is not installed"))
}

fn newest(dir: &Path, prefix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()
        .map(|(_, p)| p)
}

fn tail(path: &Path, lines: usize, matching: Option<&str>) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text
        .lines()
        .filter(|l| !l.contains("is implemented in both"))
        .filter(|l| matching.is_none_or(|m| l.contains(m)))
        .collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// An app's os_log messages (when it was launched with logs on) and the translator's report of its last run, which
/// names a crash's cause and guest backtrace.
pub fn logs(dev: &Device, bundle: &str, lines: usize, matching: Option<&str>) -> Result<String> {
    let exe = executable(dev, bundle)?;
    let mut out = String::new();
    let oslog = newest(
        &phone::data_dir(dev).join("var/log"),
        &format!("oslog.{exe}."),
    );
    match oslog {
        Some(p) => {
            out.push_str(&format!("== os_log ({})\n", p.display()));
            out.push_str(&tail(&p, lines, matching));
        }
        None => out.push_str(&format!(
            "== os_log: none yet (`agent-sim launch {bundle} --logs` starts the app with its os_log captured)"
        )),
    }
    let name = dev
        .dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let short = name.strip_prefix("device-a64-").unwrap_or(&name);
    let run =
        crate::device::cache_dir()?.join(format!("a64prefix-{short}/private/var/tmp/x86sim-a64"));
    if let Some(p) = newest(&run, &format!("{exe}.")) {
        let report = tail(&p, lines.min(60), None);
        let crashed = report.contains("guest exception") || report.contains("killing pid");
        out.push_str(&format!(
            "\n\n== last run ({}){}\n{report}",
            p.display(),
            if crashed { ": crashed" } else { "" }
        ));
    }
    Ok(out)
}

/// The root of the iOS runtime the phone boots (its /usr/bin tools run inside the phone through `x86sim spawn`).
/// `<device>/runtime` names its directory under `runtimes/` (x86Sim's `device create` writes it); x86Sim links
/// `RuntimeRoot` there once the runtime is unpacked. A phone without one boots x86Sim's default runtime: the first
/// one installed is taken.
fn runtime_root(dev: &Device) -> Result<PathBuf> {
    let base = crate::device::data_dir()?.join("runtimes");
    let named = std::fs::read_to_string(dev.dir.join("runtime"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(name) = named {
        let root = base.join(&name).join("RuntimeRoot");
        if !root.join("usr/bin/defaults").is_file() {
            bail!(
                "the phone's runtime {name} is not installed under {}",
                base.display()
            );
        }
        return Ok(root);
    }
    for rt in std::fs::read_dir(&base)?.flatten() {
        let root = rt.path().join("RuntimeRoot");
        if root.join("usr/bin/defaults").is_file() {
            return Ok(root);
        }
    }
    bail!("no iOS runtime under {}", base.display())
}

/// Light or dark mode, as `simctl ui appearance` sets it: UIKit's style preference, then its change notification.
pub fn appearance(dev: &Device, mode: &str) -> Result<String> {
    let v = match mode {
        "light" => "1",
        "dark" => "2",
        _ => bail!("appearance light or dark"),
    };
    let rt = runtime_root(dev)?;
    let defaults = rt.join("usr/bin/defaults");
    let notify = rt.join("usr/bin/notifyutil");
    phone::x86sim_ok(
        dev,
        &[
            "spawn",
            &defaults.to_string_lossy(),
            "write",
            "com.apple.uikitservices.userInterfaceStyleMode",
            "UserInterfaceStyleMode",
            "-int",
            v,
        ],
    )?;
    phone::x86sim_ok(
        dev,
        &[
            "spawn",
            &notify.to_string_lossy(),
            "-p",
            "com.apple.uikitservices.userInterfaceStyleMode.changed",
        ],
    )?;
    Ok(format!("appearance {mode}"))
}

/// A request to agent-sim's hook in SpringBoard (hooks/ax.c), and its answer.
fn springboard(dev: &Device, req: &str) -> Result<String> {
    let dir = phone::data_dir(dev).join("tmp/agent-sim");
    std::fs::create_dir_all(&dir)?;
    let answer = dir.join("answer.txt");
    let _ = std::fs::remove_file(&answer);
    std::fs::write(dir.join("cmd.txt"), req)?;
    let sb = phone::process_ids(dev, |_, exe| exe == "SpringBoard")?;
    let Some(&(linux, inner)) = sb.first() else {
        return Err(phone::not_running(dev));
    };
    phone::ask_hook(&dir, linux, inner).context("SpringBoard is still starting")?;
    let start = Instant::now();
    loop {
        if let Ok(t) = std::fs::read_to_string(&answer) {
            return Ok(t);
        }
        // the first pasteboard read after a boot waits for the pasteboard service to start
        if start.elapsed() > Duration::from_secs(15) {
            // not left for the next signal to act on
            let _ = std::fs::remove_file(dir.join("cmd.txt"));
            bail!("no answer from SpringBoard (are agent-sim's hooks loaded?)");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Visual effects on or off (Reduce Transparency and Reduce Motion), restarting a running phone so every app
/// draws with the new setting.
pub fn effects(name: &str, mode: &str) -> Result<String> {
    let dev = Device::find(name)?;
    let reduce = match mode {
        "full" => "false",
        "reduced" => "true",
        _ => bail!("effects full or reduced"),
    };
    let defaults = runtime_root(&dev)?.join("usr/bin/defaults");
    for key in ["ReduceMotionEnabled", "EnhancedBackgroundContrastEnabled"] {
        phone::x86sim_ok(
            &dev,
            &[
                "spawn",
                &defaults.to_string_lossy(),
                "write",
                "com.apple.Accessibility",
                key,
                "-bool",
                reduce,
            ],
        )?;
    }
    if phone::is_running(&dev) {
        up(name)?;
        return Ok(format!("effects {mode}; phone restarted"));
    }
    Ok(format!("effects {mode}"))
}

/// Sets the general pasteboard through SpringBoard. Reading arbitrary third-party clipboard content
/// synchronously from SpringBoard's main queue deadlocks iOS's paste-permission flow, so reads return
/// an explicit error instead of hanging the phone.
pub fn clipboard(dev: &Device, set: Option<&str>) -> Result<String> {
    match set {
        Some(t) => {
            let r = springboard(dev, &format!("pasteboard-set\n{t}"))?;
            if r != "ok" {
                bail!("clipboard set failed: {r}");
            }
            Ok("clipboard set".into())
        }
        None => bail!(
            "reading the clipboard is not supported yet (iOS paste authorization blocks SpringBoard's main thread; `agent-sim clipboard TEXT` still sets it)"
        ),
    }
}

/// Overrides status bar items for clean screenshots, as `simctl status_bar override`; `clear` puts them back.
pub fn status_bar(
    dev: &Device,
    time: Option<&str>,
    battery: Option<u8>,
    charging: bool,
    clear: bool,
) -> Result<String> {
    if clear {
        springboard(dev, "statusbar-clear")?;
        return Ok("status bar back to normal".into());
    }
    let mut req = String::from("statusbar\n");
    if let Some(t) = time {
        req.push_str(&format!("time={t}\n"));
    }
    if let Some(b) = battery {
        req.push_str(&format!(
            "battery={}\ncharging={}\n",
            b.min(100),
            charging as u8
        ));
    }
    let r = springboard(dev, &req)?;
    if r != "ok" {
        bail!("status bar: {r}");
    }
    Ok("status bar overridden".into())
}

/// Services accepted by `x86sim privacy` / `simctl privacy`.
pub const PRIVACY_SERVICES: &[&str] = &[
    "all",
    "location",
    "location-always",
    "calendar",
    "contacts",
    "contacts-limited",
    "photos",
    "photos-add",
    "media-library",
    "microphone",
    "motion",
    "reminders",
    "siri",
    "camera",
    "homekit",
];

fn resolve_host_file(path: &Path) -> Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !abs.is_file() {
        bail!("{}: no such file", path.display());
    }
    Ok(abs)
}

fn validate_coords(lat: f64, lon: f64) -> Result<(f64, f64)> {
    if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
        bail!("latitude must be between -90 and 90 (got {lat})");
    }
    if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
        bail!("longitude must be between -180 and 180 (got {lon})");
    }
    Ok((lat, lon))
}

pub fn parse_lat_lon(coords: &str) -> Result<(f64, f64)> {
    let mut parts = coords.trim().split(',');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        bail!("expected LAT,LON (e.g. 51.5074,-0.1278), got {coords:?}");
    };
    let lat: f64 = a
        .trim()
        .parse()
        .with_context(|| format!("invalid latitude in {coords:?}"))?;
    let lon: f64 = b
        .trim()
        .parse()
        .with_context(|| format!("invalid longitude in {coords:?}"))?;
    validate_coords(lat, lon)
}

pub fn location_set(dev: &Device, lat: f64, lon: f64) -> Result<String> {
    let (lat, lon) = validate_coords(lat, lon)?;
    let dev_dir = dev.dir.to_string_lossy();
    let pair = format!("{lat},{lon}");
    phone::x86sim_ok(dev, &["location", &dev_dir, "set", &pair])?;
    Ok(format!("location set to {pair}"))
}

pub fn location_clear(dev: &Device) -> Result<String> {
    let dev_dir = dev.dir.to_string_lossy();
    phone::x86sim_ok(dev, &["location", &dev_dir, "clear"])?;
    Ok("location cleared".into())
}

pub fn addmedia(dev: &Device, paths: &[PathBuf]) -> Result<String> {
    if paths.is_empty() {
        bail!("addmedia requires at least one image or video file");
    }
    let resolved: Vec<String> = paths
        .iter()
        .map(|p| Ok(resolve_host_file(p)?.to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    let dev_dir = dev.dir.to_string_lossy();
    let mut args: Vec<&str> = Vec::with_capacity(resolved.len() + 2);
    args.push("addmedia");
    args.push(&dev_dir);
    for p in &resolved {
        args.push(p.as_str());
    }
    let out = phone::x86sim_ok(dev, &args)?;
    let msg = out.trim();
    if msg.is_empty() {
        Ok(format!("added {}", resolved.len()))
    } else {
        Ok(msg.to_string())
    }
}

fn validate_privacy<'a>(
    action: &'a str,
    service: &'a str,
    bundle_id: Option<&'a str>,
) -> Result<Option<&'a str>> {
    if !matches!(action, "grant" | "revoke" | "reset") {
        bail!("privacy action must be grant, revoke or reset (got {action:?})");
    }
    if !PRIVACY_SERVICES.contains(&service) {
        bail!(
            "unknown privacy service {service:?} (expected one of: {})",
            PRIVACY_SERVICES.join(", ")
        );
    }
    let bundle = bundle_id.map(str::trim).filter(|b| !b.is_empty());
    if action != "reset" && bundle.is_none() {
        bail!("privacy {action} requires a bundle id");
    }
    if action == "reset" && matches!(service, "location" | "location-always") && bundle.is_none() {
        bail!("privacy reset {service} requires a bundle id");
    }
    Ok(bundle)
}

pub fn privacy(
    dev: &Device,
    action: &str,
    service: &str,
    bundle_id: Option<&str>,
) -> Result<String> {
    let bundle = validate_privacy(action, service, bundle_id)?;
    let dev_dir = dev.dir.to_string_lossy();
    let mut args = vec!["privacy", &dev_dir, action, service];
    if let Some(b) = bundle {
        args.push(b);
    }
    phone::x86sim_ok(dev, &args)?;
    Ok(match bundle {
        Some(b) => format!("privacy {action} {service} for {b}"),
        None => format!("privacy {action} {service}"),
    })
}

fn validate_push_payload(bundle_id: Option<&str>, bytes: &[u8]) -> Result<String> {
    let payload: serde_json::Value =
        serde_json::from_slice(bytes).context("parsing push notification JSON payload")?;
    let Some(obj) = payload.as_object() else {
        bail!("push payload must be a JSON object");
    };
    if !obj.get("aps").is_some_and(serde_json::Value::is_object) {
        bail!("push payload must contain an \"aps\" JSON object");
    }
    let target = bundle_id
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .or_else(|| {
            obj.get("Simulator Target Bundle")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|b| !b.is_empty())
        })
        .context(
            "push requires a bundle id (argument or \"Simulator Target Bundle\" in the payload)",
        )?;
    Ok(target.to_string())
}

pub fn push_json(dev: &Device, bundle_id: Option<&str>, payload_bytes: &[u8]) -> Result<String> {
    let target = validate_push_payload(bundle_id, payload_bytes)?;
    let dev_dir = dev.dir.to_string_lossy();
    let out = phone::x86sim_ok_stdin(dev, &["push", &dev_dir, &target, "-"], payload_bytes)?;
    let msg = out.trim();
    if msg.is_empty() {
        Ok(format!("Notification sent to '{target}'"))
    } else {
        Ok(msg.to_string())
    }
}

pub fn push_cli(dev: &Device, args: &[String]) -> Result<String> {
    use std::io::Read;
    let (bundle, src) = match args {
        [src] => (None, src.as_str()),
        [bundle, src] => (Some(bundle.as_str()), src.as_str()),
        _ => bail!("usage: agent-sim push [BUNDLE_ID] PAYLOAD.json|-"),
    };
    let bytes = if src == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("reading push payload from stdin")?;
        buf
    } else {
        let path = resolve_host_file(Path::new(src))?;
        std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?
    };
    push_json(dev, bundle, &bytes)
}

/// A fresh phone under the same name: its apps, data and settings gone, as `simctl erase`.
pub fn erase(name: &str) -> Result<()> {
    let dir = crate::device::data_dir()?
        .join("var")
        .join(format!("device-a64-{name}"));
    if !dir.is_dir() {
        bail!("no phone named {name}");
    }
    let _ = phone::script("phone.sh", &["stop", name], &[]);
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    phone::script("setup-vm.sh", &[], &[("DEVICE", name)])
}

/// Records the screen to an MP4 (H.264) with ffmpeg, as `simctl io recordVideo`: `secs` long, at `fps`.
pub fn record(dev: &Device, out: &Path, secs: f64, fps: u32) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut ring = Ring::new(&dev.ring_path()?);
    let first = ring
        .latest()
        .context("no frame yet: is the phone running?")?;
    let (w, h) = (first.width, first.height);
    let mut ff = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
            "-s",
        ])
        .arg(format!("{w}x{h}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
        ])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .context("running ffmpeg (sudo apt install ffmpeg)")?;
    let mut stdin = ff.stdin.take().unwrap();
    let period = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let total = (secs * fps as f64).ceil() as u64;
    let start = Instant::now();
    let mut last = first.bgra;
    for i in 0..total {
        if let Some(f) = ring.latest()
            && f.width == w
            && f.height == h
        {
            last = f.bgra;
        }
        stdin.write_all(&last)?;
        let next = period * (i as u32 + 1);
        if let Some(wait) = next.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    drop(stdin);
    if !ff.wait()?.success() {
        bail!("ffmpeg failed");
    }
    Ok(format!("{} ({secs:.1} s at {fps} fps)", out.display()))
}

#[cfg(test)]
mod tests {
    use super::{
        RELAUNCH_SETTLE, TypeStep, clipboard, drag_progress, launch_args, parse_lat_lon, rotate,
        settle_left, type_steps, upright_bounds, upright_to_panel, validate_pinch,
        validate_privacy, validate_push_payload,
    };
    use crate::device::Device;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn launch_args_forwards_logs_env_and_app_args() {
        let args = launch_args(
            "com.example.app",
            true,
            &["A=1".into(), "B=two=2".into()],
            &["-flag".into(), "YES".into()],
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "launch",
                "--oslog",
                "--env",
                "A=1",
                "--env",
                "B=two=2",
                "com.example.app",
                "-flag",
                "YES",
            ]
        );
    }

    #[test]
    fn relaunch_waits_only_for_what_is_left_of_the_settle_time() {
        assert_eq!(settle_left(Duration::ZERO), Some(RELAUNCH_SETTLE));
        assert_eq!(
            settle_left(Duration::from_millis(1000)),
            Some(Duration::from_millis(500))
        );
        assert_eq!(settle_left(RELAUNCH_SETTLE), None);
        assert_eq!(settle_left(Duration::from_secs(60)), None);
    }

    #[test]
    fn launch_args_rejects_malformed_env() {
        assert!(launch_args("com.example.app", false, &["NOEQUALS".into()], &[]).is_err());
        assert!(launch_args("com.example.app", false, &["=val".into()], &[]).is_err());
    }

    #[test]
    fn type_steps_separates_return_keys() {
        assert_eq!(type_steps("hello"), vec![TypeStep::Text("hello")]);
        assert_eq!(
            type_steps("hello\n"),
            vec![TypeStep::Text("hello"), TypeStep::Return]
        );
        assert_eq!(type_steps("\n"), vec![TypeStep::Return]);
        assert_eq!(
            type_steps("a\nb\n"),
            vec![
                TypeStep::Text("a"),
                TypeStep::Return,
                TypeStep::Text("b"),
                TypeStep::Return,
            ]
        );
        assert_eq!(type_steps(""), Vec::<TypeStep>::new());
    }

    #[test]
    fn drag_progress_eases_out_at_endpoint() {
        assert_eq!(drag_progress(0.0, false), 0.0);
        assert_eq!(drag_progress(0.5, false), 0.5);
        assert_eq!(drag_progress(1.0, false), 1.0);
        assert_eq!(drag_progress(0.0, true), 0.0);
        assert_eq!(drag_progress(1.0, true), 1.0);
        // Final step delta is much smaller than initial step delta when easing out
        let first_step = drag_progress(0.1, true) - drag_progress(0.0, true);
        let last_step = drag_progress(1.0, true) - drag_progress(0.9, true);
        assert!(last_step < first_step * 0.05);
    }

    #[test]
    fn lat_lon_parsing_and_bounds() {
        assert_eq!(
            parse_lat_lon("51.5074,-0.1278").unwrap(),
            (51.5074, -0.1278)
        );
        assert_eq!(
            parse_lat_lon(" -33.8688 , 151.2093 ").unwrap(),
            (-33.8688, 151.2093)
        );
        assert!(parse_lat_lon("91,0").is_err());
        assert!(parse_lat_lon("0,181").is_err());
        assert!(parse_lat_lon("51.5").is_err());
        assert!(parse_lat_lon("1,2,3").is_err());
        assert!(parse_lat_lon("NaN,0").is_err());
    }

    #[test]
    fn privacy_validation_matches_simctl_rules() {
        assert_eq!(
            validate_privacy("grant", "location", Some("com.example.app")).unwrap(),
            Some("com.example.app")
        );
        assert_eq!(validate_privacy("reset", "photos", None).unwrap(), None);
        assert!(validate_privacy("grant", "location", None).is_err());
        assert!(validate_privacy("revoke", "camera", None).is_err());
        assert!(validate_privacy("reset", "location", None).is_err());
        assert!(validate_privacy("reset", "location-always", None).is_err());
        assert!(validate_privacy("grant", "bogus", Some("com.example.app")).is_err());
    }

    #[test]
    fn push_payload_validation_requires_aps_and_bundle() {
        let with_target = br#"{"Simulator Target Bundle":"com.example.app","aps":{"alert":"hi"}}"#;
        assert_eq!(
            validate_push_payload(None, with_target).unwrap(),
            "com.example.app"
        );
        let no_target = br#"{"aps":{"alert":"hi"}}"#;
        assert_eq!(
            validate_push_payload(Some("com.other.app"), no_target).unwrap(),
            "com.other.app"
        );
        assert!(validate_push_payload(None, no_target).is_err());
        assert!(validate_push_payload(Some("com.other.app"), br#"{"alert":"hi"}"#).is_err());
        assert!(validate_push_payload(Some("com.other.app"), b"not json").is_err());
    }

    #[test]
    fn clipboard_read_returns_error_without_signalling_springboard() {
        let dev = Device {
            dir: PathBuf::from("/nonexistent"),
        };
        let err = clipboard(&dev, None).unwrap_err().to_string();
        assert!(err.contains("not supported"), "unexpected error: {err}");
    }

    use crate::phone::Screen;

    #[test]
    fn upright_to_panel_matches_orientation_turns() {
        let scr = Screen {
            width: 400.0,
            height: 800.0,
        };
        let approx = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| {
            assert!(
                (ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6,
                "({ax},{ay}) != ({bx},{by})"
            );
        };
        approx(
            upright_to_panel(scr, 100.0, 200.0, "portrait"),
            (0.25, 0.25),
        );
        // landscapeLeft: upright (800x400), top-left (0,0) maps to panel (1,0)
        assert_eq!(upright_bounds(scr, "landscapeLeft"), (800.0, 400.0));
        approx(
            upright_to_panel(scr, 200.0, 100.0, "landscapeLeft"),
            (0.75, 0.25),
        );
        // landscapeRight: upright (800x400), top-left (0,0) maps to panel (0,1)
        approx(
            upright_to_panel(scr, 200.0, 100.0, "landscapeRight"),
            (0.25, 0.75),
        );
        approx(
            upright_to_panel(scr, 100.0, 200.0, "portraitUpsideDown"),
            (0.75, 0.75),
        );
    }

    #[test]
    fn pinch_validation_checks_bounds_and_radii() {
        assert!(validate_pinch(200.0, 400.0, 40.0, 140.0, (400.0, 800.0)).is_ok());
        assert!(validate_pinch(200.0, 400.0, 140.0, 40.0, (400.0, 800.0)).is_ok());
        assert!(validate_pinch(200.0, 400.0, -10.0, 40.0, (400.0, 800.0)).is_err());
        assert!(validate_pinch(50.0, 400.0, 20.0, 80.0, (400.0, 800.0)).is_err());
        assert!(validate_pinch(200.0, 900.0, 20.0, 80.0, (400.0, 800.0)).is_err());
    }

    // a 2x1 image (red, green) turned each way
    #[test]
    fn rotation_keeps_the_content_upright() {
        let img = vec![255, 0, 0, 0, 255, 0];
        assert_eq!(rotate(img.clone(), 2, 1, "portrait"), (img.clone(), 2, 1));
        assert_eq!(
            rotate(img.clone(), 2, 1, "landscapeLeft"),
            (vec![0, 255, 0, 255, 0, 0], 1, 2)
        );
        assert_eq!(
            rotate(img.clone(), 2, 1, "landscapeRight"),
            (vec![255, 0, 0, 0, 255, 0], 1, 2)
        );
        assert_eq!(
            rotate(img, 2, 1, "portraitUpsideDown"),
            (vec![0, 255, 0, 255, 0, 0], 2, 1)
        );
    }
}

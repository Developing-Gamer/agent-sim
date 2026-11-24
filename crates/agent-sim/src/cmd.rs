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
            phone::Health::Running => "running",
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
    Ok(out.trim().to_string())
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

pub fn swipe(dev: &Device, from: (f64, f64), to: (f64, f64), ms: u64) -> Result<String> {
    let scr = Screen::of(dev)?;
    let mut h = hid(dev);
    let steps = (ms / 16).clamp(4, 120);
    let (a, b) = scr.unit(from.0, from.1);
    h.touch("down", a, b)?;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let (u, v) = scr.unit(x, y);
        std::thread::sleep(Duration::from_millis(ms / steps));
        h.touch("move", u, v)?;
    }
    let (u, v) = scr.unit(to.0, to.1);
    h.touch("up", u, v)?;
    Ok(format!(
        "swiped ({:.0},{:.0}) to ({:.0},{:.0})",
        from.0, from.1, to.0, to.1
    ))
}

fn validate_pinch(
    cx: f64,
    cy: f64,
    r1: f64,
    r2: f64,
    bounds: (f64, f64),
) -> Result<()> {
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
    swipe(dev, from, to, 300)?;
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
    let Some(pid) = front.get("pid").and_then(|p| p.as_u64()).filter(|p| *p > 0) else {
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
        out.push("--terminate-running-process".into());
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
    // an app started right after it was stopped sometimes dies during startup: start it again once
    for attempt in 0..2 {
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
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let run = home.join(format!(
        ".cache/x86sim/a64prefix-{short}/private/var/tmp/x86sim-a64"
    ));
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

/// The iOS runtime's root (its /usr/bin tools run inside the phone through `x86sim spawn`).
fn runtime_root() -> Result<PathBuf> {
    let base = crate::device::data_dir()?.join("runtimes");
    for rt in std::fs::read_dir(&base)?.flatten() {
        let profiles = rt
            .path()
            .join("extracted/Library/Developer/CoreSimulator/Profiles/Runtimes");
        for sim in std::fs::read_dir(&profiles).into_iter().flatten().flatten() {
            let root = sim.path().join("Contents/Resources/RuntimeRoot");
            if root.join("usr/bin/defaults").is_file() {
                return Ok(root);
            }
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
    let rt = runtime_root()?;
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
    let defaults = runtime_root()?.join("usr/bin/defaults");
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

/// The general pasteboard, through SpringBoard (a background process may not read it): `None` reads it.
pub fn clipboard(dev: &Device, set: Option<&str>) -> Result<String> {
    match set {
        Some(t) => {
            springboard(dev, &format!("pasteboard-set\n{t}"))?;
            Ok("clipboard set".into())
        }
        None => springboard(dev, "pasteboard-get"),
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
    use super::{TypeStep, launch_args, rotate, type_steps, upright_bounds, upright_to_panel, validate_pinch};

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
                "--terminate-running-process",
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

    use crate::phone::Screen;

    #[test]
    fn upright_to_panel_matches_orientation_turns() {
        let scr = Screen {
            width: 400.0,
            height: 800.0,
        };
        let approx = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| {
            assert!((ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6, "({ax},{ay}) != ({bx},{by})");
        };
        approx(upright_to_panel(scr, 100.0, 200.0, "portrait"), (0.25, 0.25));
        // landscapeLeft: upright (800x400), top-left (0,0) maps to panel (1,0)
        assert_eq!(upright_bounds(scr, "landscapeLeft"), (800.0, 400.0));
        approx(upright_to_panel(scr, 200.0, 100.0, "landscapeLeft"), (0.75, 0.25));
        // landscapeRight: upright (800x400), top-left (0,0) maps to panel (0,1)
        approx(upright_to_panel(scr, 200.0, 100.0, "landscapeRight"), (0.25, 0.75));
        approx(upright_to_panel(scr, 100.0, 200.0, "portraitUpsideDown"), (0.75, 0.75));
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

//! `agent-sim snapshot`: the screen as accessibility elements, with references (`@e3`) that `tap` accepts.
//!
//! hooks/ax.c in each phone process writes its visible elements as JSON when it gets SIGUSR2: SpringBoard (the home
//! screen, system alerts, the status bar, and front.json: the app in front) and that app.

use crate::device::Device;
use crate::phone;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    #[serde(rename = "ref", default)]
    pub reference: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    /// x, y, width, height in screen points
    pub frame: [f64; 4],
    /// the process the element belongs to (the app's executable, or SpringBoard)
    pub owner: String,
}

impl Element {
    pub fn center(&self) -> (f64, f64) {
        (
            self.frame[0] + self.frame[2] / 2.0,
            self.frame[1] + self.frame[3] / 2.0,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// the app in front (bundle id), or None on the home screen / lock screen / notification center / app switcher
    pub app: Option<String>,
    /// the device is on the Lock Screen (Cover Sheet UI locked): no app is in front
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Notification Center (Cover Sheet pulled down while unlocked) covers the screen
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notification_center: bool,
    /// the iOS App Switcher is open over the home screen or foreground app
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub switcher: bool,
    /// hosted modal UI presented over the app (for example ExtensionKit PhotosPicker or Share Sheet), when active
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modal: Option<String>,
    /// the hosted modal did not answer (still starting, or busy)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modal_not_answering: bool,
    /// the app in front did not answer (still starting, or busy): only SpringBoard's elements are listed
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub app_not_answering: bool,
    /// effective UI orientation of the elements in this snapshot (portrait, landscapeLeft, landscapeRight, portraitUpsideDown)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    pub elements: Vec<Element>,
}

#[derive(Deserialize, Clone)]
struct HostedScene {
    pid: u32,
    #[serde(default)]
    #[allow(dead_code)]
    bundle: String,
    frame: [f64; 4],
}

#[derive(Deserialize)]
struct RawDump {
    process: String,
    windows: Vec<RawWindow>,
    /// the app's web views on screen: x, y, width, height (screen points)
    #[serde(default)]
    webviews: Vec<[f64; 4]>,
    /// guest PIDs of the WebContent processes backing the app's visible WKWebViews
    #[serde(default)]
    webview_pids: Vec<u32>,
    /// remote scenes hosted inside the app's visible UI (for example UIActivityViewController / SharingUIService)
    #[serde(default)]
    hosted_scenes: Vec<HostedScene>,
    /// interface orientation reported by the process's UIWindowScene
    #[serde(default)]
    orientation: Option<String>,
}
#[derive(Deserialize, Clone)]
struct RawWindow {
    class: String,
    level: f64,
    elements: Vec<RawElement>,
}
#[derive(Deserialize, Clone)]
struct RawElement {
    label: Option<String>,
    value: Option<String>,
    hint: Option<String>,
    id: Option<String>,
    traits: Vec<String>,
    frame: [f64; 4],
}
#[derive(Deserialize)]
struct Front {
    bundle: Option<String>,
    pid: u32,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    hosted_pid: u32,
    #[serde(default)]
    hosted_scene: Option<String>,
}

fn role(traits: &[String]) -> &'static str {
    let has = |t: &str| traits.iter().any(|x| x == t);
    if has("text-field") || has("search-field") {
        "textfield"
    } else if has("keyboard-key") {
        "key"
    } else if has("link") {
        "link"
    } else if has("button") {
        "button"
    } else if has("adjustable") {
        "adjustable"
    } else if has("header") {
        "heading"
    } else if has("image") {
        "image"
    } else if has("static-text") {
        "text"
    } else {
        "element"
    }
}

fn wait_for(path: &Path, timeout: Duration) -> Result<String> {
    let start = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            return Ok(s);
        }
        if start.elapsed() > timeout {
            bail!(
                "no answer from the phone ({} did not appear); are agent-sim's hooks loaded (scripts/phone.sh)?",
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Asks one process for its elements: `name` is the file it writes (ax-<pid>.json, SpringBoard ax-springboard.json).
/// Unix time in ms, written into a request so the hook can report how long the request waited.
fn sent_stamp() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:.0}", t.as_secs_f64() * 1e3)
}

fn dump(
    dir: &Path,
    (linux_pid, inner): (u32, u32),
    name: &str,
    timeout: Duration,
) -> Result<RawDump> {
    let file = dir.join(name);
    let _ = std::fs::remove_file(&file);
    if let Some(p) = name
        .strip_prefix("ax-")
        .and_then(|n| n.strip_suffix(".json"))
    {
        if p.parse::<u32>().is_ok() {
            let _ = std::fs::write(dir.join(format!("req-{p}")), sent_stamp());
        }
    }
    phone::ask_hook(dir, linux_pid, inner)?;
    let text = wait_for(&file, timeout)?;
    serde_json::from_str(&text).with_context(|| format!("reading {}", file.display()))
}

/// Narrows WebContent candidates to the guest PIDs backing the foreground app's visible WKWebViews.
/// Falls back to all WebContent processes when the hook did not report any PIDs (older hooks or cold web views).
fn filter_web_procs(procs: Vec<(u32, u32)>, webview_pids: &[u32]) -> Vec<(u32, u32)> {
    if webview_pids.is_empty() {
        return procs;
    }
    procs
        .into_iter()
        .filter(|&(_, inner)| webview_pids.contains(&inner))
        .collect()
}

/// The elements of the web pages shown in `webviews` (x, y, w, h), in screen points (WebKit reports them so).
fn web_windows(
    dev: &Device,
    dir: &Path,
    webviews: &[[f64; 4]],
    webview_pids: &[u32],
) -> Vec<RawWindow> {
    let Ok(procs) = phone::process_ids(dev, |_, exe| exe == "com.apple.WebKit.WebContent") else {
        return Vec::new();
    };
    let procs = filter_web_procs(procs, webview_pids);
    if procs.is_empty() {
        return Vec::new();
    }
    // a page still loading or zooming moves under the reading: read again until two readings agree
    let (mut last, mut live) = read_web(dir, &procs, webviews, Duration::from_secs(8));
    for _ in 0..3 {
        if live.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        let (next, still) = read_web(dir, &live, webviews, Duration::from_secs(3));
        let frames = |v: &[RawWindow]| -> Vec<[i64; 4]> {
            v.iter()
                .flat_map(|w| w.elements.iter())
                .map(|e| e.frame.map(|f| f.round() as i64))
                .collect()
        };
        let same = frames(&next) == frames(&last);
        if !next.is_empty() {
            last = next;
        }
        live = still;
        if same {
            break;
        }
    }
    last
}

/// One reading: signals each WebContent process and takes the answers that come within `budget`. Returns the
/// windows and the processes that have pages (worth asking again).
fn read_web(
    dir: &Path,
    procs: &[(u32, u32)],
    webviews: &[[f64; 4]],
    budget: Duration,
) -> (Vec<RawWindow>, Vec<(u32, u32)>) {
    let file = |inner: u32| dir.join(format!("ax-{inner}.json"));
    let stamp = |inner: u32| {
        std::fs::metadata(file(inner))
            .and_then(|m| m.modified())
            .ok()
    };
    // an answer is new when its file changed after the signal; an in-flight one is never deleted
    let before: Vec<_> = procs.iter().map(|&(_, i)| stamp(i)).collect();
    for &(linux, inner) in procs {
        // the hook's watcher thread picks this up within 150 ms; the signal is the fast path when it gets through
        let _ = std::fs::write(dir.join(format!("req-{inner}")), sent_stamp());
        let _ = phone::ask_hook(dir, linux, inner);
    }
    let mut pending: Vec<usize> = (0..procs.len()).collect();
    let mut out = Vec::new();
    let mut live = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let start = Instant::now();
    while !pending.is_empty() && start.elapsed() < budget {
        pending.retain(|&k| {
            let (_, inner) = procs[k];
            let now = stamp(inner);
            if now.is_none() || now == before[k] {
                return true;
            }
            let Ok(d) = std::fs::read_to_string(file(inner))
                .map_err(anyhow::Error::from)
                .and_then(|t| Ok(serde_json::from_str::<RawDump>(&t)?))
            else {
                return true; // written but not complete yet
            };
            if d.windows.iter().any(|w| !w.elements.is_empty()) {
                live.push(procs[k]);
            }
            for w in d.windows {
                let mut elements = Vec::new();
                for e in w.elements {
                    let (cx, cy) = (e.frame[0] + e.frame[2] / 2.0, e.frame[1] + e.frame[3] / 2.0);
                    let inside = webviews.iter().any(|v| {
                        cx >= v[0] && cx <= v[0] + v[2] && cy >= v[1] && cy <= v[1] + v[3]
                    });
                    let key = format!(
                        "{:?}{:?}{:.0}{:.0}",
                        e.label, e.value, e.frame[0], e.frame[1]
                    );
                    if inside && seen.insert(key) {
                        elements.push(e);
                    }
                }
                out.push(RawWindow {
                    class: "WebPage".into(),
                    level: 0.0,
                    elements,
                });
            }
            false
        });
        std::thread::sleep(Duration::from_millis(20));
    }
    (out, live)
}

fn on_screen(frame: [f64; 4], bounds: (f64, f64)) -> bool {
    let (cx, cy) = (frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0);
    cx >= 0.0 && cy >= 0.0 && cx <= bounds.0 && cy <= bounds.1
}

/// Translates scene-local element frames (for example SharingUIService inside a popover) into screen points
/// using the host `_UIScenePresentationView`'s frame. Elements that already lie outside the scene's local
/// bounds are left untouched.
fn translate_scene_windows(windows: &mut [RawWindow], scene_frame: [f64; 4]) {
    let [sx, sy, sw, sh] = scene_frame;
    if sx.abs() <= 1.0 && sy.abs() <= 1.0 {
        return;
    }
    let already_screen_coords = windows.iter().flat_map(|w| w.elements.iter()).any(|e| {
        let (cx, cy) = (e.frame[0] + e.frame[2] / 2.0, e.frame[1] + e.frame[3] / 2.0);
        cx > sw + 2.0 || cy > sh + 2.0
    });
    if already_screen_coords {
        return;
    }
    for w in windows {
        for e in &mut w.elements {
            e.frame[0] += sx;
            e.frame[1] += sy;
        }
    }
}

/// True when SpringBoard's App Switcher is showing app cards over the occluded Home Screen or foreground app.
fn is_app_switcher(springboard: &RawDump) -> bool {
    springboard.windows.iter().any(|w| {
        w.class == "SBMainSwitcherWindow"
            && w.elements
                .iter()
                .any(|e| e.id.as_deref().is_some_and(|i| i.starts_with("card:")))
    })
}

/// Effective interface orientation for bounds checks and input mapping.
/// Prefers the foreground window scene's reported interface orientation so portrait-locked apps
/// (and the iPhone Home Screen) keep portrait coordinates even when the physical device is turned
/// sideways. Falls back to element-extent detection when talking to an older prebuilt hook.
fn resolve_orientation(
    reported: Option<&str>,
    device_orient: &str,
    screen: phone::Screen,
    parts: &[(&str, &RawWindow)],
) -> String {
    if let Some(o) = reported
        && matches!(
            o,
            "portrait" | "portraitUpsideDown" | "landscapeLeft" | "landscapeRight"
        )
    {
        return o.to_string();
    }
    if matches!(device_orient, "landscapeLeft" | "landscapeRight") {
        let mut any_past_portrait_w = false;
        let mut any_past_landscape_h = false;
        for (_, w) in parts {
            if w.class.contains("StatusBar") {
                continue;
            }
            for e in &w.elements {
                let (cx, cy) = (e.frame[0] + e.frame[2] / 2.0, e.frame[1] + e.frame[3] / 2.0);
                if cx < 0.0 || cy < 0.0 {
                    continue;
                }
                if cx > screen.width + 1.0 && cx <= screen.height + 1.0 {
                    any_past_portrait_w = true;
                }
                if cy > screen.width + 1.0 && cy <= screen.height + 1.0 {
                    any_past_landscape_h = true;
                }
            }
        }
        if !any_past_portrait_w && any_past_landscape_h {
            return "portrait".to_string();
        }
    }
    match device_orient {
        "unknown" | "" => "portrait".to_string(),
        o => o.to_string(),
    }
}

pub fn snapshot(dev: &Device) -> Result<Snapshot> {
    let dir = phone::data_dir(dev).join("tmp/agent-sim");
    std::fs::create_dir_all(&dir)?;
    let sb = phone::process_ids(dev, |_, exe| exe == "SpringBoard")?;
    let Some(&sb) = sb.first() else {
        return Err(phone::not_running(dev));
    };
    let _ = std::fs::remove_file(dir.join("front.json"));
    let springboard =
        dump(&dir, sb, "ax-springboard.json", Duration::from_secs(5)).map_err(|e| {
            // its hook is loaded (it answered before) but it is silent: the phone is hung, not missing a hook
            if dir.join(format!("hooked-{}", sb.1)).exists() {
                anyhow::anyhow!(
                    "SpringBoard does not answer: the phone is hung (or still busy); {}",
                    phone::HUNG_FIX
                )
            } else {
                e
            }
        })?;
    wait_for(&dir.join("front.json"), Duration::from_secs(2))?;
    let front: Front = serde_json::from_str(&std::fs::read_to_string(dir.join("front.json"))?)?;
    let mut app_not_answering = false;
    let mut modal_not_answering = false;
    // System overlay precedence (highest to lowest):
    // 1. Lock Screen (`locked`)
    // 2. Notification Center (`notification_center`)
    // 3. App Switcher (`switcher`)
    // 4. Hosted modal (`modal` / `scene_modals`) over the foreground app
    // 5. Foreground app (`app`)
    // 6. Home Screen
    let locked = front.locked;
    let notification_center = !locked
        && springboard
            .windows
            .iter()
            .any(|w| w.class.contains("CoverSheet") && !w.elements.is_empty());
    let cover_sheet = locked || notification_center;
    let switcher = !cover_sheet && is_app_switcher(&springboard);
    let modal = if front.hosted_pid > 0 && !cover_sheet && !switcher {
        let p = front.hosted_pid;
        let pids = phone::processes(dev, |inner, _| inner == p)?;
        match pids.first() {
            Some(&linux) => match dump(
                &dir,
                (linux, p),
                &format!("ax-{p}.json"),
                Duration::from_secs(3),
            ) {
                Ok(d) => Some(d),
                Err(_) => {
                    modal_not_answering = true;
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    let app = if cover_sheet || switcher || modal.is_some() || modal_not_answering {
        None
    } else {
        match (front.pid, &front.bundle) {
            (p, Some(_)) if p > 0 => {
                let pids = phone::processes(dev, |inner, _| inner == p)?;
                match pids.first() {
                    Some(&linux) => {
                        match dump(
                            &dir,
                            (linux, p),
                            &format!("ax-{p}.json"),
                            Duration::from_secs(3),
                        ) {
                            Ok(d) => Some(d),
                            Err(_) => {
                                app_not_answering = true;
                                None
                            }
                        }
                    }
                    None => None,
                }
            }
            _ => None,
        }
    };

    // Remote FBScenes hosted inside the app's visible view hierarchy (for example third-party UIActivityViewController
    // popovers hosted by SharingUIService, which do not appear in RunningBoard's UISceneHosting endowments).
    let mut scene_modals: Vec<RawDump> = Vec::new();
    if let Some(a) = &app {
        for scene in a.hosted_scenes.iter().rev() {
            let Ok(pids) = phone::processes(dev, |inner, _| inner == scene.pid) else {
                continue;
            };
            let Some(&linux) = pids.first() else {
                continue;
            };
            if let Ok(mut d) = dump(
                &dir,
                (linux, scene.pid),
                &format!("ax-{}.json", scene.pid),
                Duration::from_secs(3),
            ) && d.windows.iter().any(|w| !w.elements.is_empty())
            {
                translate_scene_windows(&mut d.windows, scene.frame);
                scene_modals.push(d);
            }
        }
    }

    // web views: their content is in WebKit's WebContent processes, in points from the web view's content origin
    let web = match &app {
        Some(a) if !a.webviews.is_empty() => web_windows(dev, &dir, &a.webviews, &a.webview_pids),
        _ => Vec::new(),
    };

    // reading order: system alerts (and the Cover Sheet when locked / Notification Center) over everything,
    // hosted modal (when active), embedded remote scenes (such as Share Sheet), web content, the app,
    // the app switcher or home screen when unlocked with no app in front, the status bar
    let mut parts: Vec<(&str, &RawWindow)> = Vec::new();
    let sb_windows = |pred: &dyn Fn(&RawWindow) -> bool| -> Vec<(&str, &RawWindow)> {
        let mut v: Vec<_> = springboard
            .windows
            .iter()
            .filter(|w| pred(w))
            .map(|w| ("SpringBoard", w))
            .collect();
        v.sort_by(|a, b| b.1.level.total_cmp(&a.1.level));
        v
    };
    parts.extend(sb_windows(&|w| {
        w.level > 1000.0 && !w.class.contains("StatusBar")
    }));
    if cover_sheet {
        // the Cover Sheet (Lock Screen or Notification Center) and status bar only: what is behind it cannot be reached
    } else if let Some(m) = &modal {
        let mut v: Vec<_> = m.windows.iter().map(|w| (m.process.as_str(), w)).collect();
        v.sort_by(|x, y| y.1.level.total_cmp(&x.1.level));
        parts.extend(v);
    } else if !modal_not_answering {
        for m in &scene_modals {
            let mut v: Vec<_> = m.windows.iter().map(|w| (m.process.as_str(), w)).collect();
            v.sort_by(|x, y| y.1.level.total_cmp(&x.1.level));
            parts.extend(v);
        }
        parts.extend(web.iter().map(|w| ("WebContent", w)));
        if let Some(a) = &app {
            let mut v: Vec<_> = a.windows.iter().map(|w| (a.process.as_str(), w)).collect();
            v.sort_by(|x, y| y.1.level.total_cmp(&x.1.level));
            parts.extend(v);
        } else if !app_not_answering {
            parts.extend(sb_windows(&|w| {
                w.level <= 1000.0
                    && !w.class.contains("StatusBar")
                    && (!switcher || !(w.class.contains("HomeScreen") || w.class.contains("Dock")))
            }));
        }
    }
    parts.extend(sb_windows(&|w| w.class.contains("StatusBar")));

    let screen = phone::Screen::of(dev)?;
    let reported_orient = if cover_sheet || switcher {
        springboard.orientation.as_deref()
    } else {
        modal
            .as_ref()
            .and_then(|m| m.orientation.as_deref())
            .or_else(|| app.as_ref().and_then(|a| a.orientation.as_deref()))
            .or(springboard.orientation.as_deref())
    };
    let device_orient = crate::cmd::current_orientation(dev);
    let orient = resolve_orientation(reported_orient, &device_orient, screen, &parts);
    let bounds = crate::cmd::upright_bounds(screen, &orient);
    let mut elements = Vec::new();
    for (owner, w) in parts {
        let home =
            owner == "SpringBoard" && (w.class.contains("HomeScreen") || w.class.contains("Dock"));
        for e in &w.elements {
            // off screen (a page scrolled away, a view sliding out): not something to read or tap
            if !on_screen(e.frame, bounds) {
                continue;
            }
            let mut states = Vec::new();
            if e.traits.iter().any(|t| t == "selected") {
                states.push("selected".to_string());
            }
            if e.traits.iter().any(|t| t == "not-enabled") {
                states.push("disabled".to_string());
            }
            elements.push(Element {
                reference: format!("e{}", elements.len() + 1),
                role: match role(&e.traits) {
                    "element" if home => "app",
                    r => r,
                }
                .to_string(),
                label: e.label.clone().filter(|s| !s.is_empty()),
                value: e.value.clone().filter(|s| !s.is_empty()),
                hint: e.hint.clone().filter(|s| !s.is_empty()),
                id: e.id.clone().filter(|s| !s.is_empty()),
                states,
                frame: e.frame,
                owner: owner.to_string(),
            });
        }
    }
    let modal_name = modal
        .as_ref()
        .or_else(|| scene_modals.first())
        .map(|m| m.process.clone())
        .or_else(|| {
            modal_not_answering
                .then(|| front.hosted_scene.clone())
                .flatten()
        });
    let snap = Snapshot {
        app: front.bundle.filter(|_| {
            app.is_some() || app_not_answering || modal.is_some() || modal_not_answering
        }),
        locked,
        notification_center,
        switcher,
        modal: modal_name,
        modal_not_answering,
        app_not_answering,
        orientation: Some(orient),
        elements,
    };
    std::fs::write(
        phone::state_dir(dev)?.join("snapshot.json"),
        serde_json::to_vec(&snap)?,
    )?;
    Ok(snap)
}

/// The elements of the last snapshot (what `@eN` refers to).
pub fn last(dev: &Device) -> Result<Snapshot> {
    let p = phone::state_dir(dev)?.join("snapshot.json");
    let text =
        std::fs::read_to_string(&p).context("no snapshot yet: run `agent-sim snapshot` first")?;
    Ok(serde_json::from_str(&text)?)
}

/// Text in double quotes, with only quotes, backslashes and line breaks escaped.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line per element: `@e3 button "General"`, with its value, states and center point.
///
/// Header precedence (highest overlay first):
/// 1. `lock screen` (`locked`)
/// 2. `notification center` (`notification_center`)
/// 3. `app switcher` (`switcher`)
/// 4. `app <bundle> (modal ...)` / `app <bundle>`
/// 5. `home screen`
pub fn format(s: &Snapshot) -> String {
    let header = if s.locked {
        "lock screen".to_string()
    } else if s.notification_center {
        "notification center".to_string()
    } else if s.switcher {
        "app switcher".to_string()
    } else {
        match &s.app {
            Some(a) if s.modal_not_answering => format!(
                "app {a} (modal {}not answering yet: starting or busy; snapshot again in a moment)",
                s.modal
                    .as_deref()
                    .map(|m| format!("{m} "))
                    .unwrap_or_default()
            ),
            Some(a) => match &s.modal {
                Some(m) => format!("app {a} (modal {m})"),
                None if s.app_not_answering => format!(
                    "app {a} (not answering yet: starting or busy; snapshot again in a moment)"
                ),
                None => format!("app {a}"),
            },
            None => "home screen".to_string(),
        }
    };
    let mut out = format!("{header}\n");
    for e in &s.elements {
        let (x, y) = e.center();
        out.push_str(&format!("@{} {}", e.reference, e.role));
        if let Some(l) = &e.label {
            out.push_str(&format!(" {}", quote(l)));
        }
        if let Some(v) = &e.value {
            out.push_str(&format!(" value={}", quote(v)));
        }
        if let Some(i) = &e.id {
            out.push_str(&format!(" id={}", quote(i)));
        }
        for st in &e.states {
            out.push_str(&format!(" {st}"));
        }
        out.push_str(&format!(" ({x:.0},{y:.0})\n"));
    }
    out
}

/// Finds an element by reference (`@e3` / `e3`) in the last snapshot, along with that snapshot's UI orientation.
pub fn resolve_with_orientation(dev: &Device, r: &str) -> Result<(Element, Option<String>)> {
    let want = r.trim_start_matches('@');
    let snap = last(dev)?;
    let orient = snap.orientation;
    let el = snap
        .elements
        .into_iter()
        .find(|e| e.reference == want)
        .with_context(|| format!("no element {r} in the last snapshot"))?;
    Ok((el, orient))
}

/// The element `text` names: one whose label, value or id equals it (ignoring case), else one that contains it.
/// Among equally good matches, something to act on (a button, link or field) wins over text and headings.
pub fn find<'a>(s: &'a Snapshot, text: &str) -> Option<&'a Element> {
    // Files and other Foundation-backed labels come decomposed (NFD); text typed on Linux is composed (NFC).
    let fold = |v: &str| v.nfc().collect::<String>().to_lowercase();
    let t = fold(text);
    let fields = |e: &'a Element| {
        [&e.label, &e.value, &e.id]
            .into_iter()
            .flatten()
            .map(|v| fold(v))
    };
    let actionable = |e: &&&'a Element| {
        matches!(
            e.role.as_str(),
            "button" | "link" | "textfield" | "key" | "adjustable" | "app"
        )
    };
    let exact: Vec<&Element> = s
        .elements
        .iter()
        .filter(|e| fields(e).any(|v| v == t))
        .collect();
    let partial: Vec<&Element> = s
        .elements
        .iter()
        .filter(|e| fields(e).any(|v| v.contains(&t)))
        .collect();
    exact
        .iter()
        .find(actionable)
        .or(exact.first())
        .or_else(|| partial.iter().find(actionable))
        .or(partial.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_traits() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(role(&t(&["button", "static-text"])), "button");
        assert_eq!(role(&t(&["search-field", "text-field"])), "textfield");
        assert_eq!(role(&t(&["static-text"])), "text");
        assert_eq!(role(&t(&[])), "element");
    }

    #[test]
    fn text_taps_prefer_controls() {
        let el = |r: &str, role: &str, label: &str| Element {
            reference: r.into(),
            role: role.into(),
            label: Some(label.into()),
            value: None,
            hint: None,
            id: None,
            states: vec![],
            frame: [0.0; 4],
            owner: "x".into(),
        };
        let s = Snapshot {
            app: None,
            locked: false,
            notification_center: false,
            switcher: false,
            modal: None,
            modal_not_answering: false,
            app_not_answering: false,
            orientation: None,
            elements: vec![
                el("e1", "heading", "Sign in"),
                el("e2", "button", "Sign in"),
                el("e3", "text", "Sign in to continue"),
            ],
        };
        assert_eq!(find(&s, "sign in").unwrap().reference, "e2");
        assert_eq!(find(&s, "continue").unwrap().reference, "e3");
    }

    #[test]
    fn text_matches_whatever_the_unicode_normalization() {
        let decomposed: String = "Απολυτίκιο Αγίου Γεωργίου, PDF document".nfd().collect();
        let s = Snapshot {
            app: None,
            locked: false,
            notification_center: false,
            switcher: false,
            modal: None,
            modal_not_answering: false,
            app_not_answering: false,
            orientation: None,
            elements: vec![Element {
                reference: "e1".into(),
                role: "element".into(),
                label: Some(decomposed.clone()),
                value: None,
                hint: None,
                id: None,
                states: vec![],
                frame: [0.0; 4],
                owner: "x".into(),
            }],
        };
        let composed: String = "απολυτίκιο".nfc().collect();
        assert!(!decomposed.to_lowercase().contains(&composed));
        assert_eq!(find(&s, &composed).unwrap().reference, "e1");
        assert_eq!(find(&s, &decomposed).unwrap().reference, "e1");
    }

    #[test]
    fn format_includes_hosted_modal_state() {
        let active = Snapshot {
            app: Some("com.example.app".into()),
            locked: false,
            notification_center: false,
            switcher: false,
            modal: Some("PhotosPicker".into()),
            modal_not_answering: false,
            app_not_answering: false,
            orientation: None,
            elements: vec![],
        };
        assert_eq!(
            format(&active),
            "app com.example.app (modal PhotosPicker)\n"
        );
        let busy = Snapshot {
            app: Some("com.example.app".into()),
            locked: false,
            notification_center: false,
            switcher: false,
            modal: Some("PhotosPicker".into()),
            modal_not_answering: true,
            app_not_answering: false,
            orientation: None,
            elements: vec![],
        };
        assert!(format(&busy).contains("modal PhotosPicker not answering yet"));
    }

    #[test]
    fn format_header_precedence() {
        // Explicit precedence: lock screen > notification center > app switcher > app (modal) > app > home screen
        let mut s = Snapshot {
            app: Some("com.example.app".into()),
            locked: true,
            notification_center: true,
            switcher: true,
            modal: Some("SharingUIService".into()),
            modal_not_answering: false,
            app_not_answering: false,
            orientation: None,
            elements: vec![],
        };
        assert_eq!(format(&s), "lock screen\n");
        s.locked = false;
        assert_eq!(format(&s), "notification center\n");
        s.notification_center = false;
        assert_eq!(format(&s), "app switcher\n");
        s.switcher = false;
        assert_eq!(format(&s), "app com.example.app (modal SharingUIService)\n");
        s.modal = None;
        assert_eq!(format(&s), "app com.example.app\n");
        s.app = None;
        assert_eq!(format(&s), "home screen\n");
    }

    #[test]
    fn filter_web_procs_scopes_to_visible_webviews() {
        let procs = vec![(1001, 24445), (1002, 24999)];
        assert_eq!(filter_web_procs(procs.clone(), &[]), procs);
        assert_eq!(
            filter_web_procs(procs.clone(), &[24999]),
            vec![(1002, 24999)]
        );
        assert!(filter_web_procs(procs, &[99999]).is_empty());
    }

    #[test]
    fn raw_dump_deserializes_optional_webview_pids() {
        let legacy: RawDump =
            serde_json::from_str(r#"{"process":"App","windows":[],"webviews":[]}"#).unwrap();
        assert!(legacy.webview_pids.is_empty());
        let scoped: RawDump = serde_json::from_str(
            r#"{"process":"SafariViewService","windows":[],"webviews":[[0,0,402,874]],"webview_pids":[24999]}"#,
        )
        .unwrap();
        assert_eq!(scoped.webview_pids, vec![24999]);
    }

    #[test]
    fn on_screen_uses_upright_bounds() {
        let scr = phone::Screen {
            width: 402.0,
            height: 874.0,
        };
        let landscape = crate::cmd::upright_bounds(scr, "landscapeLeft");
        // Right-half toolbar button in landscape (x=700, y=30) must not be dropped by portrait width 402
        assert!(on_screen([680.0, 10.0, 40.0, 40.0], landscape));
        assert!(!on_screen(
            [680.0, 10.0, 40.0, 40.0],
            (scr.width, scr.height)
        ));
        assert!(!on_screen([100.0, 450.0, 40.0, 40.0], landscape));
    }

    #[test]
    fn resolve_orientation_handles_portrait_locked_apps_in_landscape() {
        let scr = phone::Screen {
            width: 402.0,
            height: 874.0,
        };
        let portrait_win = RawWindow {
            class: "UIWindow".into(),
            level: 0.0,
            elements: vec![
                RawElement {
                    label: Some("Top".into()),
                    value: None,
                    hint: None,
                    id: None,
                    traits: vec!["button".into()],
                    frame: [150.0, 250.0, 100.0, 44.0],
                },
                RawElement {
                    label: Some("Bottom".into()),
                    value: None,
                    hint: None,
                    id: None,
                    traits: vec!["button".into()],
                    frame: [150.0, 600.0, 100.0, 44.0],
                },
            ],
        };
        let landscape_win = RawWindow {
            class: "UIWindow".into(),
            level: 0.0,
            elements: vec![RawElement {
                label: Some("Tabs".into()),
                value: None,
                hint: None,
                id: None,
                traits: vec!["button".into()],
                frame: [760.0, 10.0, 40.0, 44.0],
            }],
        };
        // Explicit interface orientation from hook wins over physical device orientation
        assert_eq!(
            resolve_orientation(
                Some("portrait"),
                "landscapeLeft",
                scr,
                &[("App", &portrait_win)]
            ),
            "portrait"
        );
        assert_eq!(
            resolve_orientation(
                Some("landscapeLeft"),
                "landscapeLeft",
                scr,
                &[("Safari", &landscape_win)]
            ),
            "landscapeLeft"
        );
        // Legacy hook fallback (reported=None): portrait-locked app keeps portrait bounds
        assert_eq!(
            resolve_orientation(None, "landscapeLeft", scr, &[("App", &portrait_win)]),
            "portrait"
        );
        // Legacy hook fallback: landscape app keeps landscapeLeft
        assert_eq!(
            resolve_orientation(None, "landscapeLeft", scr, &[("Safari", &landscape_win)]),
            "landscapeLeft"
        );
    }

    #[test]
    fn translate_scene_windows_offsets_popover_local_frames() {
        let mut wins = vec![RawWindow {
            class: "_UIHostedWindow".into(),
            level: 0.0,
            elements: vec![RawElement {
                label: Some("Copy".into()),
                value: None,
                hint: None,
                id: None,
                traits: vec!["button".into()],
                frame: [19.5, 229.5, 82.0, 134.5],
            }],
        }];
        translate_scene_windows(&mut wins, [9.0, 501.0, 384.0, 364.0]);
        assert_eq!(wins[0].elements[0].frame, [28.5, 730.5, 82.0, 134.5]);
        // Running again on already screen-space coordinates must not double-offset
        translate_scene_windows(&mut wins, [9.0, 501.0, 384.0, 364.0]);
        assert_eq!(wins[0].elements[0].frame, [28.5, 730.5, 82.0, 134.5]);
    }

    #[test]
    fn detects_app_switcher_from_main_switcher_cards() {
        let dump: RawDump = serde_json::from_str(
            r#"{
                "process": "SpringBoard",
                "windows": [
                    {
                        "class": "SBMainSwitcherWindow",
                        "level": 5,
                        "elements": [{"label": "Settings", "id": "card:com.apple.Preferences:scene", "traits": [], "frame": [60, 120, 280, 600]}]
                    },
                    {
                        "class": "SBHomeScreenWindow",
                        "level": -2,
                        "elements": [{"label": "Safari", "traits": [], "frame": [20, 700, 60, 60]}]
                    }
                ]
            }"#,
        )
        .unwrap();
        assert!(is_app_switcher(&dump));
        let home: RawDump = serde_json::from_str(
            r#"{
                "process": "SpringBoard",
                "windows": [
                    {"class": "SBMainSwitcherWindow", "level": 5, "elements": []},
                    {"class": "SBHomeScreenWindow", "level": -2, "elements": [{"label": "Safari", "traits": [], "frame": [20, 700, 60, 60]}]}
                ]
            }"#,
        )
        .unwrap();
        assert!(!is_app_switcher(&home));
    }

    #[test]
    fn raw_dump_deserializes_hosted_scenes() {
        let dump: RawDump = serde_json::from_str(
            r#"{
                "process": "DemoApp",
                "windows": [],
                "hosted_scenes": [{"pid": 22984, "bundle": "com.apple.SharingUIService", "frame": [9.0, 501.0, 384.0, 364.0]}]
            }"#,
        )
        .unwrap();
        assert_eq!(dump.hosted_scenes.len(), 1);
        assert_eq!(dump.hosted_scenes[0].pid, 22984);
        assert_eq!(dump.hosted_scenes[0].bundle, "com.apple.SharingUIService");
        assert_eq!(dump.hosted_scenes[0].frame, [9.0, 501.0, 384.0, 364.0]);
    }
}

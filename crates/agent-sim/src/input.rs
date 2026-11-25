//! Touches, buttons and keys to x86Sim's simhid socket (one text command per line).

use anyhow::{Context, Result};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A touch shorter than this does not register as a tap in iOS; a click's press and release arrive together.
const MIN_TAP: Duration = Duration::from_millis(80);
/// Touches that begin this close to an edge are system gestures (home indicator, Control Center), as in
/// Apple's Simulator.
const EDGE: f64 = 0.04;

pub struct Hid {
    path: PathBuf,
    sock: Option<UnixStream>,
    last_down: Instant,
    kb_warmed: bool,
}

/// The HID usage (page 7) for a character, and whether it needs shift (US QWERTY layout).
pub fn key_usage(c: char) -> Option<(u8, bool)> {
    Some(match c {
        'a'..='z' => (4 + (c as u8 - b'a'), false),
        'A'..='Z' => (4 + (c as u8 - b'A'), true),
        '1'..='9' => (0x1e + (c as u8 - b'1'), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\u{8}' => (0x2a, false),
        '\t' => (0x2b, false),
        ' ' => (0x2c, false),
        '-' => (0x2d, false),
        '_' => (0x2d, true),
        '=' => (0x2e, false),
        '+' => (0x2e, true),
        '[' => (0x2f, false),
        '{' => (0x2f, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        '!' => (0x1e, true),
        '@' => (0x1f, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        _ => return None,
    })
}

fn edge(x: f64, y: f64) -> &'static str {
    if y >= 1.0 - EDGE {
        "bottom"
    } else if y <= EDGE {
        "top"
    } else if x <= EDGE {
        "left"
    } else if x >= 1.0 - EDGE {
        "right"
    } else {
        "none"
    }
}

impl Hid {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            sock: None,
            last_down: Instant::now(),
            kb_warmed: false,
        }
    }

    fn send(&mut self, line: &str) -> Result<()> {
        for attempt in 0..2 {
            if self.sock.is_none() {
                self.sock = Some(
                    UnixStream::connect(&self.path)
                        .with_context(|| format!("connecting to {}", self.path.display()))?,
                );
            }
            let sock = self.sock.as_mut().unwrap();
            if sock.write_all(format!("{line}\n").as_bytes()).is_ok() {
                return Ok(());
            }
            self.sock = None;
            self.kb_warmed = false;
            if attempt == 1 {
                anyhow::bail!("simhid closed the connection");
            }
        }
        Ok(())
    }

    /// `phase` is down, move or up; x and y are 0..1 from the top left.
    pub fn touch(&mut self, phase: &str, x: f64, y: f64) -> Result<()> {
        let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        match phase {
            "down" => {
                self.last_down = Instant::now();
                self.send(&format!("touch down {x:.5} {y:.5} {}", edge(x, y)))
            }
            "move" => self.send(&format!("touch move {x:.5} {y:.5}")),
            "up" => {
                let held = self.last_down.elapsed();
                if held < MIN_TAP {
                    std::thread::sleep(MIN_TAP - held);
                }
                self.kb_warmed = false;
                self.send(&format!("touch up {x:.5} {y:.5}"))
            }
            _ => Ok(()),
        }
    }

    /// Two-finger touch (`phase` is down, move or up); coordinates are 0..1 on the physical panel.
    pub fn touch2(&mut self, phase: &str, p1: (f64, f64), p2: (f64, f64)) -> Result<()> {
        let (x1, y1) = (p1.0.clamp(0.0, 1.0), p1.1.clamp(0.0, 1.0));
        let (x2, y2) = (p2.0.clamp(0.0, 1.0), p2.1.clamp(0.0, 1.0));
        match phase {
            "down" => {
                self.last_down = Instant::now();
                self.send(&format!("touch2 down {x1:.5} {y1:.5} {x2:.5} {y2:.5}"))
            }
            "move" => self.send(&format!("touch2 move {x1:.5} {y1:.5} {x2:.5} {y2:.5}")),
            "up" => {
                let held = self.last_down.elapsed();
                if held < MIN_TAP {
                    std::thread::sleep(MIN_TAP - held);
                }
                self.kb_warmed = false;
                self.send(&format!("touch2 up {x1:.5} {y1:.5} {x2:.5} {y2:.5}"))
            }
            _ => Ok(()),
        }
    }

    /// `name` is home, lock, volup or voldown.
    pub fn button(&mut self, name: &str, down: bool) -> Result<()> {
        if !matches!(name, "home" | "lock" | "volup" | "voldown") {
            return Ok(());
        }
        self.send(&format!(
            "button {name} {}",
            if down { "down" } else { "up" }
        ))
    }

    /// Types `s` key by key. Keys pressed back to back were dropped (a web field kept one character of
    /// seventeen, a search field five of nine): each key is held and followed by a pause, as a fast typist's are.
    pub fn text(&mut self, s: &str) -> Result<()> {
        const HOLD: Duration = Duration::from_millis(25);
        const GAP: Duration = Duration::from_millis(40);
        // simhid sleeps on the first key command of a session for up to SIMHID_KB_TOUCH_GUARD_MS (1000 ms)
        // after a touch lift or SIMHID_KB_FIRST_SETTLE_MS (800 ms) on first attach. Lines sent while it
        // sleeps queue in the socket and then drain back-to-back in <1 ms, scrambling Shift and dropping
        // trailing keys. Warm up with a harmless Shift release and wait for simhid's settle to finish first.
        const WARMUP_SETTLE: Duration = Duration::from_millis(1050);
        let keys: Vec<(u8, bool)> = s.chars().filter_map(key_usage).collect();
        if keys.is_empty() {
            return Ok(());
        }
        if !self.kb_warmed && (keys.len() > 1 || keys[0].1) {
            self.send("key up 225")?;
            std::thread::sleep(WARMUP_SETTLE);
        }
        self.kb_warmed = true;
        for (usage, shift) in keys {
            if shift {
                self.send("key down 225")?;
                std::thread::sleep(HOLD);
            }
            self.send(&format!("key down {usage}"))?;
            std::thread::sleep(HOLD);
            self.send(&format!("key up {usage}"))?;
            if shift {
                std::thread::sleep(HOLD);
                self.send("key up 225")?;
                std::thread::sleep(HOLD);
            }
            std::thread::sleep(GAP);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_hid_usages() {
        assert_eq!(key_usage('a'), Some((4, false)));
        assert_eq!(key_usage('Z'), Some((29, true)));
        assert_eq!(key_usage('0'), Some((0x27, false)));
        assert_eq!(key_usage('\n'), Some((0x28, false)));
        assert_eq!(key_usage('é'), None);
    }

    #[test]
    fn key_usage_covers_all_printable_ascii() {
        for b in b' '..=b'~' {
            let c = b as char;
            assert!(key_usage(c).is_some(), "missing HID usage for {c:?}");
        }
        assert_eq!(key_usage('^'), Some((0x23, true)));
        assert_eq!(key_usage('['), Some((0x2f, false)));
        assert_eq!(key_usage('{'), Some((0x2f, true)));
        assert_eq!(key_usage(']'), Some((0x30, false)));
        assert_eq!(key_usage('}'), Some((0x30, true)));
        assert_eq!(key_usage('\\'), Some((0x31, false)));
        assert_eq!(key_usage('|'), Some((0x31, true)));
        assert_eq!(key_usage('`'), Some((0x35, false)));
        assert_eq!(key_usage('~'), Some((0x35, true)));
        assert_eq!(key_usage('<'), Some((0x36, true)));
        assert_eq!(key_usage('>'), Some((0x37, true)));
    }

    #[test]
    fn edges_follow_apples_simulator() {
        assert_eq!(edge(0.5, 0.99), "bottom");
        assert_eq!(edge(0.5, 0.01), "top");
        assert_eq!(edge(0.5, 0.5), "none");
    }
}

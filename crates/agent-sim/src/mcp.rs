//! `agent-sim mcp`: agent-sim's commands as Model Context Protocol tools, over stdio (JSON-RPC, one message per
//! line), for Claude Code, Cursor, Codex and other MCP clients.
//!
//! Actions that change the screen (tap, type, scroll, swipe, press, launch, open) answer with a fresh snapshot after
//! the screen settles, so an agent sees the result without another call.

use crate::cmd;
use crate::device::Device;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    let device = json!({"type": "string", "description": "Phone name (default: the CLI's --device, AGENT_SIM_DEVICE or p)"});
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        let mut p = props.as_object().cloned().unwrap_or_default();
        p.insert("device".into(), device.clone());
        json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": p, "required": required},
        })
    };
    json!([
        tool(
            "snapshot",
            "Read the phone's screen: one line per accessibility element (role, label, value, id, center point) with a reference like @e3 that tap accepts. Covers the app in front, system alerts, the home screen and the status bar.",
            json!({}),
            &[]
        ),
        tool(
            "screenshot",
            "The phone's screen as a PNG image. Prefer snapshot; use this to check visual details.",
            json!({}),
            &[]
        ),
        tool(
            "tap",
            "Tap an element: a reference from the last snapshot (@e3), text an element's label/value/id contains, or a point as \"x y\" in screen points.",
            json!({"target": {"type": "string"}, "hold_ms": {"type": "integer", "description": "Hold time; 600+ is a long press"}}),
            &["target"]
        ),
        tool(
            "type",
            "Type text into the focused text field (tap the field first). \\n presses Return.",
            json!({"text": {"type": "string"}}),
            &["text"]
        ),
        tool(
            "scroll",
            "Scroll the content. down shows what is below.",
            json!({"direction": {"type": "string", "enum": ["up", "down", "left", "right"]}, "amount": {"type": "number", "description": "1 = about a third of the screen"}}),
            &["direction"]
        ),
        tool(
            "swipe",
            "Drag from one point to another, in screen points. Start at a screen edge for system gestures (from the bottom edge goes home).",
            json!({"x1": {"type": "number"}, "y1": {"type": "number"}, "x2": {"type": "number"}, "y2": {"type": "number"}, "ms": {"type": "integer"}}),
            &["x1", "y1", "x2", "y2"]
        ),
        tool(
            "pinch",
            "Two-finger pinch centered at (cx, cy) in screen points: finger half-spacing goes from r1 to r2 (r2 > r1 zooms in, r2 < r1 zooms out).",
            json!({
                "cx": {"type": "number"},
                "cy": {"type": "number"},
                "r1": {"type": "number", "description": "Starting half-distance between fingers in screen points"},
                "r2": {"type": "number", "description": "Ending half-distance between fingers in screen points"},
                "ms": {"type": "integer", "description": "Duration in ms, default 500"}
            }),
            &["cx", "cy", "r1", "r2"]
        ),
        tool(
            "orientation",
            "Get or set the phone's orientation.",
            json!({"name": {"type": "string", "enum": cmd::ORIENTATIONS, "description": "Omit to read current orientation"}}),
            &[]
        ),
        tool(
            "rotate",
            "Rotate the phone 90 degrees left (counterclockwise) or right (clockwise).",
            json!({"direction": {"type": "string", "enum": ["left", "right"]}}),
            &["direction"]
        ),
        tool(
            "press",
            "Press a hardware button: home, lock, volup, voldown, siri, action.",
            json!({"button": {"type": "string"}}),
            &["button"]
        ),
        tool(
            "launch",
            "Start an app by bundle id (see apps), with optional os_log capture, environment variables and launch arguments.",
            json!({
                "bundle_id": {"type": "string"},
                "logs": {"type": "boolean", "description": "Capture os_log messages (restarts the app if running)"},
                "env": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Environment variables for the app"},
                "args": {"type": "array", "items": {"type": "string"}, "description": "Launch arguments passed to the app"}
            }),
            &["bundle_id"]
        ),
        tool(
            "terminate",
            "Stop an app by bundle id.",
            json!({"bundle_id": {"type": "string"}}),
            &["bundle_id"]
        ),
        tool(
            "install",
            "Install an app from a path on this machine: an .ipa (device builds are converted for the simulator; App Store downloads are encrypted and refused) or a simulator .app.",
            json!({"path": {"type": "string"}}),
            &["path"]
        ),
        tool(
            "uninstall",
            "Remove an installed app.",
            json!({"bundle_id": {"type": "string"}}),
            &["bundle_id"]
        ),
        tool(
            "apps",
            "List installed apps (user apps; all=true for system apps too).",
            json!({"all": {"type": "boolean"}}),
            &[]
        ),
        tool(
            "open_url",
            "Open a URL: web pages open in Safari, other schemes in the app that handles them (deep links).",
            json!({"url": {"type": "string"}}),
            &["url"]
        ),
        tool(
            "wait_for",
            "Wait until text appears on screen (or disappears, with gone=true). Returns the matching element.",
            json!({"text": {"type": "string"}, "timeout": {"type": "number", "description": "Seconds, default 20"}, "gone": {"type": "boolean"}}),
            &["text"]
        ),
        tool(
            "boot",
            "Boot the phone and wait for its home screen (about 15-40 s). Phones usually run already.",
            json!({}),
            &[]
        ),
    ])
}

fn s<'a>(args: &'a Value, k: &str) -> Result<&'a str> {
    args.get(k)
        .and_then(Value::as_str)
        .with_context(|| format!("missing argument {k}"))
}
fn f(args: &Value, k: &str) -> Result<f64> {
    args.get(k)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing number {k}"))
}
fn parse_env(args: &Value) -> Result<Vec<String>> {
    let Some(v) = args.get("env") else {
        return Ok(Vec::new());
    };
    let map = v
        .as_object()
        .context("env must be an object of string values")?;
    let mut out = Vec::with_capacity(map.len());
    for (k, val) in map {
        let s = val
            .as_str()
            .with_context(|| format!("env.{k} must be a string"))?;
        out.push(format!("{k}={s}"));
    }
    Ok(out)
}
fn parse_string_list(args: &Value, k: &str) -> Result<Vec<String>> {
    let Some(v) = args.get(k) else {
        return Ok(Vec::new());
    };
    let arr = v
        .as_array()
        .with_context(|| format!("{k} must be an array of strings"))?;
    arr.iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .with_context(|| format!("{k} items must be strings"))
        })
        .collect()
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The result of an action, then the screen it led to.
fn then_snapshot(dev: &Device, done: String) -> Result<Value> {
    std::thread::sleep(Duration::from_millis(900));
    let snap = cmd::snapshot(dev, false).unwrap_or_else(|e| format!("(no snapshot: {e:#})"));
    Ok(json!([{"type": "text", "text": format!("{done}\n\n{snap}")}]))
}

fn call(default_device: &str, name: &str, args: &Value) -> Result<Value> {
    let dev_name = args
        .get("device")
        .and_then(Value::as_str)
        .unwrap_or(default_device)
        .to_string();
    if name == "boot" {
        cmd::up(&dev_name)?;
        let dev = Device::find(&dev_name)?;
        return then_snapshot(&dev, format!("{dev_name} is up"));
    }
    let dev = Device::find(&dev_name)?;
    let text = |t: String| Ok(json!([{"type": "text", "text": t}]));
    match name {
        "snapshot" => text(cmd::snapshot(&dev, false)?),
        "screenshot" => Ok(json!([{
            "type": "image",
            "mimeType": "image/png",
            "data": b64(&cmd::screenshot_png(&dev)?),
        }])),
        "tap" => {
            let target: Vec<String> = s(args, "target")?
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let target = if target.len() == 2 && target.iter().all(|t| t.parse::<f64>().is_ok()) {
                target
            } else {
                vec![s(args, "target")?.to_string()]
            };
            let hold = args.get("hold_ms").and_then(Value::as_u64).unwrap_or(80);
            let done = cmd::tap(&dev, &target, hold)?;
            then_snapshot(&dev, done)
        }
        "type" => {
            let keys = args.get("keys").and_then(Value::as_bool).unwrap_or(false);
            let done = cmd::type_text(&dev, &s(args, "text")?.replace("\\n", "\n"), keys)?;
            then_snapshot(&dev, done)
        }
        "scroll" => {
            let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
            let done = cmd::scroll(&dev, s(args, "direction")?, amount)?;
            then_snapshot(&dev, done)
        }
        "swipe" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(300);
            let done = cmd::swipe(
                &dev,
                (f(args, "x1")?, f(args, "y1")?),
                (f(args, "x2")?, f(args, "y2")?),
                ms,
            )?;
            then_snapshot(&dev, done)
        }
        "pinch" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(500);
            let done = cmd::pinch(
                &dev,
                f(args, "cx")?,
                f(args, "cy")?,
                f(args, "r1")?,
                f(args, "r2")?,
                ms,
            )?;
            then_snapshot(&dev, done)
        }
        "orientation" => {
            let name = args.get("name").and_then(Value::as_str);
            let done = cmd::set_orientation(&dev, name)?;
            if name.is_some() {
                then_snapshot(&dev, done)
            } else {
                text(done)
            }
        }
        "rotate" => {
            let done = cmd::rotate_device(&dev, s(args, "direction")?)?;
            then_snapshot(&dev, done)
        }
        "press" => {
            let done = cmd::press(&dev, s(args, "button")?)?;
            then_snapshot(&dev, done)
        }
        "launch" => {
            let logs = args.get("logs").and_then(Value::as_bool).unwrap_or(false);
            let env = parse_env(args)?;
            let app_args = parse_string_list(args, "args")?;
            let done = cmd::launch(&dev, s(args, "bundle_id")?, logs, &env, &app_args)?;
            std::thread::sleep(Duration::from_millis(1500));
            then_snapshot(&dev, done)
        }
        "terminate" => text(cmd::terminate(&dev, s(args, "bundle_id")?)?),
        "appearance" => {
            let done = cmd::appearance(&dev, s(args, "mode")?)?;
            then_snapshot(&dev, done)
        }
        "status_bar" => text(cmd::status_bar(
            &dev,
            args.get("time").and_then(Value::as_str),
            args.get("battery")
                .and_then(Value::as_u64)
                .map(|b| b.min(100) as u8),
            args.get("charging")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            args.get("clear").and_then(Value::as_bool).unwrap_or(false),
        )?),
        "clipboard" => text(cmd::clipboard(
            &dev,
            args.get("text").and_then(Value::as_str),
        )?),
        "logs" => text(cmd::logs(
            &dev,
            s(args, "bundle_id")?,
            args.get("lines").and_then(Value::as_u64).unwrap_or(100) as usize,
            args.get("grep").and_then(Value::as_str),
        )?),
        "install" => text(cmd::install(&dev, &PathBuf::from(s(args, "path")?))?),
        "uninstall" => text(cmd::uninstall(&dev, s(args, "bundle_id")?)?),
        "apps" => text(cmd::apps(
            &dev,
            args.get("all").and_then(Value::as_bool).unwrap_or(false),
        )?),
        "open_url" => {
            let done = cmd::open_url(&dev, s(args, "url")?)?;
            std::thread::sleep(Duration::from_millis(1500));
            then_snapshot(&dev, done)
        }
        "wait_for" => {
            let t = args.get("timeout").and_then(Value::as_f64).unwrap_or(20.0);
            text(cmd::wait(
                &dev,
                s(args, "text")?,
                Duration::from_secs_f64(t.clamp(0.0, 600.0)),
                args.get("gone").and_then(Value::as_bool).unwrap_or(false),
            )?)
        }
        _ => bail!("unknown tool {name}"),
    }
}

fn reply(out: &mut impl Write, id: &Value, result: Result<Value, (i64, String)>) -> Result<()> {
    let msg = match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    writeln!(out, "{msg}")?;
    out.flush()?;
    Ok(())
}

pub fn run(default_device: &str) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                reply(
                    &mut out,
                    &Value::Null,
                    Err((-32700, format!("parse error: {e}"))),
                )?;
                continue;
            }
        };
        let Some(id) = msg.get("id").cloned() else {
            continue; // a notification (initialized, cancelled): nothing to answer
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "agent-sim", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "agent-sim drives an iOS simulator. Read the screen with snapshot (text, with @e references), act with tap/type/scroll/press, and install, launch and stop apps. Actions return the screen that follows.",
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(match call(default_device, name, &args) {
                    Ok(content) => json!({"content": content}),
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true})
                    }
                })
            }
            _ => Err((-32601, format!("unknown method {method}"))),
        };
        reply(&mut out, &id, result)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn every_tool_has_a_schema() {
        let list = tools();
        let arr = list.as_array().unwrap();
        for t in arr {
            assert!(t["name"].is_string());
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        let names: Vec<&str> = arr.iter().filter_map(|t| t["name"].as_str()).collect();
        for want in ["pinch", "orientation", "rotate"] {
            assert!(names.contains(&want), "missing MCP tool {want}");
        }
    }
}

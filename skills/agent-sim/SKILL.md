---
name: agent-sim
description: Test and drive iOS apps in an iOS simulator on a Linux machine with the agent-sim CLI. Use when asked to run, test, check or demo an iOS app (.ipa or simulator .app), reproduce an iOS bug, or look at an iOS screen, and the machine is Linux with agent-sim installed.
---

# agent-sim

`agent-sim` runs Apple's iOS simulator runtime on Linux (through x86Sim) and drives it from the command line.
Every command takes `--device NAME` (default `p`, or `$AGENT_SIM_DEVICE`).

## The loop

1. `agent-sim status`: is a phone running? If not, `agent-sim up` (15-40 s).
2. `agent-sim install path/to/App.ipa` (or a simulator `.app`, a `.tar.gz` or `.zip` holding one such as an EAS Build artifact, or an http(s) URL of any of these), then `agent-sim launch <bundle id>` (add `--env KEY=VALUE` and `-- -arg value` to pass environment variables and launch arguments).
   `agent-sim apps` lists installed apps and their bundle ids.
3. `agent-sim snapshot`: read the screen. One element per line:
   `@e4 button "Sign in" id="login.submit" (201,640)`: reference, role, label, value/id, center point.
4. Act:
   - `agent-sim tap @e4` (reference from the last snapshot), `agent-sim tap "Sign in"` (text), `agent-sim tap 201 640`
     (screen points)
   - `agent-sim type "me@example.com"`: tap the field first; `\n` presses Return
   - `agent-sim scroll down` (shows what is below), `agent-sim swipe x1 y1 x2 y2`
   - `agent-sim press home`
   - `agent-sim open https://example.com` or a deep link
5. `agent-sim wait "Welcome" --timeout 20` instead of sleeping; `--gone` waits for something to disappear.
6. Snapshot again after every action: references change when the screen does.

## Tips

- Prefer `snapshot` over `screenshot`; use `screenshot -o screen.png` to check visual details or to show a human.
- System alerts (permission prompts) appear first in the snapshot; answer them with `tap "Allow"` or
  `tap "Don't Allow"`.
- A tap on an element that does not react: check it is not `disabled` in the snapshot, or scroll it into view.
- App Store downloads are encrypted and cannot be installed; use an Ad Hoc / Development export or a simulator
  build.
- To give an app documents to open, `agent-sim files push report.pdf --to Inbox` copies them into the Files app's
  On My iPhone (`files ls`, `files rm` to check and clean up); then pick them in the app's document picker.
- `agent-sim pool take` hands out an already booted phone when several agents share a machine; give it back with
  `agent-sim pool release NAME`.
- As an MCP server (`agent-sim mcp`) the same commands are tools, and actions return the next snapshot.

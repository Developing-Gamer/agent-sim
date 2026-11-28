# Parity with Apple's Simulator

What Apple's iOS Simulator on a Mac does, and where agent-sim stands on each, measured on a phone rather than
assumed. The probes are repeatable: each row names what was checked.

## Why it does not behave like the Mac by itself

On a Mac, the simulator is not emulated. iOS's own programs run natively on macOS: the same CPU architecture (Apple
silicon), the real XNU kernel's Mach messaging, signals and threads, Metal on the Mac's GPU, and Xcode's
CoreSimulator plus an in-simulator helper (SimulatorBridge) behind every `simctl` command.

On Linux every one of those layers is a re-implementation:

| Layer | On a Mac | Here |
|---|---|---|
| Kernel interfaces (Mach IPC, BSD syscalls, signals, threads) | XNU itself | Darling's re-implementation on Linux (darlingserver) |
| CPU | native arm64 | every instruction translated to x86-64 (x86Sim's dynarmic), cached ahead of time |
| Graphics | Metal on the GPU | metal2vulkan on Mesa's CPU Vulkan driver |
| Services (launchd jobs) | the runtime's full set | a reduced set, chosen for speed |
| simctl features | CoreSimulator and SimulatorBridge | x86Sim's own commands, and agent-sim's |

So differences come in three kinds, and each is handled differently:

1. **Emulation gaps**: a re-implemented layer behaves differently from the real one in some case iOS relies on. These
   are bugs to find and fix in x86Sim or Darling (patches/x86sim) or to work around inside the phone (hooks/).
   Examples found so far: thread stack queries from another thread (web workers crashed), a fault handler replaced
   by an app's crash reporter (apps crawled), lost "port died" notifications (stale permission prompts), and a lost
   root owner on Darling's launcher after a VM restore (no boot).
2. **Missing features**: what CoreSimulator and SimulatorBridge provide on a Mac and nobody had built here
   (dark mode, status bar overrides, the clipboard...). These are agent-sim commands.
3. **Trade-offs**: agent-sim's defaults trade some fidelity for speed (2x rendering, reduced effects); `setup
   --fidelity high` and `effects full` turn them off.

## Matrix

Status: **same** as on a Mac, **close** (works, with a noted difference), **gap** (missing or broken), **better**
(something the Mac does not do).

### Running apps

| Feature | Mac | agent-sim | Status |
|---|---|---|---|
| Install a simulator build (`simctl install`) | yes | `agent-sim install App.app`, or an archive or URL of one (an EAS Build artifact) | same |
| Install a device build (.ipa) | no (needs a device) | `agent-sim install App.ipa`, binaries retagged for the simulator; App Store builds refused (encrypted) | better |
| Launch, terminate, list apps | yes | `launch`, `terminate`, `apps` | same |
| Deep links and URLs (`simctl openurl`) | yes | `agent-sim open`; `prefs:` links checked | same |
| App state kept when going home and back | yes | checked with Settings | same |
| Built-in apps open and draw | all | all on the home screen but Siri; Maps has no map tiles | close |
| Web pages, including web workers | yes | yes (x86Sim patch for workers) | same |
| Apps with crash reporters at full speed | yes | yes (x86Sim patch) | same |
| App logs (Console, `simctl spawn log`) | yes | `agent-sim logs` (os_log of an app launched with `--logs`, and the crash report of its last run) | close |
| XCTest / XCUITest runs (`xcodebuild test`) | yes | not yet | gap |
| Debugger (lldb attach) and Safari Web Inspector | yes | not yet | gap |

### Simulated hardware and system features

| Feature | Mac | agent-sim | Status |
|---|---|---|---|
| Touch, multi-touch, edge gestures (home, Control Center, Notification Center) | yes | yes; `agent-sim pinch CX CY R1 R2` for two fingers | same |
| App switcher | yes | yes | same |
| Rotation | yes | `agent-sim rotate left\|right`, `agent-sim orientation NAME`; screenshots come out upright | same |
| Hardware keyboard typing | yes | `agent-sim type --keys` | same |
| Typing text into any field | yes | `agent-sim type` inserts as the software keyboard does (key events lost characters in web fields) | close |
| Software keyboard on screen | toggled with Cmd-K | hidden (a hardware keyboard is attached, as on a Mac by default) | gap |
| Location (`simctl location`) | yes | yes; apps get prompts and positions | same |
| Photos (`simctl addmedia`) | yes | yes | same |
| Documents in the Files app | drag a file onto the window | `agent-sim files push`, `files ls`, `files rm` (On My iPhone) | same |
| Push notifications (`simctl push`) | yes, to apps allowed to notify | the same rule; untested end to end with an app that asked | close |
| Permissions (`simctl privacy`) | yes | yes; stale prompts of quit apps dismissed | same |
| Face ID / Touch ID | yes | enroll, match, no match | same |
| Shake, memory warning | yes | yes | same |
| Dark mode (`simctl ui appearance`) | yes | `agent-sim appearance dark` | same |
| Status bar overrides (`simctl status_bar`) | yes | `agent-sim status-bar --time 9:41 --battery 100 --charging`; cellular and Wi-Fi bars not yet | close |
| Clipboard (`simctl pbcopy` / `pbpaste`) | yes | `agent-sim clipboard TEXT` sets it, `agent-sim clipboard` prints it (x86Sim 0.6 or later; older releases cannot read it, see [KNOWN_ISSUES](KNOWN_ISSUES.md)) | same |
| Screenshots (`simctl io screenshot`) | yes | `agent-sim screenshot` | same |
| Screen recording (`simctl io recordVideo`) | yes | `agent-sim record` (x86Sim's recorder wrote empty files) | same |
| Erase a device (`simctl erase`) | yes | `agent-sim erase` | same |
| Clone a device (`simctl clone`) | yes | not yet; `agent-sim pool` keeps booted phones instead | gap |
| Siri | yes | does not open | gap |
| Maps tiles | yes | none (map data not available to these runtimes) | gap |

### For agents

| Feature | Mac | agent-sim | Status |
|---|---|---|---|
| Read the screen as text | Accessibility Inspector (by hand); XCUITest queries (in tests) | `agent-sim snapshot`: app, web content, alerts, home screen, status bar, with references | better |
| Act by element or text | XCUITest | `tap @e3`, `tap "Sign in"`, `wait "Welcome"` | better |
| MCP server | no | `agent-sim mcp` | better |
| Watch live in a browser | no (a window on the Mac) | `agent-sim serve` | better |
| Several phones per machine | yes | yes, with shared memory (KSM) and a pool of booted phones | same |
| Saved running phones | no | VM memory snapshots (19 s to a running phone on Freestyle) | better |

### Speed and fidelity

| | Mac (Apple silicon) | agent-sim (8 vCPU cloud VM, no GPU) |
|---|---|---|
| Boot to home screen | a few seconds | 12–13 s on Zen 5, 25–40 s on Zen 4c / Zen 3 |
| Scrolling | 60–120 fps | about 55 fps on Zen 5 |
| Touch to screen | a frame or two | about 14 ms on Zen 5 |
| Rendering | 3x, all effects | 2x with Reduce Transparency and Reduce Motion by default; `setup --fidelity high` for 3x and all effects (half the scrolling frame rate) |
| `snapshot` | (none) | 35 ms home screen, 60 ms Settings, about 0.5 s a long web page (settled, Zen 5); seconds while the phone is busy |

## Next, in order

1. The software keyboard (show it as Cmd-K does on a Mac), and status bar signal bars.
2. XCTest and XCUITest runs, so existing UI test suites run unchanged.
3. A debugger story (lldb, Web Inspector).
4. Siri, Maps tiles (x86Sim's `fetch-macos-host.sh` provides the map data from a Mac), and device cloning.

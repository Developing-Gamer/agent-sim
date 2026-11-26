# agent-sim

**iOS simulators for AI agents, on ordinary Linux machines.** No Mac, no GPU.

<p align="center"><img src="docs/images/dino.gif" width="540" alt="The dino game in Safari on an iOS simulator running on a Linux VM, played by a script that reads the screen and taps"></p>

<p align="center">Safari on Apple's iOS runtime, on a Linux VM, recorded at 60 fps (the GIF is 15).
<a href="https://github.com/Developing-Gamer/agent-sim/releases/download/v0.1.0/dino.mp4">Full clip</a> ·
<a href="https://github.com/Developing-Gamer/agent-sim/releases/download/v0.1.0/agent-sim-demo.mp4">An agent driving
Settings and Safari</a></p>

> **agent-sim is built on [x86Sim](https://github.com/x86sim/x86sim) by [designgears](https://github.com/designgears).**
> x86Sim is what runs Apple's iOS simulator on Linux, and it has done that since its first release. agent-sim is the
> agent layer on top of it. If agent-sim is useful to you, [star x86Sim](https://github.com/x86sim/x86sim) too.

agent-sim takes the iOS simulator that x86Sim runs on an x86-64 Linux VM and gives agents what they need to test iOS
apps: install an `.ipa`, read the screen as text, tap, type, scroll, and watch it live in a browser. One command line
for scripts and CI, and an MCP server for Claude Code, Cursor, Codex and other agents.

```
$ agent-sim launch com.apple.Preferences && agent-sim snapshot
app com.apple.Preferences
@e1 button "Apple Account, Sign in to access your iCloud data, the App Store, Apple services, and more." (201,213)
@e2 button "General" id="com.apple.settings.general" (201,320)
@e3 button "Accessibility" id="com.apple.settings.accessibility" (201,372)
...
$ agent-sim tap @e2
tapped @e2 button "General"
```

> **Status: early.** It works end to end; expect rough edges, and see [what does not work yet](docs/KNOWN_ISSUES.md).

## Quick start

On an x86-64 Linux machine with Ubuntu 24.04 (8+ CPUs, 16 GB of memory, 60 GB of free disk):

```sh
curl -fsSL https://raw.githubusercontent.com/Developing-Gamer/agent-sim/main/install.sh | bash
agent-sim setup      # once, about half an hour: x86Sim, Apple's iOS runtime (8 GB, from Apple), a first phone
agent-sim up         # boot it: 12-30 s, depending on the CPU
agent-sim snapshot
```

From a checkout instead: `cargo build --release`, then `scripts/setup-vm.sh` and `target/release/agent-sim`.

`agent-sim setup --fidelity high --device hifi` adds a phone that looks exactly like Apple's simulator (3x, every
blur and animation) for screenshots; the default draws at 2x with reduced effects and scrolls twice as fast.

## Use it from an agent

As an MCP server:

```sh
claude mcp add agent-sim -- agent-sim mcp          # Claude Code
```

```json
{ "mcpServers": { "agent-sim": { "command": "agent-sim", "args": ["mcp"] } } }
```

An agent on another machine (your laptop) runs the same server over SSH; MCP's stdio passes through unchanged:

```sh
claude mcp add agent-sim -- ssh my-linux-vm agent-sim mcp
```

The tools are `snapshot`, `tap`, `type`, `scroll`, `swipe`, `press`, `launch`, `terminate`, `install`,
`uninstall`, `apps`, `open_url`, `wait_for`, `screenshot` and `boot`. Actions answer with the screen that follows,
so an agent sees the result of each step without asking again. For agents that work in a shell,
[skills/agent-sim/SKILL.md](skills/agent-sim/SKILL.md) teaches the command line.

## The command line

```sh
agent-sim install MyApp.ipa        # a device build is converted for the simulator; a simulator .app works too
agent-sim launch com.example.myapp
agent-sim snapshot                 # the screen as text, one element per line (--json for frames, ids, states)
agent-sim tap @e4                  # an element from the last snapshot; or: tap "Sign in"; or: tap 201 320
agent-sim type "hello@example.com"
agent-sim scroll down
agent-sim wait "Welcome" --timeout 20
agent-sim screenshot -o screen.png
agent-sim press home
agent-sim open https://example.com
```

`snapshot` reads the accessibility tree of the app in front, the web pages it shows, and SpringBoard, so system
alerts, the home screen and the status bar are in it too. Out-of-process pickers (the photo picker) are read as
`app <bundle> (modal PhotosPicker)`, and a locked phone reads `lock screen`. Every command takes `--device NAME`; `agent-sim --help`
lists them all.

The simulator controls Apple's `simctl` has are here too: `appearance dark`, `status-bar --time 9:41`, `clipboard TEXT`,
`rotate right`, `orientation landscapeLeft`, `pinch`, `record -o video.mp4`, `erase`, `logs`, plus x86Sim's `location`, `push`, `privacy` and `addmedia`.
[docs/PARITY.md](docs/PARITY.md) compares every feature with Apple's Simulator on a Mac.

React Native and Expo apps, development builds with Metro included: [docs/REACT_NATIVE.md](docs/REACT_NATIVE.md).

Device builds run when they are not encrypted. App Store downloads are (FairPlay), so use an Ad Hoc, Development or
Enterprise export, or build for the simulator.

## Watch it in a browser

```sh
agent-sim serve --listen 0.0.0.0:7979
```

It prints a URL with an access token. The page streams the screen as H.264 over a WebSocket (decoded with
WebCodecs, so a recent Chrome, Edge or Safari) and sends touches, keys, scrolling and the Home and Lock buttons
back. Touches that start at a screen edge are system gestures: a swipe up from the bottom goes home. Keep the token
private.

## Many phones

```sh
agent-sim pool up 3                # prepare and boot pool1..pool3
phone=$(agent-sim pool take)       # a booted phone nobody else holds
agent-sim --device "$phone" snapshot
agent-sim pool release "$phone"    # it restarts in the background for the next taker
```

Phones share memory through the kernel's KSM: three take 6.8 GB together instead of 8.7 GB. `pool up` turns KSM on
with sudo (it asks on a terminal; without one it needs a cached sudo, or skips KSM and says so; `AGENT_SIM_KSM=0`
leaves it off). On a cloud provider with VM memory snapshots, a booted phone can be saved and brought back running in
about 20 seconds ([docs/BENCHMARKS.md](docs/BENCHMARKS.md)).

## What it can and cannot tell you

Pick the checks you need before putting agent-sim in CI:

| Check | agent-sim today |
|---|---|
| Agent-driven or scripted UI checks (launch, tap, type, read the screen, record) | yes |
| XCUITest suites | not yet |
| Real-device behavior (camera, push delivery, signing, FairPlay/DRM, device performance) | no: use a real device |

[docs/PARITY.md](docs/PARITY.md) compares every feature with Apple's Simulator on a Mac.

## How it works

[x86Sim](https://github.com/x86sim/x86sim) runs Apple's arm64 simulator runtime on x86-64 Linux: Darling provides
the macOS kernel interfaces, dynarmic translates the ARM code, and Mesa's CPU Vulkan driver draws Metal through
metal2vulkan. agent-sim builds the agent-facing product on top of it:

- **A phone set up for speed:** a pre-translated runtime, the services apps need and none they do not, the screen
  drawn at 2x, and reduced transparency and motion. Scrolling runs at about 55 fps on a Zen 5 VM, and a touch
  reaches the screen in about 14 ms ([docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- **Fixes in x86Sim** ([patches/x86sim](patches/x86sim)): web workers in Safari, and apps with crash reporters
  no longer crawl.
- **Hooks in the phone** ([hooks](hooks)): an accessibility reader for `snapshot`, and stale permission prompts
  dismissed as on a device.
- **The agent interface:** the CLI, the MCP server, `.ipa` conversion, the phone pool and the browser stream.

## Documentation

- [docs/PARITY.md](docs/PARITY.md): feature by feature against Apple's Simulator on a Mac
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md): speed, memory, several phones, snapshots
- [docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md): what does not work yet and why
- [docs/ROADMAP.md](docs/ROADMAP.md): what comes next
- [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md)

## Credits

agent-sim builds on [x86Sim](https://github.com/x86sim/x86sim), which runs Apple's simulator runtimes on Linux
through [Darling](https://github.com/darlinghq/darling), [dynarmic](https://github.com/azahar-emu/dynarmic) and
[metal2vulkan](https://github.com/steelbrain/metal2vulkan). Fixes agent-sim needs in x86Sim itself live in
[patches/x86sim](patches/x86sim) until they are upstream.

Apple's runtimes are downloaded from Apple onto your own machine by x86Sim's runtime downloader and are never part of
this repository.

agent-sim is an independent project. It is not affiliated with, endorsed by or sponsored by Apple, and iOS, Safari and
Xcode are trademarks of Apple Inc. Apple's simulator runtimes come with Apple's own license terms; check that your use
complies with them.

## License

[Apache-2.0](LICENSE), except the patches in `patches/x86sim`, which follow x86Sim's GPL-3.0-or-later.

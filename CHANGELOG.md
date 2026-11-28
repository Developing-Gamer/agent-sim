# Changelog

## Unreleased

- `agent-sim test` (and the MCP tool `test`) runs XCTest and XCUITest bundles on the phone through x86Sim 0.7's
  `x86sim test`: an `.xctestrun` from `xcodebuild build-for-testing`, an app's unit tests, logic tests, a UI test
  runner with its app, or a Swift package; `--only-testing`, `--skip-testing`, `--junit`. Exit status 0 when every
  test passed, 1 when one failed, 2 when nothing could run. XCTest's frameworks come from Xcode, which x86Sim sets up
  from an Xcode .xip.
- A phone that draws no frame in 3 minutes is restarted with a new Darling prefix (a machine that went down with the
  phone running left one that Darling's server died on), and stopping a phone ends root-owned leftovers in its unit
  when sudo needs no password, so the restart is not refused as "already loaded".
- `clipboard` without text prints the pasteboard again, through x86Sim's `pbpaste` (0.6 and later; 0.2 s, also when an
  app owns the pasteboard). It never reads it from SpringBoard, which hung the phone.
- New setups use x86Sim 0.7.3 (from 0.5.3): about half the memory per phone, native fast and high fidelity
  (`x86sim fidelity`, so no "(at 2x)" device type is added, and a phone made for an older release is moved to the new
  setting when setup runs again), and its own XCUITest runs (`x86sim test`). Machines set up earlier keep the release
  they have until `agent-sim setup` runs.
- Phones boot under a umask of 0002 (Ubuntu's default for a user with their own group): x86Sim 0.7.2 and later made the
  container's system daemons folder group-writable there, and Darling's launchd then started none of them, so the
  phone never drew a frame. Phones and setup now run with umask 022, and older prefixes are repaired.
- On x86Sim 0.7.x apps no longer quit with "shared code cache full" and SpringBoard no longer restarts in a loop: apps
  and app extensions translate their own code in a full code cache of their own, and daemons keep sharing the
  pre-translated image.
- `wait` keeps asking a phone that is up but busy (SpringBoard still starting) until its timeout, instead of failing
  at the first slow answer.
- Setup fetches dynarmic only when one of x86Sim's translator patches applies, so a release that includes them all no
  longer fails there.
- `agent-sim doctor` checks what a phone needs on this machine (Linux x86-64, memory, user namespaces, the systemd
  user session, curl, tar, python3, ffmpeg, a Vulkan driver, x86Sim, the hooks, the iOS runtime, the phone and whether
  SpringBoard answers) and says what is missing, so a failed setup can be reported with something to go on.
- Sign in with Apple reaches Apple's own sheet (or its "Sign in to your Apple Account" alert on a phone without one)
  instead of failing at once with ASAuthorizationError 1000: the `apps` service set starts Apple's account daemon,
  akd, which only runs when asked for.
- `tap "TEXT"`, `wait` and the MCP tools that find elements by text match accented and non-Latin text whatever its
  Unicode normalization: the Files app and other Foundation-backed labels come decomposed (NFD), so a typed (NFC)
  `tap "Απολυτίκιο"` found nothing.
- `install` takes an archive holding an .app (.tar.gz, .tgz, .tar, .zip), as EAS Build and CI artifacts come, and an
  http(s) URL of any file it takes, so an EAS simulator build installs with `agent-sim install <artifact URL>`.
- `files push PATH... [--to DIR]`, `files ls [DIR]` and `files rm PATH` (and the MCP tool `files`) put documents in the
  Files app's On My iPhone, list and remove them. Names are written decomposed (NFD), so files with accented or
  non-Latin names open; copied in by hand with the composed names Linux tools write, they were listed but "not found".
- `snapshot`, `tap`, `swipe`, `scroll` and `record` work in landscape: elements right of the portrait width are no
  longer dropped, taps land where they are aimed, and recordings come out upright. Portrait-locked apps stay portrait.
- `tap "Settings" --hold-ms 900` no longer reads the flag as part of the target; `type --keys` no longer leaves Shift
  stuck or drops characters, covers all printable US ASCII, and refuses other characters with a clear message.
- `logs` finds the right app's os_log and last run when several apps share an executable name (every Flutter app is
  `Runner`), and reports signal crashes (SIGABRT, SIGSEGV...) as crashed.
- `snapshot` of a web view inside another app's in-app browser no longer lists the app behind it, and
  SafariViewService windows that UIKit shows late are no longer empty.
- Back-to-back `status-bar`, `clipboard` and `type` requests no longer wait out a 15 s timeout.
- The MCP server lists `appearance`, `status_bar`, `clipboard`, `logs` and `type`'s `keys` (26 tools).
- Dead `hooked-<pid>` markers are removed before SpringBoard looks processes up, so long sessions keep `snapshot` fast.
- Setup and the scripts find x86Sim's data and cache directories as its AppRun does (`X86SIM_DATA`, `X86SIM_CACHE`),
  reuse an iOS runtime that is already unpacked, create the phone on `RUNTIME_BUILD`, keep the user-namespace
  Darling prefix under `X86SIM_DARLING_MODE=userns`, and take the Vulkan driver and Metal programs from `x86sim.env`.
- `pool up` goes on without KSM when sudo cannot ask for a password.
- `rotate left|right`, `orientation [NAME]` and `pinch CX CY R1 R2` (two fingers), in the CLI and the MCP server.
- `snapshot` reads a phone on the Lock Screen as `lock screen` (Cover Sheet and status bar only), not as the home screen.
- `snapshot` reads out-of-process modals such as the photo picker (`app <bundle> (modal PhotosPicker)`), and `type`
  goes into their fields; before, only the covered app was listed.
- `snapshot` shows the status bar clock as overridden by `status-bar --time`, as the screenshot does.
- `clipboard` no longer reads the pasteboard (it hung SpringBoard until the phone was restarted); `clipboard TEXT` still
  sets it.
- `snapshot` is no longer empty on a phone whose first-boot reset turned app accessibility off: every boot checks the
  two settings and turns them back on (issue #8).
- `status` reports a phone whose SpringBoard does not answer as `hung`, and `snapshot` and x86Sim commands that time
  out say so and that `agent-sim up` restarts it (issue #6).
- `launch` waits out 1.5 s after `terminate` of the same app, and `launch --logs` stops a running app itself, so a
  quick relaunch does not die during startup (issue #6).
- `location set|clear`, `addmedia`, `privacy grant|revoke|reset` and `push`, in the CLI and the MCP server.
- `scroll` slows into its end point and holds before lifting, so the list stops where `scroll` returns and the next
  tap lands (it kept coasting in 5 of 6 tries before, 0 of 6 now).
- x86Sim fix 0003: long-running phones no longer leak a 4-12 KB reserved mapping per allocation in the Metal
  processes (backboardd grew by 625 such mappings over three rounds of use before, 61 now).
- `patch-x86sim.sh` skips a fix that the x86Sim release already contains or has fixed its own way, so a new x86Sim
  release does not stop setup.
- `launch --env KEY=VALUE -- ARGS`, Return as a real key press in `type`, sign-in sheets in `snapshot`, and setup on
  Arch and Fedora.
- New setups use x86Sim 0.5.3 (Arch and other distributions with libxml2 2.14 or later, correct thread CPU time).
  A machine set up on an older release keeps using it until `agent-sim setup` runs again, which upgrades it.
- `install` thins universal (x86_64 + arm64) simulator builds to arm64, so Flutter's and Xcode's default simulator
  builds launch.
- `phone.sh` leaves `VK_ICD_FILENAMES` unset when lavapipe is not installed, so desktops use their GPU's driver.
- Web pages no longer stay blank, and `open` brings Safari up after it was quit: reading the screen sent SIGUSR2 to
  processes still starting, which ended them; now only processes whose hook is ready are asked.
- `open` checks that Safari started for a web link and asks again once if it did not.
- `phone.sh` passes Darling's `DSERVER_*` log settings through to the phone.
- `status` reports a phone whose Darling server exited as crashed, and commands that read the screen say how to
  restart it, instead of timing out.
- The README opens with the dino-game clip.

## 0.1.0

First release.

- `agent-sim` command line: `setup`, `up`, `down`, `status`, `snapshot` (the screen as accessibility elements
  with references), `tap`, `type`, `scroll`, `swipe`, `press`, `wait`, `screenshot`, `install` (.app, and .ipa
  device builds converted for the simulator), `uninstall`, `launch`, `terminate`, `apps`, `open`, `logs`, `pool`,
  `serve` (live H.264 view in a browser) and `mcp` (Model Context Protocol server).
- Setup for one command on Ubuntu 24.04: x86Sim, Apple's iOS 27.2 runtime, a pre-translated runtime, an iPhone 17
  Pro drawn at 2x, the system services apps expect, reduced effects and app accessibility.
- Fixes to x86Sim (patches/x86sim): web workers in Safari; apps with crash reporters no longer crawl.
- Hooks in the phone (hooks/): the accessibility reader, and permission prompts dismissed when their app quits.
- Several phones per VM with shared memory (KSM), and a pool of booted phones.

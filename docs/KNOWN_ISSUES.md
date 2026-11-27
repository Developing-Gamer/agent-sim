# Known issues

What does not work yet, what we know about the cause, and what agent-sim does about it. Issues whose fix belongs in
x86Sim are reported there with the evidence below.

## Fixed or worked around in agent-sim

| Issue | Cause | What agent-sim does |
|---|---|---|
| Every web page crashes ("This webpage was reloaded because a problem occurred") | WebKit's web content process reserves a 16 GB "Gigacage" at startup; the reservation fails under x86Sim (`FATAL: Could not allocate gigacage memory with maxAlignment = 17179869184`) | `phone.sh` sets WebKit's own switch, `GIGACAGE_ENABLED=0`, for `com.apple.WebKit.WebContent` through x86Sim's per-program environment file (`~/.cache/x86sim/a64-penv`) |
| More than a core busy with nothing on screen | x86Sim's `slim` service list leaves out `com.apple.sleepd`; SpringBoard retries reaching it about 780 times a second (each try a burst of `mach_port_construct`/`mach_msg` calls through darlingserver) | `phone.sh` adds sleepd back |
| Photos closes at once; Wallet and Watch stay black | services they depend on are not in `slim` | `phone.sh` adds back assetsd, passd, financed, nanoregistryd, accountsd, amsaccountsd, mediaremoted, sharingd and medialibraryd |
| Pages that start a web worker or service worker crash ("A problem repeatedly occurred") | WebKit asks for a new thread's stack bounds from the creating thread; x86Sim described the child's native stack, not the guest stack it runs on, and JavaScriptCore's `StackBounds::contains` check failed in `sanitizeStackForVM` | `patches/x86sim/0001-guest-stacks-of-new-threads.patch`, built by `scripts/patch-x86sim.sh` during setup |
| Settings shows gray or blank row icons | Settings finds no rendered icon in the shared icon store and shows a placeholder; the store stays empty because iconservicesagent crashes rendering them: CoreUI force-unwraps `sharedCIContext`, which is nil when the process has no Metal device (x86Sim gives Metal only to the processes named in `X86SIM_METAL`) | `phone.sh` gives iconservicesagent Metal too (`X86SIM_METAL=backboardd,iconservicesagent`). A phone that ran without it keeps the blank icons it cached until its icon store is rebuilt (delete `data/Containers/Shared/SystemGroup/systemgroup.com.apple.lsd.iconscache/Library/Caches/com.apple.IconsCache` with the phone stopped) |
| Apps never get a location prompt, and other system features they use are missing | x86Sim's `slim` list leaves out locationd and every service that only starts on demand (notifications, pasteboard, Spotlight, contacts, calendars, StoreKit, StatusKit...) | `phone.sh` adds them (`AGENT_SIM_SERVICES=apps`, the default) |
| Typing into web fields lost characters | hardware key events reached native fields but most were dropped in web fields | `type` inserts the text into the focused field as the software keyboard does (`--keys` keeps key presses) |
| `snapshot` did not show web content | WebKit draws web views in its WebContent process | the accessibility hook also reads each page there, and the CLI merges it in screen points |
| The first boot after a VM restart stalls on the Apple logo, or Darling never starts | three causes, all found: x86Sim's user-namespace mode (used when unprivileged namespaces are allowed) stalls its first boot after its holder process is recreated; a Darling prefix made in that mode is refused by system mode; and the first boot of a phone can wait for good on a service | `setup-vm.sh` installs Darling for x86Sim's system mode (sudo); `phone.sh` replaces a prefix from the other mode, and restarts a boot that shows no frame for 3 minutes or uses no CPU for 30 s |
| Cold boots take minutes | x86Sim runs the phone through a desktop throttle (disk reads and writes at 40 MB/s, low CPU weight) | `phone.sh` turns it off (`X86SIM_IO_MAX=0`) |
| News stays black; any app with a crash reporter runs very slowly | An app's `SIGSEGV` handler made Darling replace x86Sim's fault handler, so every routine translator fault (Swift's tagged pointers are non-canonical addresses on x86-64) cost a darlingserver round trip; News' startup spun in Swift string code at a few hundred iterations a second | `patches/x86sim/0002-keep-the-fault-handler-after-guest-sigaction.patch` |
| A permission prompt stays on screen after the app that asked quits or crashes | Darling delivers a Mach dead-name notification within a process but loses it when the port's owner is another process that died. SpringBoard watches the prompt's reply port for that notification and tccd watches the asking app's connection the same way, so neither noticed | `hooks/springboard.c`, loaded into SpringBoard by `phone.sh`, checks each alert once a second and cancels it when the process that posted it, or the app a permission prompt names, is gone. tccd records the canceled prompt as "denied, prompt canceled" (reason 13); reset with `x86sim privacy reset` |
| Sign in with Apple fails at once with ASAuthorizationError 1000 (the log has `com.apple.ak.authorizationservices.xpc was invalidated: Connection init failed at lookup with error 3 - No such process`) | Apple's account daemon, akd, serves that connection; it starts only when asked for and is in neither x86Sim's service lists nor agent-sim's | `phone.sh` adds `com.apple.akd` to the `apps` set. Continue with Apple then shows Apple's "Sign in to your Apple Account" alert on a phone with no account, as iOS does. At rest akd does not run, so the boot and idle cost did not change (docs/BENCHMARKS.md method, before and after) |
| Built-in apps' permission prompts get in the way of other apps | a prompt stays up over every app until answered, as on iOS | `setup-vm.sh` grants the built-in apps their permissions up front, as on a phone that has been set up; apps you install still ask |
| Web pages stayed blank (or froze) for good; `agent-sim open` after Safari had quit left the home screen; an app read right after it launched sometimes died | `snapshot` (and `wait`, `type` and the other commands that read the screen) asked processes for their elements with SIGUSR2, whose default action ends a process. A process that was still starting, before the hook had installed its handler, was killed: Safari when `wait` polled right after `open` started it, and the WebContent process Safari starts for each new site while the page loaded. Safari does not notice a dead WebContent process (the lost dead-name notifications below), so its page stayed blank. Measured: 0 of 5 `open`s after quitting Safari came up; 11 WebContent processes piled up over a session and pages took 10-24 s | the hook writes `hooked-<pid>` once its handler is in place, and the CLI signals only processes that have it (a process still starting reads as "not answering yet"). After: 10 of 10 `open`s after a quit, 60 of 60 page loads in 2-14 s with up to 5 WebContent processes. A page whose WebContent process dies some other way stays frozen until a reload, which brings it back |
| `snapshot` is empty after a fresh setup: `home screen` with no elements, Settings shows only `app com.apple.Preferences`, `wait` times out (issue #8) | UIKit describes controls only while `ApplicationAccessibilityEnabled` and `AccessibilityEnabled` are 1. Setup writes them after the first boot; on one machine both read 0 afterwards, with `AXSDidResetD22Preferences` set, which points to a first-boot preferences migration that ran after the write (not seen on the VMs agent-sim is tested on) | `phone.sh wait` checks both keys after every boot and writes them back (about 1 s); a running phone takes the change at once. Reproduced by setting both to 0 and restarting the phone: before, every snapshot was empty; after, the boot turns them on again |
| Stopping an app and starting it again at once: the new process dies during startup, and after a few in a row SpringBoard stops answering while `status` says running (issue #6) | Probably RunningBoard and FrontBoard still tearing the old process down when the launch arrives (the reporter's logs show the new process dying before `dyld_sim` finishes), with `x86sim launch --terminate-running-process` stopping and starting inside one command; not confirmed. `status` only looked for the SpringBoard and Darling processes, which a hung phone still has | `launch` waits out 1.5 s after `terminate` of the same app; `launch --logs` stops a running app itself and waits for its exit instead of using that flag. `status` asks SpringBoard and prints `hung` when it stays silent for 8 s, and `snapshot` and any x86Sim command that times out say the phone is hung and that `agent-sim up` restarts it. The startup deaths did not reproduce on an 8 vCPU Zen 3 VM (8 of 8 relaunches lived with and without the pause), so the pause follows the reporter's measurements (8 of 8 with it); a hung SpringBoard was simulated by stopping its process |
| `agent-sim clipboard` (read) timed out and left SpringBoard hung until the phone was restarted | The hook read `UIPasteboard.generalPasteboard.string` on SpringBoard's main queue; iOS's paste authorization needs that same queue. Seen on iOS 27.2 with x86Sim 0.5.3 even when agent-sim had set the text itself (the read hung SpringBoard each time) | Reading is not supported: `clipboard` without text says so at once and the hook answers `unsupported` without touching the pasteboard, so an older `agent-sim` cannot hang a phone with the new hook. `clipboard TEXT` still sets it |

## Open

### Lost dead-name notifications (root cause, in Darling)

When a process dies, Darling turns the send rights other processes hold to its ports into dead names, but the
dead-name notifications they asked for never arrive (a test inside one process gets both the raw notification and
libdispatch's `DISPATCH_MACH_SEND_DEAD` event). Anything that relies on noticing a peer's death across processes, such
as an XPC connection's interruption, is affected. The SpringBoard hook covers permission prompts; the fix belongs in
Darling's darlingserver.

Note that a permission prompt staying over other apps while its app is still running is iOS behavior, not this
bug: SpringBoard ignores Home while such an alert is up ("an alert item or sheet is showing and can't be dismissed").

### Safari can stay frozen with its main thread suspended (x86Sim / Darling)

Seen once in about 150 navigations and Safari launches: Safari stops answering (`snapshot` says "not answering yet",
`launch` gets no reply), with no CPU use. Its main thread is parked in x86Sim's thread-suspend handler (signal 34
blocked on that thread alone, waiting on darlingserver) while every other thread runs, and bringing Safari back to the
front does not release it, so iOS is not holding it suspended on purpose: a suspend's matching resume is lost. Safari's
JavaScriptCore thread suspends and resumes the main thread for garbage collection (`thread_suspend`,
`thread_get_state`, `thread_resume`, microseconds apart), and x86Sim parks a thread that is running translated code
only at its next safe point, after the suspend returned; x86Sim already fixed one race of this kind in darlingserver
(a resume that arrived before the thread slept). Not reproduced on demand yet. Reloading does not help, because Safari
itself is stuck; `agent-sim terminate com.apple.mobilesafari`, then `agent-sim open <url>` again, does.

### The phone can stop all at once (Darling's server exits)

Seen twice in about six hours of heavy use: every app and service freezes and no new frame is drawn (the stream and
`record` show a still picture, `snapshot` and `launch` time out). Darling's server process has exited, with no crash in
the kernel log, no core and nothing in its own log (with `DSERVER_LOG_CATEGORIES=dtape`, which `phone.sh` now passes
through, its last entries were RunningBoard resuming a process). `agent-sim status` reports the phone as crashed, and the
commands that read the screen say so; `agent-sim up` brings it back in about 20 s. Cause not found yet.

### Video does not play in Safari

A `<video>` in a web page never loads: pressing Play shows Pause, but the time stays at 0, no frame is decoded and the
length reads "indefinite" (an H.264 MP4 from a server with byte ranges, autoplay or tapped; giving WebKit's GPU process
Metal through `X86SIM_METAL` did not change it). Video outside the browser plays: the same file added with
`x86sim addmedia` plays in Photos at about 27 of its 30 fps. So the gap is in WebKit's media path (its GPU process),
not in decoding. Not investigated further yet. (YouTube asks a datacenter IP to sign in first, so it is no test.)

### `snapshot` is slow while the phone is busy

Once the phone has settled, a snapshot takes 35 ms on the home screen, 60 ms in Settings and about half a second on a
long web page (Zen 5). Right after a boot or while a page loads, the phone keeps every core busy (load 10-12 on 8
vCPUs) and the app answers in seconds; the first walk of an app also translates accessibility code on first use.
Measured inside the processes: an idle process wakes in under a millisecond, so this is CPU time, not a wake-up
problem. `snapshot` waits up to 10 s for the app (8 s for web content) rather than leave it out.

### A pre-translation pass can fail

`x86sim aot update` sometimes stops with "constant pool conflict" on one library (seen on SpringBoard after training
runs from several phones). The image already built stays in use, so the phone runs; that library's newly hot code is
translated as it runs instead. setup-vm.sh carries on with a warning. An x86Sim issue, to report upstream.

### Siri does not open

Neither the Siri app (`com.apple.SiriApp`; launching it returns no process) nor the Siri button brings up Siri's UI,
with agent-sim's services or x86Sim's standard list: assistantd starts, but the UI (`com.apple.campo`, which
RunningBoard should start on the Siri request) never does.

### Maps shows no map tiles

The map data comes from a Mac and is not part of the iOS runtime (x86Sim limitation).

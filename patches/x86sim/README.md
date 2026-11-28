# Fixes to x86Sim

Patches against x86Sim's tagged source (currently `v0.7.3`), built and installed by `scripts/patch-x86sim.sh`
(`scripts/setup-vm.sh` runs it unless `AGENT_SIM_PATCH_X86SIM=0`). They change x86Sim's code, so they are under
x86Sim's license, GPL-3.0-or-later, and are meant to go upstream; each one is dropped here once an x86Sim release
includes it.

`patch-x86sim.sh` applies each patch on its own. A patch that the x86Sim release already contains, or that no
longer matches because upstream fixed the same code its own way, is skipped with a note, and when none apply x86Sim's
own translator is used. A newer x86Sim release therefore never breaks setup.

| Patch | Upstream |
|---|---|
| `0001-guest-stacks-of-new-threads.patch` | not yet sent; does not apply to v0.7.3 (the code changed) |
| `0002-keep-the-fault-handler-after-guest-sigaction.patch` | fixed upstream by [6433b48](https://github.com/x86sim/x86sim/commit/6433b48); no longer applies to v0.7.3 |
| `0003-release-unused-reservation-tails.patch` | merged as [x86sim/x86sim#24](https://github.com/x86sim/x86sim/pull/24) (in v0.7.0); no longer applies to v0.7.3 |

| Patch | What it fixes |
|---|---|
| `0001-guest-stacks-of-new-threads.patch` | Every page that starts a web worker or service worker crashed ("A problem repeatedly occurred"). WebKit reads a new thread's stack bounds from the thread that created it, `pthread_get_stackaddr_np(child)`; x86Sim answered correctly only for the calling thread and otherwise described the native stack, which guest code never runs on, so JavaScriptCore's worker VM failed its `StackBounds::contains` check. The creating thread now maps the child's guest stack and records it under the child's handle. |
| `0002-keep-the-fault-handler-after-guest-sigaction.patch` | An app that installs a `SIGSEGV` handler (any crash reporter) slowed to a crawl; News never drew. Darling's `sigaction` put its own handler on the Linux signal in place of x86Sim's, so the translator's routine faults (a Swift tagged pointer is a non-canonical address on x86-64) each took a darlingserver round trip of about 1.4 ms instead of a few microseconds. x86Sim's handler, which also delivers real faults to the app's handler, is put back after every such `sigaction`. |
| `0003-release-unused-reservation-tails.patch` | Long-running Metal sessions eventually crashed `metalhostd` (`gheap: large allocation failed (reserve 0, errno 17)`), taking down `backboardd` and `SpringBoard`. `reserve_aligned()` passed `size + align` to `reserve_low()` (which rounds up to 16 KB) but trimmed only to `r + size + align`, leaking a 4–12 KB `PROT_NONE` VMA on every non-16K mapping until `[12 GB, 63 GB)` was fragmented with fenceposts. |

After the translator changes, the pre-translated runtime image no longer matches it and is rebuilt
(`x86sim aot update`, which `setup-vm.sh` runs after patching).

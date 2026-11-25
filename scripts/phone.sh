#!/usr/bin/env bash
# Start, stop or wait for an x86Sim phone with agent-sim's settings.
#
#   scripts/phone.sh start [DEVICE]    boot DEVICE (default p) in the background
#   scripts/phone.sh wait [DEVICE]     block until its home screen is up (prints the seconds it took)
#   scripts/phone.sh stop [DEVICE]
#   scripts/phone.sh status [DEVICE]
#
# Settings, each overridable from the environment (docs/BENCHMARKS.md has the measurements):
#   X86SIM_METAL=backboardd,iconservicesagent
#                             Metal on Mesa's CPU Vulkan driver (lavapipe) for these processes: backboardd draws the
#                             screen (smoother scrolling than software drawing); iconservicesagent renders icons with
#                             Core Image, which has no context without a Metal device (the agent crashed on every
#                             Settings row icon and left them gray)
#   LP_NUM_THREADS            lavapipe's threads: two fewer than the CPUs, 2 to 6
#   A64_JOBS=slim             x86Sim's smaller list of background services
#   AGENT_SIM_SERVICES=apps   which services to add to that list (see APPS_JOBS below; =lean for the minimum)
#   AGENT_SIM_EXTRA_JOBS      launchd labels to run on top of that list instead (space-separated). The lean set adds back
#                             what slim leaves out but the phone needs: sleepd (without it SpringBoard retries
#                             reaching it about 780 times a second and keeps more than a core busy at rest), and the
#                             services Photos, Wallet and Watch need to open (assetsd, passd, financed,
#                             nanoregistryd, accounts, media).
#   AGENT_SIM_HOOKS=1         load agent-sim's hooks (hooks/prebuilt) into SpringBoard
#   AGENT_SIM_KSM=1           share identical memory pages with other phones (KSM, when the kernel's is on)
#   X86SIM_IO_MAX=0           x86Sim's desktop throttle off: by default it runs the phone in a scope with disk reads
#                             and writes capped at 40 MB/s and low CPU and IO weight, which on a server only slows
#                             cold boots (and moves the phone out of this unit's accounting)
# The frame ring lives under x86Sim's cache directory (~/.cache/x86sim) rather than /run/user: x86Sim's container does not always see the host's
# /run/user (no login session on a server), and then no frame ever reaches the host.
set -Eeuo pipefail

ACTION=${1:?usage: phone.sh start|wait|stop|status [DEVICE]}
DEVICE=${2:-${DEVICE:-p}}
ROOT=${AGENT_SIM_HOME:-$HOME/.local/share/agent-sim}
if [ -z "${X86SIM_VERSION:-}" ]; then
	# the default release, else the newest one installed (a machine set up before the default moved keeps working
	# until setup runs again)
	X86SIM_VERSION=0.5.3
	if [ ! -x "$ROOT/x86sim-$X86SIM_VERSION/AppRun" ]; then
		newest=$(find "$ROOT" -maxdepth 2 -path "$ROOT/x86sim-*/AppRun" 2>/dev/null | sed 's|.*/x86sim-||; s|/AppRun$||' | sort -V | tail -1)
		[ -n "$newest" ] && X86SIM_VERSION=$newest
	fi
fi
X=$ROOT/x86sim-$X86SIM_VERSION/AppRun
# x86Sim's data and cache directories, as its AppRun picks them
DATA=${X86SIM_DATA:-${XDG_DATA_HOME:-$HOME/.local/share}/x86sim}
CACHE=${X86SIM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/x86sim}
DIR=$DATA/var/device-a64-$DEVICE
RINGS=$CACHE/rings
RING=$RINGS/x86sim-a64-$DEVICE-simfb.ring
UNIT=agent-sim-phone-$DEVICE

[ -x "$X" ] || { echo "phone.sh: x86Sim is not installed; run scripts/setup-vm.sh" >&2; exit 1; }
[ -d "$DIR" ] || { echo "phone.sh: no device $DEVICE at $DIR" >&2; exit 1; }
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
export DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}

stop_phone() {
	systemctl --user stop "$UNIT" 2>/dev/null || true
	"$X" shell -c "A64_DEVICE=$DIR A64_STOP_ONLY=1 cli/libexec/arm64/boot-a64.sh" >/dev/null 2>&1 || true
	systemctl --user reset-failed "$UNIT" 2>/dev/null || true
}

# Whether x86Sim's own settings file (x86sim.env, or $X86SIM_CONFIG) sets KEY. x86Sim reads that file when the phone
# boots and lets the environment override it, so agent-sim's default for KEY is passed only when the file is silent.
x86sim_config_sets() {
	local cfg=${X86SIM_CONFIG:-$DATA/x86sim.env}
	[ -f "$cfg" ] && grep -Eq "^[[:space:]]*(export[[:space:]]+)?$1=" "$cfg"
}

# Per-program environment that x86Sim applies as each phone process starts (launchd jobs and XPC services do not get
# the boot environment). WebKit's web content process reserves a 16 GB "Gigacage" at startup; under x86Sim that
# reservation fails and every page crashes ("Could not allocate gigacage memory"). WebKit's own switch turns it off.
PENV=$CACHE/a64-penv
LEAN_JOBS="com.apple.sleepd com.apple.assetsd com.apple.passd com.apple.financed com.apple.nanoregistryd
	com.apple.accountsd com.apple.amsaccountsd com.apple.mediaremoted com.apple.sharingd com.apple.medialibraryd"
# AGENT_SIM_SERVICES=apps (the default): also every service x86Sim's standard list has that only starts when asked
# for (Spotlight, notifications, pasteboard, contacts, calendars, StoreKit, Siri's, StatusKit...) and locationd, so
# apps under test find what they find on a device (docs/BENCHMARKS.md has the cost: a few seconds of boot, about
# 300 MB and 0.2 cores at rest). =lean: only the services the built-in apps need to open.
APPS_JOBS="
	com.apple.assetsubscriptiond com.apple.assistant_cdmd
	com.apple.AuthenticationServicesCore.AuthenticationServicesAgent com.apple.backgroundassets.user
	com.apple.calaccessd com.apple.campo com.apple.contactsd com.apple.CoreAuthentication.daemon
	com.apple.corespeechd com.apple.corespotlightservice com.apple.ctkd com.apple.DesktopServicesHelper
	com.apple.devicecheckd com.apple.devicemanagementclient.managedappsd com.apple.dmd com.apple.DragUI.druid
	com.apple.FileProvider com.apple.finhealthd com.apple.GameController.gamecontrollerd com.apple.gamed
	com.apple.gamesaved com.apple.geod com.apple.healthappd com.apple.homed com.apple.installcoordinationd
	com.apple.linkd com.apple.liveactivitiesd com.apple.LocalAuthenticationUIService com.apple.locationd
	com.apple.managedappdistributiond com.apple.managedconfiguration.profiled com.apple.Maps.mapssyncd
	com.apple.mediaanalysisd com.apple.mediaanalysisd.service com.apple.midiserver-sim com.apple.mobileassetd
	com.apple.nanotimekitcompaniond com.apple.pasteboard.pasted com.apple.PointerUI.pointeruid
	com.apple.PosterBoard com.apple.quicklook.ThumbnailsAgent com.apple.ScreenTimeAgent com.apple.searchd
	com.apple.sharinguiservice com.apple.shazamd com.apple.siriactionsd com.apple.siriinferenced
	com.apple.siriknowledged com.apple.sirittsd com.apple.StatusKitAgent com.apple.storekitd
	com.apple.syncdefaultsd com.apple.telephonyutilities.callservicesd com.apple.translationd
	com.apple.UsageTrackingAgent com.apple.usernotificationsd com.apple.videosubscriptionsd com.apple.voiced"
case ${AGENT_SIM_SERVICES:-apps} in
lean) DEFAULT_EXTRA_JOBS=$LEAN_JOBS ;;
*) DEFAULT_EXTRA_JOBS="$LEAN_JOBS $APPS_JOBS" ;;
esac
DEFAULT_EXTRA_JOBS=$(echo $DEFAULT_EXTRA_JOBS)
# agent-sim's own hooks (hooks/, built for the simulator), on top of the libraries x86Sim inserts into every process:
# the accessibility reader in every process (inert until UIKit is loaded and `agent-sim snapshot` asks), and in
# SpringBoard also the one that dismisses a permission prompt whose app has quit (docs/KNOWN_ISSUES.md).
HOOKS=$(cd "$(dirname "${BASH_SOURCE[0]}")/../hooks/prebuilt" 2>/dev/null && pwd || true)
ensure_penv() {
	mkdir -p "$(dirname "$PENV")"
	touch "$PENV"
	grep -qs '^com.apple.WebKit.WebContent .*GIGACAGE_ENABLED=0' "$PENV" ||
		echo "com.apple.WebKit.WebContent GIGACAGE_ENABLED=0" >>"$PENV"
	local keep base h log=""
	keep=$(grep -v 'libagentsim-' "$PENV" || true)
	printf '%s\n' "$keep" | sed '/^$/d' >"$PENV"
	[ "${AGENT_SIM_HOOKS:-1}" = 1 ] && [ -f "$HOOKS/libagentsim-ax.dylib" ] || return 0
	case $HOOKS in *[[:space:]]*) echo "phone.sh: hooks skipped: their path has a space ($HOOKS)" >&2; return 0 ;; esac
	base=$(sed -n 's/^DYLD_INSERT_LIBRARIES=//p' "$DIR/sim.env")
	h=/Volumes/SystemRoot$HOOKS
	[ -n "${AGENT_SIM_HOOK_LOG:-}" ] && log=" AGENT_SIM_HOOK_LOG=$AGENT_SIM_HOOK_LOG"
	# a later line wins for a program it names: SpringBoard's comes after the one for every program
	echo "* DYLD_INSERT_LIBRARIES=${base:+$base:}$h/libagentsim-ax.dylib" >>"$PENV"
	# AGENT_SIM_SB_EXTRA: more libraries (Darling paths, ':'-separated) and settings for SpringBoard, for debugging
	echo "SpringBoard DYLD_INSERT_LIBRARIES=${base:+$base:}$h/libagentsim-ax.dylib:$h/libagentsim-springboard.dylib${AGENT_SIM_SB_EXTRA:+:$AGENT_SIM_SB_EXTRA}$log" >>"$PENV"
}

# Darling's prefix (the container's root overlay, a cache) is made for one of x86Sim's two modes: one from the
# user-namespace mode is owned by the user, and system-mode Darling refuses it ("workdir/private must be a directory
# that only root can use"). A prefix from the other mode is removed; Darling makes a new one. With
# X86SIM_DARLING_MODE=userns, x86Sim runs user-namespace mode even when the system install exists, and the prefix is
# the right one.
fix_prefix() {
	local pre=$CACHE/a64prefix-$DEVICE
	[ -e /opt/x86sim/darling/.x86sim-complete ] || return 0
	[ "${X86SIM_DARLING_MODE:-auto}" != userns ] || return 0
	[ -d "$pre.workdir/private" ] && [ "$(stat -c %u "$pre.workdir/private")" != 0 ] || return 0
	echo "phone.sh: $DEVICE's Darling prefix is from x86Sim's user-namespace mode; making a new one" >&2
	rm -rf "$pre" "$pre.workdir" 2>/dev/null || sudo -n rm -rf "$pre" "$pre.workdir"
}

# The RuntimeRoot of the runtime this phone boots: <device>/runtime names its runtimes/ directory (x86Sim's device
# create writes it). A phone without one boots x86Sim's default runtime: the first one installed is taken.
runtime_root() {
	local rr
	if [ -s "$DIR/runtime" ]; then
		rr=$DATA/runtimes/$(<"$DIR/runtime")/RuntimeRoot
		[ -x "$rr/usr/bin/defaults" ] && echo "$rr"
		return
	fi
	for rr in "$DATA"/runtimes/*/RuntimeRoot; do
		[ -x "$rr/usr/bin/defaults" ] && { echo "$rr"; return; }
	done
	return 1
}

# `agent-sim snapshot` reads what UIKit describes, and UIKit describes controls only while app accessibility is on
# (setup turns it on, as Accessibility Inspector does in Apple's simulator). On a phone's first boot a preferences
# migration can reset both keys to 0 after setup wrote them, and every snapshot is then empty (issue #8). Each boot
# checks them and writes them back; a running phone takes the change at once.
ensure_accessibility() {
	local rr cur key off=0
	rr=$(runtime_root) || return 0
	cur=$(cd "$DATA" && X86SIM_DEVICE=$DIR "$X" x86sim spawn "$rr/usr/bin/defaults" read com.apple.Accessibility 2>/dev/null) || true
	for key in ApplicationAccessibilityEnabled AccessibilityEnabled; do
		grep -q "^ *$key = 1;" <<<"$cur" || off=1
	done
	[ "$off" = 1 ] || return 0
	for key in ApplicationAccessibilityEnabled AccessibilityEnabled; do
		(cd "$DATA" && X86SIM_DEVICE=$DIR "$X" x86sim spawn "$rr/usr/bin/defaults" write com.apple.Accessibility "$key" -bool true >/dev/null 2>&1) || true
	done
	echo "phone.sh: $DEVICE: turned on app accessibility (it was off)" >&2
}

case $ACTION in
start)
	stop_phone
	fix_prefix
	ensure_penv
	mkdir -p "$RINGS"
	# a ring left from the last boot would look like a home screen already drawn
	rm -f "$RING" "$RING".* "$DIR/data/framebuffer.bgra"
	# answers the hooks wrote for the last boot's processes
	rm -rf "$DIR/data/tmp/agent-sim"
	# x86Sim's, lavapipe's and Vulkan's own settings from this environment reach the phone too
	passthrough=()
	while IFS='=' read -r name _; do
		case $name in
		X86SIM_METAL | LP_NUM_THREADS | A64_JOBS | VK_ICD_FILENAMES | SIMFB_RING | X86SIM_IO_MAX) ;;
		X86SIM_* | A64_* | LP_* | VK_* | MESA_* | SIMFB_* | DSERVER_*) passthrough+=(--setenv="$name") ;;
		esac
	done < <(env)
	# x86Sim's desktop app shows a phone from /run/user/<uid>/<ring name> only. Where the app is in the app menu (a
	# desktop: setup-vm.sh, scripts/desktop-entry.sh), boot-a64.sh links that path to this ring, so the app shows a phone
	# agent-sim started. Elsewhere (servers, headless VMs) the ring stays only where it is.
	if [ -z "${SIMFB_RING_LINUX+x}" ] && [ -f "${XDG_DATA_HOME:-$HOME/.local/share}/applications/x86sim.desktop" ] &&
		[ -d "/run/user/$UID" ] && [ -w "/run/user/$UID" ]; then
		passthrough+=(--setenv=SIMFB_RING_LINUX="/run/user/$UID/${RING##*/}")
	fi
	# let the kernel merge this phone's memory pages with identical ones of other phones (KSM; takes effect while
	# /sys/kernel/mm/ksm/run is 1, which pool.sh sets for two or more phones; AGENT_SIM_KSM=0 opts out)
	# Needs Linux 6.4+ (per-process merging); on older kernels systemd refuses to start the unit, so probe first.
	if [ "${AGENT_SIM_KSM:-1}" = 1 ] && [ "$(cat /sys/kernel/mm/ksm/run 2>/dev/null)" = 1 ] &&
		systemd-run --user --quiet --wait --collect --property=MemoryKSM=yes true 2>/dev/null; then
		passthrough+=(--property=MemoryKSM=yes)
	fi
	# Default to lavapipe on headless VMs when it is installed; otherwise let the Vulkan loader pick a host GPU ICD.
	# A driver chosen in x86sim.env (a GPU on a desktop) is x86Sim's to apply.
	if [ -n "${VK_ICD_FILENAMES:-}" ]; then
		passthrough+=(--setenv=VK_ICD_FILENAMES="$VK_ICD_FILENAMES")
	elif ! x86sim_config_sets VK_ICD_FILENAMES && [ -f /usr/share/vulkan/icd.d/lvp_icd.json ]; then
		passthrough+=(--setenv=VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json)
	fi
	# Which programs draw through Metal (on Vulkan): agent-sim's measured default unless set here or in x86sim.env
	if [ -n "${X86SIM_METAL:-}" ]; then
		passthrough+=(--setenv=X86SIM_METAL="$X86SIM_METAL")
	elif ! x86sim_config_sets X86SIM_METAL; then
		passthrough+=(--setenv=X86SIM_METAL=backboardd,iconservicesagent)
	fi
	systemd-run --user --quiet --unit="$UNIT" --collect "${passthrough[@]}" \
		--setenv=SIMFB_RING="/Volumes/SystemRoot$RING" \
		--setenv=LP_NUM_THREADS="${LP_NUM_THREADS:-$(n=$(($(nproc) - 2)); echo $((n < 2 ? 2 : n > 6 ? 6 : n)))}" \
		--setenv=A64_JOBS="${A64_JOBS:-slim}" \
		--setenv=X86SIM_IO_MAX="${X86SIM_IO_MAX:-0}" \
		"$X" shell -c "A64_DEVICE=$DIR A64_KEEP=1 SECS=7200 cli/libexec/arm64/boot-a64.sh ${AGENT_SIM_EXTRA_JOBS-$DEFAULT_EXTRA_JOBS}"
	echo "phone.sh: starting $DEVICE (journalctl --user -u $UNIT)"
	;;
wait)
	# A boot can stall for good: the screen stays on the Apple logo, SpringBoard waits on a service that never answers
	# and the phone's processes use no CPU (seen on the first boot after a VM restart, and on a new phone). A stall
	# (not home yet, under a second of CPU in 30 s) restarts the phone once.
	start=$(date +%s)
	restarted=0
	prev=""
	while :; do
		ring_wait=$(date +%s)
		until [ -e "$RING" ]; do
			systemctl --user is-active --quiet "$UNIT" || { echo "phone.sh: $DEVICE is not running" >&2; exit 1; }
			# no first frame in 3 minutes: Darling never came up
			if [ $(($(date +%s) - ring_wait)) -gt 180 ]; then
				[ "$restarted" = 0 ] || { echo "phone.sh: $DEVICE: Darling did not start" >&2; exit 1; }
				echo "phone.sh: $DEVICE shows no frame after 3 minutes; restarting it" >&2
				"$0" start "$DEVICE" >/dev/null
				restarted=1
				ring_wait=$(date +%s)
			fi
			sleep 0.5
		done
		if X86SIM_DEVICE=$DIR SIMFB_RING_LINUX=$RING "$X" x86sim wait home 30 >/dev/null 2>&1; then
			echo "phone.sh: $DEVICE home screen after $(($(date +%s) - start)) s"
			ensure_accessibility
			exit 0
		fi
		systemctl --user is-active --quiet "$UNIT" || { echo "phone.sh: $DEVICE stopped while booting" >&2; exit 1; }
		[ $(($(date +%s) - start)) -lt "${TIMEOUT:-900}" ] || { echo "phone.sh: $DEVICE: no home screen after ${TIMEOUT:-900} s" >&2; exit 1; }
		cpu=$(systemctl --user show -p CPUUsageNSec --value "$UNIT" 2>/dev/null || true)
		cpu=${cpu//[^0-9]/}
		cpu=${cpu:-0}
		if [ -n "$prev" ] && [ "$cpu" -gt 0 ] && [ $((cpu - prev)) -lt 1000000000 ] && [ "$restarted" = 0 ]; then
			echo "phone.sh: $DEVICE stalled while booting (no CPU used for 30 s); restarting it" >&2
			"$0" start "$DEVICE" >/dev/null
			restarted=1
			prev=""
			continue
		fi
		prev=$cpu
	done
	;;
stop)
	stop_phone
	;;
status)
	systemctl --user is-active "$UNIT" || true
	;;
*)
	echo "usage: phone.sh start|wait|stop|status [DEVICE]" >&2
	exit 2
	;;
esac

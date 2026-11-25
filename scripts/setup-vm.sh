#!/usr/bin/env bash
# Turn a fresh Linux x86-64 machine into one that runs an iOS phone with x86Sim, set up the way agent-sim
# measured fastest. Re-runnable: each step skips what is already done.
#
#   scripts/setup-vm.sh
#
# Environment:
#   X86SIM_VERSION   x86Sim release (default 0.5.3)
#   RUNTIME_BUILD    iOS runtime build from Apple's catalogue (default 24B8117, iOS 27.2); the phone is created on
#                    it, and a phone already made on another runtime needs another DEVICE
#   DEVICE, MODEL    the phone to create (default p, "iPhone 17 Pro (at 2x)")
#   FIDELITY=high    a phone that looks exactly like Apple's simulator: the model's own pixel density ("iPhone 17 Pro",
#                    3x) and every visual effect (blur, transparency, motion). The default, fast, draws at 2x with
#                    Reduce Transparency and Reduce Motion on: it scrolls faster (docs/BENCHMARKS.md).
#   AGENT_SIM_PATCH_X86SIM=0   keep x86Sim's own translator (skips the fixes in patches/x86sim)
#
# Apple's iOS runtime is downloaded from Apple onto this machine (about 8 GB) and never leaves it.
set -Eeuo pipefail

X86SIM_VERSION=${X86SIM_VERSION:-0.5.3}
RUNTIME_BUILD=${RUNTIME_BUILD:-24B8117}
DEVICE=${DEVICE:-p}
FIDELITY=${FIDELITY:-fast}
case $FIDELITY in
high) MODEL=${MODEL:-"iPhone 17 Pro"} ;;
fast) MODEL=${MODEL:-"iPhone 17 Pro (at 2x)"} ;;
*) echo "setup-vm.sh: FIDELITY is fast or high" >&2; exit 2 ;;
esac
ROOT=${AGENT_SIM_HOME:-$HOME/.local/share/agent-sim}
X=$ROOT/x86sim-$X86SIM_VERSION/AppRun
DATA=${XDG_DATA_HOME:-$HOME/.local/share}/x86sim
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'setup-vm.sh: %s\n' "$*" >&2; exit 1; }

log "Checking the machine"
[ "$(uname -m)" = x86_64 ] || die "x86Sim needs an x86-64 machine"
# awk reads all of ldd's output: with head, ldd could die of SIGPIPE and pipefail stopped the script silently
glibc=$(ldd --version 2>/dev/null | awk 'NR == 1' | grep -oE '[0-9]+\.[0-9]+$')
awk "BEGIN{exit !($glibc >= 2.39)}" || die "x86Sim needs glibc 2.39 or newer (Ubuntu 24.04+), found $glibc"
free_gb=$(df -BG --output=avail "$HOME" | tail -1 | tr -dc 0-9)
[ "$free_gb" -ge 45 ] || [ -d "$DATA/runtimes" ] || die "need 45 GB free in $HOME for the runtime, have ${free_gb} GB"
[ "$(nproc)" -ge 4 ] || echo "warning: fewer than 4 CPUs; the phone will be slow"

have_vulkan_icd() {
	if [ -n "${VK_ICD_FILENAMES:-}" ]; then
		local icd
		IFS=: read -ra icds <<<"$VK_ICD_FILENAMES"
		for icd in "${icds[@]}"; do [ -f "$icd" ] && return 0; done
	fi
	compgen -G "/usr/share/vulkan/icd.d/*.json" >/dev/null || compgen -G "/etc/vulkan/icd.d/*.json" >/dev/null
}

have_runtime_packages() {
	for bin in python3 curl openssl jq ffmpeg; do
		command -v "$bin" >/dev/null || return 1
	done
	python3 -c "import PIL, dns" 2>/dev/null || return 1
	have_vulkan_icd
}

install_runtime_packages() {
	if have_runtime_packages; then
		echo "all required packages are already installed"
		return 0
	fi
	if command -v apt-get >/dev/null; then
		sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
		sudo DEBIAN_FRONTEND=noninteractive apt-get install -yqq \
			python3 python3-pil python3-dnspython curl openssl jq mesa-vulkan-drivers ffmpeg >/dev/null
	elif command -v pacman >/dev/null; then
		local pkgs=(python python-pillow python-dnspython curl openssl jq vulkan-swrast ffmpeg)
		# Arch ships libxml2.so.16, while x86Sim 0.5.2 links libxml2.so.2
		ldconfig -p 2>/dev/null | grep -q "libxml2\.so\.2" || pkgs+=(libxml2-legacy)
		sudo pacman -S --needed --noconfirm "${pkgs[@]}" >/dev/null
	elif command -v dnf >/dev/null; then
		sudo dnf install -y -q \
			python3 python3-pillow python3-dns curl openssl jq mesa-vulkan-drivers ffmpeg >/dev/null
	else
		die "missing packages (need python3 with PIL and dnspython, curl, openssl, jq, ffmpeg and a Vulkan ICD) and no supported package manager (apt-get, pacman, dnf) found"
	fi
}

log "Installing packages"
install_runtime_packages

log "Keeping a user session for x86Sim's sockets and services"
# x86Sim puts sockets under /run/user/<uid> and runs phones as user services; without a login session (a server
# reached over SSH or an API) neither exists unless the user lingers.
if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null || true)" != yes ]; then
	sudo loginctl enable-linger "$USER"
fi
for _ in $(seq 20); do [ -S "/run/user/$(id -u)/bus" ] && break; sleep 0.5; done
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus

if [ ! -x "$X" ]; then
	log "Downloading x86Sim $X86SIM_VERSION"
	tmp=$(mktemp -d)
	progress=-sS
	[ -t 1 ] && progress=--progress-bar
	curl -fL "$progress" -o "$tmp/x86sim.AppImage" \
		"https://github.com/x86sim/x86sim/releases/download/v$X86SIM_VERSION/x86sim-$X86SIM_VERSION-x86_64.AppImage"
	chmod +x "$tmp/x86sim.AppImage"
	# unpacked once, so it runs without FUSE
	(cd "$tmp" && ./x86sim.AppImage --appimage-extract >/dev/null)
	mkdir -p "$ROOT"
	rm -rf "$ROOT/x86sim-$X86SIM_VERSION"
	mv "$tmp/squashfs-root" "$ROOT/x86sim-$X86SIM_VERSION"
	rm -rf "$tmp"
fi
"$X" version

if [ "${AGENT_SIM_PATCH_X86SIM:-1}" = 1 ]; then
	log "Building x86Sim's translator with agent-sim's fixes (patches/x86sim; web workers in Safari)"
	# before pre-translating: the pre-translated image is tied to the translator build
	"$HERE/patch-x86sim.sh"
fi

# x86Sim runs Darling either from a root-owned install under /opt/x86sim ("system" mode) or, when the kernel allows
# unprivileged user namespaces, through a namespace holder process ("userns" mode). In userns mode the first boot
# after the holder is (re)created stalls (x86Sim's docs/RELEASE.md), which happened after every VM restart; system
# mode has no holder. x86Sim picks system mode whenever this install is present. After patching, so it carries the
# patched translator.
S=$ROOT/x86sim-$X86SIM_VERSION/usr/share/x86sim
if [ "$(cat /opt/x86sim/darling/.x86sim-complete 2>/dev/null)" != "$(cat "$S/DARLING_ID")" ] &&
	sudo -n true 2>/dev/null; then
	log "Installing Darling for x86Sim's system mode (/opt/x86sim, root-owned)"
	sudo sh "$S/install-helper.sh" install "$S/darling" "$(cat "$S/DARLING_ID")"
	# a holder left from userns mode would keep being used by phones started before
	for f in "$HOME"/.cache/x86sim/userns-*.pid; do [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null; rm -f "$f"; done
fi

fetched=$DATA/runtimes/.agent-sim-fetched-$RUNTIME_BUILD
if [ ! -e "$fetched" ]; then
	log "Downloading the iOS runtime $RUNTIME_BUILD from Apple (about 8 GB)"
	# Apple's CDN drops long downloads now and then; the fetch resumes where it stopped
	for attempt in 1 2 3 4 5; do
		if "$X" shell -c "python3 cli/libexec/arm64/runtime-fetch.py fetch $RUNTIME_BUILD"; then
			touch "$fetched"
			break
		fi
		[ "$attempt" = 5 ] && die "the runtime download failed 5 times"
		echo "download interrupted; resuming (attempt $((attempt + 1)) of 5)"
		sleep 5
	done
fi

log "Adding iPhone 17 Pro (at 2x): the iPhone 17 Pro's layout at 44% of the pixels"
types=$(ls "$DATA"/app/"$X86SIM_VERSION"*/cli/lib/devicetypes-a64.json | head -1)
python3 - "$types" <<'PY'
import json, sys
path = sys.argv[1]
types = json.load(open(path))
if "iPhone 17 Pro (at 2x)" not in types:
    t = dict(types["iPhone 17 Pro"])
    # the same 402x874 points: 2x instead of 3x, and the pitch with it
    t.update(w="804", h="1748", scale="2.000000", pitch="306.666667")
    types["iPhone 17 Pro (at 2x)"] = t
    json.dump(types, open(path, "w"), indent=1)
PY

DIR=$DATA/var/device-a64-$DEVICE
if [ ! -d "$DIR" ]; then
	log "Creating the phone $DEVICE ($MODEL)"
	# on RUNTIME_BUILD: without --ios, x86Sim takes its default runtime whatever RUNTIME_BUILD says
	"$X" x86sim device create --model "$MODEL" --ios "$RUNTIME_BUILD" "$DEVICE"
elif [ -s "$DIR/runtime" ] && [[ $(<"$DIR/runtime") != *-"$RUNTIME_BUILD"-arm64 ]]; then
	die "$DEVICE runs $(<"$DIR/runtime"), not $RUNTIME_BUILD: pick another DEVICE for this runtime"
fi

aot_status() {
	local s
	s=$("$X" aot --device "$DATA/var/device-a64-$DEVICE" status 2>/dev/null) || true
	grep -m1 '^image ' <<<"$s" || true
}
trained=$DIR/.agent-sim-trained
calm=$DIR/.agent-sim-settings-2
RUNTIME_ROOT=$(ls -d "$DATA"/runtimes/*-"$RUNTIME_BUILD"-arm64/extracted/Library/Developer/CoreSimulator/Profiles/Runtimes/*.simruntime/Contents/Resources/RuntimeRoot | head -1)
# Reduce Transparency and Reduce Motion (fast only): blurs and long animations are the costliest things to draw on a
# CPU. And app accessibility, as Accessibility Inspector turns it on in Apple's simulator: UIKit then describes every
# control, which `agent-sim snapshot` reads.
reduce_effects() {
	local reduce=true
	[ "$FIDELITY" = high ] && reduce=false
	for key in ReduceMotionEnabled EnhancedBackgroundContrastEnabled; do
		(cd "$DATA" && X86SIM_DEVICE=$DIR "$X" x86sim spawn "$RUNTIME_ROOT/usr/bin/defaults" \
			write com.apple.Accessibility "$key" -bool $reduce >/dev/null)
	done
	for key in ApplicationAccessibilityEnabled AccessibilityEnabled; do
		(cd "$DATA" && X86SIM_DEVICE=$DIR "$X" x86sim spawn "$RUNTIME_ROOT/usr/bin/defaults" \
			write com.apple.Accessibility "$key" -bool true >/dev/null)
	done
	touch "$calm"
}
granted=$DIR/.agent-sim-granted
# The built-in apps get their permissions up front, as on a phone that has been set up: otherwise their prompts
# (location for Maps, contacts for Wallet...) show up, and x86Sim leaves them on screen over other apps. Apps you
# install still ask.
grant_builtin() {
	local apps
	apps=$(python3 - "$DIR/data/Library/SpringBoard/IconState.plist" <<'PY'
import plistlib, sys
out = []
def walk(v):
    if isinstance(v, str) and v.startswith("com.apple.") and v not in out:
        out.append(v)
    elif isinstance(v, (list, tuple)):
        for i in v: walk(i)
    elif isinstance(v, dict):
        for i in v.values(): walk(i)
walk(plistlib.load(open(sys.argv[1], "rb")))
print(" ".join(out))
PY
)
	for app in $apps; do
		(cd "$DATA" && X86SIM_DEVICE=$DIR "$X" x86sim privacy grant all "$app" >/dev/null 2>&1) || true
	done
	touch "$granted"
}
if [[ $(aot_status) != *"(current)"* ]] || [ ! -e "$calm" ] || [ ! -e "$granted" ]; then
	if [ ! -e "$trained" ] || [ ! -e "$calm" ] || [ ! -e "$granted" ]; then
		log "Booting the phone once: it learns what runs, gets Reduce Transparency and Motion, and its apps' permissions"
		"$HERE/phone.sh" start "$DEVICE"
		TIMEOUT=1800 "$HERE/phone.sh" wait "$DEVICE"
		reduce_effects
		grant_builtin
		# a first phone runs a while so the pre-translation learns what it uses; later phones reuse that image
		[ -e "$trained" ] || [[ $(aot_status) == *"(current)"* ]] || sleep 120
		"$HERE/phone.sh" stop "$DEVICE"
		touch "$trained"
	fi
	log "Pre-translating the runtime's code, so apps do not translate it as they run"
	# the first pass can make more code worth translating: repeat until the image is current
	# a failed pass leaves the image as it was (still used): a warning, not the end of the setup
	for _ in 1 2 3; do
		"$X" aot --device "$DATA/var/device-a64-$DEVICE" update ||
			echo "setup-vm.sh: a pre-translation pass failed; the phone runs on the image as it is" >&2
		[[ $(aot_status) == *"(current)"* ]] && break
	done
	aot_status
fi

log "Done. Start the phone with:  scripts/phone.sh start $DEVICE && scripts/phone.sh wait $DEVICE"

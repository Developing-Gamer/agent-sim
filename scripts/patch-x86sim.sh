#!/usr/bin/env bash
# Rebuild x86Sim's ARM translator (libarm64jit.so) with the fixes in patches/x86sim and install it into the unpacked
# x86Sim. Re-runnable; the original library is kept next to it as libarm64jit.so.orig.
#
#   scripts/patch-x86sim.sh           build and install
#   scripts/patch-x86sim.sh restore   put x86Sim's own library back
#
# The patches change x86Sim source and are under x86Sim's license (GPL-3.0-or-later); they are meant for upstream.
# The build fetches x86Sim's tagged source and dynarmic from GitHub, needs about 2 GB and takes 5-10 minutes.
set -Eeuo pipefail

X86SIM_VERSION=${X86SIM_VERSION:-0.5.3}
ROOT=${AGENT_SIM_HOME:-$HOME/.local/share/agent-sim}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PATCHES=$HERE/../patches/x86sim
SRC=$ROOT/src/x86sim-$X86SIM_VERSION
BUILD=$ROOT/build/arm64jit-$X86SIM_VERSION
DATA=${XDG_DATA_HOME:-$HOME/.local/share}/x86sim
# the unpacked AppImage, the copy x86Sim installs under its data directory on first run (userns mode), and the
# root-owned system-mode install under /opt/x86sim (the one phones load when it exists, unless
# X86SIM_DARLING_MODE=userns: then it is unused, and patching it would only ask for root)
mapfile -t LIBS < <(find "$ROOT/x86sim-$X86SIM_VERSION" "$DATA"/app/"$X86SIM_VERSION"* -name libarm64jit.so \
	-path '*libexec/darling*' 2>/dev/null)
OPT_LIB=$(readlink -f /opt/x86sim/darling 2>/dev/null || true)/libexec/darling/usr/libexec/darling/libarm64jit.so
if [ "${X86SIM_DARLING_MODE:-auto}" != userns ] && [ -f "$OPT_LIB" ]; then LIBS+=("$OPT_LIB"); fi
[ ${#LIBS[@]} -gt 0 ] || { echo "patch-x86sim.sh: x86Sim $X86SIM_VERSION is not installed; run scripts/setup-vm.sh" >&2; exit 1; }

if [ "${1:-}" = restore ]; then
	for lib in "${LIBS[@]}"; do
		sudo_=""
		[ -w "$(dirname "$lib")" ] || sudo_="sudo"
		[ -e "$lib.orig" ] && $sudo_ cp "$lib.orig" "$lib.new" && $sudo_ mv -f "$lib.new" "$lib"
	done
	exit 0
fi

installed() {  # every copy already holds a build of exactly these patches
	[ -f "$BUILD/patches.sha" ] && [ "$(cat "$PATCHES"/*.patch | sha256sum)" = "$(cat "$BUILD/patches.sha")" ] || return 1
	for lib in "${LIBS[@]}"; do cmp -s "$BUILD/libarm64jit.so" "$lib" || return 1; done
}
if installed; then
	echo "patch-x86sim.sh: the patched translator is already installed"
	exit 0
fi

have_build_packages() {
	for bin in clang clang++ cmake ninja git; do
		command -v "$bin" >/dev/null || return 1
	done
	[ -d /usr/include/boost ]
}

if ! have_build_packages; then
	if command -v apt-get >/dev/null; then
		sudo DEBIAN_FRONTEND=noninteractive apt-get install -yqq clang cmake ninja-build git libboost-dev >/dev/null
	elif command -v pacman >/dev/null; then
		sudo pacman -S --needed --noconfirm clang cmake ninja git boost >/dev/null
	elif command -v dnf >/dev/null; then
		sudo dnf install -y -q clang cmake ninja-build git boost-devel >/dev/null
	else
		echo "patch-x86sim.sh: install clang, cmake, ninja, git and boost headers first" >&2
		exit 1
	fi
fi
if [ ! -d "$SRC/.git" ]; then
	git clone -q --depth 1 --branch "v$X86SIM_VERSION" https://github.com/x86sim/x86sim.git "$SRC"
	# the submodule URLs are SSH; the same repositories over HTTPS need no GitHub key
	git -C "$SRC" config submodule.external/dynarmic.url https://github.com/x86sim/dynarmic.git
	git -C "$SRC" submodule -q update --init --depth 1 external/dynarmic
	git -C "$SRC/external/dynarmic" submodule -q update --init --recursive --depth 1
fi
git -C "$SRC" checkout -q -- .
# Each patch is meant to land upstream. One that x86Sim's source already contains, or no longer matches because
# upstream fixed the same code its own way, is skipped with a note, so setup keeps working on newer releases.
applied=0
for p in "$PATCHES"/*.patch; do
	name=$(basename "$p")
	if git -C "$SRC" apply --check "$p" 2>/dev/null; then
		git -C "$SRC" apply "$p"
		applied=$((applied + 1))
	elif git -C "$SRC" apply --check --reverse "$p" 2>/dev/null; then
		echo "patch-x86sim.sh: x86Sim $X86SIM_VERSION already includes $name; skipping it"
	else
		echo "patch-x86sim.sh: $name does not apply to x86Sim $X86SIM_VERSION (upstream changed that code," \
			"likely fixing it there); skipping it. See patches/x86sim/README.md" >&2
	fi
done
if [ "$applied" = 0 ]; then
	echo "patch-x86sim.sh: x86Sim $X86SIM_VERSION needs none of agent-sim's fixes; using its own translator"
	"$0" restore
	mkdir -p "$BUILD" && cat "$PATCHES"/*.patch | sha256sum >"$BUILD/patches.sha"
	exit 0
fi
[ -f "$BUILD/build.ninja" ] || cmake -S "$SRC/linux/arm64jit" -B "$BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release \
	-DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ >/dev/null
nice ninja -C "$BUILD" -j"$(nproc)" arm64jit >/dev/null
cat "$PATCHES"/*.patch | sha256sum >"$BUILD/patches.sha"
# replaced by rename: running phones keep the library they loaded until they restart
for lib in "${LIBS[@]}"; do
	sudo_=""
	[ -w "$(dirname "$lib")" ] || sudo_="sudo"
	[ -e "$lib.orig" ] || $sudo_ cp "$lib" "$lib.orig"
	$sudo_ cp "$BUILD/libarm64jit.so" "$lib.new" && $sudo_ mv -f "$lib.new" "$lib"
	echo "patch-x86sim.sh: installed the patched translator in $lib"
done

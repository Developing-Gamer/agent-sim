#!/usr/bin/env bash
# A pool of phones kept booted, so an agent gets a home screen at once instead of waiting for a boot.
#
#   scripts/pool.sh up N          create and boot phones pool1..poolN (one at a time; existing ones are kept)
#   scripts/pool.sh take          hold a booted phone nobody holds and print its name (exit 1 if none is free)
#   scripts/pool.sh release NAME  give it back; it restarts in the background, so the next one starts at home
#   scripts/pool.sh status        each phone: running or not, held or free
#   scripts/pool.sh down          stop every pool phone
#
# A new phone is prepared like the first (scripts/setup-vm.sh with DEVICE set), which reuses the runtime and its
# pre-translated code: about a minute each. With two or more, phones share memory through KSM (pool.sh turns it on);
# see docs/BENCHMARKS.md for how many fit.
# Everything phone.sh reads from the environment (LP_NUM_THREADS, AGENT_SIM_EXTRA_JOBS...) applies to each phone.
set -Eeuo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DATA=${X86SIM_DATA:-${XDG_DATA_HOME:-$HOME/.local/share}/x86sim}
HELD=${XDG_CACHE_HOME:-$HOME/.cache}/agent-sim/pool
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
export DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}
mkdir -p "$HELD"

phones() { for d in "$DATA"/var/device-a64-pool*; do [ -d "$d" ] && echo "${d##*/device-a64-}"; done | sort -V; }
running() { [ "$("$HERE/phone.sh" status "$1")" = active ]; }

case ${1:-} in
up)
	n=${2:?usage: pool.sh up N}
	if [ "$n" -ge 2 ] && [ -e /sys/kernel/mm/ksm/run ]; then
		# phones share most of their memory (the same iOS runtime): merge identical pages (docs/BENCHMARKS.md)
		echo 1000 | sudo tee /sys/kernel/mm/ksm/pages_to_scan >/dev/null
		echo 1 | sudo tee /sys/kernel/mm/ksm/run >/dev/null
	fi
	for i in $(seq 1 "$n"); do
		d=pool$i
		if [ ! -e "$DATA/var/device-a64-$d/.agent-sim-granted" ]; then
			echo "pool.sh: preparing $d"
			DEVICE=$d "$HERE/setup-vm.sh" >"$HELD/$d.setup.log" 2>&1 ||
				{ echo "pool.sh: preparing $d failed; see $HELD/$d.setup.log" >&2; exit 1; }
		fi
		running "$d" && continue
		"$HERE/phone.sh" start "$d" >/dev/null
		TIMEOUT=${TIMEOUT:-900} "$HERE/phone.sh" wait "$d"
	done
	;;
take)
	for d in $(phones); do
		running "$d" || continue
		# mkdir is atomic: two agents asking at once never get the same phone
		mkdir "$HELD/$d.held" 2>/dev/null || continue
		echo "$d"
		exit 0
	done
	echo "pool.sh: no free phone; run pool.sh up with a larger N" >&2
	exit 1
	;;
release)
	d=${2:?usage: pool.sh release NAME}
	[ -d "$HELD/$d.held" ] || { echo "pool.sh: $d is not held" >&2; exit 1; }
	# restart it, then free it: a taker never gets a phone that is still booting
	(
		"$HERE/phone.sh" start "$d" >/dev/null && TIMEOUT=900 "$HERE/phone.sh" wait "$d" >/dev/null
		rm -f "$HELD/$d.releasing"
		rmdir "$HELD/$d.held"
	) >/dev/null 2>&1 </dev/null &
	echo $! >"$HELD/$d.releasing"
	disown
	;;
status)
	for d in $(phones); do
		printf '%-8s %-8s %s\n' "$d" "$("$HERE/phone.sh" status "$d")" "$([ -d "$HELD/$d.held" ] && echo held || echo free)"
	done
	;;
down)
	for d in $(phones); do
		# a release still restarting this phone would start it again
		if pid=$(cat "$HELD/$d.releasing" 2>/dev/null); then pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; fi
		rm -f "$HELD/$d.releasing"
		"$HERE/phone.sh" stop "$d"
		rmdir "$HELD/$d.held" 2>/dev/null || true
	done
	;;
*)
	echo "usage: pool.sh up N | take | release NAME | status | down" >&2
	exit 2
	;;
esac

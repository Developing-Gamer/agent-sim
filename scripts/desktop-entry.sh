#!/usr/bin/env bash
# Add x86Sim's desktop app (its launcher: the phones agent-sim made, their screens, settings) to the desktop's app menu,
# or take it out again.
#
#   scripts/desktop-entry.sh [install]   write the menu entry and its icon for the x86Sim agent-sim set up
#   scripts/desktop-entry.sh remove      take them out
#
# setup-vm.sh runs this in a graphical session. The x86Sim AppImage is unpacked (it then runs without FUSE), so the
# AppImage's own ways into the menu (appimaged, AppImageLauncher) do not see it; this installs the entry and icon the
# AppImage ships, as a freedesktop.org desktop entry under $XDG_DATA_HOME, pointed at the unpacked copy. Any desktop
# that follows the spec lists it: GNOME, KDE, Xfce, and launchers such as Walker, rofi, wofi and fuzzel.
# The file is x86sim.desktop, after the window's app id (x86sim), so docks and taskbars match the window to it.
# An x86sim.desktop that agent-sim did not write (x86Sim installed some other way) is left alone.
#
# Environment: X86SIM_VERSION (default: the release phone.sh uses), AGENT_SIM_HOME.
set -Eeuo pipefail

ACTION=${1:-install}
ROOT=${AGENT_SIM_HOME:-$HOME/.local/share/agent-sim}
SHARE=${XDG_DATA_HOME:-$HOME/.local/share}
ENTRY=$SHARE/applications/x86sim.desktop
ICON=$SHARE/icons/hicolor/256x256/apps/x86sim.png
MARK=X-AgentSim-Managed=true

die() { printf 'desktop-entry.sh: %s\n' "$*" >&2; exit 1; }
ours() { [ -f "$ENTRY" ] && grep -qx "$MARK" "$ENTRY"; }

# Let menus that cache entries see the change now rather than at the next login. Both tools are optional.
refresh() {
	command -v update-desktop-database >/dev/null && update-desktop-database -q "$SHARE/applications" 2>/dev/null
	# an icon cache only exists if something made one; a stale one would hide the new icon
	if [ -f "$SHARE/icons/hicolor/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null; then
		gtk-update-icon-cache -q -f -t "$SHARE/icons/hicolor" 2>/dev/null
	fi
	return 0
}

case $ACTION in
install) ;;
remove)
	if ours; then
		rm -f "$ENTRY" "$ICON"
		refresh
		echo "desktop-entry.sh: removed $ENTRY"
	else
		echo "desktop-entry.sh: no x86Sim menu entry from agent-sim to remove"
	fi
	exit 0
	;;
*) die "usage: desktop-entry.sh [install|remove]" ;;
esac

if [ -z "${X86SIM_VERSION:-}" ]; then
	# as phone.sh picks it: the default release, else the newest one installed
	X86SIM_VERSION=0.5.3
	if [ ! -x "$ROOT/x86sim-$X86SIM_VERSION/AppRun" ]; then
		newest=$(find "$ROOT" -maxdepth 2 -path "$ROOT/x86sim-*/AppRun" 2>/dev/null | sed 's|.*/x86sim-||; s|/AppRun$||' | sort -V | tail -1)
		[ -n "$newest" ] && X86SIM_VERSION=$newest
	fi
fi
APP=$ROOT/x86sim-$X86SIM_VERSION
[ -x "$APP/AppRun" ] || die "x86Sim $X86SIM_VERSION is not installed; run scripts/setup-vm.sh"
SRC=$APP/x86sim.desktop
[ -f "$SRC" ] || die "x86Sim $X86SIM_VERSION ships no x86sim.desktop"
src_icon=$APP/usr/share/icons/hicolor/256x256/apps/x86sim.png
[ -f "$src_icon" ] || src_icon=$APP/x86sim.png

if [ -e "$ENTRY" ] && ! ours; then
	echo "desktop-entry.sh: $ENTRY exists and agent-sim did not write it; leaving it as it is"
	exit 0
fi
# Exec takes the path quoted; the spec's escapes inside quotes are not worth carrying for paths that need them
case $APP in *[\"\`\$\\%]*) die "cannot write a menu entry for a path with \", \`, \$, \\ or %: $APP" ;; esac

# The AppImage's own entry, run from the unpacked copy: AppRun with no arguments opens the launcher. TryExec hides the
# entry if this copy goes away. Written and checked in a scratch directory (desktop-file-validate wants the .desktop
# name), then moved in under a name menus ignore and renamed, so a menu never reads half a file.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
awk -v exec="\"$APP/AppRun\"" -v tryexec="$APP/AppRun" -v mark="$MARK" '
	function ours() { print "Exec=" exec; print "TryExec=" tryexec; print mark; done = 1 }
	/^\[/ { if (group == "[Desktop Entry]") ours(); group = $0 }
	group == "[Desktop Entry]" && /^(Exec|TryExec|X-AppImage-[^=]*)=/ { next }
	{ print }
	END { if (!done) ours() }
' "$SRC" >"$work/x86sim.desktop"
if command -v desktop-file-validate >/dev/null && ! out=$(desktop-file-validate "$work/x86sim.desktop" 2>&1); then
	die "the menu entry does not validate: $out"
fi
mkdir -p "$(dirname "$ENTRY")" "$(dirname "$ICON")"
install -m 644 "$src_icon" "$ICON"
install -m 644 "$work/x86sim.desktop" "$ENTRY.new"
mv -f "$ENTRY.new" "$ENTRY"
refresh
echo "desktop-entry.sh: x86Sim $X86SIM_VERSION is in the app menu ($ENTRY)"

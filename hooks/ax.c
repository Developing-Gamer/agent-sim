// The screen as an agent reads it: on SIGUSR2, a UIKit process (an app, or SpringBoard for the home screen, alerts
// and the status bar) writes its visible accessibility elements as JSON to <device data>/tmp/agent-sim/ax-<pid>.json
// (SpringBoard: ax-springboard.json).
// SpringBoard also writes front.json: the app in front and its pid. `agent-sim snapshot` sends the signals and reads
// the files.
//
// Each element: label, value, hint, identifier, traits, frame (screen points), and whether it is enabled. The walk
// follows UIKit's accessibility containers (accessibilityElements / accessibilityElementAtIndex:) and views'
// subviews, skipping hidden ones, as VoiceOver sees the screen.
//
// WebKit's WebContent processes answer too, with the elements of the web pages they draw (ax-<pid>.json, process
// "WebContent").
//
// Loaded into every phone process by scripts/phone.sh; inert until UIKit (or WebKit's page accessibility) is loaded
// and the signal arrives.
#include <objc/runtime.h>
#include <objc/message.h>
#include <mach-o/dyld.h>
#include <dispatch/dispatch.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
#include <dirent.h>
#include <errno.h>
#include <sys/stat.h>

typedef struct { double x, y, w, h; } Rect;
#define SEL_(s) sel_registerName(s)
#define SEND(r, o, s, ...) ((r (*)(id, SEL, ##__VA_ARGS__))objc_msgSend)

static bool responds(id o, const char *s) { return o && SEND(bool, o, "respondsToSelector:", SEL)(o, SEL_("respondsToSelector:"), SEL_(s)); }
static id get(id o, const char *s) { return responds(o, s) ? SEND(id, o, s)(o, SEL_(s)) : nil; }
static bool getb(id o, const char *s) { return responds(o, s) ? SEND(bool, o, s)(o, SEL_(s)) : false; }
static int geti(id o, const char *s) { return responds(o, s) ? SEND(int, o, s)(o, SEL_(s)) : 0; }
static long getl(id o, const char *s) { return responds(o, s) ? SEND(long, o, s)(o, SEL_(s)) : 0; }
static double getd(id o, const char *s) { return responds(o, s) ? SEND(double, o, s)(o, SEL_(s)) : 0; }
static const char *utf8(id s) {
	if (!s || !responds(s, "UTF8String")) return NULL;
	return SEND(const char *, s, "UTF8String")(s, SEL_("UTF8String"));
}
static bool kind(id o, const char *cls) {
	Class c = objc_getClass(cls);
	return o && c && SEND(bool, o, "isKindOfClass:", Class)(o, SEL_("isKindOfClass:"), c);
}
static unsigned long count(id a) { return a ? SEND(unsigned long, a, "count")(a, SEL_("count")) : 0; }
static id at(id a, unsigned long i) { return SEND(id, a, "objectAtIndex:", unsigned long)(a, SEL_("objectAtIndex:"), i); }
static Rect frame_of(id o) {
	if (!responds(o, "accessibilityFrame")) return (Rect){0};
	return SEND(Rect, o, "accessibilityFrame")(o, SEL_("accessibilityFrame"));
}

// a growing JSON buffer
static char *buf;
static size_t len, cap;
static void put(const char *s, size_t n) {
	if (len + n + 1 > cap) {
		cap = (len + n + 1) * 2;
		buf = realloc(buf, cap);
	}
	memcpy(buf + len, s, n);
	len += n;
	buf[len] = 0;
}
static void puts_(const char *s) { put(s, strlen(s)); }
static void putf(const char *fmt, double a, double b, double c, double d) {
	char t[160];
	int n = snprintf(t, sizeof t, fmt, a, b, c, d);
	put(t, n);
}
static void putstr(const char *s) {
	puts_("\"");
	for (const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; p++) {
		char t[8];
		if (*p == '"' || *p == '\\') { t[0] = '\\'; t[1] = *p; put(t, 2); }
		else if (*p == '\n') puts_("\\n");
		else if (*p < 0x20) { snprintf(t, sizeof t, "\\u%04x", *p); puts_(t); }
		else put((const char *)p, 1);
	}
	puts_("\"");
}

// UIAccessibilityTraits bits, by name
static const struct { unsigned long bit; const char *name; } kTraits[] = {
	{1ul << 0, "button"}, {1ul << 1, "link"}, {1ul << 2, "image"}, {1ul << 3, "selected"}, {1ul << 4, "plays-sound"},
	{1ul << 5, "keyboard-key"}, {1ul << 6, "static-text"}, {1ul << 7, "summary"}, {1ul << 8, "not-enabled"},
	{1ul << 9, "updates-frequently"}, {1ul << 10, "search-field"}, {1ul << 12, "adjustable"},
	{1ul << 13, "allows-direct-interaction"}, {1ul << 14, "causes-page-turn"}, {1ul << 16, "header"},
	{1ul << 18, "text-field"},  // UIKit's private text entry trait: native and web text fields, search fields
};

static bool g_status_time_overridden;
static int emitted;
static void emit(id e) {
	Rect f = frame_of(e);
	if (f.w <= 0 || f.h <= 0) return;
	const char *label = utf8(get(e, "accessibilityLabel")), *value = utf8(get(e, "accessibilityValue"));
	const char *hint = utf8(get(e, "accessibilityHint")), *ident = utf8(get(e, "accessibilityIdentifier"));
	unsigned long traits = (unsigned long)getl(e, "accessibilityTraits");
	if (kind(e, "UITextField") || kind(e, "UITextView")) {  // the text itself, and what it is
		if (!value) value = utf8(get(e, "text"));
		if (!label) label = utf8(get(e, "placeholder"));
	}
	if ((kind(e, "STUIStatusBarStringView") || kind(e, "_UIStatusBarStringView")) && (traits & (1ul << 9))) {
		// Status-bar time overrides update the rendered label text while accessibilityLabel keeps wall time.
		const char *shown = utf8(get(e, "text"));
		if (!shown || !*shown) shown = utf8(get(e, "originalText"));
		if (shown && *shown && (g_status_time_overridden || !label || strncmp(label, shown, strlen(shown)) != 0))
			label = shown;
	}
	if (!label && !value && !ident && !(traits & ((1ul << 0) | (1ul << 1) | (1ul << 10) | (1ul << 18)))) return;
	puts_(emitted++ ? ",\n{" : "\n{");
	puts_("\"label\":"); putstr(label);
	if (value) { puts_(",\"value\":"); putstr(value); }
	if (hint) { puts_(",\"hint\":"); putstr(hint); }
	if (ident) { puts_(",\"id\":"); putstr(ident); }
	puts_(",\"class\":"); putstr(object_getClassName(e));
	puts_(",\"traits\":[");
	int k = 0;
	for (size_t i = 0; i < sizeof kTraits / sizeof *kTraits; i++)
		if (traits & kTraits[i].bit) { if (k++) puts_(","); putstr(kTraits[i].name); }
	if (kind(e, "UITextField") || kind(e, "UITextView")) { if (k++) puts_(","); putstr("text-field"); }
	putf("],\"frame\":[%.1f,%.1f,%.1f,%.1f]}", f.x, f.y, f.w, f.h);
}

// the web views on screen (their content is in WebContent's answer)
static Rect g_webviews[16];
static int g_webview_pids[16];
static int g_nwebviews;

static long g_visits;  // objects walked in the current dump (reported with its time)
static double g_sent;  // when the CLI asked (Unix ms, from req-<pid>), to report how long the request waited
static double unix_ms(void) {
	struct timespec t;
	clock_gettime(CLOCK_REALTIME, &t);
	return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}
static void read_sent(const char *path) {
	FILE *f = fopen(path, "r");
	if (!f) return;
	double v = 0;
	if (fscanf(f, "%lf", &v) == 1) g_sent = v;
	fclose(f);
}
static double ms_now(void) {
	struct timespec t;
	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

static void walk(id o, int depth) {
	if (!o || depth > 60 || emitted > 2000) return;
	g_visits++;
	if (getb(o, "accessibilityElementsHidden")) return;
	if (kind(o, "UIView") && (getb(o, "isHidden") || getd(o, "alpha") < 0.01)) return;
	if (g_nwebviews < 16 && kind(o, "WKWebView")) {
		Rect f = frame_of(o);
		if (f.w > 0 && f.h > 0) {
			g_webview_pids[g_nwebviews] = geti(o, "_webProcessIdentifier");
			g_webviews[g_nwebviews++] = f;
		}
	}
	if (getb(o, "isAccessibilityElement")) {
		emit(o);
		return;
	}
	id kids = get(o, "accessibilityElements");
	if (kids && count(kids)) {
		for (unsigned long i = 0, n = count(kids); i < n; i++) walk(at(kids, i), depth + 1);
		return;
	}
	long n = getl(o, "accessibilityElementCount");
	if (n > 0 && n < 100000) {
		for (long i = 0; i < n; i++)
			walk(SEND(id, o, "accessibilityElementAtIndex:", long)(o, SEL_("accessibilityElementAtIndex:"), i), depth + 1);
		return;
	}
	if (kind(o, "UIView")) {
		id subs = get(o, "subviews");
		for (unsigned long i = 0, m = count(subs); i < m; i++) walk(at(subs, i), depth + 1);
	}
}

static char dir[1024];
static void write_file(const char *name, const char *data, size_t n) {
	char path[1200], tmp[1240];
	snprintf(path, sizeof path, "%s/%s", dir, name);
	snprintf(tmp, sizeof tmp, "%s.tmp", path);
	FILE *f = fopen(tmp, "w");
	if (!f) return;
	fwrite(data, 1, n, f);
	fclose(f);
	rename(tmp, path);
}

// SpringBoard: which app is in front (bundle id and pid), or none (the home screen). Foreground system UI services
// (the keyboard's InputUI) are not the app. Skip suspended processes (FBProcessTaskStateSuspended == 3): dismissed
// remote view services such as SafariViewService can stay marked isForeground while suspended and would otherwise
// shadow the host app.
static bool pick_front_app(id app, const char **bundle_out, long *pid_out) {
	id st = app ? get(app, "processState") : nil;
	const char *b = app ? utf8(get(app, "bundleIdentifier")) : NULL;
	if (!st || !getb(st, "isForeground") || getl(st, "taskState") == 3 || !b || strstr(b, "InputUI")) return false;
	*bundle_out = b;
	*pid_out = getl(st, "pid");
	return true;
}

// Find a running-active hosted ExtensionKit modal (such as PhotosPicker.appex) whose RunningBoard
// frontboard.visibility endowment is attached to `host_pid`'s UISceneHosting environment.
static bool find_hosted_modal(long host_pid, long *out_pid, char *out_scene, size_t scenesz) {
	if (host_pid <= 0) return false;
	Class idc = objc_getClass("RBSProcessIdentifier"), hc = objc_getClass("RBSProcessHandle");
	if (!idc || !hc || !responds((id)idc, "identifierWithPid:") || !responds((id)hc, "handleForIdentifier:error:"))
		return false;
	DIR *d = opendir(dir);
	if (!d) return false;
	char pfx_paren[48], pfx_slash[48];
	snprintf(pfx_paren, sizeof pfx_paren, "UIScene:%ld(", host_pid);
	snprintf(pfx_slash, sizeof pfx_slash, "UIScene:%ld/", host_pid);
	size_t lp = strlen(pfx_paren), ls = strlen(pfx_slash);
	long self_pid = (long)getpid();
	bool found = false;
	struct dirent *ent;
	while (!found && (ent = readdir(d)) != NULL) {
		if (strncmp(ent->d_name, "hooked-", 7) != 0) continue;
		long pid = atol(ent->d_name + 7);
		if (pid <= 0 || pid == host_pid || pid == self_pid) continue;
		// Prune dead processes before making synchronous RunningBoard XPC lookups so long-running simulators
		// do not accumulate hundreds of stale hooked-<pid> files and time out write_front().
		if (kill((pid_t)pid, 0) != 0 && errno == ESRCH) {
			char stale[1200];
			snprintf(stale, sizeof stale, "%s/%s", dir, ent->d_name);
			unlink(stale);
			snprintf(stale, sizeof stale, "%s/ax-%ld.json", dir, pid);
			unlink(stale);
			continue;
		}
		id ident = SEND(id, (id)idc, "identifierWithPid:", int)((id)idc, SEL_("identifierWithPid:"), (int)pid);
		if (!ident) continue;
		id err = nil;
		id h = SEND(id, (id)hc, "handleForIdentifier:error:", id, id *)((id)hc, SEL_("handleForIdentifier:error:"), ident, &err);
		if (!h) continue;
		const char *pname = utf8(get(h, "name"));
		if (pname && strstr(pname, "InputUI")) continue;
		id st = get(h, "currentState");
		if (!st || !responds(st, "taskState")) continue;
		unsigned char task = SEND(unsigned char, st, "taskState")(st, SEL_("taskState"));
		if (task != 4) continue;  // RBSTaskStateRunningActive
		id infos = get(st, "endowmentInfos");
		if (infos && !responds(infos, "objectAtIndex:") && responds(infos, "allObjects"))
			infos = get(infos, "allObjects");
		for (unsigned long i = 0, n = count(infos); i < n; i++) {
			id info = at(infos, i);
			const char *ns = utf8(get(info, "endowmentNamespace"));
			const char *env = utf8(get(info, "environment"));
			if (!ns || !env || strcmp(ns, "com.apple.frontboard.visibility") != 0) continue;
			if (strncmp(env, pfx_paren, lp) != 0 && strncmp(env, pfx_slash, ls) != 0) continue;
			if (!strstr(env, "UISceneHosting") || strstr(env, "InputUI")) continue;
			const char *sc = strstr(env, "->/");
			sc = sc ? sc + 3 : env;
			*out_pid = pid;
			snprintf(out_scene, scenesz, "%s", sc);
			found = true;
			break;
		}
	}
	closedir(d);
	return found;
}

// The Lock Screen (Cover Sheet) is up: no app or hosted modal is in front, whatever the process states say.
static bool is_ui_locked(void) {
	Class cls = objc_getClass("SBLockScreenManager");
	if (!cls || !responds((id)cls, "sharedInstanceIfExists")) return false;
	id mgr = SEND(id, (id)cls, "sharedInstanceIfExists")((id)cls, SEL_("sharedInstanceIfExists"));
	return mgr && getb(mgr, "isUILocked");
}

static void write_front(void) {
	bool locked = is_ui_locked();
	id ctl = SEND(id, (id)objc_getClass("SBApplicationController"), "sharedInstanceIfExists")(
		(id)objc_getClass("SBApplicationController"), SEL_("sharedInstanceIfExists"));
	id sb = SEND(id, (id)objc_getClass("UIApplication"), "sharedApplication")((id)objc_getClass("UIApplication"), SEL_("sharedApplication"));
	id front = get(sb, "_accessibilityFrontMostApplication");
	id apps = get(ctl, "runningApplications");
	const char *bundle = NULL;
	long pid = 0;
	if (!locked && !pick_front_app(front, &bundle, &pid)) {
		for (unsigned long i = 0, n = count(apps); i < n; i++)
			if (pick_front_app(at(apps, i), &bundle, &pid)) break;
	}
	long hosted_pid = 0;
	char hosted_scene[256] = {0};
	if (pid > 0)
		find_hosted_modal(pid, &hosted_pid, hosted_scene, sizeof hosted_scene);
	len = 0;
	puts_("{\"bundle\":");
	if (bundle) putstr(bundle);
	else puts_("null");
	char num[128];
	snprintf(num, sizeof num, ",\"pid\":%ld,\"locked\":%s", pid, locked ? "true" : "false");
	puts_(num);
	if (hosted_pid > 0) {
		snprintf(num, sizeof num, ",\"hosted_pid\":%ld,\"hosted_scene\":", hosted_pid);
		puts_(num);
		putstr(hosted_scene);
	}
	puts_("}\n");
	write_file("front.json", buf, len);
}

// SpringBoard: requests agent-sim leaves in cmd.txt before the signal; the answer goes to answer.txt.
//   pasteboard-get                      the general pasteboard's text (SpringBoard may read it at any time; a
//                                       background process may not)
//   pasteboard-set\n<text>
//   statusbar\ntime=9:41\nbattery=100\ncharging=1   override status bar items, as `simctl status_bar override`
//   statusbar-clear
static id g_status_publisher;
static id nsstring(const char *c) {
	return SEND(id, (id)objc_getClass("NSString"), "stringWithUTF8String:", const char *)(
		(id)objc_getClass("NSString"), SEL_("stringWithUTF8String:"), c);
}
static id alloc_init(const char *cls) {
	id o = SEND(id, (id)objc_getClass(cls), "alloc")((id)objc_getClass(cls), SEL_("alloc"));
	return o ? SEND(id, o, "init")(o, SEL_("init")) : nil;
}
static const char *status_bar(char *req) {
	Class pubc = objc_getClass("STStatusBarOverridesStatusDomainPublisher");
	if (!pubc) return "no StatusKit";
	if (!g_status_publisher) g_status_publisher = alloc_init("STStatusBarOverridesStatusDomainPublisher");
	// clearing publishes an empty set of overrides (the domain's emptyData crashed SpringBoard here)
	bool clear = !strncmp(req, "statusbar-clear", 15);
	if (clear) g_status_time_overridden = false;
	id data = alloc_init("STMutableStatusBarData");
	long battery = -1;
	int charging = 0;
	for (char *line = clear ? NULL : strtok(req, "\n"); line; line = strtok(NULL, "\n")) {
		char *eq = strchr(line, '=');
		if (!eq) continue;
		*eq = 0;
		const char *k = line, *v = eq + 1;
		if (!strcmp(k, "time")) {
			g_status_time_overridden = true;
			Class sc = objc_getClass("STStatusBarDataStringEntry");
			id e = SEND(id, (id)sc, "entryWithStringValue:", id)((id)sc, SEL_("entryWithStringValue:"), nsstring(v));
			SEND(void, data, "setTimeEntry:", id)(data, SEL_("setTimeEntry:"), e);
			SEND(void, data, "setShortTimeEntry:", id)(data, SEL_("setShortTimeEntry:"), e);
		} else if (!strcmp(k, "battery")) {
			battery = atol(v);
		} else if (!strcmp(k, "charging")) {
			charging = atoi(v);
		}
	}
	if (battery >= 0) {
		Class bc = objc_getClass("STStatusBarDataBatteryEntry");
		id e = SEND(id, (id)bc, "entryWithCapacity:state:saverMode:prominentlyShowsDetailString:detailString:", long, long, bool, bool, id)(
			(id)bc, SEL_("entryWithCapacity:state:saverMode:prominentlyShowsDetailString:detailString:"), battery, charging ? 1 : 0,
			false, false, nsstring(""));
		SEND(void, data, "setMainBatteryEntry:", id)(data, SEL_("setMainBatteryEntry:"), e);
	}
	id ov = SEND(id, (id)objc_getClass("STMutableStatusBarOverridesStatusDomainData"), "alloc")(
		(id)objc_getClass("STMutableStatusBarOverridesStatusDomainData"), SEL_("alloc"));
	ov = SEND(id, ov, "initWithCustomOverrides:suppressedBackgroundActivityIdentifierListData:", id, id)(
		ov, SEL_("initWithCustomOverrides:suppressedBackgroundActivityIdentifierListData:"), data, nil);
	SEND(void, g_status_publisher, "setData:", id)(g_status_publisher, SEL_("setData:"), ov);
	return "ok";
}
static bool request(void) {
	char path[1200];
	snprintf(path, sizeof path, "%s/cmd.txt", dir);
	FILE *f = fopen(path, "r");
	if (!f) return false;
	static char req[65536];
	size_t n = fread(req, 1, sizeof req - 1, f);
	fclose(f);
	unlink(path);
	req[n] = 0;
	if (!strncmp(req, "statusbar", 9)) {
		const char *r = status_bar(req);
		write_file("answer.txt", r, strlen(r));
		return true;
	}
	if (!strncmp(req, "front", 5)) {
		write_front();
		write_file("answer.txt", "ok", 2);
		return true;
	}
	if (!strncmp(req, "pasteboard-set\n", 15)) {
		id pb = SEND(id, (id)objc_getClass("UIPasteboard"), "generalPasteboard")((id)objc_getClass("UIPasteboard"), SEL_("generalPasteboard"));
		if (!pb) {
			write_file("answer.txt", "no-pasteboard", 13);
			return true;
		}
		SEND(void, pb, "setString:", id)(pb, SEL_("setString:"), nsstring(req + 15));
		write_file("answer.txt", "ok", 2);
	} else if (!strncmp(req, "pasteboard-get", 14)) {
		// Reading generalPasteboard.string synchronously on SpringBoard's main queue deadlocks SpringBoard
		// when another app owns the pasteboard (iOS paste-permission UI blocks on the same main queue).
		write_file("answer.txt", "unsupported", 11);
	} else {
		write_file("answer.txt", "unknown", 7);
	}
	return true;
}

// WebKit's WebContent process draws web views for the apps; its accessibility tree hangs off one page object per web
// page, recorded here as WebKit creates them (there is no way to look them up afterwards).
#define MAXPAGES 64
static id g_pages[MAXPAGES];
static IMP g_page_init;
static id page_init(id self, SEL s) {
	id r = ((id (*)(id, SEL))g_page_init)(self, s);
	if (r) {
		for (int i = 0; i < MAXPAGES; i++)
			if (!g_pages[i]) { g_pages[i] = r; break; }  // page objects live as long as their page
	}
	return r;
}
static IMP g_page_dealloc;
static id g_warm[MAXPAGES];
static void page_dealloc(id self, SEL s) {
	for (int i = 0; i < MAXPAGES; i++) {
		if (g_pages[i] == self) g_pages[i] = nil;
		if (g_warm[i] == self) g_warm[i] = nil;
	}
	((void (*)(id, SEL))g_page_dealloc)(self, s);
}
static bool is_webcontent(void) { return strstr(getprogname(), "WebContent") != NULL; }

// The first walk of a page's tree makes WebKit build it; its frames then lack the web view's position until the app
// side has answered, so a page seen for the first time is walked once, and answered for half a second later.
static void dump_web_now(void *ctx);
static void dump_web(void) {
	bool cold = false;
	for (int i = 0; i < MAXPAGES; i++) {
		id page = g_pages[i];
		if (!page) continue;
		bool known = false;
		for (int k = 0; k < MAXPAGES; k++) known |= g_warm[k] == page;
		if (known) continue;
		cold = true;
		emitted = 0;
		walk(get(page, "accessibilityRootObjectWrapper"), 0);
		for (int k = 0; k < MAXPAGES; k++)
			if (!g_warm[k]) { g_warm[k] = page; break; }
	}
	len = 0;
	if (cold) dispatch_after_f(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), NULL, dump_web_now);
	else dump_web_now(NULL);
}
static void dump_web_now(void *ctx) {
	double t0 = ms_now(), waited = g_sent > 0 ? unix_ms() - g_sent : -1;
	g_sent = 0;
	g_visits = 0;
	len = 0;
	char head[256];
	snprintf(head, sizeof head, "{\"pid\":%d,\"process\":\"WebContent\",\"windows\":[", getpid());
	puts_(head);
	int w = 0;
	for (int i = 0; i < MAXPAGES; i++) {
		id page = g_pages[i];
		if (!page) continue;
		id root = get(page, "accessibilityRootObjectWrapper");
		if (!root) continue;
		if (w++) puts_(",");
		puts_("{\"class\":\"WebPage\",\"level\":0,\"elements\":[");
		emitted = 0;
		walk(root, 0);
		puts_("]}");
	}
	putf("],\"ms\":%.0f,\"visits\":%.0f,\"waited\":%.0f}\n", ms_now() - t0, g_visits, waited, 0);
	char name[64];
	snprintf(name, sizeof name, "ax-%d.json", getpid());
	write_file(name, buf, len);
}

// Visible windows in this process: UIApplication.windows can be empty or contain only a system
// UITextEffectsWindow in hosted remote view services such as SafariViewService (ASWebAuthenticationSession /
// SFSafariViewController), whose content lives in a connected scene window or _UIHostedWindow. Merge visible
// windows from app.windows and connectedScenes, and fall back to
// +[UIWindow allWindowsIncludingInternalWindows:onlyVisibleWindows:] when no non-system content window is present.
static bool is_aux_system_window(id w) {
	const char *cls = object_getClassName(w);
	return cls && (!strcmp(cls, "UITextEffectsWindow") || !strcmp(cls, "UIRemoteKeyboardWindow"));
}
static void add_visible_windows(id arr, id src, bool *has_content) {
	for (unsigned long i = 0, n = count(src); i < n; i++) {
		id w = at(src, i);
		if (getb(w, "isHidden") || getd(w, "alpha") < 0.01) continue;
		if (SEND(bool, arr, "containsObject:", id)(arr, SEL_("containsObject:"), w)) continue;
		SEND(void, arr, "addObject:", id)(arr, SEL_("addObject:"), w);
		if (has_content && !is_aux_system_window(w)) *has_content = true;
	}
}
static id app_windows(id app) {
	id arr = SEND(id, (id)objc_getClass("NSMutableArray"), "array")(
		(id)objc_getClass("NSMutableArray"), SEL_("array"));
	bool has_content = false;
	id wins = get(app, "windows");
	add_visible_windows(arr, wins, &has_content);
	id scenes = get(app, "connectedScenes");
	id all = scenes ? get(scenes, "allObjects") : nil;
	for (unsigned long i = 0, n = count(all); i < n; i++)
		add_visible_windows(arr, get(at(all, i), "windows"), &has_content);
	if (!has_content) {
		Class wc = objc_getClass("UIWindow");
		if (wc && responds((id)wc, "allWindowsIncludingInternalWindows:onlyVisibleWindows:")) {
			id iw = SEND(id, (id)wc, "allWindowsIncludingInternalWindows:onlyVisibleWindows:", bool, bool)(
				(id)wc, SEL_("allWindowsIncludingInternalWindows:onlyVisibleWindows:"), true, true);
			add_visible_windows(arr, iw, &has_content);
		}
	}
	return count(arr) ? arr : wins;
}

// An app: text for the field that has focus, in cmd-<pid>.txt ("insert\n<text>"), inserted as the software
// keyboard inserts it (UIKeyInput insertText:). Key events from the hardware keyboard reached native fields but
// lost most characters in web fields. The answer (ok, or no-field) goes to answer-<pid>.txt.
static bool app_request(void) {
	char path[1200], name[64];
	snprintf(path, sizeof path, "%s/cmd-%d.txt", dir, getpid());
	FILE *f = fopen(path, "r");
	if (!f) return false;
	static char req[65536];
	size_t n = fread(req, 1, sizeof req - 1, f);
	fclose(f);
	unlink(path);
	req[n] = 0;
	snprintf(name, sizeof name, "answer-%d.txt", getpid());
	if (strncmp(req, "insert\n", 7)) {
		write_file(name, "unknown", 7);
		return true;
	}
	id app = SEND(id, (id)objc_getClass("UIApplication"), "sharedApplication")((id)objc_getClass("UIApplication"), SEL_("sharedApplication"));
	id wins = app_windows(app), responder = nil;
	for (unsigned long i = 0, m = count(wins); i < m && !responder; i++) {
		id r = get(at(wins, i), "firstResponder");
		if (r && responds(r, "insertText:")) responder = r;
	}
	if (!responder) {
		write_file(name, "no-field", 8);
		return true;
	}
	SEND(void, responder, "insertText:", id)(responder, SEL_("insertText:"), nsstring(req + 7));
	write_file(name, "ok", 2);
	return true;
}

// Maps UIWindowScene.interfaceOrientation to x86Sim's orientation names.
// UIKit defines UIInterfaceOrientationLandscapeRight == UIDeviceOrientationLandscapeLeft (3) and
// UIInterfaceOrientationLandscapeLeft == UIDeviceOrientationLandscapeRight (4), so the numeric
// values 1..4 correspond 1:1 with portrait, portraitUpsideDown, landscapeLeft, landscapeRight.
static const char *window_orientation(id win) {
	id scene = get(win, "windowScene");
	long o = scene ? getl(scene, "interfaceOrientation") : 0;
	switch (o) {
	case 1: return "portrait";
	case 2: return "portraitUpsideDown";
	case 3: return "landscapeLeft";
	case 4: return "landscapeRight";
	default: return NULL;
	}
}

static void dump(void *ctx) {
	if (!strcmp(getprogname(), "SpringBoard")) {
		if (request()) return;
	} else if (!is_webcontent() && app_request()) return;
	len = 0;
	emitted = 0;
	if (is_webcontent()) {
		dump_web();
		return;
	}
	char rq[1200];
	snprintf(rq, sizeof rq, "%s/req-%d", dir, getpid());
	read_sent(rq);
	unlink(rq);
	double t0 = ms_now(), waited = g_sent > 0 ? unix_ms() - g_sent : -1;
	g_sent = 0;
	g_visits = 0;
	id app = SEND(id, (id)objc_getClass("UIApplication"), "sharedApplication")((id)objc_getClass("UIApplication"), SEL_("sharedApplication"));
	char head[256];
	snprintf(head, sizeof head, "{\"pid\":%d,\"process\":\"%s\",\"windows\":[", getpid(), getprogname());
	puts_(head);
	id wins = app_windows(app);
	int w = 0;
	const char *orient = NULL;
	g_nwebviews = 0;
	for (unsigned long i = 0, n = count(wins); i < n; i++) {
		id win = at(wins, i);
		if (getb(win, "isHidden") || getd(win, "alpha") < 0.01) continue;
		double level = getd(win, "windowLevel");
		const char *wo = window_orientation(win);
		if (wo && (!orient || (level >= 0 && level < 1000))) orient = wo;
		if (w++) puts_(",");
		puts_("{\"class\":"); putstr(object_getClassName(win));
		putf(",\"level\":%.0f,\"elements\":[", level, 0, 0, 0);
		emitted = 0;
		walk(win, 0);
		puts_("]}");
	}
	putf("],\"ms\":%.0f,\"visits\":%.0f,\"waited\":%.0f,\"webviews\":[", ms_now() - t0, g_visits, waited, 0);
	for (int i = 0; i < g_nwebviews; i++)
	{
		char t[160];
		int n = snprintf(t, sizeof t, "%s[%.1f,%.1f,%.1f,%.1f]", i ? "," : "", g_webviews[i].x, g_webviews[i].y,
			g_webviews[i].w, g_webviews[i].h);
		put(t, n);
	}
	puts_("],\"webview_pids\":[");
	int wp = 0;
	for (int i = 0; i < g_nwebviews; i++) {
		int wpid = g_webview_pids[i];
		if (wpid <= 0) continue;
		bool dup = false;
		for (int j = 0; j < i; j++) if (g_webview_pids[j] == wpid) { dup = true; break; }
		if (dup) continue;
		char t[32];
		int n = snprintf(t, sizeof t, "%s%d", wp++ ? "," : "", wpid);
		put(t, n);
	}
	puts_("]");
	if (orient) {
		puts_(",\"orientation\":");
		putstr(orient);
	}
	puts_("}\n");
	char name[64];
	bool sb = !strcmp(getprogname(), "SpringBoard");
	if (sb) snprintf(name, sizeof name, "ax-springboard.json");
	else snprintf(name, sizeof name, "ax-%d.json", getpid());
	write_file(name, buf, len);
	if (sb) write_front();
}

static void start(void) {
	const char *h = getenv("CFFIXED_USER_HOME");
	if (!h || !*h) h = getenv("HOME");
	if (!h) return;
	snprintf(dir, sizeof dir, "%s", h);
	char *c = strstr(dir, "/Containers/");  // an app's home is its container below the device's data directory
	if (c) *c = 0;
	strncat(dir, "/tmp/agent-sim", sizeof dir - strlen(dir) - 1);
	mkdir(dir, 0755);
	signal(SIGUSR2, SIG_IGN);
	dispatch_source_t s = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR2, 0, dispatch_get_main_queue());
	dispatch_source_set_event_handler_f(s, dump);
	dispatch_resume(s);
	// SIGUSR2's default action ends the process: the CLI signals only processes that have this file
	char ready[32];
	snprintf(ready, sizeof ready, "hooked-%d", getpid());
	write_file(ready, "1", 1);
}
// An idle WebContent process took seconds to run work queued for its main thread (its run loop did not wake for
// the main dispatch queue), so there a thread looks for a request file (req-<pid>) every 150 ms, puts the work on
// the main run loop itself and wakes it.
#include <pthread.h>
#include <dlfcn.h>
static void *watch_requests(void *arg) {
	char path[1200];
	snprintf(path, sizeof path, "%s/req-%d", dir, getpid());
	void *(*get_main)(void) = dlsym(RTLD_DEFAULT, "CFRunLoopGetMain");
	void (*perform)(void *, const void *, void (^)(void)) = dlsym(RTLD_DEFAULT, "CFRunLoopPerformBlock");
	void (*wake)(void *) = dlsym(RTLD_DEFAULT, "CFRunLoopWakeUp");
	const void **common = dlsym(RTLD_DEFAULT, "kCFRunLoopCommonModes");
	for (;;) {
		usleep(150000);
		if (access(path, F_OK) != 0) continue;
		read_sent(path);
		unlink(path);
		if (get_main && perform && wake && common) {
			void *rl = get_main();
			perform(rl, *common, ^{ dump(NULL); });
			wake(rl);
		} else {
			dispatch_async_f(dispatch_get_main_queue(), NULL, dump);
		}
	}
	return NULL;
}

static void added(const struct mach_header *mh, intptr_t slide) {
	static bool on;
	if (on) return;
	if (is_webcontent()) {
		Class c = objc_getClass("WKAccessibilityWebPageObject");
		Method m = c ? class_getInstanceMethod(c, SEL_("init")) : NULL;
		if (!m) return;
		Method d = class_getInstanceMethod(c, SEL_("dealloc"));
		if (!d) return;
		g_page_init = method_setImplementation(m, (IMP)page_init);
		g_page_dealloc = method_setImplementation(d, (IMP)page_dealloc);
		on = true;
		start();
		pthread_t t;
		if (pthread_create(&t, NULL, watch_requests, NULL) == 0) pthread_detach(t);
	} else if (objc_getClass("UIApplication")) {
		on = true;
		start();
	}
}
__attribute__((constructor)) static void init(void) {
	// a process that is not an app or WebContent never answers; one that is answers once start() has run
	signal(SIGUSR2, SIG_IGN);
	_dyld_register_func_for_add_image(added);
}

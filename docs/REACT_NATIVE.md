# React Native and Expo apps

What runs, how to build it, and the quickest way to try a JavaScript change. Checked with an Expo SDK app (React
Native, New Architecture, Hermes) built on EAS, on iOS 27.2 with x86Sim 0.5.2.

## Build for the simulator

A release (non-development) simulator build runs. With EAS, a build profile that sets the simulator flag:

```json
"build": {
  "ios-simulator": { "ios": { "simulator": true } }
}
```

`eas build --platform ios --profile ios-simulator` ends with an artifact URL; install it as it is:

```sh
agent-sim install https://expo.dev/artifacts/eas/<id>.tar.gz
agent-sim launch com.example.myapp
```

EAS simulator builds are universal (x86_64 and arm64); `install` keeps the arm64 half, which x86Sim needs. A build
made with Xcode on a Mac (`-sdk iphonesimulator -configuration Release`) installs the same way, as an `.app` or zipped.

## Reading the screen

React Native views go through UIKit's accessibility, so `snapshot` reads them like native ones: text, buttons
(`accessibilityRole="button"`), switches, sliders (`adjustable`) and text inputs.
`accessibilityLabel` is the label `tap "..."` matches, and `testID` becomes the element's `id`. Give icon-only
buttons a label, as you would for VoiceOver; `tap @e4` works either way.

## Development builds and Metro

Development builds (`expo-dev-client`) run, with Metro on the Linux machine: the app
loads its JavaScript from Metro, and Reload picks up an edit. Checked with an EAS profile that sets
`"developmentClient": true` and `"ios": {"simulator": true}`.

- The simulator reaches the machine agent-sim runs on as `localhost`, so Metro's `localhost:8081` works as it is.
  Where agent-sim runs in a VM and Metro outside it, forward the VM's port, for example
  `socat TCP-LISTEN:8081,bind=127.0.0.1,fork TCP:<host>:8081` inside the VM.
- Open the app on the server with a deep link: `agent-sim open "<scheme>://expo-development-client/?url=http%3A%2F%2Flocalhost%3A8081"`.
  A development build and a release build of the same app both register `exp+<slug>`, and the link can open the
  wrong one; use a scheme only the development build has (its `CFBundleURLTypes` in Info.plist).
- A red error screen shows in `snapshot` with its stack lines and buttons (`tap "Reload (⌘R)"`).
- An app that aborts with `dynarmic: shared code cache full (2097152000 bytes)` has outgrown x86Sim's code cache,
  2000 MB, the most `X86SIM_A64_CODECACHE_MB` allows. Seen once, with a development build on its first runs on a new
  phone; the same build has run since.

## Trying a JavaScript change without a new build

A release build runs the bundle inside the `.app` (`main.jsbundle` and its assets), so a JavaScript-only change can go
into the build you already have. Bundle with the same settings as the build and copy it over:

```sh
npx expo export:embed --platform ios --dev false --minify true \
  --entry-file node_modules/expo-router/entry.js \
  --bundle-output out/main.jsbundle --assets-dest out
cp -R out/. ~/.local/share/agent-sim/installs/<build>/MyApp.app/
agent-sim install ~/.local/share/agent-sim/installs/<build>/MyApp.app
```

`--entry-file` is the app's entry (`index.js` in a bare React Native app, where `npx react-native bundle` takes the same
options). Environment variables the bundle reads at build time (`EXPO_PUBLIC_...`) must be set for `export:embed`, as
they were for the build. A change to native code or to a package with native code needs a new build.

The simulator reaches the Linux machine's own `localhost` as `localhost` (App Transport Security allows it), so an
API server running next to agent-sim can be used by a build that points at `http://localhost:<port>`.

## Differences from a device and from Apple's Simulator

- Speed: everything is translated, so launch times and animations are slower than on a Mac and say nothing about a
  phone's.
- Opening a document from Files in another app does not hand the document over (the app opens empty). Put test
  files in On My iPhone with `agent-sim files push` and open them from the app's own document picker.
- Audio plays (`X86SIM_AUDIO=1`), but stops a few seconds after the screen locks, and no Now Playing controls show.
- Google's sign-in page, opened from an app, stopped after its first step (Next did nothing, with the phone idle).

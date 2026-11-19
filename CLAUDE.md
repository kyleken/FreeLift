# Free Lift – notes for Claude Code

Free Lift is an **Android-only** Rust app (egui/eframe, NativeActivity) that replaces the
Speediance "Free Lift" screen on a **Gym Monster** tablet. It needs no sign-in. It drives the
motor board directly over the serial port `/dev/ttyS5` (115200 baud), using the byte protocol
decoded from the original app (`com.speediance.faraday` 3.14.01).

**Read first:** `docs/DEVELOPER_GUIDE.md`. It covers every module, the machine protocol byte by
byte, the command sequences, the screen layout, building, installing and debugging.
Every number taken from the original is in `MECHANICS.md`, with the routine address in
`libapp.so`, and is listed in `mechanics.json`.

## Rules the owner set

* **Android only.** Do not add a Windows or desktop build again. `Cargo.toml` has
  `autobins = false`, and `.cargo/config.toml` makes a plain `cargo build` target
  `aarch64-linux-android`.
* Voice control must run **locally / offline**, and the owner must be able to turn it on and
  off easily. It is built (sherpa-onnx on the tablet, `voice.rs`, guide §3.9). The owner wants
  voice to be able to turn the weight on, and both listening styles with a toggle:
  **Always** and wake word **"Weight"** (sound-alikes like "wait" count). "Stop" / weight off
  must always work in both styles and act at once. Everything voice heard is in
  `/data/data/com.craftanything.freelift/files/voice-heard.txt` (read it with `adb shell cat`
  before tuning words).
* The owner doesn't code: explain results in plain words.

## Safety (real motors)

* Commands go to a machine that pulls cables under load. Change `protocol.rs`,
  `machine_link.rs` or `sync_machine()` in `app.rs` only with a test that pins the exact
  bytes. Golden frames from the tablet's own log are in `protocol::tests`.
* Never send motor commands from a script or adb yourself. The app sends them only when the
  person taps or speaks.
* Fixed-speed (isokinetic) mode is **deliberately blocked** on the real machine
  (`unsupported_on_machine`): its weight payload is not decoded.
* Keep the safety guards and their tests: connecting never starts the motors
  (`machine_connected`), weight on needs a clearly-worded command and a bare "stop" means
  weight off (`commands::parse`), smart load off waits for the cables to come home, Barbell /
  Dumbbells can't be switched with the weight on, weight off is confirmed against the
  cable force (`check_weight_off`), and sleep after no use (default 30 min) or the Sleep
  button sends weight off + disable + the board's sleep (`release_machine(true)`), and the
  next connect sends wakeup first, with the weight still off.
* Installing an APK closes Free Lift at once: check the weight is off first (screenshot or
  ask). Never tap the tablet screen with `adb shell input`.

## Screen (owner's choices)

* Fills the whole 1080 × 1920 screen; the Free Lift box keeps one fixed size; the Weight on /
  off button is full width and large.
* Setup is one-of-three buttons: **Barbell**, **Dumbbells** (the code's "independent"),
  **Assist**. One is always selected and the choice is remembered.
* Sleep after 30 min with no taps and no cable movement: black screen, motors off, one tap
  wakes it (the tap presses nothing). "Sleep after" in Setup changes it (0 = never). The
  🌙 Sleep button (top bar, between Settings and Machine) sleeps at once, same path.
* Top bar, left: **Start Workout** / **End Workout** (one fixed-size button, text centred),
  **Pause** / **Resume** right of it, then a dot for the machine link (green connected,
  orange practice) — no "No sign-in" or "Machine connected" text. The workout clock waits
  for Start Workout (or the first Weight on). The Sets box has no buttons.
* The Training summary (after End Workout) is a card in the app's own style, not a plain
  egui window.
* Layout changes must not change what any control does. Details: guide §5.
* Free Lift can be picked as the tablet's Home app (starts at power-up); the owner sets it in
  Android Settings › Apps › Default apps › Home app. Don't change the default yourself.

## Everyday commands (Git Bash, from `app/`)

```
cargo build                                              # compile for the tablet (check only)
cargo test --lib --target x86_64-pc-windows-msvc         # unit tests run on the PC
ANDROID_HOME=… ANDROID_NDK_ROOT=… cargo apk build --release --lib
cp target/release/apk/FreeLift.apk dist/FreeLift.apk
adb connect 192.168.2.50
adb push dist/FreeLift.apk /data/local/tmp/FreeLift.apk
adb shell pm install -r -t /data/local/tmp/FreeLift.apk
```

When the tablet's Wi-Fi drops during the push, the owner puts the APK on a USB drive; install
it from `/mnt/media_rw/<id>/` on the tablet (guide §6.2).

The full paths and environment variables are in the guide, section "Build, install, run".

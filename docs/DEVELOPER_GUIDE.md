# Free Lift – developer guide

This guide is the hand-off document for continuing Free Lift in Claude Code (or by hand). It
describes what the app does, how every source file works, exactly how the app talks to the
Gym Monster's motors, the screen layout, how to build, install and debug, and what is still
unknown.

Companion files:

| File | What it holds |
|---|---|
| `CLAUDE.md` | Short orientation and the owner's rules (Claude Code loads it automatically). |
| `MECHANICS.md` | Every value, formula and rule taken from the original app, with the routine address in `libapp.so` and how it was found (passes 1–10). |
| `mechanics.json` | The same values in machine-readable form (`source` = where it came from, `estimated: true` = our own choice). |
| `README.md` | The GitHub front page: the safety and liability notice, what Free Lift is, where to read next. |
| `LICENSE`, `NOTICE` | Apache License 2.0 (no warranty, limited liability); copyright, trademark note and the licences of the bundled sherpa-onnx, ONNX Runtime and speech model. |
| `ANDROID.md` | Short user-facing notes: the screen, freeing the machine, voice, installing. |
| `dist/FreeLift.apk` | The latest signed APK. |

---

## 1. What the app is

* **Purpose:** the original Speediance app's **Free Lift** mode, with no login, plus voice and
  typed commands ("weight on", "weight up", "set weight to 20", …) that can be switched on and
  off.
* **Target device:** the Gym Monster's own tablet (Rockchip RK3568, Android 11, 1080 × 1920
  portrait, density 240). The motor board is wired to the tablet's UART `/dev/ttyS5`.
* **Technology:** Rust 2021, `eframe`/`egui` 0.33.3 with the `glow` renderer, Android
  `NativeActivity` through `winit`'s `android-native-activity`, packaged with `cargo-apk`.
  `jni` 0.21 opens Android settings screens, and `libc` termios configures the serial port.
* **Package:** `com.craftanything.freelift`, label "Free Lift", min SDK 26, target SDK 33,
  portrait, signed with the debug keystore (`C:/Users/kylel/.android/debug.keystore`,
  password `android`).
* **There is no desktop version.** `src/main.rs` is an empty placeholder and is not built
  (`autobins = false`).

### What works today

| Feature | Real machine | Notes |
|---|---|---|
| Weight on / off | ✅ | enable (0x01) / pause (0x03) frames, heartbeat every 500 ms; weight off is checked against the cable force |
| Setup: Barbell / Dumbbells / Assist (one of three, always one selected) | ✅ | Barbell = one total weight; Dumbbells = separate left / right; Assist = linked handles with the spotter |
| Weight per side (Assist), barbell total | ✅ | limits from the original |
| Dumbbells (independent left / right weights) | ✅ (decoded, not yet seen in the tablet log) | two-player frames A1 then A2, 50 ms apart |
| Standard / eccentric / chain modes + extra weight | ✅ | eccentric frames verified byte-for-byte |
| Fixed-speed (isokinetic) mode | ❌ blocked | payload not decoded; the UI explains it |
| Assist (spotter) level | UI only | the enable frame's assist byte is always 0 in the original builder |
| Smart load off | ✅ (app side) | the app sends pause once both cables are back home and still for 3 s after a rep |
| Safety guards | ✅ | connecting never starts the motors; Barbell / Dumbbells can't be switched with the weight on; weight-off confirmation (§4.4) |
| Live reps, cable distance, force, power graph | ✅ | from the 60-byte live frame |
| Machine replies and safety stops | ✅ | 0x83 ack, 0x82 protection with reason text |
| Workout clock: Start / End Workout, Pause / Resume (top bar) | ✅ | the clock waits for Start Workout (or the first Weight on) |
| Sets table, rest timer, Training summary | ✅ | local only, not saved between launches; the summary is a themed card with New workout / Close |
| Sleep after no use (default 30 min) and the 🌙 Sleep button | ✅ (board sleep untested) | weight off + disable + the board's sleep (0x08) + port released, black screen with moving text; one tap wakes it and the next connect sends wakeup (0x09) (§4.4) |
| Free Lift as the tablet's Home app | ✅ (cold boot untested) | HOME intent filter; the owner picks it in Android Settings |
| Typed commands | ✅ | `commands::parse` |
| Voice control (offline) | ✅ works on the tablet, being tuned | sherpa-onnx streaming English model on the tablet; "Always" or wake word "Weight"; "stop" acts at once; Mic level bar, silence warning and a log of everything heard (§3.9) |
| Android Settings button, "Force stop Speediance" helper | ✅ | JNI intents |

---

## 2. Source map

```
app/
├─ Cargo.toml            crate "freelift", lib "freelift_core" (rlib + cdylib), cargo-apk metadata
├─ .cargo/config.toml    default target aarch64-linux-android + NDK clang linker / llvm-ar
├─ src/
│  ├─ lib.rs             module list, android_main entry, JNI helpers (Settings / App info,
│  │                     microphone permission), speech-model copy out of the APK
│  ├─ app.rs             FreeLiftApp: the whole screen, frame loop, machine sync, settings file
│  ├─ machine.rs         Machine: every Free Lift setting, the original's rules, practice simulator
│  ├─ commands.rs        Command enum + parse() for typed/voice text; the original's 21 intents
│  ├─ session.rs         Session, SetRecord, Phase (ready / lifting / rest / done), rest + power stats
│  ├─ protocol.rs        frame decoder (machine → app) and command builders (app → machine), CRC
│  ├─ machine_link.rs    serial-port thread: open/configure the port, write, heartbeat, read
│  ├─ serial_link.rs     list of candidate serial devices for the Machine window
│  ├─ mechanics.rs       all numeric constants (sourced or marked "Estimated")
│  ├─ voice.rs           offline voice: microphone → sherpa-onnx → text, wake-word gate
│  └─ main.rs            placeholder, not compiled
├─ android-libs/arm64-v8a/   libsherpa-onnx-c-api.so + libonnxruntime.so (sherpa-onnx v2.0.1)
├─ assets/voice/en-20M/      speech model (encoder/joiner int8, decoder, tokens), ~35 MB
├─ voice-test/               recordings for the PC voice test (+ make_commands.ps1)
├─ tools/win-test-runner.cmd PC test runner (puts the sherpa-onnx DLLs next to the tests)
├─ docs/DEVELOPER_GUIDE.md   this file
├─ MECHANICS.md, mechanics.json, ANDROID.md, CLAUDE.md
├─ dist/FreeLift.apk
└─ shots/                old screenshots from when a desktop build existed (not used)
```

Data flow, one frame of the UI loop:

```
 touch / keys / typed text ──► run_command(Command) ──► Machine::apply (rules, limits)
                                     │                         │
                                     ▼                         ▼
                              Session (sets, rest)      Machine state (weight_on, kg, mode…)
                                                               │
                         sync_machine() compares state with what was last sent
                                                               │
                                     MachineLink (thread) ◄────┘  Out::Send / Wait / Heartbeat / Close
                                     │   ▲
                         /dev/ttyS5  ▼   │ In::Opened / Frame / Sent / Error / Closed
                                motor board
                                     │
                         poll_link() ┘──► Machine::apply_live (reps, distance, force) ──► Live card
```

---

## 3. Module by module

### 3.1 `lib.rs`

* Declares the modules.
* `ANDROID_APP: OnceLock<AndroidApp>` keeps the Android handle for JNI calls.
* `android_main(app)` is the NativeActivity entry point. It stores the handle, sets
  `app::DATA_DIR` to the app's internal files folder, then calls
  `eframe::run_native("Free Lift", …, FreeLiftApp::new)`.
* `open_android_settings()` starts the intent `android.settings.SETTINGS`.
* `open_app_info(package)` starts `android.settings.APPLICATION_DETAILS_SETTINGS` with
  `package:<name>`; that is the page with **Force stop**.
* `start_settings_intent()` does the JNI work: attach the thread, `new Intent(action)`,
  optional `Uri.parse` + `setData`, `addFlags(FLAG_ACTIVITY_NEW_TASK = 0x10000000)`, then
  `activity.startActivity(intent)`. On error it prints and clears the Java exception.

### 3.2 `mechanics.rs` – constants

All numbers used by the rules live here. Each comment says where the value came from. Values
marked **Estimated** are our own choices.

| Constant | Value | Source |
|---|---|---|
| `LB_PER_KG` | 2.2 | 0x3934a84, pool 0x202fa0 |
| `BASE_TENSION_KG` | 3.5 | idle frames report 7.7 lb |
| `MIN_PER_SIDE_KG/LB` | 3.5 / 8 | 0x1c41778 |
| `MIN_TOTAL_KG/LB` | 7 / 16 | 0x1c41778 |
| `MAX_TOTAL_KG/LB` | 100 / 220 | 0x1c2dc80 |
| `MAX_TOTAL_KG/LB_HIGH` | 120 / 260 (Monster 2S, 3S, Ultra) | 0x1c2dc80 |
| `FUZZY_*` | threshold 25 per side / 50 total; ×0.1 ceil below, ×0.05 floor above | `_computeFuzzyDelta` 0x3932ecc |
| `STEP_KG / STEP_LB` | 0.5 / 1 | **Estimated** |
| `DEFAULT_WEIGHT_SHOWN` | 8 (kg or lb, the unit on screen) | 0x3acda18 (`powerValue ?? 8.0`) |
| `DEFAULT_EXTRA_RATIO` | 0.5 × weight | 0x3acda18 (`_slider2Value ?? weight × 0.5`) |
| `EXTRA_RATIO_CHAIN / _ECCENTRIC` | 1.5 / 0.5 | pp+0x2035f0, used by 0x1c2d7ac |
| `SPEED_MIN/MAX/DEFAULT` | 0.1 / 1.0 / 0.4 m/s | **Estimated** |
| `VOICE_INTENTS` | 21 | `assets/intent_vectors_en.json` |
| `ASSIST_LEVELS` | 2 | voice texts |
| `REST_DEFAULT_S` | 60 | **Estimated** (the original counts rest up) |
| `REST_SWITCH_DEFAULT` | true | 0x1c91bdc |
| `SMART_OFF_DEFAULT` | true | 0x1f6e21c (`autoOffWeight ?? 1`) |
| `SMART_OFF_IDLE_S` | 3.0 | **Estimated** |
| `SMART_OFF_HOME_TOL_M` / `SMART_OFF_STILL_TOL_M` | 0.08 / 0.02 m | **Estimated** (real-machine smart load off) |
| `LIVE_STALE_S` | 0.5 | **Estimated** (live data older than this counts as missing) |
| `OFF_CHECK_S` / `OFF_MARGIN_KG` / `OFF_RESENDS` | 1.0 s / 1.5 kg / 2 | **Estimated** (weight-off check) |
| `DEFAULT_BAUD` | 115200 | tablet `stty` on `/dev/ttyS5` |

### 3.3 `commands.rs` – what a person can say or type

* `Unit { Kg, Lb }`, `Side { Both, Left, Right }`,
  `WeightMode { Standard, Eccentric, Chain, Isokinetic }` (with `label()`),
  `Extra { Eccentric, Chain }`.
* `Command` covers every action: `WeightOn`, `WeightOff`,
  `Bump { dir, amount, unit, side }` ("weight up by 5", "a bit" when `amount` is `None`),
  `SetWeight { value, unit, side }`, `SetMode`, `SetExtra { kind, value, unit }`, `SetSpeed`,
  `Barbell(bool)`, `Independent(bool)`, `Assist(bool)`, `AssistLevel(u8)`,
  `SmartLoadOff(bool)`, `Units`, `StartWorkout`, `Pause`, `Resume`, `SkipRest`, `FinishSet`, `EndTraining`,
  `NextExercise`, `PreviousExercise`, `QueryWeight`, `VoiceOff`.
* `ORIGINAL_INTENTS` lists the original's 21 voice intents, each with an example sentence.
  The test `all_original_intents_understood` checks that every one parses.
* `parse(text) -> Option<Command>`:
  1. `tokenize` lowercases the text, strips punctuation and splits suffixes like "20kg". It
     joins number words into numbers ("twenty five point five" → 25.5, "oh" = 0).
  2. Keyword and phrase rules are checked in priority order: voice off → assist →
     smart load off → modes → extras → speed → barbell / independent → units → session
     commands → weight off → weight on → set weight → up/down → query.
  3. **Safety rules for on/off:** weight off is checked before weight on, so an unclear
     sentence turns the weight off. A bare "stop", "halt", "freeze" or "emergency" means
     weight off. Weight on needs every word in the sentence to be about the weight ("turn
     on the weight", "please turn the weight on"); "turn on voice", "turn on the rest timer"
     and "don't turn on the weight" are not understood instead of starting the motors.
  3. "left" or "right" picks the side; "kilos/kg/pounds/lb…" picks the unit (otherwise the
     display unit is used).
* Tests: `numbers`, `the_persons_examples`, `original_intents`, `all_original_intents_understood`.

### 3.4 `machine.rs` – settings, rules and the practice simulator

`Machine` is the single source of truth for what the machine should be doing:

| Field | Meaning |
|---|---|
| `weight_on` | the person wants the load on |
| `left_kg`, `right_kg` | **per-cable** weight in kg (always kg internally) |
| `barbell`, `independent`, `assist` | the Setup choice (`Accessory`): exactly one is true. Barbell = the screen shows the total of both cables; independent = Dumbbells (separate left / right); assist = linked handles with the spotter |
| `mode`, `eccentric_kg`, `chain_kg`, `speed` | weight mode and its extra |
| `assist_level` | spotter level 1–2 (UI only, see §4.4) |
| `smart_off` | turn the weight off after a set |
| `unit`, `model` | display unit and machine model (sets the maximum) |
| `practice_lifter` | the simulator moves the cables by itself when no machine is connected |
| `external` | true while connected: cables come from live frames, not the simulator |
| `left`, `right: Cable` | `pos` 0–1, `vel` m/s, `force_kg`, `reps`, `dist_m` (real machine) |
| `live_fresh` | set by app.rs each frame: the latest live frame is under `LIVE_STALE_S` old |
| `power_left/right/history` | the last 360 power samples (W) for the graph |

Key methods:

* **Shown weight.** `cables()` is 2 in barbell mode, else 1. `shown_kg() = left_kg × cables()`.
  `limits_shown()` returns (min, max) in the display unit: per side `min = 3.5 kg / 8 lb`,
  `max = total / 2`; in barbell mode `min = 7 kg / 16 lb`, `max = total`.
* **`fuzzy_delta(current)`** implements "a bit": ceil(current × 0.1) when current ≤ the
  threshold (25 per side, 50 in barbell mode), otherwise floor(current × 0.05).
* **`set_shown(side, value)`** refuses out-of-range values with the original's messages, snaps
  to the step, clamps, and stores into one side (Dumbbells) or both.
* **`step_button(side, dir)`** handles the − / + buttons and arrow keys.
* **`apply(&Command) -> Option<String>`** applies the original's rules and returns the reply
  text (`None` for session commands that `app.rs` handles):
  * weight changes are refused in isokinetic mode;
  * `SetSpeed` is only allowed in isokinetic mode (speed rounded to 0.05, clamped);
  * an eccentric extra needs eccentric mode and a chain extra needs chain mode. Both are
    capped by `max_extra_kg()` = max(0, min(max − weight, weight × 1.5 chain / 0.5 eccentric))
    (0x1c2d7ac), and `clamp_extra()` lowers the extra after every weight or mode change
    (0x1c4fdf0);
  * `apply_default_weight()` sets 8 in the unit on screen with an extra of half that, and
    `restore_weight(kg)` clamps a saved weight to the limits;
  * **switching mode while the weight is on turns the weight off first** (`switchWeightMode`
    0x3cc0da4);
  * **Barbell / Dumbbells / Assist are one choice** (`Accessory`, `accessory()`,
    `set_accessory()`): selecting one turns the other two off, and something is always
    selected (`Machine::default()` starts on Assist). `Barbell(true)`, `Independent(true)`,
    `Assist(true)` and `AssistLevel(n)` select one; `Barbell(false)` / `Independent(false)`
    move to Assist; `Assist(false)` keeps Assist (it can only be left by choosing another).
    Leaving Dumbbells copies left → right, and the shown weight is kept inside the new limits;
  * **switching to or from Barbell or Dumbbells is refused while the weight is on** (the
    reply asks to turn the weight off first): it would change the kind of command the motors
    get mid-set, and Dumbbells use the not-yet-verified two-player frames;
  * `machine_connected()` runs when the port opens: it turns off a weight that was turned on
    in practice mode, so connecting never starts the motors;
  * changing units re-clamps both sides.
* **`apply_live(reps, distance_m, force_kg, dt)`** is used when connected. The cable bar
  position is `distance / LIVE_TRAVEL_M (1.2 m)`; velocity comes from successive positions.
* **`step(dt) -> StepEvents`** runs once per UI frame:
  * Connected (`external`): power = force × |vel| × g per cable. Smart load off fires when the
    weight is on, ≥ 1 rep has been counted, live data is fresh (`live_fresh`, set by app.rs),
    **both cables are back home** (within `SMART_OFF_HOME_TOL_M` of the lowest distance seen
    since the set started) and neither has moved more than `SMART_OFF_STILL_TOL_M` for
    `SMART_OFF_IDLE_S`. Stillness is measured from distances, not from frame timing, and a
    pause at the top of a rep never turns the weight off.
  * Practice: sine-shaped pulls (`SIM_REP_SECONDS` 2.6 s, 8 reps, 0.6 m travel). A rep counts
    when the cable passes above 0.9 and comes back below 0.1. The force follows the mode
    (eccentric adds its extra while lowering, chain adds extra × position). Smart load off
    fires when the cables rest at the bottom.
* `fmt_num` prints one decimal and drops a trailing ".0" (`_formatWeight` 0x391164c).
* `reset_reps()` zeroes the counters at the start of a set.
* Tests cover the limits, fuzzy delta, mode rules, barbell totals, the voice flow and smart
  load off.

### 3.5 `session.rs` – sets, rest and the summary

* `Phase`: `Ready`, `Lifting`, `Rest { left_s }`, `Done`.
* `SetRecord` holds the exercise, reps L/R, kg L/R, barbell, mode, eccentric/chain extra,
  speed, `seconds`, `avg_power_w` and `rest_before_s`. `reps()` is the larger side.
  `volume_kg()` = Σ kg × reps per cable (the original's `totalCapacity`).
* `Session` holds the exercise list (10 defaults, `"Free lift"` first), `sets`, `phase`,
  `paused`, `started` (the workout clock runs: Start Workout or the first Weight on),
  `elapsed_s` (the workout clock), `set_elapsed_s`, `rest_seconds`, `rest_on`, `rest_taken_s`
  and the power average.
* `tick(dt)` does nothing while paused or Done. Otherwise:
  * The workout clock (`elapsed_s`) counts only once `started`; opening the app doesn't
    start it.
  * Lifting adds to the set time.
  * Rest counts down; it also adds to the rest taken, and at 0 the phase becomes Ready.
  * Ready after at least one set still counts as rest.
* `start_set()` stores the rest taken since the last set (`rest_before_current`) and resets
  the power average.
* Sessions live in memory only; nothing is written to disk.

### 3.6 `protocol.rs` – bytes on the wire

See §4 for the full protocol. This file contains:

* `crc16_x25` (poly 0x1021 reflected = 0x8408, init 0xFFFF, xorout 0xFFFF; check value
  0x906E for "123456789").
* Command builders: `cmd_simple`, `cmd_heartbeat`, `cmd_enable`, `cmd_enable_2player`,
  `device_mode_code`.
* `decode(frame)` and the streaming `Splitter` (it finds `AA AB`, reads the length byte,
  checks the CRC, and resyncs one byte at a time on errors; `bad_frames` counts the
  failures).
* `Frame::Live { family, sub_mode, reps_left, reps_right, left, right }` or
  `Frame::Other { kind, payload }`.
* `SideData { a, b, force_lb }` with `distance_m()` and `force_kg()`.
* `TYPE_ACK` 0x83, `TYPE_PROTECTION` 0x82, `protection_reason(code)`,
  `frame_kind_name(kind)`, `command_name(frame)` and `hex(bytes)`.
* The tests include **golden frames copied from the original's own log on the tablet**. Keep
  them passing.

### 3.7 `machine_link.rs` – the serial thread

* `TABLET_PORT = "/dev/ttyS5"`.
* `connect(port, baud)` spawns a thread and returns `MachineLink { port, tx, rx }`.
* **Messages to the thread (`Out`):**
  * `Send(bytes)`;
  * `Wait(ms)`, which sleeps before the next item (used for the 50 ms two-player gap);
  * `Heartbeat(on)`;
  * `Close { weight_off, board_sleep }` (`close(weight_off)`, and `close_and_sleep()` for the
    app's Sleep).
* **Events from the thread (`In`):** `Opened(port)`, `Frame(Frame)`, `Sent(bytes)`,
  `Error(text)`, `Closed`.
* `open_port`:
  * opens with `O_NOCTTY | O_NONBLOCK`;
  * configures with `cfmakeraw` at `B115200`, plus `CLOCAL | CREAD` (raw 8N1).
* **Thread loop:**
  1. Drain the outgoing queue. `write_all` retries `WouldBlock` for up to 200 ms, then reports
     `Error`.
  2. When the heartbeat is on and due, write `cmd_heartbeat(counter)`, advance the counter and
     schedule the next beat 500 ms later. The first beat comes 500 ms after it is switched on.
  3. Read up to 1024 bytes and feed the `Splitter`. When there is no data it sleeps 10 ms.
* **Close:** the thread writes `closing_frames(weight_off, heartbeat, board_sleep)`, then
  waits `CLOSE_DRAIN_MS` (50 ms) and closes:
  * `weight_off` true or the heartbeat running: **pause, then disable**;
  * `board_sleep` (the app's Sleep): always **pause, disable**, then after
    `SLEEP_AFTER_DISABLE_MS` (100 ms) the board's **sleep** (0x08);
  * otherwise nothing.
  Dropping the `MachineLink` sends a plain close. The bytes are pinned by
  `machine_link::tests::closing_sends_exactly_these_bytes`.

### 3.8 `serial_link.rs`

`ports()` lists whichever of `/dev/ttyS3…ttyS6` and `/dev/ttyACM0` exist, and marks
`/dev/ttyS5` as the Gym Monster board. The board's USB id (VID 0x0483, PID 0x5740, STM32
CDC) is kept for reference.

### 3.9 `voice.rs`

Everything runs on the tablet; nothing is sent online.

* **Engine:** [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) v2.0.1 (Apache-2.0)
  through its official Rust crate (`sherpa-onnx = "=2.0.1"`, feature `shared`; Android only
  links shared). The build script downloads the matching prebuilt archive into
  `target/sherpa-onnx-prebuilt/`; the two Android `.so` files are also kept in
  `android-libs/arm64-v8a` and packed into the APK by cargo-apk's `runtime_libs`. They need
  only Android system libraries (no `libc++_shared`).
* **Model:** `sherpa-onnx-streaming-zipformer-en-20M-2023-02-17` (mobile; Apache-2.0), files in
  `assets/voice/en-20M` (`MODEL_FILES`): int8 encoder (33 MB) and joiner, fp32 decoder,
  tokens. The APK carries them as assets; `lib.rs::voice_model_dir()` copies them to the app's
  own folder (`files/voice/en-20M`) the first time, again only if a file's size changes.
  The Speediance app ships sherpa-onnx too (for its wake word), so the engine is known to run
  on this tablet.
* **Microphone:** AAudio through the `ndk` crate (`audio` feature): input, shared, 16 kHz mono
  float requested (the input preset stays AAudio's default, VOICE_RECOGNITION). The actual
  rate / format / channels are read back and converted, and sherpa-onnx resamples if needed.
  100 ms reads. **ndk 0.9 bug:** `AudioStream::read` returns every non-zero frame count as
  `Err(__Unknown(n))`, so the raw code is taken back (`i32::from(err)`) and `frames_read()`
  treats >= 0 as frames (the first tablet build stopped with "microphone stopped:
  __Unknown(1600)"). The tablet has a clip-on USB mic (UM02, 44.1/48 kHz mono) and a built-in one;
  Android picks the input.
* **Permission:** the manifest has `RECORD_AUDIO`. `platform::run` checks
  `checkSelfPermission`; if missing it calls `requestPermissions` once per app run and ends.
  Android's question takes the screen (Free Lift loses focus, which stops voice and lets go of
  the machine as usual); when Free Lift is back, `update()` starts voice again and it runs.
  If refused, it explains how to allow it in Android Settings.
* **`Recognizer`** (also compiled for the PC tests): greedy search, `THREADS` = 2, endpoint
  rules 2.4 s / `END_SILENCE_S` = 0.8 s / 12 s. `feed()` returns `Heard` events:
  * **early stop:** when the running (partial) text is a stop / weight off (`is_stop`), it is
    sent at once with `early_stop: true` (once per phrase), not after the phrase ends;
  * the finished phrase at each endpoint.
* **Sound-alikes:** the model writes "weight" as "wait", "weigh", "way"… ("weight on" came out
  as "wait on"). `normalize()` rewrites every word in `WAKE_WORDS` (except weight/weights) as
  "weight" before `commands::parse`.
* **Listening styles** (`Listen`, saved as `voice_listen`, chips on the Voice card):
  * `Always`: every phrase is normalized and parsed.
  * `WakeWord`: `wake_gate(text, armed)`: a stop always runs; a phrase starting with a wake
    word (after "hey" / "ok"…) runs the rest ("Weight, set to twenty"), or the whole phrase if
    the rest isn't a command ("weight on", "weight up"); the wake word alone arms the next
    phrase for `WAKE_WINDOW_S` = 6 s ("Yes? Say the command."); anything else is ignored.
* **App side** (`app.rs`): `voice_wanted` is the switch (saved as `voice_on`); the thread runs
  only while Free Lift is on screen and awake (`update()` stops it on focus loss / sleep and
  restarts it, at most every 2 s, unless it failed). `heard()` acts on an early stop with
  `Command::WeightOff`, skips the same phrase when it arrives finished, applies the listening
  style, and logs phrases that aren't commands as "Not a command I know — ignored."
  Speaking counts as use for the sleep timer.
* **Mic level / silence:** the voice thread sends `VoiceEvent::Level` (loudest sample of
  the last `LEVEL_READS` × 100 ms). The Voice card shows a "Mic" bar while listening
  (`level_fill`: -60 dB empty, 0 dB full). If the level stays under `SILENT_PEAK` (≈ -66 dB)
  for `SILENT_FOR_S` (4 s), `mic_level()` shows `MIC_SILENT_MSG` in orange and writes it
  to the voice log; it clears when sound returns. Why: the clip-on UM02 has its own mute
  button (it reports KEY_MUTE etc. as a HID device) that silences the mic inside the mic,
  where Android can't see it, so the stream runs and "Listening" shows but only zeros
  arrive. Android's own mic mute: `adb shell dumpsys audio | grep "mic mute"`.
* **Test recording (diagnosis):** with voice listening, create the flag file
  `adb shell touch /data/data/com.craftanything.freelift/files/record-mic`. Within a second
  the voice thread removes it and saves the next `TEST_RECORD_S` (20 s) of microphone sound,
  exactly as the recogniser gets it, to `files/mic-test.wav` (16-bit mono, `wav_bytes`); the
  Voice box says when it starts (with the stream's rate / channels / format) and when it is
  saved. Copy it with `adb exec-out cat …/files/mic-test.wav > mic-test.wav`. Ask the owner
  before recording. Nothing is recorded without the flag.
* **Diagnosis tools (PC, `--ignored`):** `recognise_wavs` (files in `FREELIFT_WAVS`, `;`
  separated; `FREELIFT_WARM=1` feeds `voice-test/0.wav` first; also with the `HighPass`
  filter at `FREELIFT_HPF` Hz) and `trace_wav` (running text every 100 ms of
  `FREELIFT_WAV`, endpoints marked, fed continuously like the tablet). In Git Bash set
  `MSYS2_ENV_CONV_EXCL=FREELIFT_WAVS` so the path list isn't rewritten.
  **Findings 2026-11-18:** the tablet's clip-on mic hears a steady mains hum (60 Hz plus 40,
  120, 140 Hz; about -36 dB RMS, -25 dB peak). A 120 Hz high-pass didn't help. Windows
  text-to-speech clips are **not** a fair live test: they contain exact digital silence, and
  fed continuously (never `input_finished`) the model writes nothing for them, while
  `0.wav` (real speech) works live. `recogniser_hears_recorded_speech` only passes because
  `finish()` marks each clip finished. A real recording from the tablet (`mic-test.wav`,
  06:57, owner speaking loud and clear, background -48 dB) that the **tablet** understood
  phrase by phrase gave nothing on the PC either, also without endpoint resets
  (`FREELIFT_KEEP_EMPTY=1` in `trace_wav`) and at other gains (`FREELIFT_GAIN`). So the PC
  copy of the engine does not reproduce the tablet on loud real speech (likely the int8
  encoder's arithmetic on x86 vs ARM). **Judge recognition from the tablet's own
  `voice-heard.txt`**, not from PC replays. The hum (-25 dB peak at 06:50) was gone by
  06:57; its source is unknown.
* **Voice log file:** every phrase heard is appended to `files/voice-heard.txt` in the app's
  folder (`append_voice_log`, `voice_log_line`): local time, listening style, the
  recogniser's text, what it was read as (after `normalize` / the wake word) and every reply
  it produced, or why it was ignored. Voice errors go there too. Over 512 KB it moves to
  `voice-heard.old.txt`. Read it with
  `adb shell cat /data/data/com.craftanything.freelift/files/voice-heard.txt` and tune the
  words in `commands::parse` / `WAKE_WORDS` to what the owner's voice actually produces.
* **Recogniser spellings seen on the tablet:** "barbell" → "barbel" / "barbelle", "mode" →
  "mote". `parse` accepts those barbell spellings and "dumbbell(s)" / "dumb bells" (the
  Dumbbells Setup button, `Command::Independent(true)`).

### 3.10 `app.rs` – the screen and the machine sync

**Constants:**

| Constant | Value | Meaning |
|---|---|---|
| `RESEND_DELAY_MS` | 300 | delay before re-sending enable after a change while loaded |
| `TWO_PLAYER_GAP_MS` | 50 | A1 → A2 gap (original: Duration 0xc350 µs in 0x1c84154) |
| `MACHINE_LOG_LINES` | 40 | lines kept in the machine log |
| `RETRY_S` | 3 | reconnect interval while the port is busy |
| `ORIGINAL_PACKAGE` | `com.speediance.faraday` | app whose info page the banner opens |
| `SLEEP_DEFAULT_MIN` | 30 | default "Sleep after" minutes (owner's choice; 0 = never) |
| `SLEEP_MAX_MIN` | 240 | largest "Sleep after" the setting allows |
| `SLEEP_MOVE_M` | 0.05 | cable movement (m) that counts as use, so a workout never sleeps |

**Layout constants:**

| Constant | Value | Meaning |
|---|---|---|
| `CARD_R` / `CARD_M` / `GAP` | 18 / 16 / 12 | card corner radius, inner margin, space between cards |
| `CARD_EDGE` | 42,46,55 | card outline colour |
| `TOP_BTN_H` | 42 | top-bar button height |
| `WEIGHT_BTN_H` | 76 | Weight on / off button height (full width) |
| `SETUP_BTN_ROW_H` | 48 | Setup row with Barbell / Dumbbells / Assist |
| `SETUP_ROW_H` / `SETUP_WIDGET_H` | 30 / 29 | Setup row 2 (Machine, Rest timer, Sleep after) |
| `STEPPER_LABEL_H` | 24 | label above a weight stepper |
| `PAIR_MIN_H` | 120 | smallest Weight Mode / Voice height |

**Colours:**

| Name | RGB | Name | RGB |
|---|---|---|---|
| `BG` | 13,14,17 | `ACCENT` | 92,141,230 |
| `PANEL` | 24,26,31 | `ON` (green) | 64,196,120 |
| `PANEL2` | 33,36,43 | `WARN` (orange) | 240,160,70 |
| `TEXT` | 236,238,242 | `LEFT_C` | 110,160,245 |
| `MUTED` | 140,146,158 | `RIGHT_C` | 160,120,240 |

**Settings file:** `settings.json` in the app's internal files folder (`DATA_DIR`, for example
`/data/user/0/com.craftanything.freelift/files/settings.json`). It holds `voice_on`, `pounds`,
`rest_seconds`, `rest_on`, `smart_off`, `practice_lifter`, `baud`, `model`, `sleep_min`
(minutes before sleep, 0 = never, default 30) and `last` (the
last weight per cable, mode, barbell, eccentric/chain extra and speed, like the original's
0x1c4f0fc, plus `accessory`: Barbell / Dumbbells / Assist). The file is read in `new()`: with
no `last`, the original's defaults are used (Assist selected); a file without `accessory`
gives Barbell if `barbell` was saved, else Assist. It is written by `save_settings()` on
setting changes, when a set starts, on typed/voice weight, mode and Setup changes, when the
app leaves the screen, and on exit. Missing fields fall back to the defaults
(`serde(default)`).

**State kept for the machine:**

| Field | Meaning |
|---|---|
| `link` | `Option<MachineLink>` |
| `link_ok` | port opened |
| `loaded` | an enable was sent and no pause since |
| `heartbeat_on` | heartbeat running |
| `last_enable` | last enable bytes sent |
| `changed_at` | debounce timer |
| `counts` / `count_base` | raw machine rep counters / counters at set start |
| `ack_seen` | first 0x83 reply seen |
| `safety_stop` | reason text for the red banner |
| `off_check`, `off_alert` | weight-off check in progress (first / last pause sent, re-sends) and its red banner |
| `weight_card_h` | tallest the Free Lift box has been drawn (keeps its size fixed) |
| `port_busy`, `next_retry` | busy-port banner and reconnect timer |
| `sleep_min`, `last_use`, `use_cable_m`, `asleep` | sleep setting, time of the last tap / key / cable movement, cable positions noted then, asleep now |
| `cable_status`, `cable_frames`, `cable_last`, `last_live` | shown in the Machine window |
| `machine_log` | sent commands and machine replies |

**Frame loop (`update`, every frame):**

1. `fit_width`: zoom so the 1080-point layout fills the screen width.
2. `poll_voice()`, then `poll_link(dt)` (an `Opened` link calls `Machine::machine_connected`),
   then `check_sleep()`: a touch, key or text event, or either cable moving `SLEEP_MOVE_M`
   since last noted, resets `last_use`; after `sleep_min` minutes without one
   (`sleep_due`) it calls `go_to_sleep(false)`. The 🌙 Sleep button in the top bar calls
   `go_to_sleep(true)` at once (same path; only the message differs).
3. Android focus handling:
   * When the app **loses focus** (Home, Settings, another app) it calls
     `disconnect_machine()`. That turns the weight off and releases the port so the
     Speediance app can use it.
   * When focused again with no link, it connects to `/dev/ttyS5`, retrying every 3 s
     (not while asleep).
   * **While asleep** the frame stops here: `sleep_screen()` draws a black panel with
     a dim "Asleep · tap to wake" that jumps to a new place every `SLEEP_TEXT_MOVE_S`
     (10 s; `sleep_text_spot`, so it can't burn in) and repaints every 500 ms. A pointer release or key press calls
     `wake_up()`, and the next frame reconnects as above. Taps in the first
     `SLEEP_WAKE_GUARD_S` (0.8 s, from `slept_at`) are ignored, so the Sleep button's own
     tap can't wake it again.
4. `check_weight_off()`, then `shortcuts()`: V = voice, Space = weight on/off, ↑/↓ = step.
5. `machine.live_fresh` is set, then `machine.step(dt)` and `session.sample_power(...)`. A
   smart-off event writes a message.
6. `after_weight_change()`, then `session.tick(dt)`.
7. Draw the top panel and the column: `weight_card`, `setup_card`, then the remaining height
   is split by `fill_heights()` between `live_card`, `mode_and_voice` and `sets_card` (§5).
   Then the windows: `cable_window` ("Machine connection") and `summary_window`.
8. `after_weight_change()` and **`sync_machine()`**, so whatever the person did this frame
   reaches the motors. Then `request_repaint()`.

`on_exit` saves the settings, stops voice and disconnects.

**`run_command(heard, cmd)`:**

* Session commands are handled here:
  * `StartWorkout` (the Start Workout button, "start workout") starts the workout clock
    (`Session::started`); after End Workout it first begins a new session. The clock no
    longer runs from app start; the first Weight on also starts it;
  * `Pause` turns the weight off and freezes the clock; `Resume` restarts it;
  * `SkipRest` and `FinishSet` (weight off);
  * `EndTraining` turns the weight off, sets the phase to Done and opens the summary;
  * next / previous exercise and `VoiceOff`.
* Everything else goes to `Machine::apply`. Units, smart load off, mode, Setup (Barbell /
  Dumbbells / Assist / level), weight, extra and speed changes save the settings.
* The reply goes to the voice/log panel. It is red if it contains "can't", "cannot" or
  "isn't", or starts with "No " or "You're not".

**`after_weight_change()`** turns weight on/off transitions into sets:

* **off → on:** a new set starts. The phase becomes Lifting, `start_set()` runs and the rep
  counters reset.
* **on → off:** if at least one rep was counted, a `SetRecord` is pushed and the phase
  becomes Rest (rest timer on) or Ready (rest timer off).

**Live data (`poll_link`):**

* For `Frame::Live`, reps = `set_reps(count_base, counts)`. The machine **restarts its rep
  counters at every weight-on**, so a counter below the base resets the base to 0. This fixed
  the "245 reps" bug.
* Distance and force go to `Machine::apply_live`.

---

## 4. Talking to the machine (the important part)

Everything here was decoded from `libapp.so` (Dart AOT) and **checked byte-for-byte against
frames the original logged on this tablet** (`/data/data/com.speediance.faraday/databases/
logger.db`, text "下位机数据:发送<name>指令: [bytes]").

### 4.1 The link

* **Device:** `/dev/ttyS5`, 115200 baud, raw 8N1. On this tablet the board is a UART, not USB.
* **Exclusive use:** the Speediance app (`com.speediance.faraday`) is the tablet's **HOME
  launcher** and keeps `/dev/ttyS5` open. While it runs, opening the port fails with
  "Device or resource busy" (os error 16). Free Lift then shows the orange banner and its
  button opens Speediance's App info page, where the person taps **Force stop**. Free Lift
  retries every 3 s.
* Pressing Home restarts Speediance. Free Lift lets go of the port whenever it leaves the
  screen, so the two apps don't fight over it.
* **Free Lift as the Home app (optional):** the manifest (`Cargo.toml`,
  `package.metadata.android.application.activity`) lists a `MAIN` + `HOME` + `DEFAULT`
  intent filter next to the launcher one, so the owner can pick Free Lift in Android
  Settings › Apps › Default apps › Home app. The tablet then starts Free Lift at power-up
  and Home returns to it; Speediance doesn't start, so the port is free. `launchMode =
  singleTask` keeps it to one Free Lift (Home / the icon bring the running one back; a
  second NativeActivity would run a second `android_main` and open the port twice).
  Undo: pick the Speediance app (or Launcher3) there. Open questions: whether the motor
  board needs anything from Speediance after a cold boot (test at a low weight the first
  time), and whether Speediance, a privileged system app, takes Home back after an update.
* **Permissions:** the device node is accessible to apps on this tablet (SELinux is
  permissive). `su` is not available to apps, only to the adb shell.

### 4.2 Frames from the machine: `AA AB <len> <type> <payload…> <crc lo> <crc hi>`

* `len` is the total frame length, including the header and CRC.
* The CRC is CRC-16/X-25 over every byte before it, **low byte first**.

**Type 0x80 – live data, 60 bytes:**

| Byte(s) | Field |
|---|---|
| 0–1 | `AA AB` |
| 2 | 60 |
| 3 | `0x80` |
| 4 | training family |
| 5 | sub-mode |
| 6 | **rep counter, left** (restarts at weight-on) |
| 7 | **rep counter, right** |
| 8 | left block flag (unknown) |
| 9–16 | left `a`: f64 big-endian, taken as cable speed m/s (inferred) |
| 17–24 | left `b`: f64 BE; **distance = −round(b, 4)** metres |
| 25–32 | left **torque in lb** (÷ 2.2 for kg) |
| 33 | right block flag |
| 34–41 / 42–49 / 50–57 | right `a` / `b` / torque, as above |
| 58–59 | CRC, low byte first |

These names match the original's decoder 0x1b9c29c (`countLeft/Right`, `distanceLeft/Right`,
`torqueLeft/Right`). With the weight off the torque reads about 7.7 lb (3.5 kg) of base
tension.

**Other types** (dispatch 0x1b973ec):

| Type | Meaning | What Free Lift does |
|---|---|---|
| `0x83` | command reply; byte 4 = 1 accepted, else error | first accept → "The machine confirmed: weight on."; error → message |
| `0x82` | **safety stop** (0x1b98cdc); byte 4 = reason | stop heartbeat, mark weight off, red banner |
| `0x84` | hardware code | logged |
| `0x85` | serial number | logged |
| `0x8c` / `0x8d` | rowing / skiing status | logged |
| `0x81`, `0x8a`, `0x8b` | machine status | logged |

Safety-stop reasons: 1 "The barbell is not balanced", 2 "Loss of grip on the handle or
barbell", 3 "Cable retraction too fast", 4 "Cable extension too fast", 5 "Motor temperature
too high", 6 "The motor is locked. Please try to restore it.", other "Firmware runtime error".

### 4.3 Commands to the machine: `CC CD <len> <cmd> <payload…> <crc hi> <crc lo>`

The CRC is CRC-16/X-25 over all bytes before it, but **high byte first**, the opposite of the
machine's frames.

| Cmd | Name in the original | Frame | Used by Free Lift |
|---|---|---|---|
| `01` | enable (weight on) | 18 bytes, below | weight on / change |
| `02` | disable | `CC CD 06 02 crc` | on disconnect, after pause |
| `03` | pause (**weight off**) | `CC CD 06 03 C3 9D` | weight off |
| `04` | reset | 6 bytes | not used |
| `06` | getVersionCode | 6 bytes | not used |
| `07` | getSn | 6 bytes | not used |
| `08` | sleep | `CC CD 06 08 7D 4E` | the app's Sleep, after pause + disable |
| `09` | wakeup | `CC CD 06 09 6C C7` | first frame after reconnecting from the app's Sleep |
| `FE` | heartbeat | `CC CD 07 FE <n % 255> crc` | every 500 ms after the first enable |
| `A1` / `A2` | enable_2Player (left / right) | 17 bytes, below | independent mode |

**Enable (0x01), 18 bytes** (builder 0x3a9bdb0):

| Byte(s) | Value |
|---|---|
| 0–1 | `CC CD` |
| 2 | `0x12` (18) |
| 3 | `0x01` |
| 4 | mode: 1 standard, 2 chain, 3 eccentric, 4 fixed speed |
| 5–12 | **weight per cable in lb**, f64 big-endian. In barbell mode the screen shows the total and the original **halves** it. |
| 13 | assist / "help": always 0 in the original builder |
| 14 | 1 = not barbell, 2 = barbell |
| 15 | extra (eccentric/chain) in lb, **rounded**, halved in barbell mode |
| 16–17 | CRC, high byte first |

Golden example from the tablet (eccentric, 20 lb, +5 lb):
`CC CD 12 01 03 40 34 00 00 00 00 00 00 00 01 05 14 75`.

**Two-player enable (0xA1 left, 0xA2 right), 17 bytes** (builder 0x3acafec):

| Byte(s) | Value |
|---|---|
| 0–3 | `CC CD 11 A1` (or `A2`) |
| 4 | mode |
| 5–12 | that cable's weight in lb, f64 BE, **not halved** |
| 13 | 0 |
| 14 | extra lb, **truncated** |
| 15–16 | CRC, high byte first |

The Free Lift page sends A1, then A2 **50 ms later**. This frame is decoded from the code but
has not yet been seen in the tablet log.

Free Lift keeps kilograms internally and converts with `LB_PER_KG = 2.2` only when building
frames (`app.rs::enable_frames`).

### 4.4 Sequences Free Lift uses

1. **Connect** (app start / regaining focus): open and configure the port. Nothing is sent.
   The machine dot in the top bar turns green. If the weight was turned on in practice mode before
   the port opened, it is turned off (`Machine::machine_connected`) and the person is asked
   to tap Weight on again: **connecting never starts the motors.**
2. **Weight on** (button, Space, voice/typed):
   1. `Machine::apply(WeightOn)`; `sync_machine` sees `weight_on && !loaded`.
   2. Send the enable frame, or A1 → wait 50 ms → A2 with Dumbbells selected.
   3. Start the heartbeat (the first beat 500 ms later; it keeps running until disconnect,
      like the original, where pause does not stop it).
   4. Set `loaded = true`, `count_base = counts`, clear `safety_stop`.
   5. The machine answers `0x83 01`.
3. **Change while on** (weight, extra): the new enable frame(s) differ from `last_enable`.
   They are re-sent **300 ms** after the first change is seen (later changes in that window
   go out with it), and no pause is sent in between. Switching to or from Barbell or
   Dumbbells is refused while on (switching to Assist is allowed when the handles are
   already linked, since the frame doesn't change).
4. **Mode switch while on:** the original's rule turns the weight off first, so a pause is
   sent. The person turns the weight on again in the new mode.
5. **Weight off** (button, voice, smart load off, Pause, Finish set, End Workout): send
   **pause** `CC CD 06 03 C3 9D` and set `loaded = false`. The heartbeat continues.
   **Weight-off check** (`check_weight_off`): `OFF_CHECK_S` later the latest live force on
   both cables must be at most base tension + `OFF_MARGIN_KG`. **The machine stops streaming
   live frames shortly after the weight goes off** (seen on the tablet), so the check uses the
   last frame received after the weight-off, not whether data is still arriving. If the force
   is too high, pause is re-sent (up to `OFF_RESENDS` times), then a red banner warns the
   person (with a "Send weight off again" button). If no live frame at all arrived after the
   weight-off, it warns that the weight-off couldn't be confirmed.
   A command reply with an error code (0x83, code ≠ 1) also turns the weight off.
6. **Leaving the screen / exit / Disconnect button:** `close(heartbeat_on)` → the link
   thread sends **pause + disable** (if this connection used the weight), then closes the
   port.
   **Sleep** (`go_to_sleep`, after `sleep_min` minutes with no taps and no cable movement,
   or at once from the 🌙 Sleep button) calls `release_machine(true)`: while connected it
   sends `close_and_sleep()` → **pause, disable, sleep (0x08)**, and sets `board_asleep`
   (saved in settings). The next `In::Opened` sends **wakeup (0x09)** first, clears the
   flag and says "Woke the machine's motor board."; waking reconnects (step 1), so the
   weight is always off after a wake. Added 2026-11-18 because the cables kept their resting
   tension (7.7 lb) through Free Lift's sleep. Untested on the machine so far (§8).
7. **Safety stop (0x82):** heartbeat off, weight off, `loaded = false`, red banner with OK.
8. **Fixed-speed mode:** weight on is refused before anything is sent
   (`unsupported_on_machine`).

**Assist (spotter)** is kept in the UI and the settings, but the original's enable builder
always sends 0 in byte 13, so nothing about assist reaches the machine yet.

### 4.5 Debugging the link

* **Machine window** (🔌 Machine in the top bar): status, frame count, the last decoded live
  frame (torque, distance, speed per side), the **machine log** (every sent command in hex
  plus every reply), and a preview of the weight-on / weight-off / heartbeat frames for the
  current settings.
* **The original's own log:**
  `adb exec-out cat /data/data/com.speediance.faraday/databases/logger.db > logger.db`, then
  search for `发送…指令: [..]`. Use the same method to capture the original's frames for a
  feature that isn't decoded yet (for example fixed speed).
* **Port holder:** `adb shell "ls -l /proc/*/fd 2>/dev/null | grep ttyS5"` or
  `adb shell lsof | grep ttyS5`.
* **App log:** `adb logcat | grep -i -E "freelift|RustStdoutStderr|eframe"`.

---

## 5. Screen layout (1080 × 1920 portrait)

The UI is drawn at 1080 points wide and zoomed to the screen (`fit_width`). It is one column
inside an 18-point margin, on a dark theme (`style()`), and it **fills the whole 1080 × 1920
screen** (see the notes below). Approximate heights on the tablet are on the right.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ FREE LIFT [Start Workout|End Workout] [Pause|Resume] ●                   │  top bar  ~70
│    ⏱ 00:00  ⚙ Settings  🌙 Sleep  🔌 Machine  [kg|lb]  [🎤 Voice: OFF/ON]  │  (right-aligned)
│ ▓ red banners: safety stop / weight-off not confirmed (Send again, OK)   │  only when needed
│ ▓ orange "Speediance app is using the machine…" + App info button        │  only when needed
├──────────────────────────────────────────────────────────────────────────┤
│ ◀ Free lift ▶  SET 1                           [READY|LIFTING|REST|PAUSED]│  weight_card ~420
│                      PER SIDE / TOTAL (BARBELL)                          │  (fixed height)
│               ( − )            21 lb             ( + )                   │
│        (Dumbbells: two columns, LEFT and RIGHT steppers, same height)    │
│                    Range 8 – 110 lb · Gym Monster 2                      │
│ [              WEIGHT OFF — tap to turn on   (full width, 76 high)     ] │
│              Mode: Standard · Assist L1 · Smart load off                 │
├──────────────────────────────────────────────────────────────────────────┤
│ SETUP [Barbell] [Dumbbells] [Assist] [Level 1][Level 2]  ☐Smart load off │  setup_card ~120
│       Machine [Gym Monster 2 ▾]   ☐Rest timer [60 s]  Sleep after [30 min]│  row 2
├──────────────────────────────────────────────────────────────────────────┤
│ LIVE                  Live from the machine / Machine idle / ☐Practice   │  live_card 26 %
│  LEFT ▌bar   RIGHT ▌bar   ┌ Power L · R            scale N W ┐           │
│  reps        reps         │ guide lines; L blue + R purple,  │           │
│   0           0           │ one accent line in barbell mode  │           │
│  7.7 lb      7.7 lb       └──────────────────────────────────┘           │
├────────────────────────────────────┬─────────────────────────────────────┤
│ WEIGHT MODE                        │ VOICE CONTROL                 (o)   │  mode_and_voice
│ [ Standard ] [ Eccentric ]         │ status text                         │  28 %, same height,
│ [ Chain    ] [ Isokinetic ]        │ [type a command……] [Go]             │  voice scrolls
│ extra slider / speed slider        │ ▸ What can I say?  + reply log      │
├────────────────────────────────────┴─────────────────────────────────────┤
│ SETS                                                                     │  sets_card: the rest,
│ # Exercise Weight RepsL/R Mode Volume AvgPower Time RestBefore          │  to the bottom edge;
│ (or "No sets yet")                                                       │  newest first, scrolls
└──────────────────────────────────────────────────────────────────────────┘
 Windows: "Machine connection" (🔌). After End Workout: the Training summary, an
 `egui::Modal` drawn as a card (`card_frame`) over a dimmed screen: four tiles (time, sets,
 reps, volume), **New workout** (= StartWorkout: clears the sets, starts the clock) and
 **Close** (or tap outside; the sets stay on screen).
```

Top bar, left: **Start Workout** (green) starts the workout clock and becomes **End Workout**
(dark red; ends the session and opens the summary). The button is one fixed size
(`WORKOUT_BTN`, text centred with `add_sized`) so it doesn't move. **Pause** (becomes
**Resume**, orange) sits right of it, greyed until the workout runs. The dot is the machine
link: green = machine connected, orange = practice machine (hover text says which).
`top_bar_fits_the_screen` checks that the bar fits 1080 points with the longest texts.

Layout notes (changes the owner asked for):

* **The screen is filled top to bottom.** `update()` measures the space left under the Setup
  strip (after the item spacing of the two remaining gaps) and `fill_heights()` splits it:
  Live 26 %, Weight Mode / Voice 28 %, Sets the rest (down to the bottom edge), with minimums
  (240 / 280 / 220) so it still scrolls on a smaller screen. `live_card`, `mode_and_voice` and
  `sets_card` take that height; the reps column, the cable bars, the power graph, the four
  mode buttons (54–100 pt) and the sets table grow with it. Banners at the top shrink these
  three boxes, not the weight card.
* **Look:** every card is `card_frame()` (rounded `CARD_R` 18, thin outline `CARD_EDGE`,
  margin `CARD_M` 16, gap `GAP` 12); titles use `section_title` (accent bar + muted bold text);
  the top bar uses `top_btn` (`TOP_BTN_H` 42) and `status_pill` (green or orange, with a dot
  drawn by hand because the default fonts have no "●"). The phase pill (READY / LIFTING /
  REST / PAUSED) is tinted in its own colour. Body text is 16 pt.
* The weight is drawn at 150 pt (100 pt per side with Dumbbells), with − / + buttons scaled to
  it (`stepper_button`, outlined circles).
* The weight-on button is full width and `WEIGHT_BTN_H` (76 pt) high, text 24 pt (the owner
  asked for it bigger; it was half width × 32 pt before).
* Setup row 1: **Barbell**, **Dumbbells** (= independent left / right) and **Assist** are one
  choice (`machine::Accessory`, `Machine::set_accessory`): exactly one is always selected, and
  tapping one turns the other two off. Assist = linked handles with the spotter on (the
  machine gets the same frame as plain linked handles, since assist isn't sent). Voice
  "non barbell" / "linked" / "turn off assist" never leave nothing selected: they move to
  Assist or keep it. Switching that changes barbell or dumbbells is refused while the weight
  is on. The choice is saved in `settings.json` (`last.accessory`; older files: Barbell if
  `last.barbell`, else Assist) and restored at start. Smart load off is still a checkbox.
* The Free Lift box keeps one height: in Dumbbells mode the two smaller steppers are padded to
  the single stepper's height (`stepper_height`), and the box never shrinks below the tallest
  it has been drawn (`weight_card_h`).
* The LEFT/RIGHT labels and the − / + glyphs are centred in their buttons (`weight_stepper`).
* SETUP is a horizontal strip between the weight card and LIVE. Row 1 is `SETUP_BTN_ROW_H`
  high (the Setup buttons, `setting_btn`, 44 pt, accent fill when selected; the Level chips
  are enabled only with Assist). Row 2 uses `SETUP_ROW_H`, and `interact_size.y =
  SETUP_WIDGET_H` keeps the dropdown, labels and boxes on one centre line; it starts under
  the first button. "Sleep after" is a drag / type box (0 shows "Never", up to 240 min).
* **Sleep screen:** while asleep nothing else is drawn, only black with a dim "Asleep · tap to
  wake" that moves to a new place every 10 s (against burn-in). The wake happens on release and no buttons exist under that tap, so it can't press
  anything. The backlight stays on (Android's own screen timeout is "never" on this tablet).
* WEIGHT MODE and VOICE CONTROL sit side by side at the same height (from `fill_heights`;
  if the Weight Mode content comes out taller, Voice matches it via `pair_heights`). The
  mode-button height depends only on the box height, so sizes can't creep between frames.
  The voice content scrolls inside its box.
* The Live box says "Live from the machine" (green) while data arrives, "Machine idle · live
  data only while the weight is on" (grey) with the weight off, and "Machine connected · no
  data yet" (orange) only when the weight is on but nothing has arrived.
* Sets show the extra weight (eccentric/chain) or the speed, and L/R weights when they
  differ. The column gap spreads the table across the card; "No sets yet" is centred.
* The power graph uses a separate colour per cable, or one line in barbell mode, over three
  faint guide lines, with its current scale in the corner.

Widget helpers at the bottom of `app.rs`: `card_frame`, `card_inner`, `card`, `card_header`,
`section_title`, `sized_card`, `status_pill`, `pill`, `top_btn`, `setting_btn`, `small_btn`,
`chip`, `toggle` (an iOS-style switch), `weight_stepper`, `stepper_button` and
`stepper_height`.

---

## 6. Build, install, run

### 6.1 Tools (this PC)

* **Rust** stable with the target `aarch64-linux-android`
  (`rustup target add aarch64-linux-android`) and the host target
  `x86_64-pc-windows-msvc` for tests.
* **cargo-apk** (`cargo install cargo-apk`).
* **Android SDK:** `C:\Users\kylel\AppData\Local\Android\Sdk`, with build-tools and platform 33.
  **NDK** 27.0.12077973 in `Sdk\ndk\27.0.12077973`.
* **adb:** `C:\Users\kylel\AppData\Local\Microsoft\WinGet\Packages\Google.PlatformTools_Microsoft.Winget.Source_8wekyb3d8bbwe\platform-tools\adb.exe`.
* `.cargo/config.toml` sets the default target and points the linker at
  `…\ndk\27.0.12077973\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android26-clang.cmd`.
  The archiver is `llvm-ar.exe` from the same folder.

### 6.2 Commands (Git Bash, in `app/`)

```bash
# compile check for the tablet
cargo build

# unit tests (43) run on the PC: override the default Android target
cargo test --lib --target x86_64-pc-windows-msvc

# APK
export ANDROID_HOME='C:\Users\kylel\AppData\Local\Android\Sdk'
export ANDROID_NDK_ROOT="$ANDROID_HOME\\ndk\\27.0.12077973"
cargo apk build --release --lib          # --lib: package the cdylib (cargo-apk refuses a bin)
cp target/release/apk/FreeLift.apk dist/FreeLift.apk

# install on the tablet (wireless adb)
ADB=/c/Users/kylel/AppData/Local/Microsoft/WinGet/Packages/Google.PlatformTools_Microsoft.Winget.Source_8wekyb3d8bbwe/platform-tools/adb.exe
$ADB connect 192.168.2.50
MSYS_NO_PATHCONV=1 $ADB -s 192.168.2.50:5555 push dist/FreeLift.apk /data/local/tmp/FreeLift.apk
MSYS_NO_PATHCONV=1 $ADB -s 192.168.2.50:5555 shell pm install -r -t /data/local/tmp/FreeLift.apk
# launch
MSYS_NO_PATHCONV=1 $ADB -s 192.168.2.50:5555 shell monkey -p com.craftanything.freelift 1
```

* Copy, then install: over the tablet's slow Wi-Fi a direct `adb install` often fails
  silently. Retry the push if it times out.
* **When the Wi-Fi can't carry the file** (the tablet's Wi-Fi often drops during big
  transfers while small commands still work): the owner copies `dist/FreeLift.apk` to a USB
  drive and plugs it into the tablet. It appears under `/mnt/media_rw/<drive id>/`. Check its
  fingerprint against the PC's (`md5sum`), then install from the tablet itself:

  ```bash
  MSYS_NO_PATHCONV=1 $ADB -s 192.168.2.50:5555 shell 'md5sum /mnt/media_rw/*/FreeLift.apk'
  MSYS_NO_PATHCONV=1 $ADB -s 192.168.2.50:5555 shell 'cp /mnt/media_rw/<id>/FreeLift.apk /data/local/tmp/FreeLift.apk && pm install -r -t /data/local/tmp/FreeLift.apk'
  ```

  The same works from the tablet's Downloads folder (`/sdcard/Download/`).
* `MSYS_NO_PATHCONV=1` stops Git Bash from rewriting `/data/...` paths into Windows paths.
* The adb daemon on the tablet restarts sometimes; run `adb connect` again first.
* **Installing closes Free Lift at once.** If the weight is on, that cuts the heartbeat
  mid-set. Before installing, check the weight is off: take a screenshot
  (`adb exec-out screencap -p > screen.png`) or ask the person. Never send taps with
  `adb shell input` to check things on the screen: the Weight on button is close to the
  Setup buttons.

**GitHub:** the private repository `kyleken/FreeLift` holds this folder (`main`). The APK is
not committed (`.gitignore`); attach each build to a Release instead:
`gh release create vX.Y.Z dist/FreeLift.apk --notes "…, md5 …"`.

### 6.3 Tablet facts

* Rockchip RK3568, Android 11, 1080 × 1920 at density 240.
* Wireless adb at **192.168.2.50:5555**; the adb shell runs as root and SELinux is
  permissive.
* The motor board is `/dev/ttyS5` at 115200. The Speediance app is the HOME launcher and
  holds the port.
* The original's data is in `/data/data/com.speediance.faraday/` (`databases/logger.db` has
  the command log).

### 6.4 Craft Anything

The project was started in Craft Anything (`C:\Projects\GMcontrol\CraftAnything`). Its
`rebuild_check` has a build step, which passes, and an "app-screenshot" step that only runs a
desktop program. Because the owner wants Android only, **that screenshot step always fails
and that is expected**. Do not add a desktop build to satisfy it.

---

## 7. Tests

`cargo test --lib --target x86_64-pc-windows-msvc` runs 43 tests (plus the two `--ignored` diagnosis tools in `voice`). The PC tests use
`tools/win-test-runner.cmd` (set in `.cargo/config.toml`): Windows 11 has an older
`onnxruntime.dll` in System32 that is found first unless the sherpa-onnx DLLs sit next to the
test program, so the runner copies them there.

* **protocol:** golden enable/pause frames from the tablet, disable / sleep / wakeup bytes
  (computed independently with the CRC that reproduces the logged pause), two-player
  layout, reply decoding, side values, the CRC check value, splitter resync.
* **machine_link:** `closing_sends_exactly_these_bytes` (what each kind of close writes).
* **machine:** `decompiled_limits`, `fuzzy_delta_rule`, `mode_rules`, `barbell_total`, `extra_cap_and_defaults_like_the_original`,
  `voice_flow`, `smart_off_after_practice_set`, `connecting_never_turns_the_motors_on`,
  `no_barbell_or_independent_switch_while_loaded`, `barbell_dumbbells_assist_are_one_choice`,
  `smart_off_on_the_machine_waits_for_the_cables_to_come_home`, `no_minus_zero`.
* **commands:** numbers, the person's example phrases, all 21 original intents,
  `weight_on_only_when_clearly_asked`, `stop_turns_the_weight_off`, `setup_buttons_by_voice`.
* **session:** rest and power tracking; the rest switch defaults on and rest is measured when
  it is off; `workout_clock_waits_for_start`.
* **app:** `reps_restart_with_the_machine_counter`, `weight_off_is_checked_against_the_cable_force`,
  `sleeps_only_after_the_set_time_without_use`, `voice_log_lines`, `sleep_text_moves_around`, `top_bar_fits_the_screen`.
* **voice:** smart-unload phrases, `mic_level`, `test_recording_wav`, `high_pass_removes_hum_keeps_voice`, `microphone_read_counts`, `wake_word_gate`, `weight_sound_alikes`, and
  `recogniser_hears_recorded_speech` (PC only): the real recogniser on the model's own
  recording and on Windows text-to-speech commands in `voice-test/` ("weight on", "weight off",
  "stop" acted on early, "Weight. Set weight to twenty" in wake-word mode).

Any change to the bytes sent must keep `protocol::tests::commands_match_the_tablet_log`
passing, and should add a new golden case.

---

## 8. Known gaps and next steps

| Item | Status | How to finish it |
|---|---|---|
| Fixed-speed (isokinetic) on the machine | blocked | The weight value comes from 0x1bc324c through a dynamic call (selector 0xff1) that can't be read statically. Ask the owner to do a fixed-speed set in the Speediance app, then read `logger.db` (§4.5) for the `enable` frame with mode 4, and add it as a golden test before unblocking. |
| Voice on the tablet | listening works on the tablet; being tuned | Try both listening styles near the machine with the motors running. Tune `END_SILENCE_S`, `WAKE_WORDS`, `commands::parse` and `THREADS` from `voice-heard.txt` (§3.9). Fallbacks if the gym is too noisy: sherpa-onnx keyword spotting (`kws-models`, 15 MB) or Vosk with a grammar. |
| Two-player frames (Dumbbells) | decoded, unverified | First real test at a low weight; compare with `logger.db` if the original ever logs one. |
| Weight-off check and smart-load-off thresholds | estimated | 1.5 kg margin, 8 cm home, 2 cm still, 0.5 s stale: tune them from the Machine window's live readings on the real machine. |
| Sleep: real backlight off | not done | The sleep screen is black but the backlight stays lit. Dimming it needs JNI (`WindowManager.LayoutParams.screenBrightness = 0`, on the UI thread), restored on wake. |
| Board sleep / wakeup (0x08 / 0x09) | built, untested on the machine | First test: tap Sleep with hands clear of the cables; check the cables lose their resting tension (or what they do). Wake with a tap: the message box should show "Woke the machine's motor board." Then Weight on at the lowest weight. Unknown: whether the board needs a moment after wakeup before enable, and whether the Speediance app wakes a board Free Lift left asleep (if Speediance can't drive the machine after Free Lift's Sleep, open Free Lift once to wake it, or power-cycle the machine). |
| Free Lift as Home: cold boot without Speediance | not tested | After the owner picks Free Lift as the Home app: restart the tablet, then try Weight on at the lowest weight and check the cables pull and let go. If the board needs Speediance's start-up, Free Lift would need to send it (find it in `logger.db` first). |
| What the board does if Free Lift dies mid-set | unknown | Test at 5 kg: Weight on, stand clear, force-stop Free Lift from App info, see if the cables let go. |
| Smart load off timing | 3 s estimated | The original only stores the setting `autoOffWeight` (0 manual, 1 smart-infinite, 2 smart-finite, default 1); the timing is probably in the motor firmware. |
| +/− step, speed range | estimated | Not in the app code; speed range is filled at run time from settings. |
| Assist on the machine | not sent | The original's enable builder sends 0; the assist path is elsewhere and not decoded. |
| Saving sessions between launches | not done | Sessions live in memory; add JSON history in `DATA_DIR` if wanted. |
| 6-second "enter rest" ring | not shown | Documented in MECHANICS.md pass 9; its trigger wasn't read. |

---

## 9. Reverse-engineering toolkit (for finding more values)

* **Original APK:** `C:\Projects\GMcontrol\apk\speediance-3.14.01.apk`. Its Flutter code is
  `lib/arm64-v8a/libapp.so` (Dart 3.12.2 AOT, snapshot hash
  `ace654289f5abc240509fc941453ebc5`).
* Ghidra / `native_function_pseudocode` **cannot** read Dart AOT routines, and xrefs to
  strings are empty (Dart loads them through the object pool).
* **blutter** was built locally (MSVC needed the `/Zc:preprocessor` flag) in
  `%LOCALAPPDATA%\Temp\ca\tools\blutter`. Its output is in `%LOCALAPPDATA%\Temp\ca\blutter_out`:
  `asm/` (per-Dart-file listings), `pp.txt` (object pool: strings, constants, enums),
  `objs.txt`, `ida_script/addNames.py` (routine names).
* Helper scripts are in `%LOCALAPPDATA%\Temp\ca\scripts` and run with the Python venv
  `%LOCALAPPDATA%\Temp\ca\venv` (capstone):
  * `dartx.py uses '<regex on a pp line>'` lists the routines that load a pool object;
  * `dartx.py range 0xSTART:0xEND` disassembles with pool comments;
  * `callers.py 0xADDR` lists call sites.

  **These are in a temp folder and may be cleaned up; copy them somewhere permanent if you
  need them.**
* **Reading tips:**
  * `x22` is `null`; `null + 0x20` is `true` and `null + 0x30` is `false`.
  * Small ints are tagged Smis (value << 1) when passed as objects, but plain after
    `sbfx … #1`.
  * `[x27 + off]` is an object-pool load.
* Every finding goes into `MECHANICS.md` (with the address) and `mechanics.json`, and the code
  uses the constant from `mechanics.rs`.

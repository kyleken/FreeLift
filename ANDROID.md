# Free Lift for the Gym Monster tablet

(Developers: the full technical guide is `docs/DEVELOPER_GUIDE.md`; `CLAUDE.md` has the rules.)

The APK is `dist/FreeLift.apk` (arm64, Android 8.0+; the tablet is an RK3568 on Android 11).

## The screen

Free Lift fills the whole tablet screen (1080 × 1920, portrait). From top to bottom:

* **Top bar:** **Start Workout** starts the workout clock (it no longer starts when the app
  opens; turning the weight on also starts it). The button then says **End Workout**: tap it
  to finish and see the summary. **Pause** (right of it) turns the weight off and stops the
  clock; it then says **Resume**. Next to them is a dot: **green** = machine connected (the
  motors follow the app), **orange** = practice machine (nothing reaches the motors). On the
  right: the workout clock, ⚙ Settings, 🌙 Sleep (sleep now, see below), 🔌 Machine
  (connection details and the command log), kg / lb, and the Voice button.
* **Free Lift box (fixed size):** the exercise and set number, the weight with big − / +
  buttons, and the full-width **WEIGHT OFF — tap to turn on** button.
* **Setup:** pick one of **Barbell** (one total weight for both cables), **Dumbbells**
  (separate left and right weights) or **Assist** (linked handles with the spotter, levels 1
  and 2). One is always selected; tapping another switches straight to it. Free Lift
  remembers your choice for next time. Below them: Smart load off, the machine model, the
  rest timer and **Sleep after** (30 min unless you change it; drag or tap the box, 0 =
  Never).
* **Live:** reps and force per cable, how far each cable is out, and a power graph. The machine
  only sends this while the weight is on; with the weight off it says "Machine idle".
* **Weight Mode** (Standard, Eccentric, Chain, Isokinetic) and **Voice Control**: the on / off
  switch, **Always** or **Wake word**, a box to type a command, and what was heard.
* **Sets:** every finished set, newest first, filling the rest of the screen.

## Controlling the machine

When Free Lift opens on the tablet it connects to the motor board, `/dev/ttyS5` at 115200 baud
(the port the Speediance app uses). The dot in the top bar then turns green.

* **Weight on** sends the original's weight-on command with the current mode, weight, barbell
  setting and eccentric/chain extra (see MECHANICS.md, "Motor commands"), then starts the
  500 ms heartbeat the board expects while loaded.
* Changing the weight or extra while the weight is on re-sends the command 0.3 s after the
  change. Switching mode while loaded turns the weight off first (the original's rule).
* **Weight off** (button, voice/typed command, smart load off, Pause, End Workout) sends the
  original's pause command.
* After Weight off, Free Lift checks that the force on the cables drops back to the resting
  level. If it doesn't, it sends weight off again, then shows a red warning with a "Send
  weight off again" button.
* Connecting to the machine never turns the weight on by itself: if the weight was on in
  practice mode, it is turned off and you tap Weight on again.
* Smart load off turns the weight off only once both cables are back in and have stayed
  still for 3 s, not when you pause at the top of a rep.
* Switching to or from Barbell or Dumbbells only works while the weight is off (the app says
  "Turn the weight off first").
* Reps, cable distance and force in the Live box come from the machine.
* When Free Lift leaves the screen it turns the weight off (if on), sends disable and lets go of
  the cable so the Speediance app can use it again; it reconnects when you come back.
* **Sleep:** after 30 minutes with no taps and no cable movement (change it with **Sleep
  after**), Free Lift turns the weight off, powers the motors down, puts the machine's motor
  board to sleep (this should also release the cables' resting tension; not yet tested on
  the machine), lets go of the machine
  and turns the screen black (a dim "Asleep · tap to wake" moves to a new place every 10
  seconds, so it can't burn into the screen). Tap anywhere to wake it: that tap only wakes the screen, the
  app reconnects, and the weight stays off until you tap Weight on. Lifting counts as use,
  so it never sleeps in the middle of a workout. To sleep straight away, tap **🌙 Sleep** in
  the top bar. (The screen is black, but its backlight stays slightly lit.)
* Dumbbells (separate left / right weights) send the original's two-player command (A1 left,
  A2 right). It is decoded from the original's code but hasn't been seen in the tablet's log
  yet — try it at a low weight first.
* Assist is shown and saved, but the machine doesn't get it yet (the original's command always
  sends "no assist"), so Assist works like plain linked handles for now.
* Not available on the real machine yet (the app says so instead of sending anything):
  fixed-speed (isokinetic) mode — its weight value can't be read from the code.

**Free the machine first.** The Speediance app is the tablet's home screen and keeps the cable
open exclusively, so Free Lift shows a banner ("The Speediance app is using the machine…").
Tap **Open Speediance app info, then tap Force stop**, tap Force stop, come back — Free Lift
retries every 3 s and connects by itself. Pressing the tablet's Home button starts the
Speediance app again (Free Lift lets go of the cable whenever it leaves the screen).

The **⚙ Settings** button at the top opens Android's Settings.

This is an Android-only app. There is no Windows program: `cargo build` compiles for the
tablet (`.cargo/config.toml` sets the arm64 Android target and the NDK linker), and
Cargo.toml has `autobins = false`. Craft Anything's screenshot check can only run a desktop
program, so it reports a failure for this project — that is expected.

**First test:** stand clear of the handles, set a low weight (e.g. 5 kg), tap Weight on, check
the cables resist, then tap Weight off.

## Starting Free Lift when the tablet powers up

Free Lift can be the tablet's **Home app**, so it opens when the tablet starts (instead of
the Speediance app) and the Home button brings you back to it:

1. Open Android Settings (the ⚙ Settings button in Free Lift).
2. **Apps** › **Default apps** › **Home app** › choose **Free Lift**.

To go back, choose the Speediance app there again. The Speediance app is still installed:
open it from Settings › Apps when you need it (for example for a firmware update).

The first time after a restart, test at a low weight that Weight on works normally: Free
Lift has so far always run after the Speediance app had started the machine.

## Installing

```
adb connect 192.168.2.50
adb push dist\FreeLift.apk /data/local/tmp/FreeLift.apk
adb shell pm install -r -t /data/local/tmp/FreeLift.apk
```

(Copy first, then install: over the tablet's slow Wi-Fi a direct `adb install` keeps failing.)

**If the Wi-Fi keeps dropping:** copy `dist\FreeLift.apk` to a USB drive and plug it into the
tablet (or put it in the tablet's Downloads folder). Claude Code then checks the file and
installs it from the tablet itself, which only needs a few small commands over Wi-Fi.

**Turn the weight off before installing.** Installing closes Free Lift immediately, and the
list of sets is cleared (sets are only kept while the app is open).

## The workout and the summary

Tap **Start Workout** when you begin; the clock in the top bar starts. **Pause** turns the
weight off and stops the clock until you tap **Resume**. Tap **End Workout** when you're done:
the weight goes off and the **Training summary** shows your time, sets, reps and volume.
**New workout** clears the sets and starts a fresh workout; **Close** (or a tap outside the
summary) keeps the sets on screen.

## Building it again

```
set ANDROID_HOME=C:\Users\kylel\AppData\Local\Android\Sdk
set ANDROID_NDK_ROOT=%ANDROID_HOME%\ndk\27.0.12077973
cargo apk build --release --lib
copy target\release\apk\FreeLift.apk dist\FreeLift.apk
```

## Voice control

Voice runs entirely on the tablet (nothing is sent online). Turn it on with the switch on the
Voice Control box, the 🎤 button at the top, or the V key. The first time, Android asks
"Allow Free Lift to record audio?": tap Allow.

* **Always:** just say the command: "weight on", "set weight to twenty", "weight off".
* **Wake word:** start with "Weight": "Weight, set to twenty", or say "Weight", wait for
  "Yes? Say the command.", then say it. Words that sound like it ("wait", "weigh") count.
  Other talking is ignored.
* **"Stop" and "weight off" always work**, in both styles, and act as soon as they're heard.
* The microphone is off while Free Lift is in the background or asleep, and back on when you
  return. Everything heard shows in the Voice Control box.
* "Barbell", "Dumbbells" and "Assist" work like the three Setup buttons; "start workout",
  "pause", "resume" and "end workout" work like the top-bar buttons.
* While listening, the **Mic** bar in the Voice Control box moves when the microphone hears
  sound. If it stays empty, Free Lift says the microphone is sending only silence: check the
  clip-on mic's mute button and that it is plugged in.
* Free Lift also keeps a list of everything voice heard and what it did, in its own folder
  (`files/voice-heard.txt`), so the words can be tuned to your voice. To read it:
  `adb shell cat /data/data/com.craftanything.freelift/files/voice-heard.txt`

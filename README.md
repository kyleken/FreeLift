# Free Lift

A replacement for the **Free Lift** screen on the Speediance **Gym Monster** tablet. It needs no
sign-in and works offline. It drives the machine's motor board directly over the tablet's
serial port, and it can be controlled by voice.

> [!WARNING]
> **Read this before using Free Lift. You use it entirely at your own risk.**
>
> * **It's unofficial.** Speediance did not make, test, endorse or support it. "Speediance"
>   and "Gym Monster" are their owners' trademarks and are used only to say which machine
>   this works with.
> * **It controls the motors directly.** It sends commands to the machine's motor board.
>   They were worked out from the original app, not from official documentation. A bug,
>   a misheard voice command, a different firmware version or a different machine model
>   could make the cables pull harder, sooner or longer than you expect, or fail to let go.
> * **That can cause serious injury or damage** to you, other people (children and pets
>   especially), the machine or your home.
> * **It may void your warranty** and may break the machine's terms of use. Speediance
>   updates could stop it working, or make it behave differently, at any time.
> * **No warranty, no liability.** It is provided "as is", without warranty of any kind. To
>   the fullest extent the law allows, the authors are not liable for any injury, death,
>   damage or loss from using it, or from being unable to use it (see `LICENSE`, sections 7
>   and 8).
>
> **If you use it anyway:**
>
> * Start every session, and test every new version, at the **lowest weight**, standing clear
>   of the handles and cables.
> * Keep other people away from the machine while it's on.
> * Know how to stop it without the app: let go of the handles and switch the machine off
>   at its power switch.
> * Don't use it if you aren't comfortable with any of the above.
>
> By installing or using Free Lift you accept these terms.

## What it does

* **Weight on / off** with the original app's motor commands. Free Lift checks that the weight
  really went off by watching the cable force.
* **Setup:** Barbell (one total weight), Dumbbells (separate left / right), Assist.
* **Weight modes:** Standard, Eccentric and Chain, each with an extra weight. Fixed-speed
  (isokinetic) is blocked on the real machine until its command is decoded.
* **Live data:** reps, force and cable distance per side, and a power graph.
* **Workout:** Start / End Workout and Pause in the top bar, a sets table, a rest timer and a
  training summary.
* **Voice control, fully offline** (sherpa-onnx on the tablet). You can talk to it all the time
  or start with the wake word "Weight". "Stop" always turns the weight off at once.
* **Sleep** after no use (or with the 🌙 button): the weight goes off, the motor board sleeps
  and the screen goes black.
* It can be chosen as the tablet's **Home app**, so it starts at power-up.

## Safety

This app moves real motors that pull cables under load.

* Connecting never turns the weight on.
* A bare "stop" means weight off.
* Barbell / Dumbbells can't be switched while the weight is on.
* Weight off is confirmed against the cable force.
* Test every change at a low weight first, standing clear of the handles.

## Documents

| File | For |
|---|---|
| [`ANDROID.md`](ANDROID.md) | Using it on the tablet: the screen, freeing the machine, voice, installing |
| [`docs/DEVELOPER_GUIDE.md`](docs/DEVELOPER_GUIDE.md) | Every module, the motor protocol byte by byte, building, testing, known gaps |
| [`MECHANICS.md`](MECHANICS.md) / [`mechanics.json`](mechanics.json) | Every value taken from the original app, with where it was found |
| [`CLAUDE.md`](CLAUDE.md) | The owner's rules for Claude Code |
| [`LICENSE`](LICENSE) / [`NOTICE`](NOTICE) | Apache License 2.0; copyright, trademarks and the bundled voice files' own licences |

## Build

Android only: there is no desktop version.

```bash
cargo test --lib --target x86_64-pc-windows-msvc   # unit tests on the PC
cargo apk build --release --lib                    # needs ANDROID_HOME and ANDROID_NDK_ROOT
```

The built APK is attached to each GitHub Release. The full set-up is in the developer guide,
§6.

Tech: Rust, egui/eframe 0.33 (NativeActivity), cargo-apk, sherpa-onnx 2.0.1.

## Licence

Apache License 2.0: see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). Provided "as is",
without warranty; read the safety notice at the top of this page first.

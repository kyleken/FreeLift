# Free Lift – mechanics sheet

What the original (speediance 3.14.01, a Flutter app) does, where it was found, and what this
rebuild uses. Addresses are in `lib/arm64-v8a/libapp.so` (Dart 3.12.2 AOT snapshot
`ace654289f5abc240509fc941453ebc5`) unless a Java class is named.

## How the code was read

* Ghidra (Deep mode) has no functions in the Dart code, and `native_function_pseudocode`
  refuses every Dart routine (e.g. 0x1c41778, 0x1c2dc80, 0x3932ecc: "function_not_found").
  The built-in lifter works only on the 149 helper routines it found (e.g. 0x187ad44 =
  `String._interpolate`).
* So the Dart code was read with **blutter** (built locally for Dart 3.12.2): it recovered
  15,905 routine addresses, the object pool (every string/number constant with its pool
  offset) and the enum objects. Blutter could only write out 1,804 routine bodies for this
  Dart version, so the remaining routines were disassembled directly (capstone, arm64) with
  every `ldr xN, [x27 (PP), #off]` resolved against blutter's pool dump. Routine starts are
  the `stp x29, x30, [x15, #-0x10]!` prologues.
* The English messages are only referenced from the language tables; logic routines use
  their keys (`voice_weight_at_max`, …), which is how the routines below were found.
* Java/DEX code was read with the Android tools (smali listings).

## Weight limits

| rule | where | value |
|---|---|---|
| min weight | 0x1c41778 | per side 3.5 kg / 8 lb; when field 0x2f == 2: 7 kg / 16 lb |
| max weight (total) | 0x1c2dc80 | 100 kg / 220 lb; 120 kg / 260 lb when the model is monster2s, monster3s or monsterUltra |
| per-side max | 0x39118f4, 0x39329a8 | max ÷ 2.0 unless field 0x2f == 2 |
| kg or lb | 0x3a520a4 | reads the saved setting `isLbs`; the lb limits are their own numbers (8, 16, 220, 260), not converted |
| refuse outside range | 0x3932cbc `_validateWeight` | target < min → `voice_weight_below_min`, target > max → `voice_weight_above_max` |
| at a limit | 0x391123c `_failureMessage` | `voice_weight_at_max` "The maximum supported weight is 000. It cannot be increased.", `voice_weight_at_min` |
| number format | 0x391164c / 0x39117cc `_formatWeight` | one decimal, trailing ".0" removed |

Machine models (enum `DeviceVersion`): monster1, monster2, monster2s, monster3, monster3s,
monsterUltra. Pool constants: double 100 @ pp+0x151b20, 220 @ pp+0x1ed3a8, 120 @ pp+0x1f52a8,
260 @ pp+0x203600; 3.5 / 7 / 8 / 16 are `fmov` immediates in 0x1c41778.

Field 0x2f of the per-side settings object is a plain int, not an enum, so its meaning is
**inferred**: 2 = the shown weight covers both cables (this rebuild: barbell mode, shown as a
total), otherwise one cable (per side). The rebuild lets you pick the machine model (default
Gym Monster 2).

## "Weight up / down a bit"

`_computeFuzzyDelta` at **0x3932ecc** (called from increaseWeight 0x3ccba6c and
decreaseWeight 0x3ccb814 when no amount is spoken):

```
threshold = 25 if field 0x2f == 1 else 50
delta = ceil(current × 0.10)   if current <= threshold      (THR slot 0x760 = LibcCeil)
delta = floor(current × 0.05)  if current >  threshold      (THR slot 0x758 = LibcFloor)
target = current ± delta  → 0x39329a8 (validate against min/max, then apply)
```

0.1 @ pp+0x1ed270, 0.05 @ pp+0x1528b8, 50 @ pp+0x1ed260, 25 is an `fmov` immediate. The rebuild
uses 25 for one cable and 50 for barbell totals (same inferred mapping as above).

## Mode rules

* **switchWeightMode** 0x3cc0da4: if the weight is on it is turned off first
  ("重量开启，自动关闭后再切换模式" = weight is on, turn it off automatically, then switch);
  if already in the target mode it says so; unknown mode → failure.
* **setSpeed** 0x3ccd4dc: only in speed (isokinetic) mode; otherwise a failure reply
  (`sv_speed_isokinetic_only`).
* **Weight changes in speed mode** are refused: `voice_speed_mode_not_support_weight`
  "Speed mode does not support weight adjustment." (failure table in 0x391123c; the speed-mode
  check is 0x3911c68, also used by setSpeed and increaseWeight).
* **Eccentric / chain extra weight** needs that mode first: `sv_eccentric_mode_required`
  "Please switch to eccentric mode first.", `sv_chain_mode_required`; neither is available in
  isokinetic or rowing/skiing mode (`sv_eccentric_isokinetic_forbidden`, …).
* Speed range: the message "Speed range is 000 to 111 m/s." (`sv_speed_range`) is only
  referenced from the language tables; the numbers come from run-time settings and are
  **estimated** here (0.1–1.0 m/s).

## How the screen reaches the motors

* USB device: Java `com.speediance.faraday.usb.util.UsbDriver.ScanDevices` (classes7.dex)
  matches **vendor 0x0483, product 0x5740** (an STM32 USB serial port). The rebuild's cable
  listener marks that port and selects it automatically.
* Serial paths `/dev/ttyS3…6` and `package:libserialport` are also present.
* Receive routine: `USB::dataRec` at **0x1b961dc** (blutter name); inside it the check at
  0x1b966e4–0x1b96790 looks for 0xAA (Smi 0x154) then 0xAB (Smi 0x156).
* Older Java helper `DataUtils.calcCRC/setParamCRC`: CRC-16 with the reflected 0x1021
  polynomial (`(crc ^ 0x810) >> 1 | 0x8000`), init 0, high byte first. The frames recorded in
  the current app use the X-25 variant below instead.
* The bytes the app sends (weight on/off, set weight) go through run-time dispatch
  (0x39329a8 → 0x39f64c4 → dynamic call) that static reading cannot follow, so this route
  did not reveal them. (Superseded: the fifth pass below found the command builders through
  the CRC routine, and Free Lift now sends those commands to the machine.)

## Motor commands (screen → machine) – fifth pass

Found by following the CRC: `0x1994cb8` is CRC-16/X-25 (init 0xFFFF, `eor #0x8408`), wrapped by
`0x199445c`; **`0x1994410`** appends it to a frame. Its 15 callers are the command builders; each
starts from the pool constant `[0xCC, 0xCD]` and logs "发送<name>指令".

| command | byte 3 | builder | frame |
|---|---|---|---|
| enable (weight on) | 0x01 | 0x3a9bdb0 | `CC CD 12 01 <mode> <weight f64 BE, lb> 00 <1 non-barbell / 2 barbell> <extra lb> <crc>` |
| disable | 0x02 | 0x1bc259c | `CC CD 06 02 <crc>` |
| pause (weight off) | 0x03 | 0x1c69708 | `CC CD 06 03 C3 9D` |
| reset | 0x04 | 0x3a908f0 | `CC CD 06 04 <crc>` |
| getVersionCode | 0x06 | 0x3b21378 | `CC CD 06 06 <crc>` |
| getSn | 0x07 | 0x3a3f244 | `CC CD 06 07 <crc>` |
| sleep | 0x08 | 0x3c3bd78 | `CC CD 06 08 <crc>` |
| wakeup | 0x09 | 0x3a8ca0c | `CC CD 06 09 <crc>` |
| heartbeat | 0xFE | closure 0x19910bc | `CC CD 07 FE <counter % 255> <crc>`, every **500 ms** (Duration 0x7a120 µs, timer set up in 0x1991060 right after enable) |
| enable_2Player (independent left/right) | 0xA1 left / 0xA2 right | 0x3acafec | `CC CD 11 <A1/A2> <mode> <weight f64 BE, lb, not halved> 00 <extra lb, truncated> <crc>` (sixth pass) |
| seatControl, motorShake, queryPositionStatus, odometer reset | – | 0x3ac02d0, 0x3c353a0, 0x3a8fb94, 0x1c4ebe0 | not decoded (not used by Free Lift) |

**Sixth pass.** `enable_2Player` (0x3acafec): byte 3 is 0xA1 when the player argument is 1 and
0xA2 otherwise; the callers in the Free Lift page (FreeTrainingLogic 0x1c84154 and closure
0x1c8445c, also 0x1c8d074) pass player 1 with `Side.left` (Obj!Side@2646911) and player 2 with
`Side.right` (Obj!Side@26468d1), so independent left/right weights are two frames, one per
cable. Mode 5/6 → 9/10 as in enable; weight ×2.2 unless `isLbs`; the extra is converted the same
way and **truncated** (`fcvtzs`), not rounded; the heartbeat is started the same way (0x1991060).
Decoded from the code only — the tablet's log has no two-player frame yet. The rebuild now sends
this pair in independent mode. **Timing:** 0x1c84154 sends the left (A1) frame, then schedules
closure 0x1c8445c with Duration 0xc350 µs = **50 ms** to send the right (A2) frame. Sending
both in one write left the left cable unchanged on the real machine, so the rebuild now waits
50 ms between them too.

**Rep counters:** the machine restarts its rep counters (frame bytes 6/7) when the weight goes
on again; the rebuild counts from zero whenever a counter drops below the value noted at the
start of the set (it previously showed ~245 from wrapping).

The Free Lift caller 0x1c84154 passes enable's barbell argument from the per-side settings field
0x2f (or 2 when 0x1c5091c is true): so **field 0x2f is the barbell code (1 non-barbell,
2 barbell)**, which confirms the earlier "inferred" reading of the limits (0x1c41778, 0x39118f4)
and of the "a bit" threshold (0x3932ecc): totals and threshold 50 in barbell mode, per side
otherwise.

Fixed speed (mode 4) is still not sent: the weight that caller passes comes from 0x1c82e24 →
0x1bc324c, which scales a stored value through dynamically dispatched number operators (with
constants 5, 2.2, 2 and 0.4) whose operations can't be read statically.

* Command CRCs are written **high byte first** (the machine's own frames put the low byte first).
* Enable details (0x3a9bdb0): the weight is sent in **pounds** (×2.2 when the app is in kg,
  pp+0x202fa0); if the barbell byte is 2 the shown total is **halved** to per side; the extra
  (eccentric/chain) weight is converted the same way and rounded into one byte; byte 13
  ("help"/assist) is always 0; mode codes 5/6 (rowing/skiing) are sent as 9/10.
* Device mode codes (0x3934a84): **1 standard, 2 chain, 3 eccentric, 4 fixed speed**.
* "已上力但心跳未运行" (0x3c01420): while the weight is on the heartbeat must be running.

**Checked on the tablet** (wireless ADB 192.168.2.50, read-only): the original's log database
`logger.db` holds frames it really sent, e.g.
`发送enable指令: [204, 205, 18, 1, 3, 64, 52, 0, 0, 0, 0, 0, 0, 0, 1, 5, 20, 117]` with the note
"重量:20.0磅(9.1kg) … 模式:3 … 杠铃(1非杠铃2杠铃):1 … 额外:5磅" (20 lb, eccentric, non-barbell,
5 lb extra), a 60 lb enable, and `发送pause指令: [204, 205, 6, 3, 195, 157]`. The rebuild's encoder
(`protocol::cmd_enable`, `cmd_simple`, `cmd_heartbeat`) reproduces these byte for byte
(test `commands_match_the_tablet_log`). The original holds **/dev/ttyS5** open; `stty` shows
**115200 baud**. The tablet is an RK3568, Android 11, 1080 × 1920 at 240 dpi, SELinux permissive,
`/dev/ttyS4`/`ttyS5` readable and writable by any app.

The rebuild shows these frames for the current settings in the Machine window but **does not
send them**.

## Machine replies (seventh pass)

* **0x83 command reply** (handler 0x1b98ae0): frame byte 4 = **1 → accepted** (the first one
  after enable is logged "下位机-首次上力应答成功", "first load-on reply OK"); any other value →
  "下位机错误应答" (error reply). The recordings' `AA AB 07 83 01` is an accepted reply.
  The app now says "The machine confirmed: weight on." on the first accepted reply after each
  weight on, and reports rejected replies.
* **0x82 safety stop** (handler 0x1b98cdc, "下位机-保护触发"): the original first cancels the
  heartbeat (0x1994284), then shows a dialog with the reason from byte 4
  (helpers 0x1b9919c…0x1b99064 → keys): 1 "The barbell is not balanced", 2 "Loss of grip on the
  handle or barbell", 3 "Cable retraction too fast", 4 "Cable extension too fast", 5 "Motor
  temperature too high", 6 "The motor is locked. Please try to restore it.", other "Firmware
  runtime error". The app now does the same: stops the heartbeat, marks the weight off (no pause
  is sent, like the original) and shows the reason in a red banner.
* 0x84 carries a hardware code (0x1b968b0), 0x85 the drive serial number
  (`currentDriveDeviceSn`, 0x3a8f450).
* **Wakeup** (0x3a8ca0c) is sent only from the sleep screen (`SleepingStateCoordinator`,
  0x28ad638 / 0x3c31828), not before enable. Free Lift sends it only when reconnecting after
  its own Sleep, which sends sleep (0x08) after pause + disable (added 2026-11-18).

## Eighth pass (fixed speed, command log)

* The tablet's `logger.db` (read again over ADB, read-only) still holds only the two eccentric
  enables and the pause above. These entries are the original's training track logs that failed
  to upload ("Failed host lookup: api2.speediance.com"), so while the tablet is offline the
  original keeps every command remark it makes — a fixed-speed set done in the Speediance app
  would show its exact frame there.
* Fixed speed stays blocked: 0x1bc324c computes the weight as value (field 0x163) `op` 5, then
  `op` 2.2 when `isLbs`, `op` 2 for a flag from 0x1c559b4, `op` 0.4 when 0x1c3d9cc is false and
  0x1bc337c is true — all four through the same dynamically dispatched selector (`GDT[cid − 0xff1]`).
  Blutter doesn't name that selector; its only other uses (profile weight in
  edit_profile_page.dart, Sentry app-start timing) don't fix whether it is × or ÷.
* The original logs every command it sends with its name and bytes (0x19942d0,
  "下位机数据:发送<name>指令: [..]"). The rebuild's Machine window now keeps the same kind of log:
  each command sent (named like the original: enable, pause, enable_2Player left/right, …) and
  each reply frame from the machine.

## Machine data frame (machine → screen)

Decoded from `assets/9kg.txt` (515 frames) and `assets/row.txt` (483 frames); the checksum
matches on all 1,026:

| bytes | meaning |
|---|---|
| 0–1 | header `AA AB` |
| 2 | total frame length (0x3C = 60) |
| 3 | frame type: 0x80 = live data, 0x83 = short status (`AA AB 07 83 01 + crc`) |
| 4 | training family: 1 = strength, 6 = rowing |
| 5 | sub-mode (0 strength, 2 rowing) |
| 6 / 7 | left / right rep count |
| 8–32 | left side: tag 0x02, three big-endian f64: A, B, force (lb) |
| 33–57 | right side: same |
| 58–59 | CRC-16/X-25 (poly 0x1021 reflected, init 0xFFFF, xorout 0xFFFF) over bytes 0–57, little-endian |

Force is in pounds: 19.8 = 9 kg × 2.2, idle 7.7 lb = 3.5 kg (the minimum weight above).

### How the original decodes it (fourth pass)

* `USB::dataRec` **0x1b961dc** → splitter **0x1b9620c** (looks for AA AB, rejects bad lengths:
  "忽略非法下位机帧长度") → frame handler **0x1b973ec** (CRC check, log "下位机CRC校验失败" on
  failure), which dispatches on byte 3:
  0x80 → 0x1b9c29c, 0x81 → 0x1b9c228, 0x82 → 0x1b98cdc, 0x83 → 0x1b98ae0, 0x84 → 0x1b968b0,
  0x85 → 0x3a8f450, 0x8a → 0x1b98a00, 0x8b → 0x1b97f00, 0x8c/0x8d → 0x1b97a80
  (rowing / skiing status: "划船滑雪状态反馈已解析: mode=", 0x8d = skiing).
* Live data **0x1b9c29c** reads big-endian doubles from bytes 9, 17, 25 (left) and 34, 42, 50
  (right) and stores them under these names:
  * bytes 6/7 → `countLeft` / `countRight` (reps);
  * second value → `distanceLeft` / `distanceRight` = **−round(value, 4 decimals)**
    (toStringAsFixed(4), parse, 0 − x), i.e. cable distance in metres;
  * third value → `torqueLeft` / `torqueRight`, in lb, **÷ 2.2** when `isLbs` is off;
  * first value is passed unnamed to the live listener together with the others; its ±1.6
    range in the recordings fits cable speed in m/s (**inferred**).
* The rebuild's cable listener now shows torque, distance and (inferred) speed with these rules,
  and names the other frame types.

## Voice

The original listens locally (sherpa-onnx, wake words "hi aevis"/"xiaosu",
`assets/hotwords_en.txt`, `assets/intent_vectors_en.json`: 83 sentences, 21 intents). The
rebuild uses Windows' offline recogniser with a fixed grammar plus the same intents, and the
original's reply wording where it was found.

## Second pass (voice page-state builder)

* `core_target_open` + `native_strings_find` on libapp.so find the setting names
  `minAllowedWeightKg/Lbs`, `maxAllowedWeightKg/Lbs`, `min/maxChainWeightKg/Lbs`,
  `min/maxEccentricWeightKg/Lbs`, `maxAllowedSpeed`, but `native_xrefs_list` reports 0
  references (Dart reaches strings through the object pool), and `native_function_pseudocode`
  has no function at any Dart routine, so the routines were again located through the pool.
* **0x39363dc** packs these names into the page-state map the voice assistant reads; its only
  caller is **0x3934a84**, which computes them:
  * per-side max = `0x1c2dc80()` ÷ 2.0, and for the kg field when `isLbs` is on: ÷ 2.0 ÷ **2.2**
    (pp+0x202fa0 = 2.2 at 0x39352a4) – the same 2.2 lb/kg the machine frames use;
  * chain and eccentric min/max come from the device-interactions object (getters 0x1d24e08,
    0x1d2e25c, 0x1d28224) only when that object's field 0x2b == 2, i.e. they are reported by the
    machine at run time, not stored in the app – so the 25 kg cap here stays **estimated**;
  * a value 0.1 + 0.7 × (x − min) / (max − min) (pp+0x1ed270, pp+0x207900) is an on-screen
    level scale, not a weight rule.
* `step_weight` (pp+0x1466e8) is only a label in the body-data onboarding table, not the +/−
  step.

## Third pass (set summary)

* `native_strings_find` finds `restDurationSec`, `free_train_rest_switch_`, `smartLoadOff`,
  `rest_time`; `native_function_pseudocode` again has no function at the Dart routines
  (0x1b4502c: "function_not_found"), so they were followed through the object pool.
* **FreeTrainSetSummaryEntity.toJson** (blutter name) at **0x3a61830** calls **0x1ab8ad4**, which
  writes the per-set record the original saves after each Free Lift set: totalCapacity, weight,
  calorie, time, totalPower, isMixLoad, modeId, deviceMode, standardAmplitude,
  **restDurationSec**, leftRight, maxHeartRate, finishedCount, **leftReps**, **rightReps**,
  selectCompletionMethod, distance, **avgPower**, avgPace, avgSpeed, spm, resistance,
  outputJoule. 0x1b4502c reads the same record back.
* The rebuild's sets list now carries the strength fields of that record: weight, left/right
  reps, totalCapacity (volume = weight × reps per cable), avgPower (mean of the cable power
  while the set runs) and restDurationSec (time from the end of the previous set to the start
  of this one – which end it is measured from is **inferred**). Calories, heart rate, pace and
  rowing fields need sensors this rebuild doesn't have.
* `free_train_rest_switch_` (0x1c91c74) is a per-user saved switch (key + user id or "guest")
  for whether rest is shown. Its default was found in pass 9 (below).
* `smartLoadOff` is only a label key in the language tables; the smart-load-off timing is not in
  the app code that could be read (still **estimated** 3 s).

## Pass 9 (rest between sets)

`native_strings_find` finds `restTime` (0x6a9968, no references through Ghidra) and
`native_function_pseudocode` has no function at the Dart routines, so they were followed
through the object pool with blutter. The tablet's command log (logger.db) was read again; it
has no new commands (still two eccentric weight-on frames and one weight-off).

* **Rest timer switch – default on.** 0x1c91bdc reads the saved bool
  `free_train_rest_switch_<user id or "guest">` (key built in 0x1c91c74) and returns **true**
  when it was never saved. The Free Lift page reads it on start (0x3acda78 in 0x3acda18) and
  saves it from 0x3b33e2c. → `REST_SWITCH_DEFAULT = true`; the Setup row now has a
  **Rest timer** checkbox, saved like the original's. Off: a finished set goes straight to
  "ready", and the rest taken is still measured and saved with the next set (restDurationSec).
* **Auto-add a set after rest – default on.** 0x1c91a90 reads
  `free_train_auto_add_set_after_rest_<user>` (key built in 0x1c91b28), same default **true**.
  This rebuild already starts a new set each time the weight goes on, so nothing to change.
* **Rest counts up, no preset length.** The Free Lift state (selector closure in
  free_training_page.dart) holds `isResting`, `restElapsedSeconds` and
  `autoAddSetAfterRestEnabled`; FreeTrainCurrentSetStats.toJson (0x3aba814 → 0x1c2bd0c) has
  `currentRestDurationSec`, a measurement. No rest-length setting or default was found, so the
  rebuild's countdown length (60 s, adjustable 10–600 s) stays **ours / estimated**.
* **Entering rest: a 6-second ring.** RestCountdownController (0x1c40d78, ticked by closure
  0x1c41498 from `now − start` in ms): elapsed is clamped to 0–6000 ms; remaining = 6000 −
  elapsed; the ring progress is (4000 − clamp(remaining − 2000, 0, 4000)) / 4000, so it fills
  over the first 4 s and the last 2 s show without the ring; when it ends, closure 0x1c40fc0
  logs "自由训练-进入组间休息" ("free training – enter rest between sets") and rest begins.
  What starts this ring (which set-end event) was not read, so the rebuild does not show it.

## Pass 10 (smart load off)

`native_strings_find` finds `smartLoadOff`, `UnloadMode` and the help texts ("Smart Load
Relief", "…enable spotter mode or smart load off"); none has a Ghidra reference and
`native_function_pseudocode` has no function at the Dart routines, so they were followed
through the object pool with blutter.

* **The setting is a number, `autoOffWeight`.** 0x3a74a2c saves it with setInt and logs
  "保存智能卸力开关" ("save smart unload switch"); the preferences sheet (closure 0x1f6f1cc)
  calls it and posts a "SmartDisable" event.
* **Values = UnloadMode** (course_user_data.dart): 0 `manual`, 1 `smartInfinity`,
  2 `smartFinite`.
* **Default: 1 (smart, on).** 0x1f6e21c reads `autoOffWeight` and returns 1 when it was never
  saved. → `SMART_OFF_DEFAULT = true` (machine.rs and the saved settings use it).
* **Meaning, from the language tables** (`smartLoadOffDesc`, entries at 0x2e15ff8 / 0x2e40a38 /
  0x2e6b4b4): "The weight will automatically turn off after one set of training."
* **Timing not found.** Only the preferences page (0x1f6c4d4) and analytics (0x1f6eb84)
  read `autoOffWeight`; the guided-course player (0x2bb82ec) compares it with
  `autoInfinityMode`. No free-lift routine reads it, and the help text says a signal loss
  between screen and motor turns Smart Load Off on, which suggests the motor board does the
  unloading itself. The rebuild's 3 s of stillness after a rep stays **estimated**.

## Pass 11 (starting weight, extra-weight cap, values kept between visits)

`native_strings_find` finds no `defaultWeight`-style name for Free Lift. `fl_initial_weight` is
the fitness log's starting **body** weight ("初始体重"), and `defaultWeight` (0x1c3c948…) is a
field of the server's exercise data. `native_function_pseudocode` again has no function at the
Dart routines, so the Free Lift page's start-up routine **0x3acda18** was read with blutter.

* **Values kept between visits.** **0x1c4f0fc** saves, next to the unit (`isLbs`):
  `powerValue` (weight), `speedPowerValue` (weight in speed mode), `speedMps`,
  `selectedModelID` (weight mode), `freeTrainDeviceSelectId`, `selectedDeviceID` (barbell code)
  and `_slider2Value` (the extra-weight slider). 0x3acda18 reads them back when Free Lift opens.
  → Free Lift now saves the last weight, mode, barbell setting, eccentric/chain extra and speed
  in `settings.json` (`last`): when a set starts, on any typed/voice change, when the app leaves
  the screen, and on exit. They are restored at start.
* **Starting weight 8.** At 0x3acdf70: `double.tryParse(powerValue) ?? 8.0`. The number is in
  the unit on screen (8 kg or 8 lb per cable). It is passed through **0x1c5021c**, which clamps
  it between the minimum (0x1c41778) and the maximum (0x1c2dc80, ÷ 2 unless barbell).
  → `DEFAULT_WEIGHT_SHOWN = 8.0`, `Machine::apply_default_weight()` (the old estimate was 5 kg).
* **Starting extra = half the weight.** At 0x3ace0f4: `_slider2Value ?? weight × 0.5`.
  → `DEFAULT_EXTRA_RATIO = 0.5`.
* **Extra-weight cap.** **0x1c2d7ac**:
  `max(0, min(maxWeight − weight, weight × ratio))`. `maxWeight` comes from 0x1c2dc08 (0x1c2dc80,
  halved unless barbell). The ratio comes from the pool map **pp+0x2035f0**
  `{2: 1.5, 3: 0.5}`, keyed by weight mode (2 chain, 3 eccentric); other modes get 0.
  **0x1c4fdf0**, run after each weight change (0x1c50318), lowers the extra to this cap.
  → `EXTRA_RATIO_CHAIN = 1.5`, `EXTRA_RATIO_ECCENTRIC = 0.5`, `Machine::max_extra_kg()`. The cap
  applies to typed/voice extras, the extra slider's range, and every weight or mode change.
  This replaces the old fixed 25 kg estimate. The formula is linear, so computing it per cable
  gives the same result as the original's on-screen (barbell-total) values. With independent
  weights both sides must fit.

## Still estimated

+/− button step (0.5 kg / 1 lb), speed range, rest timer length 60 s (the original counts rest up instead), smart-load-off idle 3 s,
serial baud rate. Each was searched for in the code; see `mechanics.json`.

Free Lift's own safety values (not from the original, all estimated, in `mechanics.rs`):
smart load off on the real machine needs both cables within 8 cm of their lowest point this
set and still (within 2 cm) for 3 s with live data arriving; live data older than 0.5 s counts
as missing; after weight off the force on both cables must drop to base tension + 1.5 kg
within 1 s, otherwise weight off is re-sent up to 2 times and then a warning is shown.

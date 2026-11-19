//! Numbers from MECHANICS.md / mechanics.json, written exactly as on the sheet.
//! Addresses are in lib/arm64-v8a/libapp.so of speediance 3.14.01.

/// lb per kg (9 kg is reported as 19.8 lb in the machine's own frames).
pub const LB_PER_KG: f32 = 2.2;
/// Cable tension while the weight is off (7.7 lb in idle frames).
pub const BASE_TENSION_KG: f32 = 3.5;

/// Minimum weight, routine 0x1c41778 (per side / both cables together).
pub const MIN_PER_SIDE_KG: f32 = 3.5;
pub const MIN_PER_SIDE_LB: f32 = 8.0;
pub const MIN_TOTAL_KG: f32 = 7.0;
pub const MIN_TOTAL_LB: f32 = 16.0;

/// Maximum total weight, routine 0x1c2dc80. "High" models are monster2s, monster3s and
/// monsterUltra. Per side is half of it (routine 0x39118f4 / 0x39329a8 divide by 2.0).
pub const MAX_TOTAL_KG: f32 = 100.0;
pub const MAX_TOTAL_KG_HIGH: f32 = 120.0;
pub const MAX_TOTAL_LB: f32 = 220.0;
pub const MAX_TOTAL_LB_HIGH: f32 = 260.0;

/// "Weight up / down a bit", routine 0x3932ecc (_computeFuzzyDelta):
/// current <= threshold -> ceil(current * 0.1), else floor(current * 0.05).
/// Threshold 25 for one cable, 50 for both cables together.
pub const FUZZY_THRESHOLD_SIDE: f32 = 25.0;
pub const FUZZY_THRESHOLD_TOTAL: f32 = 50.0;
pub const FUZZY_RATIO_LOW: f32 = 0.1;
pub const FUZZY_RATIO_HIGH: f32 = 0.05;

/// +/- button step. Estimated (not found in the code).
pub const STEP_KG: f32 = 0.5;
/// Estimated.
pub const STEP_LB: f32 = 1.0;
/// Starting weight when nothing was saved, in the unit on screen (8 kg or 8 lb per cable):
/// the Free Lift page reads `double.tryParse(powerValue) ?? 8.0` (0x3acdf70 in 0x3acda18) and
/// clamps it to the limits (0x1c5021c).
pub const DEFAULT_WEIGHT_SHOWN: f32 = 8.0;
/// Starting extra (eccentric / chain) weight when nothing was saved: `_slider2Value ??
/// weight × 0.5` (0x3ace0f4 in 0x3acda18).
pub const DEFAULT_EXTRA_RATIO: f32 = 0.5;
/// Extra weight cap, 0x1c2d7ac: max(0, min(max weight − weight, weight × ratio)), with the
/// ratio from the pool map {2 chain: 1.5, 3 eccentric: 0.5} (pp+0x2035f0). 0x1c4fdf0 lowers the
/// extra to this cap whenever the weight changes.
pub const EXTRA_RATIO_CHAIN: f32 = 1.5;
pub const EXTRA_RATIO_ECCENTRIC: f32 = 0.5;
/// Isokinetic speed range, m/s. Estimated: the app fills "Speed range is 000 to 111 m/s"
/// from run-time settings, so the numbers are not in the program.
pub const SPEED_MIN: f32 = 0.1;
pub const SPEED_MAX: f32 = 1.0;
pub const SPEED_DEFAULT: f32 = 0.4;
/// The original's voice intents (assets/intent_vectors_en.json).
pub const VOICE_INTENTS: usize = 21;
/// Assist (spotter) levels ("level 1 assist", "level 2 assist").
pub const ASSIST_LEVELS: u8 = 2;
/// Rest timer default, seconds. Estimated: the original's free lift has no preset rest length
/// (it counts the rest up, `restElapsedSeconds`), so this countdown length is ours.
pub const REST_DEFAULT_S: u32 = 60;
/// Rest timer switch default: on. The original reads `free_train_rest_switch_<user>` and uses
/// `true` when it was never set (0x1c91bdc; key built in 0x1c91c74).
pub const REST_SWITCH_DEFAULT: bool = true;
/// Smart load off default: on. The original saves it as the int `autoOffWeight`
/// (UnloadMode: 0 manual, 1 smartInfinity, 2 smartFinite; saved in 0x3a74a2c) and reads it
/// back as 1 (smart) when it was never saved (0x1f6e21c).
pub const SMART_OFF_DEFAULT: bool = true;
/// Smart load off: seconds the cable must sit still after a rep. Estimated.
pub const SMART_OFF_IDLE_S: f32 = 3.0;
/// Smart load off on the real machine: a cable counts as "home" (back in) when it is within
/// this distance of the lowest point it reached since the weight went on. Estimated.
pub const SMART_OFF_HOME_TOL_M: f32 = 0.08;
/// Smart load off on the real machine: a cable counts as still while it stays within this
/// distance of where it last stopped. Estimated.
pub const SMART_OFF_STILL_TOL_M: f32 = 0.02;
/// Live data older than this is treated as missing (smart load off waits, the weight-off
/// check warns). Estimated.
pub const LIVE_STALE_S: f32 = 0.5;
/// Weight-off check: seconds after a weight-off command before the cable force is checked,
/// and between re-sends. Estimated.
pub const OFF_CHECK_S: f32 = 1.0;
/// Weight-off check: force above base tension + this margin (either cable) means the
/// machine still seems loaded. Estimated.
pub const OFF_MARGIN_KG: f32 = 1.5;
/// Weight-off check: how many times the weight-off command is re-sent before warning.
/// Estimated.
pub const OFF_RESENDS: u8 = 2;
/// Serial baud rate: /dev/ttyS5 on the tablet runs at 115200 (stty, port held by the original app).
pub const DEFAULT_BAUD: u32 = 115200;

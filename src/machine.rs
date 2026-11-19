//! The practice machine: holds every Free Lift setting and simulates the cables.
//! Rules and numbers come from MECHANICS.md (routine addresses are in libapp.so).

use crate::commands::{Command, Extra, Side, Unit, WeightMode};
use crate::mechanics as m;
use std::collections::VecDeque;

/// Practice-only cable model (not from the original; just for the on-screen lifter).
const SIM_REP_SECONDS: f32 = 2.6;
const SIM_REPS_PER_SET: u32 = 8;
const SIM_TRAVEL_M: f32 = 0.6;
const G: f32 = 9.81;
pub const HISTORY: usize = 360;
/// Cable travel used to draw the live bars from the machine's distance (display only).
const LIVE_TRAVEL_M: f32 = 1.2;

/// Machine models, the original's DeviceVersion enum (monster1 … monsterUltra).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Model {
    Monster1,
    Monster2,
    Monster2S,
    Monster3,
    Monster3S,
    MonsterUltra,
}

impl Model {
    pub const ALL: [Model; 6] =
        [Model::Monster1, Model::Monster2, Model::Monster2S, Model::Monster3, Model::Monster3S, Model::MonsterUltra];
    pub fn label(self) -> &'static str {
        match self {
            Model::Monster1 => "Gym Monster",
            Model::Monster2 => "Gym Monster 2",
            Model::Monster2S => "Gym Monster 2S",
            Model::Monster3 => "Gym Monster 3",
            Model::Monster3S => "Gym Monster 3S",
            Model::MonsterUltra => "Gym Monster Ultra",
        }
    }
    /// 0x1c2dc80: monster2s, monster3s and monsterUltra get the higher maximum.
    pub fn is_high(self) -> bool {
        matches!(self, Model::Monster2S | Model::Monster3S | Model::MonsterUltra)
    }
}

/// The Setup choice: exactly one is selected. Barbell = one total weight for both cables,
/// Dumbbells = separate left / right weights (two-player frames), Assist = linked handles
/// with the spotter on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Accessory {
    Barbell,
    Dumbbells,
    Assist,
}

impl Accessory {
    pub const ALL: [Accessory; 3] = [Accessory::Barbell, Accessory::Dumbbells, Accessory::Assist];
    pub fn label(self) -> &'static str {
        match self {
            Accessory::Barbell => "Barbell",
            Accessory::Dumbbells => "Dumbbells",
            Accessory::Assist => "Assist",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Cable {
    /// 0 = fully retracted, 1 = top of the pull.
    pub pos: f32,
    /// m/s, positive while pulling out.
    pub vel: f32,
    pub force_kg: f32,
    pub reps: u32,
    /// Real machine only: how far the cable is out, metres (live frame distance).
    pub dist_m: f32,
    at_top: bool,
}

pub struct Machine {
    pub weight_on: bool,
    pub left_kg: f32,
    pub right_kg: f32,
    pub independent: bool,
    pub barbell: bool,
    pub mode: WeightMode,
    pub eccentric_kg: f32,
    pub chain_kg: f32,
    pub speed: f32,
    pub assist: bool,
    pub assist_level: u8,
    pub smart_off: bool,
    pub unit: Unit,
    pub model: Model,
    /// Practice lifter pulls the cables automatically while the weight is on.
    pub practice_lifter: bool,
    /// The cables are driven by the real machine's live data.
    pub external: bool,
    pub left: Cable,
    pub right: Cable,
    pub power_history: VecDeque<f32>,
    /// Power of each cable (W), for the two-colour graph.
    pub power_left: VecDeque<f32>,
    pub power_right: VecDeque<f32>,
    /// Real machine: the latest live frame is recent (set by app.rs every frame).
    pub live_fresh: bool,
    sim_t: f32,
    sim_reps_this_set: u32,
    idle_s: f32,
    /// Real machine: lowest distance of each cable since the weight went on ("home").
    home_m: [f32; 2],
    /// Real machine: where each cable last stopped, and whether one moved since the last step.
    anchor_m: [f32; 2],
    moved: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            weight_on: false,
            // the unit starts as kg, so the original's default 8.0 is 8 kg per cable here;
            // apply_default_weight() redoes this after the saved unit is known
            left_kg: m::DEFAULT_WEIGHT_SHOWN,
            right_kg: m::DEFAULT_WEIGHT_SHOWN,
            independent: false,
            barbell: false,
            mode: WeightMode::Standard,
            eccentric_kg: m::DEFAULT_WEIGHT_SHOWN * m::DEFAULT_EXTRA_RATIO,
            chain_kg: m::DEFAULT_WEIGHT_SHOWN * m::DEFAULT_EXTRA_RATIO,
            speed: m::SPEED_DEFAULT,
            // one of Barbell / Dumbbells / Assist is always selected; linked handles = Assist
            assist: true,
            assist_level: 1,
            smart_off: m::SMART_OFF_DEFAULT,
            unit: Unit::Kg,
            model: Model::Monster2,
            practice_lifter: true,
            external: false,
            left: Cable { force_kg: m::BASE_TENSION_KG, ..Default::default() },
            right: Cable { force_kg: m::BASE_TENSION_KG, ..Default::default() },
            power_history: VecDeque::from(vec![0.0; HISTORY]),
            power_left: VecDeque::from(vec![0.0; HISTORY]),
            power_right: VecDeque::from(vec![0.0; HISTORY]),
            live_fresh: false,
            sim_t: 0.0,
            sim_reps_this_set: 0,
            idle_s: 0.0,
            home_m: [f32::INFINITY; 2],
            anchor_m: [0.0; 2],
            moved: false,
        }
    }
}

/// What happened during one simulation step.
#[derive(Default)]
pub struct StepEvents {
    pub smart_off_fired: bool,
}

/// One decimal, trailing ".0" dropped (_formatWeight at 0x391164c / 0x39117cc).
pub fn fmt_num(v: f32) -> String {
    let s = format!("{:.1}", v);
    // an empty sum is -0.0 and tiny negatives round to "-0.0": show them as 0
    let s = if s == "-0.0" { "0.0".to_string() } else { s };
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

impl Machine {
    // ---------- units ----------
    pub fn to_unit(&self, kg: f32) -> f32 {
        match self.unit {
            Unit::Kg => kg,
            Unit::Lb => kg * m::LB_PER_KG,
        }
    }
    pub fn from_unit(&self, v: f32, unit: Option<Unit>) -> f32 {
        match unit.unwrap_or(self.unit) {
            Unit::Kg => v,
            Unit::Lb => v / m::LB_PER_KG,
        }
    }
    pub fn unit_label(&self) -> &'static str {
        match self.unit {
            Unit::Kg => "kg",
            Unit::Lb => "lb",
        }
    }
    pub fn fmt(&self, kg: f32) -> String {
        format!("{} {}", fmt_num(self.to_unit(kg)), self.unit_label())
    }
    pub fn step_kg(&self) -> f32 {
        match self.unit {
            Unit::Kg => m::STEP_KG,
            Unit::Lb => m::STEP_LB / m::LB_PER_KG,
        }
    }
    /// Round to the display step so lb values stay whole.
    fn snap(&self, kg: f32) -> f32 {
        let s = self.step_kg();
        (kg / s).round() * s
    }

    // ---------- the weight the person sees ----------
    /// Cables the shown weight covers: 2 in barbell mode (total of both sides), else 1.
    /// Routines 0x1c41778 and 0x39118f4 switch on this; the barbell mapping is inferred.
    fn cables(&self) -> f32 {
        if self.barbell { 2.0 } else { 1.0 }
    }
    pub fn shown_kg(&self) -> f32 {
        self.left_kg * self.cables()
    }
    fn side_kg(&self, side: Side) -> f32 {
        match side {
            Side::Right if self.independent && !self.barbell => self.right_kg,
            _ => self.left_kg,
        }
    }
    /// Shown weight of a side, in the display unit.
    fn shown(&self, side: Side) -> f32 {
        self.to_unit(self.side_kg(side) * self.cables())
    }
    /// A value spoken in `unit` (or the display unit) converted to the display unit.
    fn to_display(&self, v: f32, unit: Option<Unit>) -> f32 {
        self.to_unit(self.from_unit(v, unit))
    }
    /// (min, max) of the shown weight in the display unit: 0x1c41778 and 0x1c2dc80.
    pub fn limits_shown(&self) -> (f32, f32) {
        let max_total = match (self.unit, self.model.is_high()) {
            (Unit::Kg, false) => m::MAX_TOTAL_KG,
            (Unit::Kg, true) => m::MAX_TOTAL_KG_HIGH,
            (Unit::Lb, false) => m::MAX_TOTAL_LB,
            (Unit::Lb, true) => m::MAX_TOTAL_LB_HIGH,
        };
        if self.barbell {
            let min = if self.unit == Unit::Kg { m::MIN_TOTAL_KG } else { m::MIN_TOTAL_LB };
            (min, max_total)
        } else {
            let min = if self.unit == Unit::Kg { m::MIN_PER_SIDE_KG } else { m::MIN_PER_SIDE_LB };
            (min, max_total / 2.0)
        }
    }
    /// "A bit": _computeFuzzyDelta at 0x3932ecc.
    pub fn fuzzy_delta(&self, current: f32) -> f32 {
        let threshold = if self.barbell { m::FUZZY_THRESHOLD_TOTAL } else { m::FUZZY_THRESHOLD_SIDE };
        if current <= threshold {
            (current * m::FUZZY_RATIO_LOW).ceil()
        } else {
            (current * m::FUZZY_RATIO_HIGH).floor()
        }
    }
    /// Largest extra (eccentric / chain) weight per cable, kg, for the current mode and weight:
    /// 0x1c2d7ac, max(0, min(max − weight, weight × ratio)), ratio 1.5 chain / 0.5 eccentric,
    /// 0 in the other modes. The formula is linear, so per cable gives the same result as the
    /// original's shown (barbell-total) values. With independent weights both sides must fit.
    pub fn max_extra_kg(&self) -> f32 {
        let ratio = match self.mode {
            WeightMode::Chain => m::EXTRA_RATIO_CHAIN,
            WeightMode::Eccentric => m::EXTRA_RATIO_ECCENTRIC,
            _ => return 0.0,
        };
        let hi_kg = self.from_unit(self.limits_shown().1, None) / self.cables();
        let sides: &[f32] = if self.independent && !self.barbell { &[self.left_kg, self.right_kg] } else { &[self.left_kg] };
        sides
            .iter()
            .map(|w| (hi_kg - w).min(w * ratio).max(0.0))
            .fold(f32::INFINITY, f32::min)
    }

    /// 0x1c4fdf0: after a weight change, lower the extra of the current mode to its cap.
    fn clamp_extra(&mut self) {
        let cap = self.max_extra_kg();
        match self.mode {
            WeightMode::Eccentric => self.eccentric_kg = self.eccentric_kg.min(cap),
            WeightMode::Chain => self.chain_kg = self.chain_kg.min(cap),
            _ => {}
        }
    }

    /// The original's starting values when nothing was saved: 8 in the unit on screen
    /// (0x3acdf70), extra = half of it (0x3ace0f4), clamped to the limits (0x1c5021c).
    pub fn apply_default_weight(&mut self) {
        let (lo, hi) = self.limits_shown();
        let shown = (m::DEFAULT_WEIGHT_SHOWN * self.cables()).clamp(lo, hi);
        let _ = self.set_shown(Side::Both, shown);
        let extra = self.snap(self.left_kg * m::DEFAULT_EXTRA_RATIO);
        self.eccentric_kg = extra;
        self.chain_kg = extra;
    }

    /// Restore a saved weight (kg per cable) the way 0x3acda18 does: clamp to the limits.
    pub fn restore_weight(&mut self, kg: f32) {
        let (lo, hi) = self.limits_shown();
        let shown = (self.to_unit(kg) * self.cables()).clamp(lo, hi);
        let _ = self.set_shown(Side::Both, shown);
    }

    fn fmt_shown(&self, v: f32) -> String {
        format!("{} {}", fmt_num(v), self.unit_label())
    }

    /// Set the shown weight of a side (display unit), refusing values outside the limits
    /// with the original's messages (_validateWeight at 0x3932cbc).
    fn set_shown(&mut self, side: Side, shown: f32) -> Result<(), String> {
        let (lo, hi) = self.limits_shown();
        if shown > hi + 1e-3 {
            return Err(format!(
                "The maximum available weight is {}. I can't set it to {}.",
                self.fmt_shown(hi),
                self.fmt_shown(shown)
            ));
        }
        if shown < lo - 1e-3 {
            return Err(format!(
                "The minimum available weight is {}. I can't set it to {}.",
                self.fmt_shown(lo),
                self.fmt_shown(shown)
            ));
        }
        let lo_kg = self.from_unit(lo, None) / self.cables();
        let hi_kg = self.from_unit(hi, None) / self.cables();
        let kg = self.snap(self.from_unit(shown, None) / self.cables()).clamp(lo_kg, hi_kg);
        match (side, self.independent && !self.barbell) {
            (Side::Left, true) => self.left_kg = kg,
            (Side::Right, true) => self.right_kg = kg,
            _ => {
                self.left_kg = kg;
                self.right_kg = kg;
            }
        }
        self.clamp_extra();
        Ok(())
    }

    /// Used by the +/- buttons: one step, kept inside the limits.
    pub fn step_button(&mut self, side: Side, dir: i8) -> Result<(), String> {
        let step = self.to_unit(self.step_kg()) * self.cables();
        let (lo, hi) = self.limits_shown();
        let target = (self.shown(side) + step * dir as f32).clamp(lo, hi);
        self.set_shown(side, target)
    }

    pub fn weight_text(&self) -> String {
        if self.independent && !self.barbell && (self.left_kg - self.right_kg).abs() > 1e-3 {
            format!("left {} and right {}", self.fmt(self.left_kg), self.fmt(self.right_kg))
        } else {
            self.fmt(self.shown_kg())
        }
    }

    /// Which of Barbell / Dumbbells / Assist is selected.
    pub fn accessory(&self) -> Accessory {
        if self.barbell {
            Accessory::Barbell
        } else if self.independent {
            Accessory::Dumbbells
        } else {
            Accessory::Assist
        }
    }

    /// Select Barbell, Dumbbells or Assist; the other two are turned off. Refused while the
    /// weight is on if it would change barbell or dumbbells: that changes the kind of command
    /// the motors get mid-set (and dumbbells use the not-yet-verified two-player frames).
    pub fn set_accessory(&mut self, acc: Accessory) -> Result<(), String> {
        let (barbell, independent) = match acc {
            Accessory::Barbell => (true, false),
            Accessory::Dumbbells => (false, true),
            Accessory::Assist => (false, false),
        };
        if self.weight_on && (barbell != self.barbell || independent != self.independent) {
            return Err(format!(
                "Turn the weight off first: I can't switch to {} while the weight is on.",
                acc.label().to_lowercase()
            ));
        }
        // barbell and linked handles use one weight for both cables
        if !independent {
            self.right_kg = self.left_kg;
        }
        self.barbell = barbell;
        self.independent = independent;
        self.assist = acc == Accessory::Assist;
        // keep the shown weight inside the new limits (both sides are equal at this point)
        let (lo, hi) = self.limits_shown();
        let cur = self.shown(Side::Both).clamp(lo, hi);
        let _ = self.set_shown(Side::Both, cur);
        Ok(())
    }

    /// The real machine has just connected. Anything that was turned on in practice mode is
    /// turned off, so the motors only start when the person asks again. Returns true if the
    /// weight was on.
    pub fn machine_connected(&mut self) -> bool {
        self.external = true;
        self.idle_s = 0.0;
        let was_on = self.weight_on;
        self.weight_on = false;
        was_on
    }

    /// Apply a machine command; returns the reply to show. Session commands return None.
    pub fn apply(&mut self, cmd: &Command) -> Option<String> {
        Some(match cmd {
            Command::WeightOn => {
                if self.weight_on {
                    "Weight is already on.".into()
                } else {
                    self.weight_on = true;
                    self.sim_reps_this_set = 0;
                    self.idle_s = 0.0;
                    format!("Weight on: {}.", self.weight_text())
                }
            }
            Command::WeightOff => {
                if !self.weight_on {
                    "Weight is already off.".into()
                } else {
                    self.weight_on = false;
                    "Weight off.".into()
                }
            }
            Command::Bump { .. } | Command::SetWeight { .. } if self.mode == WeightMode::Isokinetic => {
                // voice_speed_mode_not_support_weight (failure list at 0x391123c)
                "Speed mode does not support weight adjustment.".into()
            }
            Command::Bump { dir, amount, unit, side } => {
                // increaseWeight 0x3ccba6c / decreaseWeight 0x3ccb814
                let current = self.shown(*side);
                let delta = match amount {
                    Some(a) => self.to_display(*a, *unit),
                    None => self.fuzzy_delta(current),
                };
                let (lo, hi) = self.limits_shown();
                if *dir > 0 && current >= hi - 1e-3 {
                    return Some(format!(
                        "The maximum supported weight is {}. It cannot be increased.",
                        self.fmt_shown(hi)
                    ));
                }
                if *dir < 0 && current <= lo + 1e-3 {
                    return Some(format!(
                        "The minimum supported weight is {}. It cannot be decreased.",
                        self.fmt_shown(lo)
                    ));
                }
                match self.set_shown(*side, current + delta * *dir as f32) {
                    Ok(()) => format!("Done. Weight has been adjusted to {}.", self.weight_text()),
                    Err(e) => e,
                }
            }
            Command::SetWeight { value, unit, side } => {
                let shown = self.to_display(*value, *unit);
                match self.set_shown(*side, shown) {
                    Ok(()) => match side {
                        Side::Left if self.independent => format!("Left weight set to {}.", self.fmt(self.left_kg)),
                        Side::Right if self.independent => format!("Right weight set to {}.", self.fmt(self.right_kg)),
                        _ => format!("Done. Weight has been adjusted to {}.", self.weight_text()),
                    },
                    Err(e) => e,
                }
            }
            Command::SetMode(mode) => {
                // switchWeightMode at 0x3cc0da4
                if self.mode == *mode {
                    format!("Already in {} mode.", mode.label().to_lowercase())
                } else if self.weight_on {
                    self.weight_on = false;
                    self.mode = *mode;
                    self.clamp_extra();
                    format!(
                        "The weight was on, so it was turned off first. Switched to {} mode.",
                        mode.label().to_lowercase()
                    )
                } else {
                    self.mode = *mode;
                    self.clamp_extra();
                    format!("Switched to {} mode.", mode.label().to_lowercase())
                }
            }
            Command::SetExtra { kind, value, unit } => {
                if self.mode == WeightMode::Isokinetic {
                    return Some(match kind {
                        Extra::Eccentric => "Eccentric weight isn't available in isokinetic mode.".into(),
                        Extra::Chain => "Chain weight isn't available in isokinetic mode.".into(),
                    });
                }
                // sv_eccentric_mode_required / sv_chain_mode_required
                match kind {
                    Extra::Eccentric if self.mode != WeightMode::Eccentric => {
                        return Some("Please switch to eccentric mode first.".into())
                    }
                    Extra::Chain if self.mode != WeightMode::Chain => {
                        return Some("Please switch to chain mode first.".into())
                    }
                    _ => {}
                }
                // cap from 0x1c2d7ac (weight × 1.5 chain / × 0.5 eccentric, within the maximum)
                let kg = self.snap(self.from_unit(*value, *unit)).max(0.0).min(self.max_extra_kg());
                match kind {
                    Extra::Eccentric => {
                        self.eccentric_kg = kg;
                        format!("Eccentric weight adjusted to {}.", self.fmt(kg))
                    }
                    Extra::Chain => {
                        self.chain_kg = kg;
                        format!("Chain weight adjusted to {}.", self.fmt(kg))
                    }
                }
            }
            Command::SetSpeed(v) => {
                // setSpeed at 0x3ccd4dc: only in speed (isokinetic) mode
                if self.mode != WeightMode::Isokinetic {
                    return Some("Speed adjustment is only available in isokinetic mode.".into());
                }
                self.speed = ((v * 20.0).round() / 20.0).clamp(m::SPEED_MIN, m::SPEED_MAX);
                format!("Speed adjusted to {} meters per second.", fmt_num(self.speed))
            }
            // Barbell, Dumbbells and Assist are one choice: exactly one is on (see Accessory).
            // Turning one "off" by voice moves to linked handles (Assist) instead.
            Command::Barbell(true) => match self.set_accessory(Accessory::Barbell) {
                Ok(()) => "Barbell mode on. The weight shown is the total of both sides.".into(),
                Err(e) => e,
            },
            Command::Barbell(false) if self.barbell => match self.set_accessory(Accessory::Assist) {
                Ok(()) => "Non-barbell mode: left and right linked, with assist.".into(),
                Err(e) => e,
            },
            Command::Barbell(false) => "Non-barbell mode.".into(),
            Command::Independent(true) => match self.set_accessory(Accessory::Dumbbells) {
                Ok(()) => "Dumbbells: separate left and right weights.".into(),
                Err(e) => e,
            },
            Command::Independent(false) if self.independent => match self.set_accessory(Accessory::Assist) {
                Ok(()) => "Left and right weights are linked, with assist.".into(),
                Err(e) => e,
            },
            Command::Independent(false) => "Left and right weights are linked.".into(),
            Command::Assist(true) => match self.set_accessory(Accessory::Assist) {
                Ok(()) => "Assist mode has been enabled.".into(),
                Err(e) => e,
            },
            Command::Assist(false) if self.assist => {
                "Assist stays on until you choose Barbell or Dumbbells instead.".into()
            }
            Command::Assist(false) => "Assist is already off.".into(),
            Command::AssistLevel(l) => match self.set_accessory(Accessory::Assist) {
                Ok(()) => {
                    self.assist_level = (*l).clamp(1, m::ASSIST_LEVELS);
                    format!("Assist has been adjusted to level {}.", self.assist_level)
                }
                Err(e) => e,
            },
            Command::SmartLoadOff(on) => {
                self.smart_off = *on;
                if *on {
                    "Smart load off is on: the weight turns off after each set.".into()
                } else {
                    "Smart load off is off.".into()
                }
            }
            Command::Units(u) => {
                self.unit = *u;
                // keep weights inside the limits of the new unit (8 lb vs 3.5 kg minimum)
                let (lo, hi) = self.limits_shown();
                for side in [Side::Left, Side::Right] {
                    let cur = self.shown(side);
                    if cur < lo || cur > hi {
                        let _ = self.set_shown(side, cur.clamp(lo, hi));
                    }
                }
                format!("Showing weights in {}.", self.unit_label())
            }
            Command::QueryWeight => {
                let state = if self.weight_on { "on" } else { "off" };
                let mut s = format!("The current weight is {} (weight {state}).", self.weight_text());
                match self.mode {
                    WeightMode::Eccentric => s += &format!(" Eccentric weight is {}.", self.fmt(self.eccentric_kg)),
                    WeightMode::Chain => s += &format!(" Chain weight is {}.", self.fmt(self.chain_kg)),
                    WeightMode::Isokinetic => s += &format!(" Speed is {} m/s.", fmt_num(self.speed)),
                    WeightMode::Standard => {}
                }
                s
            }
            _ => return None,
        })
    }

    /// Force one cable pulls with right now, from the mode rules.
    fn force_for(&self, set_kg: f32, c: &Cable) -> f32 {
        if !self.weight_on {
            return m::BASE_TENSION_KG;
        }
        match self.mode {
            WeightMode::Standard => set_kg,
            WeightMode::Eccentric => {
                if c.vel < 0.0 { set_kg + self.eccentric_kg } else { set_kg }
            }
            WeightMode::Chain => set_kg + self.chain_kg * c.pos,
            WeightMode::Isokinetic => set_kg * (0.85 + 0.3 * (std::f32::consts::PI * c.pos).sin()),
        }
    }

    fn push_power(&mut self, left_w: f32, right_w: f32) {
        for (h, v) in [
            (&mut self.power_left, left_w),
            (&mut self.power_right, right_w),
            (&mut self.power_history, left_w + right_w),
        ] {
            h.pop_front();
            h.push_back(v);
        }
    }

    /// Live data from the real machine (frame type 0x80, decoded like 0x1b9c29c).
    /// `reps` are the machine's counters minus the value at the start of the set;
    /// `distance_m` is how far each cable is out; `force_kg` the torque in kg.
    pub fn apply_live(&mut self, reps: [u32; 2], distance_m: [f32; 2], force_kg: [f32; 2], dt: f32) {
        for (i, c) in [&mut self.left, &mut self.right].into_iter().enumerate() {
            let d = distance_m[i];
            let pos = (d / LIVE_TRAVEL_M).clamp(0.0, 1.0);
            c.vel = if dt > 0.0 { (pos - c.pos) * LIVE_TRAVEL_M / dt } else { 0.0 };
            c.pos = pos;
            c.dist_m = d;
            c.force_kg = force_kg[i];
            c.reps = reps[i];
            // smart load off: lowest point since the weight went on, and movement since the
            // cable last stopped (distance-based, so it doesn't depend on frame timing)
            self.home_m[i] = self.home_m[i].min(d);
            if (d - self.anchor_m[i]).abs() > m::SMART_OFF_STILL_TOL_M {
                self.anchor_m[i] = d;
                self.moved = true;
            }
        }
    }

    /// Real machine: both cables are back near the lowest point they reached this set.
    fn cables_home(&self) -> bool {
        [self.left.dist_m, self.right.dist_m]
            .iter()
            .zip(self.home_m)
            .all(|(d, home)| home.is_finite() && *d <= home + m::SMART_OFF_HOME_TOL_M)
    }

    /// Advance the machine by dt seconds. With `external` set, the cables come from the real
    /// machine (apply_live) and only power and smart load off are computed here.
    pub fn step(&mut self, dt: f32) -> StepEvents {
        if self.external {
            let mut ev = StepEvents::default();
            let pl = self.left.force_kg * self.left.vel.abs() * G;
            let pr = self.right.force_kg * self.right.vel.abs() * G;
            self.push_power(pl, pr);
            // Fires only when the cables are back in (not when someone pauses at the top of a
            // rep), have stayed put, and the machine's live data is arriving.
            let moved = std::mem::take(&mut self.moved);
            let ready = self.weight_on
                && self.smart_off
                && self.live_fresh
                && self.left.reps.max(self.right.reps) > 0
                && self.cables_home();
            if ready && !moved {
                self.idle_s += dt;
                if self.idle_s >= m::SMART_OFF_IDLE_S {
                    self.weight_on = false;
                    self.idle_s = 0.0;
                    ev.smart_off_fired = true;
                }
            } else {
                self.idle_s = 0.0;
            }
            return ev;
        }
        let mut ev = StepEvents::default();
        let lifting = self.weight_on && self.practice_lifter && self.sim_reps_this_set < SIM_REPS_PER_SET;
        let period = if self.mode == WeightMode::Isokinetic {
            (2.0 * SIM_TRAVEL_M / self.speed.max(0.05)).max(1.2)
        } else {
            SIM_REP_SECONDS
        };
        if lifting {
            self.sim_t += dt;
        }
        let (lk, rk) = (self.left_kg, self.right_kg);
        let mut power = [0.0f32; 2];
        for (i, set_kg) in [(0, lk), (1, rk)] {
            let phase = if i == 0 { 0.0 } else { 0.07 };
            let mut c = if i == 0 { self.left } else { self.right };
            let new_pos = if lifting {
                let x = (self.sim_t / period + phase) * std::f32::consts::TAU;
                (1.0 - x.cos()) / 2.0
            } else {
                (c.pos - dt * 1.5).max(0.0)
            };
            c.vel = (new_pos - c.pos) * SIM_TRAVEL_M / dt.max(1e-4);
            c.pos = new_pos;
            if c.pos > 0.9 {
                c.at_top = true;
            }
            if c.at_top && c.pos < 0.1 {
                c.at_top = false;
                if self.weight_on {
                    c.reps += 1;
                    if i == 0 {
                        self.sim_reps_this_set += 1;
                    }
                }
            }
            c.force_kg = self.force_for(set_kg, &c);
            power[i] = c.force_kg * G * c.vel.abs();
            if i == 0 { self.left = c } else { self.right = c }
        }
        self.push_power(power[0], power[1]);

        // smart load off: cable still after at least one rep
        let still = self.left.pos < 0.05 && self.right.pos < 0.05;
        if self.weight_on && self.smart_off && self.left.reps.max(self.right.reps) > 0 && still {
            self.idle_s += dt;
            if self.idle_s >= m::SMART_OFF_IDLE_S {
                self.weight_on = false;
                self.idle_s = 0.0;
                ev.smart_off_fired = true;
            }
        } else {
            self.idle_s = 0.0;
        }
        ev
    }

    pub fn reset_reps(&mut self) {
        self.left.reps = 0;
        self.right.reps = 0;
        self.sim_reps_this_set = 0;
        self.sim_t = 0.0;
        // the real machine's "home" is learned again for each set
        self.home_m = [self.left.dist_m, self.right.dist_m];
        self.idle_s = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::parse;

    #[test]
    fn no_minus_zero() {
        // the summary's volume with no sets is an empty sum, -0.0
        assert_eq!(fmt_num(Vec::<f32>::new().into_iter().sum()), "0");
        assert_eq!(fmt_num(-0.04), "0");
        assert_eq!(fmt_num(-1.0), "-1");
        assert_eq!(fmt_num(2.5), "2.5");
    }

    #[test]
    fn voice_flow() {
        let mut mc = Machine::default();
        mc.apply(&parse("weight on").unwrap());
        assert!(mc.weight_on);
        mc.apply(&parse("set weight to twenty").unwrap());
        assert_eq!(mc.left_kg, 20.0);
        // fuzzy delta: 20 <= 25 -> ceil(2.0) = 2
        mc.apply(&parse("weight up").unwrap());
        assert_eq!(mc.left_kg, 22.0);
        mc.apply(&parse("weight down by three").unwrap());
        assert_eq!(mc.left_kg, 19.0);
        let r = mc.apply(&parse("set weight to two hundred").unwrap()).unwrap();
        assert!(r.contains("maximum available weight is 50 kg"), "{r}");
        mc.apply(&parse("use pounds").unwrap());
        mc.apply(&parse("set weight to 44").unwrap());
        assert!((mc.left_kg - 20.0).abs() < 0.01);
        mc.apply(&parse("weight off").unwrap());
        assert!(!mc.weight_on);
    }

    #[test]
    fn decompiled_limits() {
        let mut mc = Machine::default();
        assert_eq!(mc.limits_shown(), (3.5, 50.0));
        mc.model = Model::Monster3S;
        assert_eq!(mc.limits_shown(), (3.5, 60.0));
        mc.unit = Unit::Lb;
        assert_eq!(mc.limits_shown(), (8.0, 130.0));
        mc.barbell = true;
        assert_eq!(mc.limits_shown(), (16.0, 260.0));
        mc.unit = Unit::Kg;
        mc.model = Model::Monster2;
        assert_eq!(mc.limits_shown(), (7.0, 100.0));
    }

    #[test]
    fn fuzzy_delta_rule() {
        let mut mc = Machine::default();
        assert_eq!(mc.fuzzy_delta(5.0), 1.0); // ceil(0.5)
        assert_eq!(mc.fuzzy_delta(25.0), 3.0); // ceil(2.5)
        assert_eq!(mc.fuzzy_delta(40.0), 2.0); // floor(2.0)
        mc.barbell = true;
        assert_eq!(mc.fuzzy_delta(40.0), 4.0); // threshold 50 -> ceil(4.0)
        assert_eq!(mc.fuzzy_delta(90.0), 4.0); // floor(4.5)
    }

    #[test]
    fn mode_rules() {
        let mut mc = Machine::default();
        let r = mc.apply(&parse("set eccentric weight to five").unwrap()).unwrap();
        assert!(r.contains("switch to eccentric mode first"));
        let r = mc.apply(&parse("set speed to point five").unwrap()).unwrap();
        assert!(r.contains("only available in isokinetic"));
        mc.apply(&Command::WeightOn);
        let r = mc.apply(&Command::SetMode(WeightMode::Isokinetic)).unwrap();
        assert!(!mc.weight_on && r.contains("turned off first"));
        let r = mc.apply(&parse("weight up").unwrap()).unwrap();
        assert!(r.contains("Speed mode does not support"));
    }

    #[test]
    fn extra_cap_and_defaults_like_the_original() {
        // nothing saved: 8 in the unit on screen, extra = half (0x3acdf70 / 0x3ace0f4)
        let mut mc = Machine::default();
        mc.apply_default_weight();
        assert_eq!(mc.shown_kg(), 8.0);
        assert_eq!(mc.eccentric_kg, 4.0);
        let mut lb = Machine { unit: Unit::Lb, ..Machine::default() };
        lb.apply_default_weight();
        assert!((lb.to_unit(lb.left_kg) - 8.0).abs() < 1e-3);
        // 0x1c2d7ac: max(0, min(max − w, w × ratio)); eccentric 0.5, chain 1.5, others 0
        mc.restore_weight(20.0);
        assert_eq!(mc.max_extra_kg(), 0.0); // standard mode
        mc.apply(&Command::SetMode(WeightMode::Eccentric));
        assert_eq!(mc.max_extra_kg(), 10.0);
        mc.apply(&Command::SetMode(WeightMode::Chain));
        assert_eq!(mc.max_extra_kg(), 30.0);
        mc.restore_weight(40.0); // 50 kg per cable max: headroom 10 < 60
        assert_eq!(mc.max_extra_kg(), 10.0);
        mc.apply(&parse("set chain weight to twenty five").unwrap());
        assert_eq!(mc.chain_kg, 10.0);
        // 0x1c4fdf0: lowering the weight lowers the extra to the new cap
        mc.restore_weight(4.0);
        assert!((mc.chain_kg - 6.0).abs() < 1e-3);
    }

    #[test]
    fn barbell_total() {
        let mut mc = Machine::default();
        mc.apply(&Command::Barbell(true));
        mc.apply(&parse("set weight to forty").unwrap());
        assert_eq!(mc.left_kg, 20.0);
        assert_eq!(mc.shown_kg(), 40.0);
    }

    #[test]
    fn smart_off_after_practice_set() {
        let mut mc = Machine::default();
        mc.apply(&Command::WeightOn);
        let mut fired = false;
        for _ in 0..(60 * 40) {
            if mc.step(1.0 / 60.0).smart_off_fired {
                fired = true;
                break;
            }
        }
        assert!(fired);
        assert_eq!(mc.left.reps, SIM_REPS_PER_SET);
    }

    #[test]
    fn connecting_never_turns_the_motors_on() {
        // weight turned on in practice mode while the Speediance app held the port
        let mut mc = Machine::default();
        mc.apply(&Command::WeightOn);
        assert!(mc.machine_connected());
        assert!(!mc.weight_on && mc.external);
        // nothing was on: nothing to report
        assert!(!mc.machine_connected());
    }

    #[test]
    fn no_barbell_or_independent_switch_while_loaded() {
        let mut mc = Machine::default();
        mc.apply(&Command::WeightOn);
        let r = mc.apply(&Command::Barbell(true)).unwrap();
        assert!(!mc.barbell && r.contains("Turn the weight off first"), "{r}");
        let r = mc.apply(&Command::Independent(true)).unwrap();
        assert!(!mc.independent && r.contains("Turn the weight off first"), "{r}");
        // asking for what is already set is fine
        assert!(!mc.apply(&Command::Barbell(false)).unwrap().contains("first"));
        mc.apply(&Command::WeightOff);
        mc.apply(&Command::Barbell(true));
        assert!(mc.barbell);
        mc.apply(&Command::Barbell(false));
        mc.apply(&Command::Independent(true));
        assert!(mc.independent);
    }

    #[test]
    fn barbell_dumbbells_assist_are_one_choice() {
        let mut mc = Machine::default();
        let one = |mc: &Machine| [mc.barbell, mc.independent, mc.assist].iter().filter(|b| **b).count();
        assert_eq!(mc.accessory(), Accessory::Assist); // something is selected from the start
        assert_eq!(one(&mc), 1);
        // straight from barbell to dumbbells, no deselecting in between
        mc.apply(&Command::Barbell(true));
        assert_eq!((mc.accessory(), one(&mc)), (Accessory::Barbell, 1));
        mc.apply(&Command::Independent(true));
        assert_eq!((mc.accessory(), one(&mc)), (Accessory::Dumbbells, 1));
        mc.apply(&Command::AssistLevel(2));
        assert_eq!((mc.accessory(), one(&mc), mc.assist_level), (Accessory::Assist, 1, 2));
        // "turn off" by voice never leaves nothing selected
        mc.apply(&Command::Assist(false));
        assert_eq!(mc.accessory(), Accessory::Assist);
        mc.apply(&Command::Barbell(true));
        mc.apply(&Command::Barbell(false));
        assert_eq!((mc.accessory(), one(&mc)), (Accessory::Assist, 1));
        // assist can be picked with the weight on when it doesn't change the cables' command
        mc.apply(&Command::WeightOn);
        assert!(mc.set_accessory(Accessory::Assist).is_ok());
        assert!(mc.set_accessory(Accessory::Dumbbells).is_err());
        assert_eq!(mc.accessory(), Accessory::Assist);
    }

    /// Feed `secs` of live frames at 50 Hz with both cables at `dist`; true if smart off fired.
    fn live_for(mc: &mut Machine, secs: f32, dist: f32, reps: u32) -> bool {
        let dt = 1.0 / 50.0;
        let mut fired = false;
        for _ in 0..(secs / dt) as usize {
            mc.apply_live([reps; 2], [dist; 2], [20.0; 2], dt);
            fired |= mc.step(dt).smart_off_fired;
        }
        fired
    }

    #[test]
    fn smart_off_on_the_machine_waits_for_the_cables_to_come_home() {
        let mut mc = Machine::default();
        assert!(!mc.machine_connected());
        mc.live_fresh = true;
        mc.apply(&Command::WeightOn);
        mc.reset_reps();
        assert!(!live_for(&mut mc, 1.0, 0.05, 0)); // at rest before the first rep
        // one rep, then a 5 s pause at the top (e.g. standing between squat reps): stays on
        for i in 0..50 {
            mc.apply_live([0; 2], [0.05 + 0.6 * i as f32 / 50.0; 2], [20.0; 2], 0.02);
            mc.step(0.02);
        }
        assert!(!live_for(&mut mc, 5.0, 0.65, 1));
        assert!(mc.weight_on);
        // back home but no live data arriving: still waits
        mc.live_fresh = false;
        assert!(!live_for(&mut mc, 5.0, 0.06, 1));
        // back home with live data: off after SMART_OFF_IDLE_S
        mc.live_fresh = true;
        assert!(!live_for(&mut mc, m::SMART_OFF_IDLE_S - 0.5, 0.06, 1));
        assert!(live_for(&mut mc, 1.0, 0.06, 1));
        assert!(!mc.weight_on);
    }
}

//! A Free Lift session: exercise, sets, rest timer and summary. Nothing here needs an account.

use crate::commands::WeightMode;
use crate::mechanics as m;

#[derive(Clone)]
pub struct SetRecord {
    pub exercise: String,
    pub reps_left: u32,
    pub reps_right: u32,
    pub left_kg: f32,
    pub right_kg: f32,
    pub barbell: bool,
    pub mode: WeightMode,
    /// Extra eccentric weight (Eccentric mode), kg.
    pub eccentric_kg: f32,
    /// Extra chain weight (Chain mode), kg.
    pub chain_kg: f32,
    /// Constant speed (Isokinetic mode), m/s.
    pub speed: f32,
    pub seconds: f32,
    /// Average power during the set, W (`avgPower` in FreeTrainSetSummaryEntity, 0x1ab8ad4).
    pub avg_power_w: f32,
    /// Rest taken before this set, s (`restDurationSec` in the same record).
    pub rest_before_s: f32,
}

impl SetRecord {
    pub fn reps(&self) -> u32 {
        self.reps_left.max(self.reps_right)
    }
    /// Volume (`totalCapacity`) = weight moved × reps, counting each cable with its own weight.
    pub fn volume_kg(&self) -> f32 {
        self.left_kg * self.reps_left as f32 + self.right_kg * self.reps_right as f32
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Ready,
    Lifting,
    Rest { left_s: f32 },
    Done,
}

pub struct Session {
    pub exercises: Vec<String>,
    pub current: usize,
    pub sets: Vec<SetRecord>,
    pub phase: Phase,
    pub paused: bool,
    /// The workout clock runs: set by Start Workout (or the first Weight on), not by opening
    /// the app.
    pub started: bool,
    /// Workout clock, s (only counts while `started` and not paused).
    pub elapsed_s: f32,
    pub set_elapsed_s: f32,
    pub rest_seconds: u32,
    /// Rest timer switch (the original's `free_train_rest_switch_`). Off: a finished set goes
    /// straight to "ready"; the rest taken is still measured and saved with the next set.
    pub rest_on: bool,
    /// Time spent since the last set ended (rest actually taken), s.
    pub rest_taken_s: f32,
    /// Power summed over the current set's frames, and the frame count, for the average.
    pub power_sum: f32,
    pub power_samples: u32,
    /// Rest taken before the set that is running now, s.
    pub rest_before_current: f32,
}

pub const DEFAULT_EXERCISES: [&str; 10] = [
    "Free lift",
    "Bench press",
    "Bent-over row",
    "Squat",
    "Shoulder press",
    "Lat pulldown",
    "Biceps curl",
    "Triceps pushdown",
    "Deadlift",
    "Chest fly",
];

impl Default for Session {
    fn default() -> Self {
        Self {
            exercises: DEFAULT_EXERCISES.iter().map(|s| s.to_string()).collect(),
            current: 0,
            sets: Vec::new(),
            phase: Phase::Ready,
            paused: false,
            started: false,
            elapsed_s: 0.0,
            set_elapsed_s: 0.0,
            rest_seconds: m::REST_DEFAULT_S,
            rest_on: m::REST_SWITCH_DEFAULT,
            rest_taken_s: 0.0,
            power_sum: 0.0,
            power_samples: 0,
            rest_before_current: 0.0,
        }
    }
}

impl Session {
    pub fn exercise(&self) -> &str {
        &self.exercises[self.current.min(self.exercises.len() - 1)]
    }
    pub fn next_exercise(&mut self) {
        self.current = (self.current + 1) % self.exercises.len();
    }
    pub fn previous_exercise(&mut self) {
        self.current = (self.current + self.exercises.len() - 1) % self.exercises.len();
    }
    pub fn sets_of_current(&self) -> usize {
        self.sets.iter().filter(|s| s.exercise == self.exercise()).count()
    }
    pub fn total_reps(&self) -> u32 {
        self.sets.iter().map(|s| s.reps()).sum()
    }
    pub fn total_volume_kg(&self) -> f32 {
        self.sets.iter().map(|s| s.volume_kg()).sum()
    }
    pub fn tick(&mut self, dt: f32) {
        if self.paused || self.phase == Phase::Done {
            return;
        }
        if self.started {
            self.elapsed_s += dt;
        }
        match &mut self.phase {
            Phase::Lifting => self.set_elapsed_s += dt,
            Phase::Rest { left_s } => {
                self.rest_taken_s += dt;
                *left_s -= dt;
                if *left_s <= 0.0 {
                    self.phase = Phase::Ready;
                }
            }
            // waiting after the rest timer (or before a set) still counts as rest
            Phase::Ready if !self.sets.is_empty() => self.rest_taken_s += dt,
            _ => {}
        }
    }

    /// A set begins: remember the rest taken since the last set and reset the power average.
    pub fn start_set(&mut self) {
        self.rest_before_current = if self.sets.is_empty() { 0.0 } else { self.rest_taken_s };
        self.rest_taken_s = 0.0;
        self.power_sum = 0.0;
        self.power_samples = 0;
    }

    /// Add one power sample (W) while a set is running.
    pub fn sample_power(&mut self, watts: f32) {
        if self.phase == Phase::Lifting && !self.paused {
            self.power_sum += watts;
            self.power_samples += 1;
        }
    }

    /// Average power of the running set, W.
    pub fn avg_power(&self) -> f32 {
        if self.power_samples == 0 { 0.0 } else { self.power_sum / self.power_samples as f32 }
    }
}

pub fn clock(s: f32) -> String {
    let s = s.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_and_power_tracking() {
        let mut s = Session::default();
        s.phase = Phase::Lifting;
        s.start_set();
        s.sample_power(100.0);
        s.sample_power(300.0);
        assert_eq!(s.avg_power(), 200.0);
        assert_eq!(s.rest_before_current, 0.0); // first set: no rest before it
        s.sets.push(SetRecord {
            exercise: "x".into(),
            reps_left: 1,
            reps_right: 1,
            left_kg: 10.0,
            right_kg: 10.0,
            barbell: false,
            mode: WeightMode::Standard,
            eccentric_kg: 0.0,
            chain_kg: 0.0,
            speed: 0.4,
            seconds: 5.0,
            avg_power_w: 200.0,
            rest_before_s: 0.0,
        });
        s.phase = Phase::Rest { left_s: 60.0 };
        for _ in 0..90 {
            s.tick(1.0); // 60 s timer, then 30 s more waiting
        }
        s.phase = Phase::Lifting;
        s.start_set();
        assert!((s.rest_before_current - 90.0).abs() < 1e-3);
        assert_eq!(s.avg_power(), 0.0);
    }

    #[test]
    fn workout_clock_waits_for_start() {
        let mut s = Session::default();
        s.tick(5.0);
        assert_eq!(s.elapsed_s, 0.0); // opening the app doesn't start the clock
        s.started = true;
        s.tick(5.0);
        assert_eq!(s.elapsed_s, 5.0);
        s.paused = true;
        s.tick(5.0);
        assert_eq!(s.elapsed_s, 5.0); // paused: clock stops
        s.paused = false;
        s.phase = Phase::Done;
        s.tick(5.0);
        assert_eq!(s.elapsed_s, 5.0); // ended: clock stops
    }

    #[test]
    fn rest_switch_defaults_on_and_rest_is_measured_when_off() {
        // the original uses true when free_train_rest_switch_ was never saved (0x1c91bdc)
        assert!(Session::default().rest_on);
        let mut s = Session { rest_on: false, ..Session::default() };
        s.sets.push(SetRecord {
            exercise: "x".into(),
            reps_left: 1,
            reps_right: 1,
            left_kg: 10.0,
            right_kg: 10.0,
            barbell: false,
            mode: WeightMode::Standard,
            eccentric_kg: 0.0,
            chain_kg: 0.0,
            speed: 0.4,
            seconds: 5.0,
            avg_power_w: 0.0,
            rest_before_s: 0.0,
        });
        s.phase = Phase::Ready; // timer off: straight to ready
        for _ in 0..45 {
            s.tick(1.0);
        }
        s.start_set();
        assert!((s.rest_before_current - 45.0).abs() < 1e-3);
    }
}

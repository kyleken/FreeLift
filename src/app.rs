//! The Free Lift window.

use crate::commands::{self, Command, Side, Unit, WeightMode};
use crate::machine::{Accessory, Machine, Model};
use crate::mechanics as m;
use crate::protocol::Frame;
use crate::machine_link::{self, In, MachineLink};
use crate::serial_link;
use std::time::{Duration, Instant};

/// Wait this long after a change before re-sending the weight-on frame while the weight is on.
const RESEND_DELAY_MS: u64 = 300;
/// Gap between the left (A1) and right (A2) two-player frames: the original sends the right
/// one from a 50 ms timer (Duration 0xc350 µs in 0x1c84154 → closure 0x1c8445c).
const TWO_PLAYER_GAP_MS: u64 = 50;
/// Lines kept in the Machine window's command log.
const MACHINE_LOG_LINES: usize = 40;
/// Try the machine port again this often while another app holds it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const RETRY_S: u64 = 3;
/// The original app, which holds the machine port while it runs.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const ORIGINAL_PACKAGE: &str = "com.speediance.faraday";
/// Sleep after this many minutes with no taps and no cable movement (owner's choice; 0 = never).
const SLEEP_DEFAULT_MIN: u32 = 30;
/// Longest "Sleep after" the setting allows, in minutes.
const SLEEP_MAX_MIN: u32 = 240;
/// A cable moving this far (metres) counts as use, so a workout never puts the app to sleep.
const SLEEP_MOVE_M: f32 = 0.05;
use crate::session::{clock, Phase, Session, SetRecord};
use crate::voice::{self, Gate, Listen, Voice, VoiceEvent};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Stroke, Vec2};
use std::collections::VecDeque;
use std::path::PathBuf;

const BG: Color32 = Color32::from_rgb(13, 14, 17);
const PANEL: Color32 = Color32::from_rgb(24, 26, 31);
const PANEL2: Color32 = Color32::from_rgb(33, 36, 43);
const TEXT: Color32 = Color32::from_rgb(236, 238, 242);
const MUTED: Color32 = Color32::from_rgb(140, 146, 158);
const ACCENT: Color32 = Color32::from_rgb(92, 141, 230);
const ON: Color32 = Color32::from_rgb(64, 196, 120);
const WARN: Color32 = Color32::from_rgb(240, 160, 70);
const LEFT_C: Color32 = Color32::from_rgb(110, 160, 245);
const RIGHT_C: Color32 = Color32::from_rgb(160, 120, 240);

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Settings {
    voice_on: bool,
    pounds: bool,
    rest_seconds: u32,
    #[serde(default = "default_rest_on")]
    rest_on: bool,
    smart_off: bool,
    practice_lifter: bool,
    baud: u32,
    #[serde(default = "default_model")]
    model: Model,
    /// The last Free Lift values, restored at start like the original (saved in 0x1c4f0fc,
    /// read back in 0x3acda18). None until the first save: then the original's defaults apply.
    #[serde(default)]
    last: Option<LastLift>,
    /// Minutes with no taps and no cable movement before the app sleeps (0 = never).
    #[serde(default = "default_sleep_min")]
    sleep_min: u32,
    /// Voice: every phrase is a command, or phrases start with the wake word "Weight".
    #[serde(default)]
    voice_listen: Listen,
    /// The app's Sleep put the motor board to sleep; send wakeup at the next connect (kept
    /// across restarts, so a board left asleep is always woken).
    #[serde(default)]
    board_asleep: bool,
}

/// What the original keeps between visits: powerValue (weight), selectedModelID (mode),
/// selectedDeviceID (barbell), _slider2Value (extra), speedMps (speed).
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct LastLift {
    kg_per_cable: f32,
    mode: WeightMode,
    barbell: bool,
    eccentric_kg: f32,
    chain_kg: f32,
    speed: f32,
    /// Barbell / Dumbbells / Assist. Missing in older files: taken from `barbell`.
    #[serde(default)]
    accessory: Option<Accessory>,
}

fn default_model() -> Model {
    Model::Monster2
}

fn default_sleep_min() -> u32 {
    SLEEP_DEFAULT_MIN
}

fn default_rest_on() -> bool {
    m::REST_SWITCH_DEFAULT
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            voice_on: false,
            pounds: false,
            rest_seconds: m::REST_DEFAULT_S,
            rest_on: m::REST_SWITCH_DEFAULT,
            smart_off: m::SMART_OFF_DEFAULT,
            practice_lifter: true,
            baud: m::DEFAULT_BAUD,
            model: default_model(),
            last: None,
            sleep_min: SLEEP_DEFAULT_MIN,
            voice_listen: Listen::Always,
            board_asleep: false,
        }
    }
}

/// Reps of the running set from the machine's rep counters (frame bytes 6/7).
/// The machine restarts its counters when the weight goes on again, so a counter below the
/// value noted at the start of the set means it restarted: count from zero from then on.
fn set_reps(base: &mut [u8; 2], counts: [u8; 2]) -> [u32; 2] {
    let mut out = [0u32; 2];
    for i in 0..2 {
        if counts[i] < base[i] {
            base[i] = 0;
        }
        out[i] = (counts[i] - base[i]) as u32;
    }
    out
}

/// What the weight-off check decides (see `FreeLiftApp::check_weight_off`).
#[derive(Debug, PartialEq)]
enum OffCheck {
    /// Too early to tell.
    Wait,
    /// Both cables are back at base tension: the weight is off.
    Confirmed,
    /// Still loaded: send weight off again.
    Resend,
    /// Still loaded after every re-send: warn the person.
    StillLoaded,
    /// No live data at all since the weight-off, so it can't be confirmed: warn the person.
    NoData,
}

/// After a weight-off command, the machine's force reading should fall back to base tension.
/// `elapsed_s` is the time since the last weight-off was sent, `resends` how many extra
/// weight-offs have been sent already. `force_kg` is the latest live reading and
/// `data_since_off` says whether any live frame arrived after the first weight-off. The
/// machine stops streaming live data shortly after the weight goes off (seen on the tablet),
/// so the last frame it sent is what counts, not whether data is still arriving.
fn weight_off_check(elapsed_s: f32, force_kg: [f32; 2], data_since_off: bool, resends: u8) -> OffCheck {
    if elapsed_s < m::OFF_CHECK_S {
        return OffCheck::Wait;
    }
    if !data_since_off {
        return OffCheck::NoData;
    }
    if force_kg.iter().all(|f| *f <= m::BASE_TENSION_KG + m::OFF_MARGIN_KG) {
        OffCheck::Confirmed
    } else if resends < m::OFF_RESENDS {
        OffCheck::Resend
    } else {
        OffCheck::StillLoaded
    }
}

/// On Android: the app's private data folder (set in android_main).
pub static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where the sleep screen's text goes during period `n` (fractions of the screen's width and
/// height, kept away from the edges). A new, scattered place each period, so no pixel stays
/// lit for long (burn-in).
fn sleep_text_spot(n: u64) -> (f32, f32) {
    // a simple integer hash, so neighbouring periods land far apart
    let mut h = n.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    let fx = 0.2 + 0.6 * ((h & 0xFFFF) as f32 / 65535.0);
    let fy = 0.1 + 0.8 * (((h >> 16) & 0xFFFF) as f32 / 65535.0);
    (fx, fy)
}

/// Time to sleep: the "Sleep after" minutes (0 = never) have passed with no use.
fn sleep_due(idle_s: f32, sleep_min: u32) -> bool {
    sleep_min > 0 && idle_s >= sleep_min as f32 * 60.0
}

/// Either cable has moved far enough from where it was last noted to count as use.
fn cables_moved(now_m: [f32; 2], noted_m: [f32; 2]) -> bool {
    now_m.iter().zip(noted_m).any(|(a, b)| (a - b).abs() >= SLEEP_MOVE_M)
}

fn settings_path() -> Option<PathBuf> {
    DATA_DIR.get().map(|dir| dir.join("settings.json"))
}

/// Everything voice heard, one line each, in the app's own folder
/// (`/data/data/com.craftanything.freelift/files/voice-heard.txt`), so the words can be tuned
/// to the owner's voice. Over `VOICE_LOG_MAX` bytes it moves to `voice-heard.old.txt`.
const VOICE_LOG: &str = "voice-heard.txt";
const VOICE_LOG_OLD: &str = "voice-heard.old.txt";
const VOICE_LOG_MAX: u64 = 512 * 1024;

fn append_voice_log(line: &str) {
    use std::io::Write;
    let Some(dir) = DATA_DIR.get() else { return };
    let path = dir.join(VOICE_LOG);
    if std::fs::metadata(&path).map(|m| m.len() > VOICE_LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join(VOICE_LOG_OLD));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Local date and time for the voice log ("2026-11-18 06:29:41").
fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    #[cfg(unix)]
    unsafe {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            );
        }
    }
    format!("t={secs}")
}

/// One line of the voice log: when, listening style, what the recogniser wrote, what it was
/// read as (if different), and what the app did.
fn voice_log_line(stamp: &str, listen: Listen, early: bool, heard: &str, ran: Option<&str>, did: &[String]) -> String {
    let style = match listen {
        Listen::Always => "always",
        Listen::WakeWord => "wake",
    };
    let mut s = format!("{stamp}  {style:<6}  heard \"{heard}\"");
    if early {
        s.push_str(" (partial, acted on at once)");
    }
    if let Some(r) = ran.filter(|r| *r != heard) {
        s.push_str(&format!("  read as \"{r}\""));
    }
    s.push_str("  ->  ");
    s.push_str(&did.join(" / "));
    s
}

/// The layout is drawn for a 1080-point-wide portrait screen; on a tablet, scale it to fit.
#[cfg(target_os = "android")]
fn fit_width(ctx: &egui::Context) {
    let w = ctx.content_rect().width();
    if w > 100.0 && (w - 1080.0).abs() > 8.0 {
        ctx.set_zoom_factor(ctx.zoom_factor() * w / 1080.0);
    }
}

#[derive(Clone)]
struct LogLine {
    heard: Option<String>,
    reply: String,
    ok: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum VoiceState {
    Off,
    Starting,
    Listening,
    Failed,
}

pub struct FreeLiftApp {
    machine: Machine,
    session: Session,
    log: VecDeque<LogLine>,
    /// Lines ever added to `log` (the voice log uses it to find a phrase's replies).
    said: u64,
    typed: String,
    /// The running voice thread (stopped while the app is in the background or asleep).
    voice: Option<Voice>,
    voice_state: VoiceState,
    voice_error: Option<String>,
    /// The Voice switch is on (saved); the thread is (re)started whenever the app is on screen.
    voice_wanted: bool,
    voice_listen: Listen,
    /// The motor board was put to sleep (0x08) by the app's Sleep: wakeup (0x09) is sent first
    /// at the next connect. Saved.
    board_asleep: bool,
    /// Wake-word mode: "Weight" was said on its own; the next phrase until then is a command.
    wake_armed_until: Option<Instant>,
    /// A stop was acted on early; skip it when the same phrase arrives finished.
    early_stop_done: bool,
    /// Next time the voice thread may be started again (Android).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    next_voice_try: Option<Instant>,
    /// Microphone level (loudest sample, smoothed for the bar), when it went quiet, and
    /// whether the "sending silence" warning is showing.
    mic_peak: f32,
    mic_quiet_since: Option<Instant>,
    mic_silent: bool,
    // cable listener
    show_cable: bool,
    ports: Vec<serial_link::PortInfo>,
    port_choice: String,
    baud: u32,
    /// Two-way link to the motor board (machine_link.rs).
    link: Option<MachineLink>,
    link_ok: bool,
    cable_status: String,
    cable_frames: u64,
    cable_last: Option<Frame>,
    last_live: Option<Instant>,
    /// Machine rep counters (frame bytes 6/7) and their value when the set started.
    counts: [u8; 2],
    count_base: [u8; 2],
    /// We sent enable and no pause since.
    loaded: bool,
    last_enable: Vec<u8>,
    changed_at: Option<Instant>,
    heartbeat_on: bool,
    /// The machine port is held by another app (the Speediance app opens it exclusively).
    port_busy: bool,
    /// The machine accepted the current weight-on (reply 0x83 with code 1).
    ack_seen: bool,
    /// Last safety stop reason from the machine (frame 0x82), shown until dismissed.
    safety_stop: Option<String>,
    /// A weight-off was sent and is being checked: when the first one was sent, when the
    /// last one was sent, and how many re-sends.
    off_check: Option<(Instant, Instant, u8)>,
    /// Red warning when the weight-off couldn't be confirmed, shown until dismissed.
    off_alert: Option<String>,
    /// Commands sent and replies received, newest first (shown in the Machine window).
    machine_log: VecDeque<String>,
    /// When to try connecting again (Android).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    next_retry: Option<Instant>,
    // summary
    show_summary: bool,
    was_on: bool,
    /// Natural content heights of the Weight Mode and Voice Control boxes last frame,
    /// so both can be drawn the same height.
    pair_heights: [f32; 2],
    /// Tallest the Free Lift box has been drawn, so it keeps one fixed size.
    weight_card_h: f32,
    /// Sleep: minutes of no use before it (0 = never), when the app was last used, the cable
    /// positions noted then, and whether it is asleep (black screen, motors off, cable let go).
    sleep_min: u32,
    last_use: Instant,
    use_cable_m: [f32; 2],
    asleep: bool,
    /// When it went to sleep: taps in the first `SLEEP_WAKE_GUARD_S` don't wake it (the Sleep
    /// button's own tap).
    slept_at: Instant,
}

impl FreeLiftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style(&cc.egui_ctx);
        let settings: Settings = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut machine = Machine::default();
        machine.unit = if settings.pounds { Unit::Lb } else { Unit::Kg };
        machine.smart_off = settings.smart_off;
        machine.practice_lifter = settings.practice_lifter;
        machine.model = settings.model;
        match &settings.last {
            Some(l) => {
                // the last Barbell / Dumbbells / Assist choice (weight is off at start)
                let acc = l.accessory.unwrap_or(if l.barbell { Accessory::Barbell } else { Accessory::Assist });
                machine.barbell = acc == Accessory::Barbell;
                machine.independent = acc == Accessory::Dumbbells;
                machine.assist = acc == Accessory::Assist;
                machine.mode = l.mode;
                machine.speed = l.speed.clamp(m::SPEED_MIN, m::SPEED_MAX);
                machine.eccentric_kg = l.eccentric_kg.max(0.0);
                machine.chain_kg = l.chain_kg.max(0.0);
                // clamped to the limits, which also lowers the extra to its cap (0x1c4fdf0)
                machine.restore_weight(l.kg_per_cable);
            }
            None => machine.apply_default_weight(),
        }
        let mut session = Session::default();
        session.rest_seconds = settings.rest_seconds;
        session.rest_on = settings.rest_on;
        let mut app = Self {
            machine,
            session,
            log: VecDeque::new(),
            said: 0,
            typed: String::new(),
            voice: None,
            voice_state: VoiceState::Off,
            voice_error: None,
            voice_wanted: settings.voice_on,
            voice_listen: settings.voice_listen,
            board_asleep: settings.board_asleep,
            wake_armed_until: None,
            early_stop_done: false,
            next_voice_try: None,
            mic_peak: 0.0,
            mic_quiet_since: None,
            mic_silent: false,
            show_cable: false,
            ports: Vec::new(),
            port_choice: String::new(),
            baud: settings.baud,
            link: None,
            link_ok: false,
            cable_status: "Not connected".into(),
            cable_frames: 0,
            cable_last: None,
            last_live: None,
            counts: [0, 0],
            count_base: [0, 0],
            loaded: false,
            last_enable: Vec::new(),
            changed_at: None,
            heartbeat_on: false,
            port_busy: false,
            ack_seen: false,
            safety_stop: None,
            off_check: None,
            off_alert: None,
            machine_log: VecDeque::new(),
            next_retry: None,
            show_summary: false,
            was_on: false,
            pair_heights: [0.0, 0.0],
            weight_card_h: 0.0,
            sleep_min: settings.sleep_min.min(SLEEP_MAX_MIN),
            last_use: Instant::now(),
            use_cable_m: [0.0, 0.0],
            asleep: false,
            slept_at: Instant::now(),
        };
        app.say(None, "Ready. Tap Start Workout, or just set a weight and turn it on.", true);
        // on the tablet the voice thread starts from update() once the app is on screen
        #[cfg(not(target_os = "android"))]
        if settings.voice_on {
            app.start_voice();
        }
        // On the tablet, connect to the motor board straight away.
        #[cfg(target_os = "android")]
        if std::path::Path::new(machine_link::TABLET_PORT).exists() {
            app.connect_machine(machine_link::TABLET_PORT.to_string());
        }
        app
    }

    fn save_settings(&self) {
        let s = Settings {
            voice_on: self.voice_wanted,
            pounds: self.machine.unit == Unit::Lb,
            rest_seconds: self.session.rest_seconds,
            rest_on: self.session.rest_on,
            smart_off: self.machine.smart_off,
            practice_lifter: self.machine.practice_lifter,
            baud: self.baud,
            model: self.machine.model,
            last: Some(LastLift {
                kg_per_cable: self.machine.left_kg,
                mode: self.machine.mode,
                barbell: self.machine.barbell,
                eccentric_kg: self.machine.eccentric_kg,
                chain_kg: self.machine.chain_kg,
                speed: self.machine.speed,
                accessory: Some(self.machine.accessory()),
            }),
            sleep_min: self.sleep_min,
            voice_listen: self.voice_listen,
            board_asleep: self.board_asleep,
        };
        if let Some(p) = settings_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(j) = serde_json::to_string_pretty(&s) {
                let _ = std::fs::write(p, j);
            }
        }
    }

    fn say(&mut self, heard: Option<String>, reply: impl Into<String>, ok: bool) {
        self.log.push_front(LogLine { heard, reply: reply.into(), ok });
        self.log.truncate(40);
        self.said += 1;
    }

    // ---------------- voice ----------------
    fn start_voice(&mut self) {
        if self.voice.is_some() {
            return;
        }
        self.voice = Some(voice::start());
        self.voice_state = VoiceState::Starting;
        self.voice_error = None;
        self.wake_armed_until = None;
        self.early_stop_done = false;
        self.mic_peak = 0.0;
        self.mic_quiet_since = None;
        self.mic_silent = false;
    }
    /// Stop the microphone thread (the Voice switch stays as it is).
    fn stop_voice(&mut self) {
        if let Some(v) = self.voice.take() {
            v.stop();
        }
        self.voice_state = VoiceState::Off;
        self.wake_armed_until = None;
    }
    /// The Voice switch, top button, V key and "stop listening".
    fn set_voice(&mut self, on: bool) {
        self.voice_wanted = on;
        if on {
            self.voice_state = VoiceState::Off;
            self.next_voice_try = None;
            self.start_voice();
        } else {
            self.stop_voice();
        }
        self.save_settings();
    }
    fn toggle_voice(&mut self) {
        let on = !self.voice_wanted;
        self.set_voice(on);
        if !on {
            self.say(None, "Voice control off.", true);
        }
    }
    fn set_listen(&mut self, listen: Listen) {
        if listen == self.voice_listen {
            return;
        }
        self.voice_listen = listen;
        self.wake_armed_until = None;
        self.say(
            None,
            match listen {
                Listen::Always => "Voice: always listening — just say the command.",
                Listen::WakeWord => "Voice: say \"Weight\" first, e.g. \"Weight, set to twenty\". \"Stop\" always works.",
            },
            true,
        );
        self.save_settings();
    }

    fn poll_voice(&mut self) {
        let mut events = Vec::new();
        if let Some(v) = &self.voice {
            while let Ok(e) = v.rx.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                VoiceEvent::Listening => {
                    self.voice_state = VoiceState::Listening;
                    self.say(None, "Voice control on — listening (offline).", true);
                }
                VoiceEvent::Level(p) => self.mic_level(p),
                VoiceEvent::Note(msg) => {
                    append_voice_log(&format!("{}  {msg}", stamp()));
                    self.say(None, msg, true);
                }
                VoiceEvent::Heard { text, early_stop } => {
                    // speaking to the app counts as use (sleep timer)
                    self.last_use = Instant::now();
                    self.heard(text, early_stop);
                }
                VoiceEvent::Error(msg) => {
                    self.voice_state = VoiceState::Failed;
                    self.voice_error = Some(msg.clone());
                    append_voice_log(&format!("{}  voice problem: {msg}", stamp()));
                    self.say(None, format!("Voice control problem: {msg}"), false);
                }
                VoiceEvent::Stopped => {
                    if self.voice_state != VoiceState::Failed {
                        self.voice_state = VoiceState::Off;
                    }
                    self.voice = None;
                }
            }
        }
    }

    /// Microphone level from the voice thread: drives the level bar, and warns when the mic
    /// sends nothing but silence (e.g. the clip-on mic's own mute button).
    fn mic_level(&mut self, p: f32) {
        self.mic_peak = p.max(self.mic_peak * 0.6);
        if p < voice::SILENT_PEAK {
            let since = *self.mic_quiet_since.get_or_insert_with(Instant::now);
            if !self.mic_silent && since.elapsed().as_secs_f32() >= voice::SILENT_FOR_S {
                self.mic_silent = true;
                append_voice_log(&format!("{}  microphone sends silence (loudest {p:.5})", stamp()));
                self.say(None, MIC_SILENT_MSG, false);
            }
        } else {
            self.mic_quiet_since = None;
            if self.mic_silent {
                self.mic_silent = false;
                append_voice_log(&format!("{}  microphone sound is back", stamp()));
                self.say(None, "Microphone sound is back.", true);
            }
        }
    }

    /// "Mic" and a bar that moves with the sound the microphone hears.
    fn mic_level_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Mic").size(13.0).color(MUTED));
            let w = (ui.available_width() - 8.0).max(40.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(5), PANEL2);
            let fill = voice::level_fill(self.mic_peak);
            if fill > 0.0 {
                let mut r = rect;
                r.set_width(rect.width() * fill);
                p.rect_filled(r, CornerRadius::same(5), if self.mic_silent { WARN } else { ON });
            }
        });
    }

    /// A phrase from the voice thread. Stop / weight off always acts (and is acted on before
    /// the phrase ends); in wake-word mode other phrases must start with "Weight".
    /// Also writes the phrase and what came of it to the voice log file.
    fn heard(&mut self, text: String, early_stop: bool) {
        let before = self.said;
        let (ran, note) = self.act_on_heard(&text, early_stop);
        // the replies this phrase produced, oldest first (none: use the note)
        let new = (self.said - before) as usize;
        let mut did: Vec<String> = self.log.iter().take(new).rev().map(|l| l.reply.clone()).collect();
        if did.is_empty() {
            did.push(note.to_string());
        }
        append_voice_log(&voice_log_line(&stamp(), self.voice_listen, early_stop, &text, ran.as_deref(), &did));
    }

    /// Returns the text run as a command (if any) and a note for when nothing was shown.
    fn act_on_heard(&mut self, text: &str, early_stop: bool) -> (Option<String>, &'static str) {
        let text = text.to_string();
        if early_stop {
            self.early_stop_done = true;
            self.run_command(Some(text), Command::WeightOff);
            return (None, "weight off");
        }
        if std::mem::take(&mut self.early_stop_done) && voice::is_stop(&text) {
            // the same "stop" phrase, finished: already done
            return (None, "(the same stop, already done)");
        }
        let run = match self.voice_listen {
            Listen::Always => Some(voice::normalize(&text)),
            Listen::WakeWord => {
                let armed = self.wake_armed_until.map(|t| Instant::now() < t).unwrap_or(false);
                match voice::wake_gate(&text, armed) {
                    Gate::Run(t) => {
                        self.wake_armed_until = None;
                        Some(t)
                    }
                    Gate::Arm => {
                        self.wake_armed_until = Some(Instant::now() + Duration::from_secs_f32(voice::WAKE_WINDOW_S));
                        self.say(Some(text), "Yes? Say the command.", true);
                        None
                    }
                    Gate::Ignore => None,
                }
            }
        };
        if let Some(t) = &run {
            match commands::parse(t) {
                Some(cmd) => self.run_command(Some(t.clone()), cmd),
                None => self.say(Some(t.clone()), "Not a command I know — ignored.", false),
            }
        }
        (run, "(ignored: didn't start with \"Weight\")")
    }

    fn handle_text(&mut self, text: &str) {
        match commands::parse(text) {
            Some(cmd) => self.run_command(Some(text.to_string()), cmd),
            None => self.say(Some(text.to_string()), "Sorry, I don't know that command.", false),
        }
    }

    // ---------------- commands ----------------
    fn run_command(&mut self, heard: Option<String>, cmd: Command) {
        let reply: String = match &cmd {
            Command::VoiceOff => {
                self.set_voice(false);
                "Voice control off.".into()
            }
            Command::StartWorkout => {
                if self.workout_running() {
                    "The workout is already running.".into()
                } else {
                    self.start_workout();
                    "Workout started. The clock is running.".into()
                }
            }
            Command::Pause => {
                self.session.paused = true;
                if self.machine.weight_on {
                    self.machine.apply(&Command::WeightOff);
                    self.after_weight_change();
                }
                "Training paused. The weight is off.".into()
            }
            Command::Resume => {
                self.session.paused = false;
                if self.session.phase == Phase::Done {
                    self.session = Session { rest_seconds: self.session.rest_seconds, rest_on: self.session.rest_on, ..Session::default() };
                }
                "Training resumed.".into()
            }
            Command::SkipRest => {
                if matches!(self.session.phase, Phase::Rest { .. }) {
                    self.session.phase = Phase::Ready;
                    "Rest skipped. Ready for the next set.".into()
                } else {
                    "You're not resting right now.".into()
                }
            }
            Command::FinishSet => {
                if self.machine.weight_on {
                    self.machine.apply(&Command::WeightOff);
                    self.after_weight_change();
                    "Set finished. Weight off.".into()
                } else {
                    "No set is running.".into()
                }
            }
            Command::EndTraining => {
                if self.machine.weight_on {
                    self.machine.apply(&Command::WeightOff);
                    self.after_weight_change();
                }
                self.session.phase = Phase::Done;
                self.show_summary = true;
                format!(
                    "Training ended: {} sets, {} reps.",
                    self.session.sets.len(),
                    self.session.total_reps()
                )
            }
            Command::NextExercise => {
                self.session.next_exercise();
                format!("Next exercise: {}.", self.session.exercise())
            }
            Command::PreviousExercise => {
                self.session.previous_exercise();
                format!("Previous exercise: {}.", self.session.exercise())
            }
            other => {
                let r = self.machine.apply(other).unwrap_or_default();
                self.after_weight_change();
                if matches!(
                    other,
                    Command::Units(_)
                        | Command::SmartLoadOff(_)
                        | Command::SetMode(_)
                        | Command::Barbell(_)
                        | Command::Independent(_)
                        | Command::Assist(_)
                        | Command::AssistLevel(_)
                        | Command::SetWeight { .. }
                        | Command::Bump { .. }
                        | Command::SetExtra { .. }
                        | Command::SetSpeed(_)
                ) {
                    self.save_settings();
                }
                r
            }
        };
        let ok = !(reply.contains("can't")
            || reply.contains("cannot")
            || reply.contains("isn't")
            || reply.starts_with("No ")
            || reply.starts_with("You're not"));
        self.say(heard, reply, ok);
    }

    /// − / + buttons and arrow keys: one fixed step, kept inside the machine's range.
    fn button_step(&mut self, side: Side, dir: i8) {
        if self.machine.mode == WeightMode::Isokinetic {
            self.say(None, "Speed mode does not support weight adjustment.", false);
            return;
        }
        if let Err(e) = self.machine.step_button(side, dir) {
            self.say(None, e, false);
        }
    }

    /// Start / finish sets when the weight goes on or off.
    fn after_weight_change(&mut self) {
        let on = self.machine.weight_on;
        if on && !self.was_on {
            if self.session.phase == Phase::Done {
                self.session = Session { rest_seconds: self.session.rest_seconds, rest_on: self.session.rest_on, ..Session::default() };
            }
            self.session.phase = Phase::Lifting;
            self.session.paused = false;
            // lifting without tapping Start Workout first still starts the workout clock
            self.session.started = true;
            self.session.set_elapsed_s = 0.0;
            self.session.start_set();
            self.machine.reset_reps();
            // keep the weight / mode / extra used, like the original's 0x1c4f0fc
            self.save_settings();
        } else if !on && self.was_on && self.session.phase == Phase::Lifting {
            let reps_l = self.machine.left.reps;
            let reps_r = self.machine.right.reps;
            if reps_l.max(reps_r) > 0 {
                self.session.sets.push(SetRecord {
                    exercise: self.session.exercise().to_string(),
                    reps_left: reps_l,
                    reps_right: reps_r,
                    left_kg: self.machine.left_kg,
                    right_kg: self.machine.right_kg,
                    barbell: self.machine.barbell,
                    mode: self.machine.mode,
                    eccentric_kg: self.machine.eccentric_kg,
                    chain_kg: self.machine.chain_kg,
                    speed: self.machine.speed,
                    seconds: self.session.set_elapsed_s,
                    avg_power_w: self.session.avg_power(),
                    rest_before_s: self.session.rest_before_current,
                });
                self.session.phase = if self.session.rest_on {
                    Phase::Rest { left_s: self.session.rest_seconds as f32 }
                } else {
                    Phase::Ready
                };
            } else {
                self.session.phase = Phase::Ready;
            }
        }
        self.was_on = on;
    }

    // ---------------- cable listener ----------------
    /// List COM ports and pre-select the Gym Monster board (USB 0483:5740) if it's plugged in.
    /// Note any use this frame (a touch, a key, or a cable moving) and go to sleep after
    /// `sleep_min` minutes without any.
    fn check_sleep(&mut self, ctx: &egui::Context) {
        if self.asleep {
            return;
        }
        let touched = ctx.input(|i| {
            i.pointer.any_down()
                || i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::PointerButton { .. } | egui::Event::Touch { .. } | egui::Event::Key { .. } | egui::Event::Text(_)
                    )
                })
        });
        let cables = [self.machine.left.dist_m, self.machine.right.dist_m];
        if touched || cables_moved(cables, self.use_cable_m) {
            self.last_use = Instant::now();
            self.use_cable_m = cables;
        }
        if sleep_due(self.last_use.elapsed().as_secs_f32(), self.sleep_min) {
            self.go_to_sleep(false);
        }
    }

    /// Weight off, motors powered down (disable) and the cable let go, then a black screen.
    /// `now`: the Sleep button (rather than `sleep_min` minutes without use).
    fn go_to_sleep(&mut self, now: bool) {
        let was_on = self.machine.weight_on;
        // the same safe stop as leaving the screen (weight off + disable + heartbeat stop),
        // always sent, then the motor board's own sleep; waking sends wakeup first
        self.release_machine(true);
        // microphone off too; it starts again on wake if the Voice switch is on
        self.stop_voice();
        self.asleep = true;
        self.slept_at = Instant::now();
        self.save_settings();
        let why = if now { "Sleep button".to_string() } else { format!("No use for {} min", self.sleep_min) };
        let board = if self.board_asleep { "motor board asleep" } else { "machine not connected, only the screen sleeps" };
        self.say(
            None,
            format!("{why}: {}{board}, screen asleep.", if was_on { "weight off, " } else { "" }),
            true,
        );
    }

    /// A tap woke the screen: reconnect with the weight off.
    fn wake_up(&mut self) {
        self.asleep = false;
        self.last_use = Instant::now();
        self.next_retry = None;
        self.say(None, "Awake. The weight is off: tap Weight on when you're ready.", true);
    }

    /// Black screen while asleep. The first tap only wakes it (on release, so the same tap
    /// can't press a button underneath).
    fn sleep_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
            let r = ui.max_rect();
            // the text moves to a new place every SLEEP_TEXT_MOVE_S, so it can't burn in
            let (fx, fy) = sleep_text_spot(self.slept_at.elapsed().as_secs() / SLEEP_TEXT_MOVE_S);
            ui.painter().text(
                egui::pos2(r.left() + r.width() * fx, r.top() + r.height() * fy),
                egui::Align2::CENTER_CENTER,
                "Asleep · tap to wake",
                FontId::proportional(22.0),
                Color32::from_gray(45),
            );
        });
        let woke = ctx.input(|i| {
            i.pointer.any_released() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        // the Sleep button's own tap (and any bounce) doesn't wake it straight away
        if woke && self.slept_at.elapsed().as_secs_f32() >= SLEEP_WAKE_GUARD_S {
            self.wake_up();
        }
    }

    fn refresh_ports(&mut self) {
        self.ports = serial_link::ports();
        if let Some(p) = self.ports.iter().find(|p| p.is_machine) {
            if self.port_choice.is_empty() {
                self.port_choice = p.name.clone();
            }
        }
    }

    fn machine_log(&mut self, line: String) {
        self.machine_log.push_front(line);
        self.machine_log.truncate(MACHINE_LOG_LINES);
    }

    fn connect_machine(&mut self, port: String) {
        self.link = Some(machine_link::connect(&port, self.baud));
        self.link_ok = false;
        self.loaded = false;
        self.heartbeat_on = false;
        self.last_enable.clear();
        self.cable_status = format!("Connecting to {port}…");
        self.cable_frames = 0;
    }

    fn disconnect_machine(&mut self) {
        self.release_machine(false);
    }

    /// Let go of the machine: weight off + disable if this connection used the weight, and
    /// with `board_sleep` (the app's Sleep) always weight off + disable + the board's sleep.
    fn release_machine(&mut self, board_sleep: bool) {
        if let Some(l) = self.link.take() {
            if board_sleep && self.link_ok {
                l.close_and_sleep();
                self.board_asleep = true;
            } else {
                l.close(self.heartbeat_on);
            }
        }
        self.link_ok = false;
        self.loaded = false;
        self.heartbeat_on = false;
        self.off_check = None;
        self.machine.external = false;
        if self.machine.weight_on {
            self.machine.weight_on = false;
            self.after_weight_change();
        }
        self.cable_status = "Not connected".into();
    }

    fn poll_link(&mut self, dt: f32) {
        let mut evs = Vec::new();
        if let Some(l) = &self.link {
            while let Ok(e) = l.rx.try_recv() {
                evs.push(e);
            }
        }
        for e in evs {
            match e {
                In::Opened(p) => {
                    self.link_ok = true;
                    self.cable_status = format!("Connected to the machine on {p}");
                    // the app's Sleep put the board to sleep: wake it before anything else
                    if self.board_asleep {
                        if let Some(l) = &self.link {
                            l.send(crate::protocol::cmd_simple(crate::protocol::CMD_WAKEUP));
                        }
                        self.board_asleep = false;
                        self.save_settings();
                        self.say(None, "Woke the machine's motor board.", true);
                    }
                    self.say(None, format!("Connected to the machine ({p}). Weight on now drives the real motors."), true);
                    // Never start the motors just because the port opened: a weight turned on
                    // in practice mode (e.g. while the Speediance app held the port) is
                    // turned off, and the person taps Weight on again.
                    if self.machine.machine_connected() {
                        self.after_weight_change();
                        self.say(None, "The weight was on in practice mode, so it's off now. Tap Weight on when you're ready.", true);
                    }
                }
                In::Frame(f) => {
                    self.cable_frames += 1;
                    if let Frame::Live { reps_left, reps_right, left, right, .. } = &f {
                        self.counts = [*reps_left, *reps_right];
                        let reps = if self.machine.weight_on {
                            set_reps(&mut self.count_base, self.counts)
                        } else {
                            [self.machine.left.reps, self.machine.right.reps]
                        };
                        self.machine.apply_live(
                            reps,
                            [left.distance_m() as f32, right.distance_m() as f32],
                            [left.force_kg() as f32, right.force_kg() as f32],
                            dt,
                        );
                        self.last_live = Some(Instant::now());
                    }
                    if let Frame::Other { kind, payload } = &f {
                        let code = payload.first().copied().unwrap_or(0);
                        self.machine_log(format!(
                            "Reply 0x{kind:02X} ({}): {}",
                            crate::protocol::frame_kind_name(*kind),
                            crate::protocol::hex(payload)
                        ));
                        if *kind == crate::protocol::TYPE_ACK && self.loaded {
                            // 0x1b98ae0: 1 = accepted, anything else = error reply
                            if code == 1 {
                                if !self.ack_seen {
                                    self.ack_seen = true;
                                    self.say(None, "The machine confirmed: weight on.", true);
                                }
                            } else {
                                // the screen must not say "on" when the machine refused:
                                // turn the weight off (sync_machine sends weight off)
                                self.say(None, format!("The machine rejected the command (reply code {code}). Weight off."), false);
                                if self.machine.weight_on {
                                    self.machine.weight_on = false;
                                    self.after_weight_change();
                                }
                            }
                        } else if *kind == crate::protocol::TYPE_PROTECTION {
                            // 0x1b98cdc: the machine has stopped; the original cancels the heartbeat
                            let reason = crate::protocol::protection_reason(code);
                            if let Some(l) = &self.link {
                                l.heartbeat(false);
                            }
                            self.heartbeat_on = false;
                            self.loaded = false;
                            if self.machine.weight_on {
                                self.machine.weight_on = false;
                                self.after_weight_change();
                            }
                            self.safety_stop = Some(reason.to_string());
                            self.say(None, format!("Machine safety stop: {reason}."), false);
                        }
                    }
                    self.cable_last = Some(f);
                }
                In::Sent(bytes) => {
                    // like the original's "下位机数据:发送<name>指令: [bytes]" log (0x19942d0)
                    let name = crate::protocol::command_name(&bytes);
                    self.machine_log(format!("Sent {name}: {}", crate::protocol::hex(&bytes)));
                }
                In::Error(msg) => {
                    let busy = msg.contains("busy") || msg.contains("os error 16");
                    // say it once, not on every retry
                    if !(busy && self.port_busy) {
                        self.say(None, format!("Machine: {msg}"), false);
                    }
                    self.port_busy = busy;
                    self.cable_status = msg;
                }
                In::Closed => {
                    self.link = None;
                    self.link_ok = false;
                    self.off_check = None;
                    self.machine.external = false;
                    if !self.cable_status.starts_with("Couldn't") {
                        self.cable_status = "Not connected".into();
                    }
                }
            }
        }
    }

    /// The weight-on frame for the current settings (the original's enable, 0x3a9bdb0).
    fn enable_frame(&self) -> Vec<u8> {
        self.enable_frames().concat()
    }

    /// The weight-on frame(s) to send, in order: one enable, or in independent mode the
    /// left (A1) then the right (A2) two-player frame, sent TWO_PLAYER_GAP_MS apart.
    fn enable_frames(&self) -> Vec<Vec<u8>> {
        use crate::protocol as p;
        let mc = &self.machine;
        let shown_lb = (mc.shown_kg() * m::LB_PER_KG) as f64;
        let extra_kg = match mc.mode {
            WeightMode::Eccentric => mc.eccentric_kg,
            WeightMode::Chain => mc.chain_kg,
            _ => 0.0,
        };
        let mode = p::device_mode_code(mc.mode);
        // Independent left/right weights: the Free Lift page sends one two-player frame per
        // side (player 1 = left, player 2 = right; 0x1c84154 / 0x1c8445c → 0x3acafec).
        if mc.independent && !mc.barbell {
            let extra_lb = (extra_kg * m::LB_PER_KG) as f64;
            return vec![
                p::cmd_enable_2player(false, mode, (mc.left_kg * m::LB_PER_KG) as f64, extra_lb),
                p::cmd_enable_2player(true, mode, (mc.right_kg * m::LB_PER_KG) as f64, extra_lb),
            ];
        }
        // in barbell mode the original treats the extra as a total too and halves it
        let extra_lb = (extra_kg * m::LB_PER_KG * if mc.barbell { 2.0 } else { 1.0 }) as f64;
        vec![p::cmd_enable(mode, shown_lb, mc.barbell, extra_lb)]
    }

    /// Why the real machine can't take the current settings yet, if so.
    fn unsupported_on_machine(&self) -> Option<&'static str> {
        if self.machine.mode == WeightMode::Isokinetic {
            return Some("Fixed-speed (isokinetic) mode isn't available on the real machine yet: its command isn't decoded.");
        }
        None
    }

    /// Make the real machine follow the app: weight on → enable with the current settings
    /// (re-sent RESEND_DELAY_MS after a change while on), weight off → pause. The heartbeat
    /// starts with the first enable and keeps running until disconnect, like the original
    /// (pause cancels a different timer, 0x1b98c4c; only disable stops it, 0x1994284).
    fn sync_machine(&mut self) {
        if !self.link_ok || self.link.is_none() {
            return;
        }
        let pause = crate::protocol::cmd_simple(crate::protocol::CMD_PAUSE);
        if self.machine.weight_on {
            if let Some(why) = self.unsupported_on_machine() {
                self.machine.weight_on = false;
                self.after_weight_change();
                if self.loaded {
                    self.link.as_ref().unwrap().send(pause);
                    self.loaded = false;
                    self.off_check = Some((Instant::now(), Instant::now(), 0));
                }
                self.say(None, why, false);
                return;
            }
            let frames = self.enable_frames();
            let frame = frames.concat();
            let link = self.link.as_ref().unwrap();
            if !self.loaded {
                link.send_spaced(&frames, TWO_PLAYER_GAP_MS);
                if !self.heartbeat_on {
                    link.heartbeat(true);
                    self.heartbeat_on = true;
                }
                self.loaded = true;
                self.ack_seen = false;
                self.safety_stop = None;
                self.off_check = None;
                self.last_enable = frame;
                self.count_base = self.counts;
                self.changed_at = None;
            } else if frame != self.last_enable {
                let t = *self.changed_at.get_or_insert_with(Instant::now);
                if t.elapsed() >= Duration::from_millis(RESEND_DELAY_MS) {
                    link.send_spaced(&frames, TWO_PLAYER_GAP_MS);
                    self.last_enable = frame;
                    self.changed_at = None;
                }
            }
        } else if self.loaded {
            self.link.as_ref().unwrap().send(pause);
            self.loaded = false;
            self.changed_at = None;
            self.off_check = Some((Instant::now(), Instant::now(), 0));
        }
    }

    /// Live data from the machine arrived recently.
    fn live_fresh(&self) -> bool {
        self.last_live.map(|t| t.elapsed().as_secs_f32() < m::LIVE_STALE_S).unwrap_or(false)
    }

    /// After a weight-off, make sure the machine really let go: the force on both cables must
    /// fall back to base tension. If not, send weight off again, then warn.
    fn check_weight_off(&mut self) {
        let Some((first, sent, resends)) = self.off_check else { return };
        if self.loaded || !self.link_ok {
            self.off_check = None;
            return;
        }
        let force = [self.machine.left.force_kg, self.machine.right.force_kg];
        let data_since_off = self.last_live.map(|t| t > first).unwrap_or(false);
        match weight_off_check(sent.elapsed().as_secs_f32(), force, data_since_off, resends) {
            OffCheck::Wait => {}
            OffCheck::Confirmed => self.off_check = None,
            OffCheck::Resend => {
                if let Some(l) = &self.link {
                    l.send(crate::protocol::cmd_simple(crate::protocol::CMD_PAUSE));
                }
                self.off_check = Some((first, Instant::now(), resends + 1));
                self.machine_log("Weight still on after weight off: sending weight off again".into());
            }
            OffCheck::StillLoaded => {
                self.off_check = None;
                let msg = format!(
                    "The machine still reports {} on the cables after Weight off. Stand clear. Tap \"Send weight off again\", and switch the machine off if the cables stay tight.",
                    self.machine.fmt(force[0].max(force[1]))
                );
                self.say(None, msg.clone(), false);
                self.off_alert = Some(msg);
            }
            OffCheck::NoData => {
                self.off_check = None;
                let msg = "Couldn't confirm the weight is off: the machine sent no live data after Weight off. Stand clear and check the cables.".to_string();
                self.say(None, msg.clone(), false);
                self.off_alert = Some(msg);
            }
        }
    }

    /// The workout clock is running (Start Workout tapped, not ended yet; paused counts).
    fn workout_running(&self) -> bool {
        self.session.started && self.session.phase != Phase::Done
    }

    /// Start Workout: after End Workout this begins a new session (clears the sets, like the
    /// summary's "Start a new session"), then starts the clock.
    fn start_workout(&mut self) {
        if self.session.phase == Phase::Done {
            self.session = Session { rest_seconds: self.session.rest_seconds, rest_on: self.session.rest_on, ..Session::default() };
            self.machine.reset_reps();
            self.show_summary = false;
        }
        self.session.started = true;
        self.session.paused = false;
    }

    // ---------------- UI ----------------
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("FREE").size(28.0).strong().color(ACCENT));
            ui.label(RichText::new("LIFT").size(28.0).strong().color(TEXT));
            ui.add_space(8.0);
            // Start Workout / End Workout: one fixed size, text centred (add_sized)
            let running = self.workout_running();
            let (txt, fill) = if running { (WORKOUT_END, END_FILL) } else { (WORKOUT_START, ON) };
            let b = egui::Button::new(RichText::new(txt).size(16.0).strong().color(TEXT))
                .fill(fill)
                .corner_radius(CornerRadius::same(12));
            if ui.add_sized(WORKOUT_BTN, b).clicked() {
                let cmd = if running { Command::EndTraining } else { Command::StartWorkout };
                self.run_command(None, cmd);
            }
            // Pause / Resume, right of it (only while the workout runs)
            let paused = self.session.paused;
            let b = egui::Button::new(RichText::new(if paused { PAUSE_RESUME } else { PAUSE_PAUSE }).size(16.0).color(TEXT))
                .fill(if paused { WARN } else { PANEL2 })
                .corner_radius(CornerRadius::same(12));
            if ui
                .add_enabled_ui(running, |ui| ui.add_sized(PAUSE_BTN, b))
                .inner
                .on_hover_text("Pause: weight off, clock stopped")
                .clicked()
            {
                self.run_command(None, if paused { Command::Resume } else { Command::Pause });
            }
            // machine link: just a dot (green = machine connected, orange = practice machine)
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(DOT_SIZE), egui::Sense::hover());
            let col = if self.link_ok { ON } else { WARN };
            ui.painter().circle_filled(r.center(), DOT_SIZE / 2.0 - 1.0, col);
            resp.on_hover_text(if self.link_ok { "Machine connected" } else { "Practice machine: nothing reaches the motors" });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // voice toggle — the big, easy switch
                let (label, fill) = match self.voice_state {
                    VoiceState::Off => ("🎤  Voice: OFF", PANEL2),
                    VoiceState::Starting => ("🎤  Voice: starting…", PANEL2),
                    VoiceState::Listening => ("🎤  Voice: ON", ON),
                    VoiceState::Failed => ("🎤  Voice: error", WARN),
                };
                let b = egui::Button::new(RichText::new(label).size(16.0).strong().color(TEXT))
                    .fill(fill)
                    .corner_radius(CornerRadius::same(20))
                    .min_size(Vec2::new(170.0, TOP_BTN_H));
                if ui.add(b).on_hover_text("Turn voice control on or off (shortcut: V)").clicked() {
                    self.toggle_voice();
                }
                ui.add_space(8.0);
                let unit_label = if self.machine.unit == Unit::Kg { "kg" } else { "lb" };
                if ui
                    .add(top_btn(unit_label).min_size(Vec2::new(52.0, TOP_BTN_H)))
                    .on_hover_text("Switch kg / lb")
                    .clicked()
                {
                    let u = if self.machine.unit == Unit::Kg { Unit::Lb } else { Unit::Kg };
                    self.run_command(None, Command::Units(u));
                }
                if ui
                    .add(top_btn("🔌 Machine"))
                    .clicked()
                {
                    self.show_cable = !self.show_cable;
                    self.refresh_ports();
                }
                // sleep now instead of waiting for "Sleep after": the same safe stop
                if ui
                    .add(top_btn("🌙 Sleep"))
                    .on_hover_text("Weight off, motors powered down, screen black. Tap to wake.")
                    .clicked()
                {
                    self.go_to_sleep(true);
                }
                // Android: open the system Settings screen (the app lets go of the machine
                // while it's in the background, see update()).
                #[cfg(target_os = "android")]
                if ui
                    .add(top_btn("⚙ Settings"))
                    .on_hover_text("Open Android settings")
                    .clicked()
                {
                    if let Err(e) = crate::open_android_settings() {
                        self.say(None, format!("Couldn't open Android settings: {e}"), false);
                    }
                }
                ui.add_space(8.0);
                ui.label(RichText::new(format!("⏱ {}", clock(self.session.elapsed_s))).size(20.0).color(TEXT));
            });
        });
        self.busy_banner(ui);
    }

    /// Shown while the Speediance app holds the machine port: how to free it.
    fn busy_banner(&mut self, ui: &mut egui::Ui) {
        // safety stop from the machine (frame 0x82), like the original's dialog
        if let Some(reason) = self.safety_stop.clone() {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(70, 24, 24))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Machine safety stop: {reason}. The weight is off.")).strong().color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("OK").clicked() {
                                self.safety_stop = None;
                            }
                        });
                    });
                });
        }
        // weight off not confirmed by the machine (check_weight_off)
        if let Some(msg) = self.off_alert.clone() {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(70, 24, 24))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(msg).strong().color(TEXT));
                    ui.horizontal(|ui| {
                        if self.link_ok && ui.button("Send weight off again").clicked() {
                            if let Some(l) = &self.link {
                                l.send(crate::protocol::cmd_simple(crate::protocol::CMD_PAUSE));
                            }
                            self.loaded = false;
                            self.off_check = Some((Instant::now(), Instant::now(), 0));
                            self.off_alert = None;
                        }
                        if ui.button("OK").clicked() {
                            self.off_alert = None;
                        }
                    });
                });
        }
        if !self.port_busy || self.link_ok {
            return;
        }
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(58, 42, 20))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("The Speediance app is using the machine, so Free Lift can't control it yet. Open its app info, tap Force stop, then come back — Free Lift connects by itself.")
                        .color(TEXT),
                );
                #[cfg(target_os = "android")]
                {
                    let b = egui::Button::new(RichText::new("Open Speediance app info, then tap Force stop").strong().color(TEXT))
                        .fill(WARN)
                        .min_size(Vec2::new(0.0, 34.0));
                    if ui.add(b).clicked() {
                        if let Err(e) = crate::open_app_info(ORIGINAL_PACKAGE) {
                            self.say(None, format!("Couldn't open app info: {e}"), false);
                        }
                    }
                }
            });
    }

    /// The Free Lift box. Its height stays the same whatever is selected: the two dumbbell
    /// steppers get the same height as the single one, and the box never shrinks below the
    /// tallest it has been drawn (e.g. with the Skip rest button showing).
    fn weight_card(&mut self, ui: &mut egui::Ui) {
        let min_h = self.weight_card_h;
        let natural = sized_card(ui, min_h, 0.0, |ui| {
            ui.horizontal(|ui| {
                if ui.add(small_btn("◀")).on_hover_text("Previous exercise").clicked() {
                    self.run_command(None, Command::PreviousExercise);
                }
                ui.label(RichText::new(self.session.exercise()).size(22.0).strong().color(TEXT));
                if ui.add(small_btn("▶")).on_hover_text("Next exercise").clicked() {
                    self.run_command(None, Command::NextExercise);
                }
                ui.label(
                    RichText::new(format!("SET {}", self.session.sets_of_current() + 1)).size(15.0).strong().color(MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (txt, col) = match self.session.phase {
                        Phase::Ready => ("READY".to_string(), MUTED),
                        Phase::Lifting => ("LIFTING".to_string(), ON),
                        Phase::Rest { left_s } => (format!("REST {}", clock(left_s)), WARN),
                        Phase::Done => ("FINISHED".to_string(), ACCENT),
                    };
                    let (txt, col) = if self.session.paused { ("PAUSED".to_string(), WARN) } else { (txt, col) };
                    pill(ui, &txt, col.gamma_multiply(0.18), col);
                    if matches!(self.session.phase, Phase::Rest { .. }) && ui.add(small_btn("Skip rest")).clicked() {
                        self.run_command(None, Command::SkipRest);
                    }
                });
            });
            ui.add_space(6.0);

            let independent = self.machine.independent && !self.machine.barbell;
            if independent {
                ui.columns(2, |cols| {
                    for (i, col) in cols.iter_mut().enumerate() {
                        let side = if i == 0 { Side::Left } else { Side::Right };
                        let kg = if i == 0 { self.machine.left_kg } else { self.machine.right_kg };
                        let label = if i == 0 { "LEFT" } else { "RIGHT" };
                        if let Some((side, dir)) = weight_stepper(col, &self.machine, kg, label, side, 100.0) {
                            self.button_step(side, dir);
                        }
                    }
                });
                // same height as the single stepper, so the box doesn't change size
                ui.add_space(stepper_height(150.0) - stepper_height(100.0));
            } else {
                let label = if self.machine.barbell { "TOTAL (BARBELL)" } else { "PER SIDE" };
                if let Some((side, dir)) = weight_stepper(ui, &self.machine, self.machine.shown_kg(), label, Side::Both, 150.0) {
                    self.button_step(side, dir);
                }
            }
            let (lo, hi) = self.machine.limits_shown();
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Range {} – {} {}  ·  {}",
                        crate::machine::fmt_num(lo),
                        crate::machine::fmt_num(hi),
                        self.machine.unit_label(),
                        self.machine.model.label()
                    ))
                    .size(15.0)
                    .color(MUTED),
                );
            });

            ui.add_space(10.0);
            let on = self.machine.weight_on;
            let (txt, fill) = if on { ("WEIGHT ON  —  tap to turn off", ON) } else { ("WEIGHT OFF  —  tap to turn on", PANEL2) };
            // The main control: full width of the card and tall enough to hit mid-workout.
            let size = Vec2::new(ui.available_width(), WEIGHT_BTN_H);
            let b = egui::Button::new(RichText::new(txt).size(24.0).strong().color(TEXT))
                .fill(fill)
                .stroke(Stroke::new(2.0, if on { ON } else { ACCENT }))
                .corner_radius(CornerRadius::same(20))
                .min_size(size);
            let clicked = ui
                .vertical_centered(|ui| ui.add_sized(size, b).on_hover_text("Shortcut: space bar").clicked())
                .inner;
            if clicked {
                self.run_command(None, if on { Command::WeightOff } else { Command::WeightOn });
            }
            let mut extra = format!("Mode: {}", self.machine.mode.label());
            match self.machine.mode {
                WeightMode::Eccentric => extra += &format!("  ·  +{} while lowering", self.machine.fmt(self.machine.eccentric_kg)),
                WeightMode::Chain => extra += &format!("  ·  +{} at the top", self.machine.fmt(self.machine.chain_kg)),
                WeightMode::Isokinetic => extra += &format!("  ·  {:.2} m/s", self.machine.speed),
                WeightMode::Standard => {}
            }
            if self.machine.assist {
                extra += &format!("  ·  Assist L{}", self.machine.assist_level);
            }
            if self.machine.smart_off {
                extra += "  ·  Smart load off";
            }
            ui.add_space(4.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(extra).size(15.0).color(MUTED)));
        });
        self.weight_card_h = self.weight_card_h.max(natural);
        ui.add_space(GAP);
    }

    /// LIVE: reps, cable bars and power graph, drawn `total_h` tall (layout in update()).
    fn live_card(&mut self, ui: &mut egui::Ui, total_h: f32) {
        let inner = card_inner(total_h);
        sized_card(ui, inner, 0.0, |ui| {
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 26.0), Layout::left_to_right(Align::Center), |ui| {
                section_title(ui, "LIVE");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.link_ok {
                        let fresh = self.last_live.map(|t| t.elapsed().as_secs_f32() < 2.0).unwrap_or(false);
                        // the machine only streams live data while the weight is on
                        let (txt, col) = if fresh {
                            ("Live from the machine", ON)
                        } else if !self.machine.weight_on {
                            ("Machine idle · live data only while the weight is on", MUTED)
                        } else {
                            ("Machine connected · no data yet", WARN)
                        };
                        ui.label(RichText::new(txt).size(14.0).color(col));
                    } else {
                        let mut p = self.machine.practice_lifter;
                        if ui.checkbox(&mut p, "Practice lifter").on_hover_text("Simulated cables move by themselves while the weight is on").changed() {
                            self.machine.practice_lifter = p;
                            self.save_settings();
                        }
                    }
                });
            });
            // everything below the title fills the rest of the card
            let h = (inner - 26.0 - ui.spacing().item_spacing.y).max(140.0);
            ui.horizontal(|ui| {
                let cables = [("LEFT", self.machine.left, LEFT_C), ("RIGHT", self.machine.right, RIGHT_C)];
                for (name, c, col) in cables {
                    ui.vertical(|ui| {
                        ui.set_width(130.0);
                        // centre the reps block next to its bar
                        ui.add_space(((h - 170.0) / 2.0).max(0.0));
                        ui.label(RichText::new(name).size(14.0).strong().color(MUTED));
                        ui.label(RichText::new("reps").size(13.0).color(MUTED));
                        ui.label(RichText::new(format!("{}", c.reps)).size(76.0).strong().color(col));
                        ui.label(RichText::new(self.machine.fmt(c.force_kg)).size(18.0).color(TEXT));
                    });
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(26.0, h), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, CornerRadius::same(13), PANEL2);
                    let fill_h = rect.height() * c.pos.clamp(0.0, 1.0);
                    let r = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - fill_h), rect.max);
                    p.rect_filled(r, CornerRadius::same(13), col);
                    ui.add_space(16.0);
                }
                // power chart
                let w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(14), PANEL2);
                // faint guide lines at a quarter, half and three quarters of the scale
                for k in 1..=3 {
                    let y = rect.bottom() - 10.0 - (rect.height() - 30.0) * k as f32 / 4.0;
                    p.line_segment(
                        [egui::pos2(rect.left() + 8.0, y), egui::pos2(rect.right() - 8.0, y)],
                        Stroke::new(1.0, Color32::from_rgb(46, 50, 60)),
                    );
                }
                // one line per cable in its own colour (same colours as the L/R reps);
                // in barbell mode both cables move one bar, so one line for the total
                let mc = &self.machine;
                let lines: Vec<(&VecDeque<f32>, Color32)> = if mc.barbell {
                    vec![(&mc.power_history, ACCENT)]
                } else {
                    vec![(&mc.power_left, LEFT_C), (&mc.power_right, RIGHT_C)]
                };
                let max = lines
                    .iter()
                    .flat_map(|(h, _)| h.iter().cloned())
                    .fold(50.0f32, f32::max);
                for (hist, col) in &lines {
                    let n = hist.len().max(2) as f32;
                    let pts: Vec<egui::Pos2> = hist
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            egui::pos2(
                                rect.left() + 8.0 + (rect.width() - 16.0) * i as f32 / (n - 1.0),
                                rect.bottom() - 10.0 - (rect.height() - 30.0) * (v / max),
                            )
                        })
                        .collect();
                    p.add(egui::Shape::line(pts, Stroke::new(2.5, *col)));
                }
                let label = if mc.barbell {
                    format!("Power  {:.0} W", mc.power_history.back().copied().unwrap_or(0.0))
                } else {
                    format!(
                        "Power  L {:.0} W  ·  R {:.0} W",
                        mc.power_left.back().copied().unwrap_or(0.0),
                        mc.power_right.back().copied().unwrap_or(0.0)
                    )
                };
                p.text(rect.left_top() + Vec2::new(14.0, 10.0), egui::Align2::LEFT_TOP, label, FontId::proportional(16.0), TEXT);
                p.text(
                    rect.right_top() + Vec2::new(-14.0, 10.0),
                    egui::Align2::RIGHT_TOP,
                    format!("scale {:.0} W", max),
                    FontId::proportional(13.0),
                    MUTED,
                );
            });
        });
        ui.add_space(GAP);
    }

    /// SETS: the table fills the rest of the screen (`total_h`, layout in update()).
    fn sets_card(&mut self, ui: &mut egui::Ui, total_h: f32) {
        let inner = card_inner(total_h);
        sized_card(ui, inner, 0.0, |ui| {
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 30.0), Layout::left_to_right(Align::Center), |ui| {
                section_title(ui, "SETS");
            });
            if self.session.sets.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No sets yet").size(20.0).strong().color(TEXT));
                    ui.label(RichText::new("Turn the weight on, lift, then turn it off — the set is saved and the rest timer starts.").size(15.0).color(MUTED));
                });
                return;
            }
            let table_h = (inner - 30.0 - ui.spacing().item_spacing.y).max(120.0);
            egui::ScrollArea::vertical().id_salt("sets_scroll").max_height(table_h).auto_shrink([false, true]).show(ui, |ui| {
                // Columns follow the original's set summary (FreeTrainSetSummaryEntity, 0x1ab8ad4):
                // weight, left/right reps, totalCapacity, avgPower, time, restDurationSec.
                // The column gap spreads the table across the card's width.
                let col_gap = ((ui.available_width() - 820.0) / 8.0).clamp(16.0, 40.0);
                egui::Grid::new("sets").striped(true).num_columns(9).spacing([col_gap, 10.0]).show(ui, |ui| {
                    for h in ["#", "Exercise", "Weight", "Reps L / R", "Mode", "Volume", "Avg power", "Time", "Rest before"] {
                        ui.label(RichText::new(h).size(14.0).strong().color(MUTED));
                    }
                    ui.end_row();
                    for (i, s) in self.session.sets.iter().enumerate().rev() {
                        ui.label(format!("{}", i + 1));
                        ui.label(&s.exercise);
                        let weight = if s.barbell {
                            format!("{} total", self.machine.fmt(s.left_kg + s.right_kg))
                        } else if (s.left_kg - s.right_kg).abs() > 1e-3 {
                            format!("L {} / R {}", self.machine.fmt(s.left_kg), self.machine.fmt(s.right_kg))
                        } else {
                            self.machine.fmt(s.left_kg)
                        };
                        ui.label(weight);
                        ui.label(format!("{} / {}", s.reps_left, s.reps_right));
                        let mode = match s.mode {
                            WeightMode::Eccentric => format!("Eccentric +{}", self.machine.fmt(s.eccentric_kg)),
                            WeightMode::Chain => format!("Chain +{}", self.machine.fmt(s.chain_kg)),
                            WeightMode::Isokinetic => format!("Isokinetic {:.2} m/s", s.speed),
                            WeightMode::Standard => "Standard".to_string(),
                        };
                        ui.label(mode);
                        ui.label(self.machine.fmt(s.volume_kg()));
                        ui.label(format!("{:.0} W", s.avg_power_w));
                        ui.label(clock(s.seconds));
                        ui.label(if i == 0 { "–".to_string() } else { clock(s.rest_before_s) });
                        ui.end_row();
                    }
                });
            });
        });
    }

    /// Height the boxes below the Setup strip get, from the space left on the screen:
    /// (live, mode & voice, sets), each a total card height.
    fn fill_heights(rest: f32) -> (f32, f32, f32) {
        let rest = (rest - 2.0 * GAP).max(0.0);
        let live = (rest * 0.26).max(240.0);
        let pair = (rest * 0.28).max(280.0);
        let sets = (rest - live - pair).max(220.0);
        (live, pair, sets)
    }

    /// Weight Mode (left) and Voice Control (right): same height, set by the Weight Mode
    /// content; Voice Control scrolls inside that height.
    fn mode_and_voice(&mut self, ui: &mut egui::Ui, total_h: f32) {
        // both boxes are drawn total_h tall; the button height depends only on that, so the
        // sizes can't creep from frame to frame
        let target = card_inner(total_h).max(PAIR_MIN_H);
        // the four mode buttons share the height left after the title and the slider
        let btn_h = ((target - 130.0) / 2.0).clamp(54.0, 100.0);
        // if the Weight Mode content came out taller last frame, Voice Control matches it
        let voice_h = target.max(self.pair_heights[0]);
        let mut natural = [0.0f32; 2];
        ui.columns(2, |cols| {
            let (a, b) = cols.split_at_mut(1);
            natural[0] = sized_card(&mut a[0], voice_h, 0.0, |ui| self.mode_body(ui, btn_h));
            natural[1] = sized_card(&mut b[0], voice_h, 0.0, |ui| self.voice_body(ui, voice_h));
        });
        self.pair_heights = natural;
        ui.add_space(GAP);
    }

    fn mode_body(&mut self, ui: &mut egui::Ui, btn_h: f32) {
                card_header(ui, "WEIGHT MODE", |_| {});
                // Big buttons in a 2 × 2 grid filling the box.
                let gap = 10.0;
                let w = (ui.available_width() - gap) / 2.0;
                let h = btn_h;
                let left_pad = 0.0;
                for row in [[WeightMode::Standard, WeightMode::Eccentric], [WeightMode::Chain, WeightMode::Isokinetic]] {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.add_space(left_pad);
                        for mode in row {
                            let selected = self.machine.mode == mode;
                            let b = egui::Button::new(RichText::new(mode.label()).size(20.0).strong().color(TEXT))
                                .fill(if selected { ACCENT } else { PANEL2 })
                                .stroke(Stroke::new(1.0, if selected { ACCENT } else { CARD_EDGE }))
                                .corner_radius(CornerRadius::same(16));
                            if ui.add_sized([w, h], b).clicked() {
                                self.run_command(None, Command::SetMode(mode));
                            }
                        }
                    });
                }
                ui.add_space(4.0);
                self.mode_detail(ui);
    }

    fn mode_detail(&mut self, ui: &mut egui::Ui) {
                let u = self.machine.unit;
                let to = |kg: f32| if u == Unit::Lb { kg * m::LB_PER_KG } else { kg };
                match self.machine.mode {
                    WeightMode::Eccentric | WeightMode::Chain => {
                        let is_ecc = self.machine.mode == WeightMode::Eccentric;
                        let cur = if is_ecc { self.machine.eccentric_kg } else { self.machine.chain_kg };
                        let mut v = to(cur);
                        let label = if is_ecc { "Extra while lowering" } else { "Extra at the top of the pull" };
                        ui.label(RichText::new(label).color(MUTED));
                        if ui.add(egui::Slider::new(&mut v, 0.0..=to(self.machine.max_extra_kg())).suffix(format!(" {}", self.machine.unit_label())).step_by(if u == Unit::Lb { 1.0 } else { 0.5 } as f64)).changed() {
                            let kind = if is_ecc { commands::Extra::Eccentric } else { commands::Extra::Chain };
                            let reply = self.machine.apply(&Command::SetExtra { kind, value: v, unit: None });
                            let _ = reply;
                        }
                    }
                    WeightMode::Isokinetic => {
                        let mut v = self.machine.speed;
                        ui.label(RichText::new("Constant speed").color(MUTED));
                        if ui.add(egui::Slider::new(&mut v, m::SPEED_MIN..=m::SPEED_MAX).suffix(" m/s").step_by(0.05)).changed() {
                            self.machine.apply(&Command::SetSpeed(v));
                        }
                    }
                    WeightMode::Standard => {
                        ui.label(RichText::new("Same weight all the way up and down.").color(MUTED));
                    }
                }
    }

    /// Setup as one horizontal strip between the Free Lift box and the Live box.
    fn setup_card(&mut self, ui: &mut egui::Ui) {
            card(ui, |ui| {
              ui.spacing_mut().item_spacing = Vec2::new(14.0, 8.0);
              let row_h = SETUP_ROW_H;
              let mut label_w = 0.0;
              // Row 1: every control centred on the same line (no nested groups). Barbell,
              // Dumbbells (independent left / right) and Assist are one choice: tapping one
              // selects it and turns the other two off (Machine::set_accessory).
              ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), SETUP_BTN_ROW_H), Layout::left_to_right(Align::Center), |ui| {
                // title width (accent bar + text), so row 2 lines up under the first button
                let start = ui.cursor().left();
                section_title(ui, "SETUP");
                label_w = ui.cursor().left() - start - ui.spacing().item_spacing.x;
                let current = self.machine.accessory();
                for acc in Accessory::ALL {
                    let hint = match acc {
                        Accessory::Barbell => "One total weight for both sides",
                        Accessory::Dumbbells => "Separate left and right weights",
                        Accessory::Assist => "Linked handles with the spotter",
                    };
                    if ui.add(setting_btn(acc.label(), acc == current)).on_hover_text(hint).clicked() && acc != current {
                        let cmd = match acc {
                            Accessory::Barbell => Command::Barbell(true),
                            Accessory::Dumbbells => Command::Independent(true),
                            Accessory::Assist => Command::Assist(true),
                        };
                        self.run_command(None, cmd);
                    }
                }
                for l in 1..=m::ASSIST_LEVELS {
                    let c = chip(&format!("Level {l}"), self.machine.assist && self.machine.assist_level == l)
                        .min_size(Vec2::new(0.0, 44.0));
                    if ui.add_enabled(self.machine.assist, c).clicked() {
                        self.run_command(None, Command::AssistLevel(l));
                    }
                }
                let mut so = self.machine.smart_off;
                if ui.checkbox(&mut so, "Smart load off").on_hover_text("Weight off after each set").changed() {
                    self.run_command(None, Command::SmartLoadOff(so));
                }
              });
              // Row 2: starts under the first checkbox; label, box, label, box on one line.
              ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), row_h), Layout::left_to_right(Align::Center), |ui| {
                // The dropdown is placed using interact_size and then grows to 29 px; make the
                // layout expect 29 px so it is centred on the row like the labels.
                ui.spacing_mut().interact_size.y = SETUP_WIDGET_H;
                ui.add_space(label_w + ui.spacing().item_spacing.x);
                {
                    let w = ui.painter().layout_no_wrap("Machine".into(), FontId::proportional(15.0), TEXT).size().x;
                    ui.add_sized([w, SETUP_WIDGET_H], egui::Label::new("Machine"));
                    let mut model = self.machine.model;
                    egui::ComboBox::from_id_salt("model").selected_text(model.label()).show_ui(ui, |ui| {
                        for m in Model::ALL {
                            ui.selectable_value(&mut model, m, m.label());
                        }
                    });
                    if model != self.machine.model {
                        self.machine.model = model;
                        let (lo, hi) = self.machine.limits_shown();
                        let _ = self.machine.step_button(Side::Both, 0);
                        self.say(
                            None,
                            format!(
                                "{}: weight range {}–{} {}.",
                                model.label(),
                                crate::machine::fmt_num(lo),
                                crate::machine::fmt_num(hi),
                                self.machine.unit_label()
                            ),
                            true,
                        );
                        self.save_settings();
                    }
                }
                ui.add_space(10.0);
                // the original's free-lift rest switch (on unless turned off, 0x1c91bdc)
                let mut rest_on = self.session.rest_on;
                if ui.checkbox(&mut rest_on, "Rest timer").on_hover_text("Count down a rest after each set").changed() {
                    self.session.rest_on = rest_on;
                    if !rest_on && matches!(self.session.phase, Phase::Rest { .. }) {
                        self.session.phase = Phase::Ready;
                    }
                    self.save_settings();
                }
                let mut r = self.session.rest_seconds;
                let rest_box = egui::DragValue::new(&mut r).range(10..=600).speed(1.0).suffix(" s");
                if ui.add_enabled_ui(self.session.rest_on, |ui| ui.add_sized([64.0, SETUP_WIDGET_H], rest_box)).inner.changed() {
                    self.session.rest_seconds = r;
                    self.save_settings();
                }
                ui.add_space(10.0);
                // black screen and motors off after this long with no taps or cable movement
                let w = ui.painter().layout_no_wrap("Sleep after".into(), FontId::proportional(15.0), TEXT).size().x;
                ui.add_sized([w, SETUP_WIDGET_H], egui::Label::new("Sleep after"))
                    .on_hover_text("Screen off and motors powered down after this long unused; tap to wake");
                let mut sm = self.sleep_min;
                let sleep_box = egui::DragValue::new(&mut sm)
                    .range(0..=SLEEP_MAX_MIN)
                    .speed(1.0)
                    .custom_formatter(|n, _| if n < 1.0 { "Never".into() } else { format!("{n:.0} min") });
                if ui.add_sized([84.0, SETUP_WIDGET_H], sleep_box).changed() {
                    self.sleep_min = sm;
                    self.last_use = Instant::now();
                    self.save_settings();
                }
              });
            });
    }

    fn voice_body(&mut self, ui: &mut egui::Ui, box_h: f32) {
        let top = ui.min_rect().top();
        let mut clicked = false;
        let mut listen = None;
        card_header(ui, "VOICE CONTROL", |ui| {
            let mut on = self.voice_wanted;
            clicked = toggle(ui, &mut on).clicked();
            // right to left: the switch, then the listening style
            for (l, label, hint) in [
                (Listen::WakeWord, "Wake word", "Start each command with \"Weight\""),
                (Listen::Always, "Always", "Every phrase is a command"),
            ] {
                let c = chip(label, self.voice_listen == l).min_size(Vec2::new(0.0, 26.0));
                if ui.add(c).on_hover_text(hint).clicked() {
                    listen = Some(l);
                }
            }
        });
        if clicked {
            self.toggle_voice();
        }
        if let Some(l) = listen {
            self.set_listen(l);
        }
        let used = ui.min_rect().bottom() - top + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical()
            .id_salt("voice_scroll")
            .max_height((box_h - used).max(60.0))
            .auto_shrink([false, false])
            .show(ui, |ui| self.voice_scroll_content(ui));
    }

    fn voice_scroll_content(&mut self, ui: &mut egui::Ui) {
        {
            let armed = self.wake_armed_until.map(|t| Instant::now() < t).unwrap_or(false);
            let status = match self.voice_state {
                VoiceState::Off if self.voice_wanted => "Starting the microphone…".to_string(),
                VoiceState::Off => "Off. Turn it on with the switch, the top button or the V key.".to_string(),
                VoiceState::Starting => "Starting the microphone…".to_string(),
                VoiceState::Listening if self.mic_silent => MIC_SILENT_MSG.to_string(),
                VoiceState::Listening if armed => "Yes? Say the command…".to_string(),
                VoiceState::Listening => match self.voice_listen {
                    Listen::Always => "Listening on this tablet (nothing is sent online). Just say a command.".to_string(),
                    Listen::WakeWord => "Listening on this tablet (nothing is sent online). Start with \"Weight\", e.g. \"Weight, set to twenty\". \"Stop\" always works.".to_string(),
                },
                VoiceState::Failed => self.voice_error.clone().unwrap_or_default(),
            };
            let warn = self.voice_state == VoiceState::Failed || (self.voice_state == VoiceState::Listening && self.mic_silent);
            ui.label(RichText::new(status).color(if warn { WARN } else { MUTED }));
            if self.voice_state == VoiceState::Listening {
                self.mic_level_bar(ui);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.typed)
                        .hint_text("Type a command, e.g. set weight to 20")
                        .desired_width(ui.available_width() - 60.0),
                );
                let go = ui.button("Go").clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go && !self.typed.trim().is_empty() {
                    let t = std::mem::take(&mut self.typed);
                    self.handle_text(&t);
                    resp.request_focus();
                }
            });
            ui.add_space(4.0);
            ui.collapsing("What can I say?", |ui| {
                for s in [
                    "Wake word mode: \"Weight, …\" or \"Weight\" then the command",
                    "stop  ·  weight off (always heard, acted on at once)",
                    "weight on  ·  weight off",
                    "weight up  ·  weight down  ·  weight up by five",
                    "set weight to twenty  ·  set weight to forty pounds",
                    "set left weight to twelve point five",
                    "switch to eccentric / chain / isokinetic / standard mode",
                    "set eccentric weight to five  ·  set speed to point five",
                    "barbell  ·  dumbbells  ·  assist (the Setup buttons)",
                    "barbell mode  ·  non barbell mode  ·  independent mode",
                    "turn on assist  ·  assist level two  ·  turn off assist",
                    "what is the weight",
                    "start workout  ·  pause  ·  resume  ·  skip rest  ·  finish set  ·  end workout",
                    "next exercise  ·  previous exercise",
                    "use pounds  ·  use kilos  ·  stop listening",
                ] {
                    ui.label(RichText::new(s).color(MUTED));
                }
                ui.add_space(4.0);
                ui.label(RichText::new(format!(
                    "All {} of the original app's voice commands are understood, for example:",
                    commands::ORIGINAL_INTENTS.len()
                )).color(TEXT));
                for (_, example) in commands::ORIGINAL_INTENTS {
                    ui.label(RichText::new(format!("“{example}”")).color(MUTED));
                }
            });
            ui.add_space(4.0);
            for l in &self.log {
                if let Some(h) = &l.heard {
                    ui.label(RichText::new(format!("“{h}”")).color(TEXT).italics());
                }
                ui.label(RichText::new(&l.reply).color(if l.ok { ACCENT } else { WARN }));
                ui.add_space(3.0);
            }
        }
    }

    fn cable_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_cable;
        egui::Window::new("Machine connection").open(&mut open).default_width(420.0).show(ctx, |ui| {
            ui.label(if self.link_ok {
                "Connected: Weight on / off and every weight or mode change go to the real machine."
            } else {
                "Not connected: the practice machine runs everything on this screen."
            });
            ui.label(RichText::new(
                "On the tablet the motor board is /dev/ttyS5 at 115200 baud; the app connects to it when it starts. Close the Speediance app first — it uses the same cable.",
            ).color(MUTED));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("port")
                    .selected_text(if self.port_choice.is_empty() { "Choose port" } else { &self.port_choice })
                    .show_ui(ui, |ui| {
                        for p in &self.ports {
                            let label = if p.is_machine { format!("{} (Gym Monster board)", p.name) } else { p.name.clone() };
                            ui.selectable_value(&mut self.port_choice, p.name.clone(), label);
                        }
                    });
                if ui.button("↻").on_hover_text("Refresh ports").clicked() {
                    self.refresh_ports();
                }
                ui.add(egui::DragValue::new(&mut self.baud).range(1200..=3_000_000).suffix(" baud"));
            });
            ui.horizontal(|ui| {
                if self.link.is_none() {
                    if ui.add_enabled(!self.port_choice.is_empty(), egui::Button::new("Connect to machine")).clicked() {
                        let p = self.port_choice.clone();
                        self.connect_machine(p);
                        self.save_settings();
                    }
                } else if ui.button("Disconnect (weight off)").clicked() {
                    self.disconnect_machine();
                }
            });
            ui.label(format!("{}  ·  {} frames received", self.cable_status, self.cable_frames));
            if self.ports.is_empty() {
                ui.label(RichText::new("No serial ports found on this device.").color(MUTED));
            }
            if let Some(Frame::Live { family, reps_left, reps_right, left, right, .. }) = &self.cable_last {
                ui.separator();
                ui.label(format!("Training family {family}  ·  reps L {reps_left} / R {reps_right}"));
                // names as the original's decoder (0x1b9c29c) stores them
                for (n, s) in [("Left", left), ("Right", right)] {
                    ui.label(format!(
                        "{n}: torque {:.1} lb ({:.1} kg)  ·  distance {:.3} m  ·  speed {:.2} m/s (inferred)",
                        s.force_lb,
                        s.force_kg(),
                        s.distance_m(),
                        s.a
                    ));
                }
            } else if let Some(Frame::Other { kind, payload }) = &self.cable_last {
                ui.separator();
                ui.label(format!(
                    "Last frame type 0x{kind:02X} ({}): {:02X?}",
                    crate::protocol::frame_kind_name(*kind),
                    payload
                ));
            }
            ui.separator();
            ui.label(RichText::new("Machine log (newest first)").strong());
            if self.machine_log.is_empty() {
                ui.label(RichText::new("Nothing sent yet.").color(MUTED));
            }
            egui::ScrollArea::vertical().id_salt("machine_log").max_height(180.0).show(ui, |ui| {
                for l in &self.machine_log {
                    ui.label(RichText::new(l).monospace().size(12.0));
                }
            });
            ui.separator();
            ui.label(RichText::new("Commands the original would send for these settings").strong());
            ui.label(RichText::new("Built exactly like the original (checked against frames it logged on your tablet). Sent to the machine when connected.").color(MUTED));
            for (name, bytes) in self.command_preview() {
                ui.label(RichText::new(format!("{name}:  {}", crate::protocol::hex(&bytes))).monospace().size(13.0));
            }
            if self.machine.independent && !self.machine.barbell {
                ui.label(RichText::new("Independent left/right: two frames, A1 for the left cable and A2 for the right (the original's two-player command, 0x3acafec). Decoded from its code; the tablet's log hasn't shown one yet, so try a low weight first.").color(WARN));
            }
        });
        self.show_cable = open;
    }

    /// Weight on / weight off / heartbeat frames for the current settings (protocol.rs).
    fn command_preview(&self) -> Vec<(&'static str, Vec<u8>)> {
        use crate::protocol as p;
        let on = self.enable_frame();
        vec![
            ("Weight on ", on),
            ("Weight off", p::cmd_simple(p::CMD_PAUSE)),
            ("Heartbeat ", p::cmd_heartbeat(0)),
        ]
    }

    /// Training summary after End Workout: a card in the app's own style over a dimmed screen.
    /// Tapping outside it or Close keeps the sets on screen; New workout starts a fresh one.
    fn summary_window(&mut self, ctx: &egui::Context) {
        if !self.show_summary {
            return;
        }
        let mut close = false;
        let mut new_workout = false;
        let stats = [
            ("TIME", clock(self.session.elapsed_s)),
            ("SETS", self.session.sets.len().to_string()),
            ("REPS", self.session.total_reps().to_string()),
            ("VOLUME", self.machine.fmt(self.session.total_volume_kg())),
        ];
        let resp = egui::Modal::new(egui::Id::new("training_summary"))
            .frame(card_frame().inner_margin(Margin::same(24)))
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(SUMMARY_W);
                ui.horizontal(|ui| {
                    section_title(ui, "TRAINING SUMMARY");
                });
                ui.add_space(4.0);
                ui.label(RichText::new("Workout finished. The weight is off.").size(16.0).color(MUTED));
                ui.add_space(12.0);
                // 2 × 2 tiles: small muted label, big value
                let gap = 12.0;
                let tile_w = (SUMMARY_W - gap) / 2.0;
                for row in stats.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (label, value) in row {
                            egui::Frame::new()
                                .fill(PANEL2)
                                .stroke(Stroke::new(1.0, CARD_EDGE))
                                .corner_radius(CornerRadius::same(14))
                                .inner_margin(Margin::symmetric(16, 12))
                                .show(ui, |ui| {
                                    ui.set_width(tile_w - 32.0 - 2.0);
                                    // label above the number (the row is horizontal)
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.label(RichText::new(*label).size(14.0).strong().color(MUTED));
                                        ui.label(RichText::new(value).size(34.0).strong().color(TEXT));
                                    });
                                });
                        }
                    });
                    ui.add_space(gap - ui.spacing().item_spacing.y);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let b = egui::Button::new(RichText::new("New workout").size(18.0).strong().color(TEXT))
                        .fill(ON)
                        .corner_radius(CornerRadius::same(14));
                    if ui.add_sized(Vec2::new(tile_w, SUMMARY_BTN_H), b).clicked() {
                        new_workout = true;
                    }
                    let b = egui::Button::new(RichText::new("Close").size(18.0).color(TEXT))
                        .fill(PANEL2)
                        .stroke(Stroke::new(1.0, CARD_EDGE))
                        .corner_radius(CornerRadius::same(14));
                    if ui.add_sized(Vec2::new(tile_w, SUMMARY_BTN_H), b).clicked() {
                        close = true;
                    }
                });
            });
        if new_workout {
            // clears the sets and starts the clock, like Start Workout after End Workout
            self.run_command(None, Command::StartWorkout);
        } else if close || resp.should_close() {
            self.show_summary = false;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let (v, space, up, down) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::V),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if v {
            self.toggle_voice();
        }
        if space {
            let c = if self.machine.weight_on { Command::WeightOff } else { Command::WeightOn };
            self.run_command(None, c);
        }
        if up || down {
            let dir = if up { 1 } else { -1 };
            self.button_step(Side::Both, dir);
        }
    }
}

impl eframe::App for FreeLiftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        fit_width(ctx);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);

        self.poll_voice();
        self.poll_link(dt);
        self.check_sleep(ctx);
        // When the app leaves the screen: weight off (if it was on) and let go of the cable so
        // the Speediance app can use it; take it back when Free Lift returns.
        #[cfg(target_os = "android")]
        {
            let focused = ctx.input(|i| i.focused);
            if !focused && self.link.is_some() {
                if self.machine.weight_on {
                    self.say(None, "App left the screen — weight off.", true);
                }
                self.disconnect_machine();
                self.save_settings();
            } else if focused && !self.asleep && self.link.is_none() && std::path::Path::new(machine_link::TABLET_PORT).exists() {
                // (re)connect now, then every RETRY_S while the port is busy
                let due = self.next_retry.map(|t| Instant::now() >= t).unwrap_or(true);
                if due {
                    self.next_retry = Some(Instant::now() + Duration::from_secs(RETRY_S));
                    self.connect_machine(machine_link::TABLET_PORT.to_string());
                }
            }
            if self.link_ok {
                self.port_busy = false;
                self.next_retry = None;
            }
            // Voice: the microphone only runs while Free Lift is on screen and awake. It starts
            // again by itself (also after Android's microphone question), unless it failed.
            if (!focused || self.asleep) && self.voice.is_some() {
                self.stop_voice();
            } else if focused
                && !self.asleep
                && self.voice_wanted
                && self.voice.is_none()
                && self.voice_state != VoiceState::Failed
                && self.next_voice_try.map(|t| Instant::now() >= t).unwrap_or(true)
            {
                self.next_voice_try = Some(Instant::now() + Duration::from_secs(2));
                self.start_voice();
            }
        }
        if self.asleep {
            // nothing else runs: no machine link, no timers, just the black screen
            self.sleep_screen(ctx);
            ctx.request_repaint_after(Duration::from_millis(500));
            return;
        }
        self.check_weight_off();
        self.shortcuts(ctx);
        self.machine.live_fresh = self.live_fresh();
        let ev = self.machine.step(dt);
        self.session.sample_power(self.machine.power_history.back().copied().unwrap_or(0.0));
        if ev.smart_off_fired {
            self.say(None, "Smart load off: set complete, weight off.", true);
        }
        self.after_weight_change();
        self.session.tick(dt);

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin { left: 18, right: 18, top: 14, bottom: 12 }),
            )
            .show_separator_line(false)
            .show(ctx, |ui| self.top_bar(ui));
        // Portrait layout (1080 × 1920): one column that fills the screen. The weight card and
        // the Setup strip take what they need; Live, Weight Mode / Voice and Sets share the
        // rest of the height, with Sets reaching the bottom edge. It still scrolls if a
        // smaller screen can't fit the minimum sizes.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin { left: 18, right: 18, top: 6, bottom: 18 }))
            .show(ctx, |ui| {
                let screen_h = ui.available_height();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let top = ui.cursor().top();
                    self.weight_card(ui);
                    self.setup_card(ui);
                    let used = ui.cursor().top() - top;
                    // two more card gaps follow (after Live and after Mode / Voice), each with
                    // the layout's item spacing on top of GAP
                    let gaps = 2.0 * ui.spacing().item_spacing.y + 2.0;
                    let (live_h, pair_h, sets_h) = Self::fill_heights(screen_h - used - gaps);
                    self.live_card(ui, live_h);
                    self.mode_and_voice(ui, pair_h);
                    self.sets_card(ui, sets_h);
                });
            });
        self.cable_window(ctx);
        self.summary_window(ctx);
        // after every button, voice or typed command this frame: make the real machine follow
        self.after_weight_change();
        self.sync_machine();
        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
        self.stop_voice();
        // weight off + disable + heartbeat stop (machine_link Shutdown)
        self.disconnect_machine();
    }
}

// ---------------- small widgets ----------------

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(18, 20, 24);
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(45, 50, 60);
    v.widgets.active.weak_bg_fill = ACCENT;
    v.selection.bg_fill = ACCENT;
    v.override_text_color = Some(TEXT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(16.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(16.0));
    });
}

/// Card look: rounded panel with a thin outline. Every box on the screen uses it.
const CARD_R: u8 = 18;
const CARD_M: i8 = 16;
/// Space between cards.
const GAP: f32 = 12.0;
const CARD_EDGE: Color32 = Color32::from_rgb(42, 46, 55);

fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, CARD_EDGE))
        .corner_radius(CornerRadius::same(CARD_R))
        .inner_margin(Margin::same(CARD_M))
}

/// Inner height of a card whose total height (outline included) is `total_h`.
fn card_inner(total_h: f32) -> f32 {
    (total_h - 2.0 * CARD_M as f32 - 2.0).max(0.0)
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    ui.add_space(GAP);
}

/// Smallest height of the Weight Mode / Voice Control boxes.
const PAIR_MIN_H: f32 = 120.0;
/// Height of each Setup row; every control in a row is centred on it.
const SETUP_ROW_H: f32 = 30.0;
/// Height the dropdown draws at; labels and the rest box use it too so their centres match.
const SETUP_WIDGET_H: f32 = 29.0;

/// Card title on the left and optional controls on the right, on one 26 px line,
/// used by both Weight Mode and Voice Control so their titles sit in the same place.
fn card_header(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 26.0), Layout::left_to_right(Align::Center), |ui| {
        section_title(ui, title);
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
}

/// A card drawn at least `min_h` tall (content height), with `top_pad` space above the content.
/// Returns the content's natural height so the caller can match two cards.
fn sized_card(ui: &mut egui::Ui, min_h: f32, top_pad: f32, add: impl FnOnce(&mut egui::Ui)) -> f32 {
    let mut natural = 0.0;
    card_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let top = ui.min_rect().top();
            ui.add_space(top_pad);
            add(ui);
            natural = ui.min_rect().bottom() - top - top_pad;
            // pad up to the shared height (set_min_height measures from the wrong origin here)
            let short = min_h - natural;
            if short > 0.5 {
                ui.add_space(short - ui.spacing().item_spacing.y);
            }
        });
    natural
}

/// Card title: small, bold, muted, with an accent bar in front.
fn section_title(ui: &mut egui::Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(4.0, 16.0), egui::Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(2), ACCENT);
    ui.label(RichText::new(title).size(15.0).strong().color(MUTED));
}

fn pill(ui: &mut egui::Ui, text: &str, fill: Color32, color: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| ui.label(RichText::new(text).size(13.0).strong().color(color)));
}

/// Height of the top-bar buttons.
const TOP_BTN_H: f32 = 42.0;
/// Start / End Workout button: one size for both texts, so it doesn't jump.
const WORKOUT_BTN: Vec2 = Vec2::new(132.0, TOP_BTN_H);
const WORKOUT_START: &str = "Start Workout";
const WORKOUT_END: &str = "End Workout";
const END_FILL: Color32 = Color32::from_rgb(120, 40, 40);
/// Pause / Resume button, right of Start / End.
const PAUSE_BTN: Vec2 = Vec2::new(84.0, TOP_BTN_H);
const PAUSE_PAUSE: &str = "Pause";
const PAUSE_RESUME: &str = "Resume";
/// The machine-connected dot.
const DOT_SIZE: f32 = 18.0;
/// Training summary card: content width and button height.
const SUMMARY_W: f32 = 520.0;
const SUMMARY_BTN_H: f32 = 56.0;
const MIC_SILENT_MSG: &str = "The microphone is sending only silence. Check the clip-on mic: its mute button, and that it is plugged in.";
/// The sleep screen's text moves this often (seconds).
const SLEEP_TEXT_MOVE_S: u64 = 10;
/// Taps this soon after going to sleep don't wake it (the Sleep button's own tap).
const SLEEP_WAKE_GUARD_S: f32 = 0.8;
/// Height of the Weight on / off button.
const WEIGHT_BTN_H: f32 = 76.0;
/// Height of the Setup row with the Barbell / Dumbbells / Assist buttons.
const SETUP_BTN_ROW_H: f32 = 48.0;

/// On/off button for a setting (Barbell, Dumbbells, Assist): filled with the accent colour
/// while on.
fn setting_btn(t: &str, on: bool) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).size(17.0).strong().color(TEXT))
        .fill(if on { ACCENT } else { PANEL2 })
        .stroke(Stroke::new(1.0, if on { ACCENT } else { CARD_EDGE }))
        .corner_radius(CornerRadius::same(14))
        .min_size(Vec2::new(140.0, 44.0))
}

/// Top-bar button: same height and rounding for all of them.
fn top_btn(t: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).size(16.0))
        .fill(PANEL2)
        .corner_radius(CornerRadius::same(12))
        .min_size(Vec2::new(0.0, TOP_BTN_H))
}

fn small_btn(t: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).size(14.0)).corner_radius(CornerRadius::same(10))
}

fn chip(t: &str, selected: bool) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).size(14.0).color(TEXT))
        .fill(if selected { ACCENT } else { PANEL2 })
        .corner_radius(CornerRadius::same(14))
        .min_size(Vec2::new(0.0, 30.0))
}

fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let p = ui.painter();
    let bg = if *on { ON } else { PANEL2 };
    p.rect_filled(rect, CornerRadius::same(13), bg);
    let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
    p.circle_filled(egui::pos2(x, rect.center().y), 10.0, TEXT);
    resp
}

const STEPPER_LABEL_H: f32 = 24.0;

/// − / + button diameter for a weight drawn at `size`.
fn stepper_button(size: f32) -> f32 {
    (size * 0.7).clamp(64.0, 100.0)
}

/// Total height of a weight stepper (label + number row) drawn at `size`.
fn stepper_height(size: f32) -> f32 {
    STEPPER_LABEL_H + (size * 1.15).max(stepper_button(size))
}

/// Big weight number with − / + buttons. Returns (side, direction) when a button is pressed.
fn weight_stepper(ui: &mut egui::Ui, mc: &Machine, kg: f32, label: &str, side: Side, size: f32) -> Option<(Side, i8)> {
    let mut out = None;
    let bw = stepper_button(size);
    let gap = 28.0;
    let label_h = STEPPER_LABEL_H;
    let row_h = stepper_height(size) - label_h;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), label_h + row_h), egui::Sense::hover());

    let num_g = ui.painter().layout_no_wrap(crate::machine::fmt_num(mc.to_unit(kg)), FontId::proportional(size), TEXT);
    let unit_g = ui.painter().layout_no_wrap(mc.unit_label().to_string(), FontId::proportional(size * 0.3), MUTED);
    let text_w = num_g.size().x + 6.0 + unit_g.size().x;
    let inner_w = text_w.max(size * 2.2);
    let total = bw * 2.0 + gap * 2.0 + inner_w;
    let left = rect.center().x - total / 2.0;
    let cy = rect.top() + label_h + row_h / 2.0;
    let minus_rect = egui::Rect::from_center_size(egui::pos2(left + bw / 2.0, cy), Vec2::splat(bw));
    let plus_rect = egui::Rect::from_center_size(egui::pos2(left + total - bw / 2.0, cy), Vec2::splat(bw));
    let mid_x = (minus_rect.right() + plus_rect.left()) / 2.0;

    // label exactly midway between the − and + buttons
    ui.painter().text(egui::pos2(mid_x, rect.top()), egui::Align2::CENTER_TOP, label, FontId::proportional(16.0), MUTED);

    // number + unit, centred between the buttons
    let x0 = mid_x - text_w / 2.0;
    let num_pos = egui::pos2(x0, cy - num_g.size().y / 2.0);
    let unit_pos = egui::pos2(x0 + num_g.size().x + 6.0, cy + num_g.size().y * 0.28 - unit_g.size().y);
    ui.painter().galley(num_pos, num_g, TEXT);
    ui.painter().galley(unit_pos, unit_g, MUTED);

    // round buttons with the sign drawn at the exact centre
    for (r, dir, tag) in [(minus_rect, -1i8, "minus"), (plus_rect, 1i8, "plus")] {
        let resp = ui.interact(r, ui.id().with((label, tag)), egui::Sense::click());
        let fill = if resp.is_pointer_button_down_on() {
            ACCENT
        } else if resp.hovered() {
            Color32::from_rgb(45, 50, 60)
        } else {
            PANEL2
        };
        let c = r.center();
        let p = ui.painter();
        p.circle_filled(c, bw / 2.0, fill);
        p.circle_stroke(c, bw / 2.0, Stroke::new(1.5, Color32::from_rgb(64, 70, 84)));
        let arm = bw * 0.18;
        let stroke = Stroke::new(bw * 0.05, TEXT);
        p.line_segment([egui::pos2(c.x - arm, c.y), egui::pos2(c.x + arm, c.y)], stroke);
        if dir > 0 {
            p.line_segment([egui::pos2(c.x, c.y - arm), egui::pos2(c.x, c.y + arm)], stroke);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            out = Some((side, dir));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// The top bar must fit the 1080-point screen (minus the 18-point side margins) with the
    /// longest texts: Start / End Workout, Pause / Resume, the dot, and the right-hand buttons.
    #[test]
    fn top_bar_fits_the_screen() {
        use super::*;
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |_| {});
        let w = |t: &str, size: f32| {
            ctx.fonts_mut(|f| f.layout_no_wrap(t.to_string(), FontId::proportional(size), TEXT).size().x)
        };
        let btn = |t: &str, min: f32| (w(t, 16.0) + 24.0).max(min);
        // both texts fit inside the fixed-size buttons
        for t in [WORKOUT_START, WORKOUT_END] {
            assert!(w(t, 16.0) + 16.0 <= WORKOUT_BTN.x, "{t}");
        }
        for t in [PAUSE_PAUSE, PAUSE_RESUME] {
            assert!(w(t, 16.0) + 16.0 <= PAUSE_BTN.x, "{t}");
        }
        let left = [w("FREE", 28.0), w("LIFT", 28.0), 8.0, WORKOUT_BTN.x, PAUSE_BTN.x, DOT_SIZE];
        let right = [
            btn("🎤  Voice: starting…", 170.0),
            8.0,
            btn("lb", 52.0),
            btn("🔌 Machine", 0.0),
            btn("🌙 Sleep", 0.0),
            btn("⚙ Settings", 0.0),
            8.0,
            w("⏱ 199:59", 20.0),
        ];
        // 8-point item spacing between widgets (add_space adds none of its own)
        let spaces = 3;
        let gaps = (left.len() + right.len() - spaces) as f32 - 1.0;
        let total: f32 = left.iter().chain(right.iter()).sum::<f32>() + 8.0 * gaps;
        assert!(total <= 1080.0 - 36.0, "top bar is {total} points wide");
    }

    use super::{cables_moved, set_reps, sleep_due, sleep_text_spot, voice_log_line, weight_off_check, OffCheck};

    #[test]
    fn sleep_text_moves_around() {
        let spots: Vec<(f32, f32)> = (0..200).map(sleep_text_spot).collect();
        for (x, y) in &spots {
            // on screen, away from the edges (the text is about 0.2 of the width wide)
            assert!((0.2..=0.8).contains(x) && (0.1..=0.9).contains(y), "{x} {y}");
        }
        // every move goes somewhere clearly different
        for w in spots.windows(2) {
            let d = ((w[0].0 - w[1].0).powi(2) + (w[0].1 - w[1].1).powi(2)).sqrt();
            assert!(d > 0.02, "{w:?}");
        }
        // and over time it covers the whole screen, not one corner
        assert!(spots.iter().any(|s| s.0 < 0.35) && spots.iter().any(|s| s.0 > 0.65));
        assert!(spots.iter().any(|s| s.1 < 0.3) && spots.iter().any(|s| s.1 > 0.7));
    }
    use crate::voice::Listen;

    #[test]
    fn voice_log_lines() {
        let did = vec!["Not a command I know — ignored.".to_string()];
        assert_eq!(
            voice_log_line("2026-11-18 06:29:41", Listen::Always, false, "wait on", Some("weight on"), &did),
            "2026-11-18 06:29:41  always  heard \"wait on\"  read as \"weight on\"  ->  Not a command I know — ignored."
        );
        // same text: no "read as"; an early stop says so
        let l = voice_log_line("t", Listen::WakeWord, true, "stop", Some("stop"), &["Weight off.".into()]);
        assert_eq!(l, "t  wake    heard \"stop\" (partial, acted on at once)  ->  Weight off.");
    }

    #[test]
    fn sleeps_only_after_the_set_time_without_use() {
        assert!(!sleep_due(29.0 * 60.0, 30));
        assert!(sleep_due(30.0 * 60.0, 30));
        // 0 = never
        assert!(!sleep_due(1e7, 0));
        // a rep moves the cable well past SLEEP_MOVE_M; resting hands don't
        assert!(cables_moved([0.40, 0.02], [0.02, 0.02]));
        assert!(!cables_moved([0.03, 0.02], [0.02, 0.02]));
    }

    #[test]
    fn weight_off_is_checked_against_the_cable_force() {
        let base = crate::mechanics::BASE_TENSION_KG;
        // too early
        assert_eq!(weight_off_check(0.2, [20.0, 20.0], true, 0), OffCheck::Wait);
        // back at base tension (7.7 lb idle frames)
        assert_eq!(weight_off_check(1.2, [base, base + 0.2], true, 0), OffCheck::Confirmed);
        // one cable still loaded: send weight off again, then warn
        assert_eq!(weight_off_check(1.2, [base, 20.0], true, 0), OffCheck::Resend);
        assert_eq!(weight_off_check(1.2, [base, 20.0], true, 1), OffCheck::Resend);
        assert_eq!(weight_off_check(1.2, [base, 20.0], true, 2), OffCheck::StillLoaded);
        // the machine stops streaming after weight off: a reading at base tension that
        // arrived after the weight-off still confirms it (seen on the tablet)
        assert_eq!(weight_off_check(3.0, [base, base], true, 0), OffCheck::Confirmed);
        // no live frame at all since the weight-off: can't confirm
        assert_eq!(weight_off_check(1.2, [base, base], false, 0), OffCheck::NoData);
    }

    #[test]
    fn reps_restart_with_the_machine_counter() {
        // counter was 11 when the weight went on again; machine restarted it at 0
        let mut base = [11u8, 11];
        assert_eq!(set_reps(&mut base, [0, 0]), [0, 0]);
        assert_eq!(set_reps(&mut base, [2, 1]), [2, 1]);
        // counter that keeps counting from the old value still works
        let mut base = [5u8, 5];
        assert_eq!(set_reps(&mut base, [7, 6]), [2, 1]);
    }
}

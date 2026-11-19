//! Voice control, entirely on the tablet: microphone (AAudio) → sherpa-onnx streaming
//! recogniser (English zipformer, 20M, int8; files in `assets/voice/en-20M`) → text →
//! `commands::parse`, the same parser typed commands use. Nothing is sent online.
//!
//! Two listening styles (the owner's choice, toggled on the Voice Control card):
//! * **Always**: every phrase heard is treated as a command.
//! * **Wake word**: a phrase must start with "Weight" (or a word that sounds like it: "wait",
//!   "weigh", …), see [`wake_gate`]. "Stop" / "weight off" always work without it.
//!
//! "Stop" / "weight off" act as soon as they appear in the running (partial) transcript,
//! without waiting for the end of the phrase (`Heard { early_stop: true }`).

use crate::commands::{self, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Words heard as the wake word "Weight". The recogniser often writes "wait", "weigh" or
/// "way" for it, so all of these count.
pub const WAKE_WORDS: &[&str] = &[
    "weight", "weights", "wait", "waits", "weigh", "weighs", "weighed", "way", "wade", "wate", "waite", "whate",
];
/// Words people put in front of a wake word ("hey weight", "ok weight").
const FILLERS: &[&str] = &["hey", "ok", "okay", "hi", "oh", "uh", "um"];
/// After "Weight" on its own, the next phrase within this many seconds is taken as a command.
pub const WAKE_WINDOW_S: f32 = 6.0;
/// Endpoint: the phrase is complete after this much silence following speech (seconds).
/// sherpa-onnx's default is 1.2 s; shorter makes commands react sooner.
pub const END_SILENCE_S: f32 = 0.8;
/// Threads the recogniser uses (the tablet has 4 cores; the screen and the machine link need
/// the rest).
pub const THREADS: i32 = 2;
/// Model files, inside `assets/voice/en-20M` in the APK and in the app's own folder on the
/// tablet.
pub const MODEL_FILES: &[&str] = &["encoder.int8.onnx", "decoder.onnx", "joiner.int8.onnx", "tokens.txt"];
/// Loudest sample (0..1) below which the microphone counts as sending silence. A working mic
/// picks up room noise well above this; the clip-on mic's own mute button sends zeros.
pub const SILENT_PEAK: f32 = 0.0005;
/// Silence this long (seconds) shows the "microphone is sending silence" warning.
pub const SILENT_FOR_S: f32 = 4.0;
/// Test recording: when this file appears in the app's folder (created over adb), the next
/// `TEST_RECORD_S` seconds of microphone sound, exactly as the recogniser gets it, are saved
/// as `TEST_RECORD_WAV` there, and the flag file is removed. Nothing is recorded otherwise.
pub const TEST_RECORD_FLAG: &str = "record-mic";
pub const TEST_RECORD_WAV: &str = "mic-test.wav";
pub const TEST_RECORD_S: u32 = 20;
/// Microphone reads (100 ms each) per `Level` event.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const LEVEL_READS: u32 = 3;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Listen {
    /// Every phrase is a command.
    #[default]
    Always,
    /// Phrases must start with the wake word "Weight".
    WakeWord,
}

pub enum VoiceEvent {
    Listening,
    /// A finished phrase, or (`early_stop`) a stop / weight-off seen before the phrase ended.
    Heard { text: String, early_stop: bool },
    /// Loudest sample (0..1) in the last ~0.3 s, for the level bar and the silence warning.
    Level(f32),
    /// A message for the Voice box (the test recording started / was saved).
    Note(String),
    Error(String),
    Stopped,
}

/// What to do with a finished phrase in wake-word mode.
#[derive(Debug, PartialEq)]
pub enum Gate {
    /// Run this text as a command.
    Run(String),
    /// The wake word on its own: take the next phrase as a command.
    Arm,
    /// Not meant for the app.
    Ignore,
}

/// Lowercase words without punctuation.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '.'))
        .map(|w| w.trim_matches(|c| c == '\'' || c == '.').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Loudest sample in a block (0..1).
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Second-order Butterworth high-pass filter, for the diagnosis tools only: it removes the
/// low mains hum (40-60 Hz) the clip-on mic picks up, but in tests it didn't make the
/// recogniser better (2026-11-18), so the app doesn't use it.
#[cfg(test)]
pub struct HighPass {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

#[cfg(test)]
impl HighPass {
    pub fn new(rate: f32, cutoff_hz: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * cutoff_hz / rate;
        let (sin, cos) = w.sin_cos();
        // Butterworth: Q = 1/√2, alpha = sin(w) / (2Q)
        let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let x0 = *s;
            let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
            self.x = [x0, self.x[0]];
            self.y = [y0, self.y[0]];
            *s = y0;
        }
    }
}

/// A 16-bit mono WAV file of `samples` (-1..1).
pub fn wav_bytes(rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    b
}

/// Level bar fill (0..1): -60 dB (or less) = empty, 0 dB = full.
pub fn level_fill(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub fn is_wake_word(w: &str) -> bool {
    WAKE_WORDS.contains(&w)
}

/// The recogniser often writes "weight" as "wait", "weigh", "way"… ("weight on" came out as
/// "wait on" in the tests): write them all as "weight" before the command parser reads them.
pub fn normalize(text: &str) -> String {
    words(text)
        .into_iter()
        .map(|w| if is_wake_word(&w) && !w.starts_with("weight") { "weight".to_string() } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stop / weight-off command (always obeyed, in either listening style).
pub fn is_stop(text: &str) -> bool {
    commands::parse(&normalize(text)) == Some(Command::WeightOff)
}

/// Wake-word mode: decide what a finished phrase means. `armed` is true for
/// [`WAKE_WINDOW_S`] after the wake word was said on its own.
pub fn wake_gate(text: &str, armed: bool) -> Gate {
    if is_stop(text) {
        return Gate::Run(normalize(text));
    }
    let w = words(text);
    let start = w.iter().take_while(|x| FILLERS.contains(&x.as_str())).count();
    let w = &w[start..];
    if let Some(first) = w.first() {
        if is_wake_word(first) {
            let rest = normalize(&w[1..].join(" "));
            if rest.is_empty() {
                return Gate::Arm;
            }
            // "Weight, set to twenty" → "set to twenty"; but "weight on" / "weight up" are
            // whole commands that start with the wake word, so try the full phrase too
            let full = normalize(&w.join(" "));
            if commands::parse(&rest).is_some() {
                return Gate::Run(rest);
            }
            // nothing understood: still show what was heard, since the app was addressed
            return Gate::Run(full);
        }
    }
    if armed && !w.is_empty() {
        return Gate::Run(normalize(&w.join(" ")));
    }
    Gate::Ignore
}

pub struct Voice {
    pub rx: Receiver<VoiceEvent>,
    stop: Arc<AtomicBool>,
}

impl Voice {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start() -> Voice {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = platform::run(tx.clone(), stop2) {
            let _ = tx.send(VoiceEvent::Error(e));
        }
        let _ = tx.send(VoiceEvent::Stopped);
    });
    Voice { rx, stop }
}

/// The recogniser and its running phrase. Fed with microphone samples on the tablet, and with
/// recorded speech in the PC tests.
#[cfg(any(target_os = "android", all(test, windows)))]
pub struct Recognizer {
    rec: sherpa_onnx::OnlineRecognizer,
    stream: sherpa_onnx::OnlineStream,
    /// A stop was already sent for the running phrase.
    early_sent: bool,
}

#[cfg(any(target_os = "android", all(test, windows)))]
impl Recognizer {
    pub fn new(dir: &std::path::Path) -> Result<Self, String> {
        use sherpa_onnx::{OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineTransducerModelConfig};
        let file = |n: &str| {
            let p = dir.join(n);
            if p.is_file() {
                Ok(Some(p.to_string_lossy().into_owned()))
            } else {
                Err(format!("speech model file missing: {}", p.display()))
            }
        };
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: file(MODEL_FILES[0])?,
                    decoder: file(MODEL_FILES[1])?,
                    joiner: file(MODEL_FILES[2])?,
                },
                tokens: file(MODEL_FILES[3])?,
                num_threads: THREADS,
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            enable_endpoint: true,
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: END_SILENCE_S,
            rule3_min_utterance_length: 12.0,
            ..Default::default()
        };
        let rec = OnlineRecognizer::create(&config).ok_or("the speech recogniser could not start")?;
        let stream = rec.create_stream();
        Ok(Self { rec, stream, early_sent: false })
    }

    /// Feed samples (-1..1, mono) and return any phrases heard.
    pub fn feed(&mut self, sample_rate: i32, samples: &[f32]) -> Vec<VoiceEvent> {
        self.stream.accept_waveform(sample_rate, samples);
        self.collect()
    }

    /// End of the recording (tests): flush the last phrase.
    #[cfg(test)]
    pub fn finish(&mut self, sample_rate: i32) -> Vec<VoiceEvent> {
        // a little silence so the last words are decoded, then end the input
        self.stream.accept_waveform(sample_rate, &vec![0.0; sample_rate as usize]);
        self.stream.input_finished();
        let mut out = self.collect();
        if let Some(t) = self.text() {
            out.push(VoiceEvent::Heard { text: t, early_stop: false });
        }
        self.rec.reset(&self.stream);
        out
    }

    fn text(&self) -> Option<String> {
        let t = self.rec.get_result(&self.stream)?.text.trim().to_lowercase();
        (!t.is_empty()).then_some(t)
    }

    fn collect(&mut self) -> Vec<VoiceEvent> {
        let mut out = Vec::new();
        while self.rec.is_ready(&self.stream) {
            self.rec.decode(&self.stream);
        }
        let partial = self.text();
        // stop / weight off: act now, don't wait for the end of the phrase
        if let Some(p) = &partial {
            if !self.early_sent && is_stop(p) {
                self.early_sent = true;
                out.push(VoiceEvent::Heard { text: p.clone(), early_stop: true });
            }
        }
        if self.rec.is_endpoint(&self.stream) {
            if let Some(t) = partial {
                out.push(VoiceEvent::Heard { text: t, early_stop: false });
            }
            self.rec.reset(&self.stream);
            self.early_sent = false;
        }
        out
    }
}

/// AAudio's read result: a frame count (>= 0), or a negative error code.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn frames_read(code: i32) -> Option<usize> {
    (code >= 0).then_some(code as usize)
}

#[cfg(target_os = "android")]
mod platform {
    use super::*;
    use ndk::audio::{AudioDirection, AudioFormat, AudioSharingMode, AudioStreamBuilder};

    /// Asked for the microphone once while the app is open; after that only explain.
    static ASKED: AtomicBool = AtomicBool::new(false);

    pub fn run(tx: Sender<VoiceEvent>, stop: Arc<AtomicBool>) -> Result<(), String> {
        if !crate::has_mic_permission()? {
            if ASKED.swap(true, Ordering::SeqCst) {
                return Err("Free Lift isn't allowed to use the microphone. Allow it in Android Settings › Apps › Free Lift › Permissions, then turn Voice on again.".into());
            }
            // Android shows "Allow Free Lift to record audio?"; Free Lift leaves the screen
            // while it is up, which stops this thread; it starts again when the app returns
            crate::request_mic_permission()?;
            return Ok(());
        }
        let dir = crate::voice_model_dir()?;
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut rec = Recognizer::new(&dir)?;
        let stream = AudioStreamBuilder::new()
            .map_err(|e| format!("microphone: {e}"))?
            .direction(AudioDirection::Input)
            .sharing_mode(AudioSharingMode::Shared)
            // the input preset is left at AAudio's default, VOICE_RECOGNITION (setting it
            // needs API 28; the app supports 26)
            .sample_rate(16_000)
            .channel_count(1)
            .format(AudioFormat::PCM_Float)
            .open_stream()
            .map_err(|e| format!("couldn't open the microphone: {e}"))?;
        let rate = stream.sample_rate();
        let channels = stream.channel_count().max(1) as usize;
        let float = stream.format() == AudioFormat::PCM_Float;
        stream.request_start().map_err(|e| format!("couldn't start the microphone: {e}"))?;
        let _ = tx.send(VoiceEvent::Listening);
        // 100 ms per read
        let frames = (rate / 10).max(160) as usize;
        let mut fbuf = vec![0f32; frames * channels];
        let mut ibuf = vec![0i16; frames * channels];
        let mut mono = Vec::with_capacity(frames);
        let (mut level, mut level_reads) = (0.0f32, 0u32);
        // test recording (see TEST_RECORD_FLAG): the flag is looked for once a second
        let app_dir = crate::app::DATA_DIR.get().cloned();
        let mut test_rec: Option<Vec<f32>> = None;
        let mut flag_reads = 0u32;
        while !stop.load(Ordering::SeqCst) {
            let r = unsafe {
                if float {
                    stream.read(fbuf.as_mut_ptr().cast(), frames as i32, 200_000_000)
                } else {
                    stream.read(ibuf.as_mut_ptr().cast(), frames as i32, 200_000_000)
                }
            };
            // ndk 0.9 reports every successful read except 0 frames as an error
            // (`__Unknown(n)`), so take the raw AAudio result: >= 0 is the frame count
            let code = match r {
                Ok(n) => n as i32,
                Err(e) => i32::from(e),
            };
            let n = frames_read(code).ok_or_else(|| format!("microphone stopped: {:?}", ndk::audio::AudioError::from(code)))?;
            if n == 0 {
                continue;
            }
            mono.clear();
            for f in 0..n {
                let mut s = 0.0;
                for c in 0..channels {
                    let i = f * channels + c;
                    s += if float { fbuf[i] } else { ibuf[i] as f32 / 32768.0 };
                }
                mono.push(s / channels as f32);
            }
            level = level.max(peak(&mono));
            level_reads += 1;
            if level_reads >= LEVEL_READS {
                let _ = tx.send(VoiceEvent::Level(level));
                level = 0.0;
                level_reads = 0;
            }
            if let Some(buf) = test_rec.as_mut() {
                buf.extend_from_slice(&mono);
                if buf.len() >= (rate as u32 * TEST_RECORD_S) as usize {
                    let wav = wav_bytes(rate as u32, buf);
                    test_rec = None;
                    let msg = match app_dir.as_ref().map(|d| std::fs::write(d.join(TEST_RECORD_WAV), wav)) {
                        Some(Ok(())) => "Test recording saved.".to_string(),
                        other => format!("Test recording couldn't be saved: {other:?}"),
                    };
                    let _ = tx.send(VoiceEvent::Note(msg));
                }
            } else {
                flag_reads += 1;
                if flag_reads >= 10 {
                    flag_reads = 0;
                    if let Some(flag) = app_dir.as_ref().map(|d| d.join(TEST_RECORD_FLAG)).filter(|f| f.exists()) {
                        let _ = std::fs::remove_file(flag);
                        test_rec = Some(Vec::with_capacity((rate as u32 * TEST_RECORD_S) as usize));
                        let _ = tx.send(VoiceEvent::Note(format!(
                            "Recording {TEST_RECORD_S} s of the microphone for a test… (stream: {rate} Hz, {channels} ch, {})",
                            if float { "float" } else { "16-bit" }
                        )));
                    }
                }
            }
            for ev in rec.feed(rate, &mono) {
                let _ = tx.send(ev);
            }
        }
        let _ = stream.request_stop();
        Ok(())
    }
}

#[cfg(not(target_os = "android"))]
mod platform {
    use super::*;
    pub fn run(_tx: Sender<VoiceEvent>, _stop: Arc<AtomicBool>) -> Result<(), String> {
        Err("Voice control runs on the tablet only. Typed commands work here.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_unload_phrases() {
        use crate::commands::{parse, Command};
        assert_eq!(parse("enable smart unload"), Some(Command::SmartLoadOff(true)));
        assert_eq!(parse("turn off smart load off"), Some(Command::SmartLoadOff(false)));
        assert_eq!(parse("disable auto off"), Some(Command::SmartLoadOff(false)));
    }

    fn run_text(g: Gate) -> Option<Command> {
        match g {
            Gate::Run(t) => commands::parse(&t),
            _ => None,
        }
    }

    #[test]
    fn wake_word_gate() {
        // the wake word and the words heard for it
        for w in ["weight", "wait", "weigh", "way"] {
            assert_eq!(wake_gate(w, false), Gate::Arm, "{w}");
            assert!(matches!(run_text(wake_gate(&format!("{w} set weight to twenty"), false)), Some(Command::SetWeight { .. })), "{w}");
        }
        assert_eq!(wake_gate("hey weight", false), Gate::Arm);
        // commands that start with "weight" work as they are
        assert_eq!(run_text(wake_gate("weight on", false)), Some(Command::WeightOn));
        assert_eq!(run_text(wake_gate("wait weight on", false)), Some(Command::WeightOn));
        // stop always works, with or without the wake word
        assert_eq!(run_text(wake_gate("stop", false)), Some(Command::WeightOff));
        assert_eq!(run_text(wake_gate("weight off", false)), Some(Command::WeightOff));
        // other speech is ignored unless the wake word was just said
        assert_eq!(wake_gate("set weight to twenty", false), Gate::Ignore);
        assert_eq!(wake_gate("turn it on", false), Gate::Ignore);
        assert!(matches!(run_text(wake_gate("set weight to twenty", true)), Some(Command::SetWeight { .. })));
        assert_eq!(wake_gate("", true), Gate::Ignore);
        // sound-alikes inside the command too
        assert_eq!(run_text(wake_gate("weight wait on", false)), Some(Command::WeightOn));
        // the owner's own voice, as the tablet heard it (voice log, 2026-11-18 06:57)
        assert_eq!(run_text(wake_gate("wait on", false)), Some(Command::WeightOn));
        assert_eq!(run_text(wake_gate("wait off", false)), Some(Command::WeightOff));
        assert!(matches!(run_text(wake_gate("wait set to twenty", false)), Some(Command::SetWeight { value, .. }) if value == 20.0));
        assert_eq!(wake_gate("wait", false), Gate::Arm);
        assert!(matches!(run_text(wake_gate("set to thirty", true)), Some(Command::SetWeight { value, .. }) if value == 30.0));
        assert_eq!(wake_gate("barbell dumbells", false), Gate::Ignore);
        assert_eq!(run_text(wake_gate("wait barbell", false)), Some(Command::Barbell(true)));
        assert_eq!(run_text(wake_gate("wait dumbbells", false)), Some(Command::Independent(true)));
        assert_eq!(run_text(wake_gate("wait off", false)), Some(Command::WeightOff));
    }

    /// Diagnosis tool, not part of the normal run: recognise the WAV files listed in the
    /// FREELIFT_WAVS environment variable (separated by ';'), as heard and through the
    /// high-pass filter at FREELIFT_HPF Hz (default 120).
    /// `FREELIFT_WAVS="a.wav;b.wav" cargo test --lib --target x86_64-pc-windows-msvc recognise_wavs -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn recognise_wavs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let cut: f32 = std::env::var("FREELIFT_HPF").ok().and_then(|v| v.parse().ok()).unwrap_or(120.0);
        let files = std::env::var("FREELIFT_WAVS").unwrap_or_default();
        for f in files.split(';').filter(|f| !f.is_empty()) {
            let wave = sherpa_onnx::Wave::read(f).expect(f);
            let rate = wave.sample_rate();
            for filtered in [false, true] {
                let mut rec = Recognizer::new(&root.join("assets/voice/en-20M")).expect("model");
                // FREELIFT_WARM=1: first feed the model's own recording, as the normal test does
                if std::env::var("FREELIFT_WARM").as_deref() == Ok("silence") {
                    rec.feed(16000, &vec![0.0; 16000 * 2]);
                } else if std::env::var("FREELIFT_WARM").is_ok() {
                    let w = sherpa_onnx::Wave::read(root.join("voice-test/0.wav").to_str().unwrap()).unwrap();
                    for c in w.samples().chunks(1600) {
                        rec.feed(w.sample_rate(), c);
                    }
                    rec.finish(w.sample_rate());
                }
                let mut hp = HighPass::new(rate as f32, cut);
                let mut out = Vec::new();
                let gain: f32 = std::env::var("FREELIFT_GAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
                for chunk in wave.samples().chunks((rate / 10) as usize) {
                    let mut c: Vec<f32> = chunk.iter().map(|s| s * gain).collect();
                    if filtered {
                        hp.process(&mut c);
                    }
                    out.extend(rec.feed(rate, &c));
                }
                out.extend(rec.finish(rate));
                let heard: Vec<String> = out
                    .into_iter()
                    .filter_map(|e| match e {
                        VoiceEvent::Heard { text, early_stop: false } => Some(text),
                        _ => None,
                    })
                    .collect();
                let name = std::path::Path::new(f).file_name().unwrap().to_string_lossy();
                println!("{name:<28} {:<10} {heard:?}", if filtered { format!("hpf {cut}") } else { "as heard".into() });
            }
        }
    }

    /// Diagnosis: the running text every 100 ms for FREELIFT_WAV, with endpoints marked.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn trace_wav() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let f = std::env::var("FREELIFT_WAV").unwrap();
        let wave = sherpa_onnx::Wave::read(&f).expect("wav");
        let rate = wave.sample_rate();
        let rec = Recognizer::new(&root.join("assets/voice/en-20M")).expect("model");
        let mut samples = wave.samples().to_vec();
        samples.extend(vec![0.0; rate as usize * 3]);
        for (i, chunk) in samples.chunks((rate / 10) as usize).enumerate() {
            rec.stream.accept_waveform(rate, chunk);
            let mut decoded = 0;
            while rec.rec.is_ready(&rec.stream) {
                rec.rec.decode(&rec.stream);
                decoded += 1;
            }
            let t = rec.text().unwrap_or_default();
            let end = rec.rec.is_endpoint(&rec.stream);
            println!("{:4.1}s decoded {decoded} {:?}{}", i as f32 / 10.0, t, if end { "  <ENDPOINT>" } else { "" });
            // FREELIFT_KEEP_EMPTY=1: don't restart at an endpoint when nothing was written
            if end && !(t.is_empty() && std::env::var("FREELIFT_KEEP_EMPTY").is_ok()) {
                rec.rec.reset(&rec.stream);
            }
        }
    }

    #[test]
    fn high_pass_removes_hum_keeps_voice() {
        let rate = 16000.0;
        let gain = |hz: f32| {
            let mut hp = HighPass::new(rate, 120.0);
            let mut x: Vec<f32> = (0..16000).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate).sin()).collect();
            hp.process(&mut x);
            peak(&x[8000..])
        };
        assert!(gain(60.0) < 0.3, "60 Hz {}", gain(60.0)); // hum: at least -10 dB
        assert!(gain(1000.0) > 0.95, "1 kHz {}", gain(1000.0)); // voice: untouched
        assert!((gain(120.0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05); // -3 dB at the cutoff
    }

    #[test]
    fn test_recording_wav() {
        let b = wav_bytes(16000, &[0.0, 1.0, -1.0, 0.5]);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(b.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 16000);
        assert_eq!(i16::from_le_bytes([b[46], b[47]]), 32767);
        assert_eq!(i16::from_le_bytes([b[48], b[49]]), -32767);
        // the recogniser reads it back
        #[cfg(windows)]
        {
            let p = std::env::temp_dir().join("freelift-wav-test.wav");
            std::fs::write(&p, &b).unwrap();
            let w = sherpa_onnx::Wave::read(p.to_str().unwrap()).expect("wav");
            assert_eq!(w.sample_rate(), 16000);
            assert_eq!(w.samples().len(), 4);
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn mic_level() {
        assert_eq!(peak(&[0.1, -0.4, 0.2]), 0.4);
        assert_eq!(peak(&[]), 0.0);
        // the bar: silence empty, full scale full, -30 dB half
        assert_eq!(level_fill(0.0), 0.0);
        assert_eq!(level_fill(1.0), 1.0);
        assert!((level_fill(10f32.powf(-1.5)) - 0.5).abs() < 1e-4);
        // the silence warning threshold is at about -66 dB, below any working mic's room noise
        assert!(level_fill(SILENT_PEAK) == 0.0);
    }

    #[test]
    fn microphone_read_counts() {
        // 100 ms at 16 kHz was read: not an error (the tablet's first voice build stopped here)
        assert_eq!(frames_read(1600), Some(1600));
        assert_eq!(frames_read(0), Some(0));
        // AAudio errors are negative (-899 = disconnected)
        assert_eq!(frames_read(-899), None);
    }

    #[test]
    fn weight_sound_alikes() {
        assert_eq!(normalize("Wait on."), "weight on");
        assert_eq!(commands::parse(&normalize("wait on")), Some(Command::WeightOn));
        assert!(is_stop("wait off"));
        assert!(is_stop("stop"));
        assert_eq!(normalize("set weights to ten"), "set weights to ten");
    }

    /// The real recogniser on recorded speech (PC only: needs the sherpa-onnx Windows
    /// library, which the build downloads, and the model in assets/voice/en-20M).
    #[cfg(windows)]
    #[test]
    fn recogniser_hears_recorded_speech() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut rec = Recognizer::new(&root.join("assets/voice/en-20M")).expect("model");
        let heard = |rec: &mut Recognizer, file: &str| -> Vec<(String, bool)> {
            let wave = sherpa_onnx::Wave::read(root.join("voice-test").join(file).to_str().unwrap()).expect(file);
            let rate = wave.sample_rate();
            let mut out = Vec::new();
            for chunk in wave.samples().chunks((rate / 10) as usize) {
                out.extend(rec.feed(rate, chunk));
            }
            out.extend(rec.finish(rate));
            out.into_iter()
                .filter_map(|e| match e {
                    VoiceEvent::Heard { text, early_stop } => Some((text, early_stop)),
                    _ => None,
                })
                .collect()
        };
        // the model's own test recording
        let all: String = heard(&mut rec, "0.wav").into_iter().map(|(t, _)| t).collect::<Vec<_>>().join(" ");
        assert!(all.contains("yellow lamps"), "{all}");
        // commands spoken by the PC's text-to-speech voice (voice-test/make_commands.ps1)
        for (file, want) in [
            ("weight_on.wav", Command::WeightOn),
            ("weight_off.wav", Command::WeightOff),
            ("stop.wav", Command::WeightOff),
        ] {
            let h = heard(&mut rec, file);
            let cmds: Vec<_> = h.iter().filter_map(|(t, _)| commands::parse(&normalize(t))).collect();
            assert!(cmds.contains(&want), "{file}: heard {h:?}");
        }
        // a stop is acted on before the phrase ends
        let h = heard(&mut rec, "stop.wav");
        assert!(h.iter().any(|(_, early)| *early), "stop.wav: {h:?}");
        // wake word, then the command in the same phrase
        // "Weight." then a pause: the wake word alone arms the next phrase, as in the app
        let h = heard(&mut rec, "wake_set_twenty.wav");
        let mut armed = false;
        let mut ran = Vec::new();
        for (t, _) in h.iter().filter(|(_, e)| !e) {
            match wake_gate(t, armed) {
                Gate::Arm => armed = true,
                g => {
                    armed = false;
                    ran.extend(run_text(g));
                }
            }
        }
        assert!(ran.iter().any(|c| matches!(c, Command::SetWeight { .. })), "wake_set_twenty.wav: heard {h:?}");
    }
}

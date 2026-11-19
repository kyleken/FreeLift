//! Turns a spoken or typed sentence into a machine command.
//! Intents follow the original's list (assets/intent_vectors_en.json), see MECHANICS.md.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Kg,
    Lb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Both,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WeightMode {
    Standard,
    Eccentric,
    Chain,
    Isokinetic,
}

impl WeightMode {
    pub fn label(self) -> &'static str {
        match self {
            WeightMode::Standard => "Standard",
            WeightMode::Eccentric => "Eccentric",
            WeightMode::Chain => "Chain",
            WeightMode::Isokinetic => "Isokinetic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    Eccentric,
    Chain,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    WeightOn,
    WeightOff,
    /// +1 up, -1 down; amount None = "a bit".
    Bump { dir: i8, amount: Option<f32>, unit: Option<Unit>, side: Side },
    SetWeight { value: f32, unit: Option<Unit>, side: Side },
    SetMode(WeightMode),
    SetExtra { kind: Extra, value: f32, unit: Option<Unit> },
    SetSpeed(f32),
    Barbell(bool),
    Independent(bool),
    Assist(bool),
    AssistLevel(u8),
    SmartLoadOff(bool),
    Units(Unit),
    /// Start the workout clock (the Start Workout button).
    StartWorkout,
    Pause,
    Resume,
    SkipRest,
    FinishSet,
    EndTraining,
    NextExercise,
    PreviousExercise,
    QueryWeight,
    VoiceOff,
}

/// The original's 21 voice intents (intent / action / mode, from assets/intent_vectors_en.json),
/// each with one example sentence this parser understands.
pub const ORIGINAL_INTENTS: [(&str, &str); crate::mechanics::VOICE_INTENTS] = [
    ("adjust_weight / set_value", "set the weight to twenty kilos"),
    ("adjust_weight / turn_on", "turn on the weight"),
    ("adjust_weight / turn_off", "turn off the weight"),
    ("adjust_weight / increase", "increase the weight a bit"),
    ("adjust_weight / decrease", "decrease the weight a bit"),
    ("playback_control / pause", "pause the training"),
    ("playback_control / resume", "resume training"),
    ("switch_weight_mode / chain", "switch to chain mode"),
    ("switch_weight_mode / standard", "switch to standard mode"),
    ("switch_weight_mode / eccentric", "switch to eccentric mode"),
    ("switch_weight_mode / isokinetic", "switch to isokinetic mode"),
    ("switch_accessory_mode / barbell", "switch to barbell mode"),
    ("switch_accessory_mode / non_barbell", "switch to non barbell mode"),
    ("query_weight", "what is the current weight"),
    ("control_assistance / turn_on", "turn on assist mode"),
    ("control_assistance / turn_off", "turn off assist mode"),
    ("control_assistance / switch_mode", "switch to assist mode two"),
    ("switch_action_group / next", "next exercise"),
    ("switch_action_group / previous", "previous exercise"),
    ("end_training", "end training"),
    ("skip_rest", "skip rest"),
];

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] =
    ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

fn word_value(w: &str) -> Option<u32> {
    if let Some(i) = ONES.iter().position(|x| *x == w) {
        return Some(i as u32);
    }
    if w == "oh" {
        return Some(0);
    }
    TENS.iter().position(|x| !x.is_empty() && *x == w).map(|i| i as u32 * 10)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Num(f32),
}

/// Lowercase, strip punctuation, join number words into numbers.
fn tokenize(text: &str) -> Vec<Tok> {
    let mut clean = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            clean.push(c);
        } else {
            clean.push(' ');
        }
    }
    let raw: Vec<String> = clean
        .split_whitespace()
        .map(|w| w.trim_matches('.').to_string())
        .filter(|w| !w.is_empty())
        .flat_map(split_unit_suffix)
        .collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let w = raw[i].as_str();
        if let Ok(v) = w.parse::<f32>() {
            out.push(Tok::Num(v));
            i += 1;
            continue;
        }
        // "point five" with no whole part
        if w == "point" {
            if let Some(d) = raw.get(i + 1).and_then(|s| word_value(s)).filter(|v| *v < 10) {
                out.push(Tok::Num(d as f32 / 10.0));
                i += 2;
                continue;
            }
        }
        // "a hundred"
        let a_hundred = w == "a" && raw.get(i + 1).map(|s| s == "hundred").unwrap_or(false);
        if word_value(w).is_some() || a_hundred {
            // last: 0 = nothing yet, 1 = ones/teens, 2 = tens, 3 = hundred
            let mut cur: u32 = 0;
            let mut last = 0u8;
            let mut j = i;
            if a_hundred {
                cur = 1;
                last = 1;
                j += 1;
            }
            while j < raw.len() {
                let t = raw[j].as_str();
                if t == "hundred" {
                    if last != 1 || cur >= 10 {
                        break;
                    }
                    cur *= 100;
                    last = 3;
                } else if t == "and"
                    && last == 3
                    && raw.get(j + 1).map(|n| word_value(n).is_some()).unwrap_or(false)
                {
                    // "one hundred and ten"
                } else if let Some(v) = word_value(t) {
                    let ok = match (last, v >= 20) {
                        (0, _) => true,
                        (2, false) => v < 10 && v > 0,
                        (3, _) => true,
                        _ => false,
                    };
                    if !ok {
                        break;
                    }
                    cur += v;
                    last = if v >= 20 { 2 } else { 1 };
                } else {
                    break;
                }
                j += 1;
            }
            let mut value = cur as f32;
            // decimals: "point five"
            if raw.get(j).map(|s| s == "point").unwrap_or(false) {
                let mut scale = 0.1;
                let mut k = j + 1;
                while let Some(d) = raw.get(k).and_then(|s| word_value(s)).filter(|v| *v < 10) {
                    value += d as f32 * scale;
                    scale /= 10.0;
                    k += 1;
                }
                if k > j + 1 {
                    j = k;
                }
            }
            // "and a half"
            if raw.get(j).map(|s| s == "and").unwrap_or(false)
                && raw.get(j + 1).map(|s| s == "a").unwrap_or(false)
                && raw.get(j + 2).map(|s| s == "half").unwrap_or(false)
            {
                value += 0.5;
                j += 3;
            }
            out.push(Tok::Num(value));
            i = j.max(i + 1);
            continue;
        }
        out.push(Tok::Word(w.to_string()));
        i += 1;
    }
    out
}

/// "20kg" -> ["20", "kg"]
fn split_unit_suffix(w: String) -> Vec<String> {
    for suf in ["kgs", "kg", "lbs", "lb"] {
        if let Some(num) = w.strip_suffix(suf) {
            if !num.is_empty() && num.parse::<f32>().is_ok() {
                return vec![num.to_string(), suf.to_string()];
            }
        }
    }
    vec![w]
}

fn unit_of(w: &str) -> Option<Unit> {
    match w {
        "kg" | "kgs" | "kilo" | "kilos" | "kilogram" | "kilograms" => Some(Unit::Kg),
        "lb" | "lbs" | "pound" | "pounds" => Some(Unit::Lb),
        _ => None,
    }
}

pub fn parse(text: &str) -> Option<Command> {
    let toks = tokenize(text);
    if toks.is_empty() {
        return None;
    }
    let words: Vec<&str> = toks
        .iter()
        .filter_map(|t| if let Tok::Word(w) = t { Some(w.as_str()) } else { None })
        .collect();
    let joined = format!(" {} ", words.join(" "));
    let has = |w: &str| words.contains(&w);
    let phrase = |p: &str| joined.contains(&format!(" {p} "));
    let any = |ws: &[&str]| ws.iter().any(|w| has(w));
    let anyp = |ps: &[&str]| ps.iter().any(|p| phrase(p));

    let numbers: Vec<f32> =
        toks.iter().filter_map(|t| if let Tok::Num(n) = t { Some(*n) } else { None }).collect();
    let first_num = numbers.first().copied();
    // number that follows "to"
    let num_after = |word: &str| -> Option<f32> {
        toks.windows(2).find_map(|w| match (&w[0], &w[1]) {
            (Tok::Word(a), Tok::Num(n)) if a == word => Some(*n),
            _ => None,
        })
    };
    let unit = words.iter().find_map(|w| unit_of(w));
    let side = if has("left") {
        Side::Left
    } else if has("right") {
        Side::Right
    } else {
        Side::Both
    };
    let off_words = ["off", "stop", "disable", "close", "deactivate", "cancel"];

    // voice itself
    if anyp(&["stop listening", "voice off", "turn off voice", "disable voice"]) {
        return Some(Command::VoiceOff);
    }

    // assist / spotter
    if any(&["assist", "assistance", "spotter"]) {
        if let Some(n) = num_after("level").or(num_after("mode")).or(if has("level") { first_num } else { None }) {
            return Some(Command::AssistLevel(n.round().clamp(1.0, 2.0) as u8));
        }
        if let Some(n) = first_num {
            return Some(Command::AssistLevel(n.round().clamp(1.0, 2.0) as u8));
        }
        if any(&off_words) {
            return Some(Command::Assist(false));
        }
        return Some(Command::Assist(true));
    }

    // smart load off
    if anyp(&["smart load off", "auto off", "auto unload", "smart unload"]) {
        let off = any(&["disable", "cancel", "deactivate"])
            || (words.first() == Some(&"turn") && words.get(1) == Some(&"off"));
        return Some(Command::SmartLoadOff(!off));
    }

    // accessory / independent. The speech recogniser writes "barbell" as "barbel" /
    // "barbelle" and "dumbbells" as "dumb bells" / "dumbells", so all of those count.
    if any(&["barbell", "barbells", "barbel", "barbels", "barbelle", "barbelles"]) || anyp(&["bar bell", "bar bells"]) {
        return Some(Command::Barbell(!(has("non") || has("no") || has("without"))));
    }
    // "Dumbbells" is the Setup button for independent left / right weights
    if any(&["dumbbell", "dumbbells", "dumbell", "dumbells", "dumbel", "dumbels"])
        || anyp(&["dumb bell", "dumb bells", "dum bell", "dum bells"])
    {
        return Some(Command::Independent(!any(&off_words)));
    }
    if has("independent") || anyp(&["separate weights", "split weight"]) {
        return Some(Command::Independent(!any(&off_words)));
    }
    if anyp(&["linked mode", "same weight", "link weights", "both sides together"]) {
        return Some(Command::Independent(false));
    }

    // extra eccentric / chain weight with a number
    if let Some(n) = first_num {
        if has("eccentric") && (has("weight") || has("load") || has("to") || has("set")) {
            return Some(Command::SetExtra { kind: Extra::Eccentric, value: n, unit });
        }
        if has("chain") && (has("weight") || has("load") || has("to") || has("set")) {
            return Some(Command::SetExtra { kind: Extra::Chain, value: n, unit });
        }
        if has("speed") || has("velocity") {
            return Some(Command::SetSpeed(n));
        }
    }

    // weight modes
    if has("eccentric") {
        return Some(Command::SetMode(WeightMode::Eccentric));
    }
    if has("chain") {
        return Some(Command::SetMode(WeightMode::Chain));
    }
    if has("isokinetic")
        || anyp(&["constant speed", "fixed speed", "speed mode", "iso kinetic"])
    {
        return Some(Command::SetMode(WeightMode::Isokinetic));
    }
    if (has("standard") || has("normal")) && (has("mode") || has("switch") || has("weight")) {
        return Some(Command::SetMode(WeightMode::Standard));
    }

    // units
    if first_num.is_none()
        && unit.is_some()
        && any(&["use", "switch", "units", "unit", "show", "change", "display"])
    {
        return Some(Command::Units(unit.unwrap()));
    }

    // session control
    if anyp(&["start workout", "start the workout", "start training", "start the training", "begin workout", "begin the workout", "begin training"]) {
        return Some(Command::StartWorkout);
    }
    if anyp(&["skip rest", "skip the rest", "without resting", "next set", "skip resting"]) {
        return Some(Command::SkipRest);
    }
    if anyp(&[
        "end training",
        "finish training",
        "end workout",
        "finish workout",
        "end the workout",
        "end session",
        "training is complete",
        "finish this training",
        "stop training",
        "end the training",
    ]) {
        return Some(Command::EndTraining);
    }
    if anyp(&["set done", "finish set", "end set", "finish the set", "end the set", "set complete"]) {
        return Some(Command::FinishSet);
    }
    if anyp(&["next exercise", "next group", "next movement"]) {
        return Some(Command::NextExercise);
    }
    if anyp(&["previous exercise", "previous group", "last exercise", "previous movement"]) {
        return Some(Command::PreviousExercise);
    }
    if has("pause") {
        return Some(Command::Pause);
    }
    if any(&["resume", "unpause", "continue"]) || anyp(&["play video"]) {
        return Some(Command::Resume);
    }

    // query
    if (any(&["what", "what's", "whats", "how", "tell"]) || anyp(&["weight information", "current weight"]))
        && !any(&["set", "change", "adjust", "increase", "decrease"])
        && first_num.is_none()
    {
        return Some(Command::QueryWeight);
    }

    let weightish = any(&["weight", "weights", "load", "resistance", "it", "lift"]);

    // on / off. Off is checked first: when a sentence could mean either, the safe answer wins.
    if first_num.is_none() {
        if anyp(&[
            "weight off",
            "weights off",
            "load off",
            "turn off",
            "switch off",
            "close weight",
            "resistance off",
        ]) || any(&["unload", "release", "disengage", "stop", "halt", "freeze", "emergency"])
            || (weightish && any(&["disable", "deactivate"]))
        {
            return Some(Command::WeightOff);
        }
        // Weight on only when the sentence is clearly about the weight: every word must be
        // one of these. "turn on voice", "turn on the rest timer" or "don't turn on the
        // weight" are not understood rather than starting the motors.
        let on_words = [
            "turn", "switch", "on", "the", "weight", "weights", "load", "resistance", "it",
            "please", "now", "put", "start", "enable", "activate", "engage", "open", "motor",
            "motors", "cable", "cables", "my", "ok", "okay",
        ];
        let clearly_weight = words.iter().all(|w| on_words.contains(w));
        if clearly_weight
            && (anyp(&[
                "weight on",
                "weights on",
                "load on",
                "turn on",
                "switch on",
                "open weight",
                "engage",
                "resistance on",
            ]) || (weightish && any(&["start", "enable", "activate"])))
        {
            return Some(Command::WeightOn);
        }
    }

    let up = any(&["up", "increase", "more", "heavier", "raise", "add", "plus", "harder"]);
    let down = any(&["down", "decrease", "less", "lighter", "reduce", "lower", "minus", "easier", "drop"]);

    // "set/increase the weight to N"
    if let Some(n) = num_after("to").or(num_after("at")) {
        return Some(Command::SetWeight { value: n, unit, side });
    }
    if up || down {
        let dir = if up && !down { 1 } else if down && !up { -1 } else if has("up") { 1 } else { -1 };
        return Some(Command::Bump { dir, amount: first_num, unit, side });
    }
    if let Some(n) = first_num {
        if weightish || any(&["set", "change", "adjust", "make", "bring", "lock", "go"]) || unit.is_some() || words.is_empty() {
            return Some(Command::SetWeight { value: n, unit, side });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(tokenize("twenty five"), vec![Tok::Num(25.0)]);
        assert_eq!(tokenize("one hundred and ten"), vec![Tok::Num(110.0)]);
        assert_eq!(tokenize("a hundred"), vec![Tok::Num(100.0)]);
        assert_eq!(tokenize("twelve point five"), vec![Tok::Num(12.5)]);
        assert_eq!(tokenize("seven and a half"), vec![Tok::Num(7.5)]);
        assert_eq!(tokenize("20kg")[0], Tok::Num(20.0));
    }

    #[test]
    fn the_persons_examples() {
        assert_eq!(parse("weight on"), Some(Command::WeightOn));
        assert_eq!(parse("Weight off"), Some(Command::WeightOff));
        assert_eq!(parse("turn on the weight"), Some(Command::WeightOn));
        assert_eq!(parse("Turn off the weight"), Some(Command::WeightOff));
        assert_eq!(
            parse("weight up"),
            Some(Command::Bump { dir: 1, amount: None, unit: None, side: Side::Both })
        );
        assert_eq!(
            parse("weight down"),
            Some(Command::Bump { dir: -1, amount: None, unit: None, side: Side::Both })
        );
        assert_eq!(
            parse("set weight to twenty"),
            Some(Command::SetWeight { value: 20.0, unit: None, side: Side::Both })
        );
        assert_eq!(
            parse("Set the weight to 99 kg"),
            Some(Command::SetWeight { value: 99.0, unit: Some(Unit::Kg), side: Side::Both })
        );
        assert_eq!(
            parse("Increase the weight to forty pounds"),
            Some(Command::SetWeight { value: 40.0, unit: Some(Unit::Lb), side: Side::Both })
        );
        assert_eq!(
            parse("increase weight by five"),
            Some(Command::Bump { dir: 1, amount: Some(5.0), unit: None, side: Side::Both })
        );
        assert_eq!(
            parse("set left weight to twelve point five"),
            Some(Command::SetWeight { value: 12.5, unit: None, side: Side::Left })
        );
    }

    #[test]
    fn weight_on_only_when_clearly_asked() {
        for s in ["weight on", "turn on the weight", "please turn the weight on", "start the weight", "engage", "switch on"] {
            assert_eq!(parse(s), Some(Command::WeightOn), "{s}");
        }
        for s in ["turn on voice", "turn on the rest timer", "don't turn on the weight", "do not turn on the weight", "turn on the lights"] {
            assert_ne!(parse(s), Some(Command::WeightOn), "{s}");
        }
    }

    #[test]
    fn stop_turns_the_weight_off() {
        for s in ["stop", "Stop!", "stop stop", "halt", "emergency", "stop it", "stop the weight", "release"] {
            assert_eq!(parse(s), Some(Command::WeightOff), "{s}");
        }
        // the more specific stops keep their meaning
        assert_eq!(parse("stop listening"), Some(Command::VoiceOff));
        assert_eq!(parse("stop training"), Some(Command::EndTraining));
        assert_eq!(parse("stop assistance mode"), Some(Command::Assist(false)));
    }

    #[test]
    fn setup_buttons_by_voice() {
        // what the tablet's recogniser actually wrote for "set mode to barbell"
        assert_eq!(parse("weight set mote to barbel"), Some(Command::Barbell(true)));
        assert_eq!(parse("weight set mote to barbelle"), Some(Command::Barbell(true)));
        assert_eq!(parse("bar bell"), Some(Command::Barbell(true)));
        // Dumbbells = the Setup button for independent weights
        for p in ["dumbbells", "switch to dumbbells", "dumb bells", "dumbells", "set mode to dumbbell"] {
            assert_eq!(parse(p), Some(Command::Independent(true)), "{p}");
        }
        assert_eq!(parse("assist"), Some(Command::Assist(true)));
        // these never turn the weight on
        for p in ["barbel", "dumb bells", "assist"] {
            assert_ne!(parse(p), Some(Command::WeightOn), "{p}");
        }
    }

    #[test]
    fn all_original_intents_understood() {
        for (intent, example) in ORIGINAL_INTENTS {
            assert!(parse(example).is_some(), "{intent}: {example}");
        }
    }

    #[test]
    fn original_intents() {
        assert_eq!(parse("Switch to chain mode"), Some(Command::SetMode(WeightMode::Chain)));
        assert_eq!(parse("switch to fixed speed mode"), Some(Command::SetMode(WeightMode::Isokinetic)));
        assert_eq!(parse("Train in eccentric mode now"), Some(Command::SetMode(WeightMode::Eccentric)));
        assert_eq!(parse("switch to standard mode"), Some(Command::SetMode(WeightMode::Standard)));
        assert_eq!(parse("Use non-barbell mode"), Some(Command::Barbell(false)));
        assert_eq!(parse("Switch to barbell mode"), Some(Command::Barbell(true)));
        assert_eq!(parse("What is the current weight?"), Some(Command::QueryWeight));
        assert_eq!(parse("Turn on assist mode"), Some(Command::Assist(true)));
        assert_eq!(parse("Stop assistance mode"), Some(Command::Assist(false)));
        assert_eq!(parse("Switch to assist mode 2"), Some(Command::AssistLevel(2)));
        assert_eq!(parse("Skip the rest time"), Some(Command::SkipRest));
        assert_eq!(parse("Finish this training session"), Some(Command::EndTraining));
        assert_eq!(parse("Pause the training"), Some(Command::Pause));
        assert_eq!(parse("Resume training"), Some(Command::Resume));
        assert_eq!(parse("start workout"), Some(Command::StartWorkout));
        assert_eq!(parse("Start the training"), Some(Command::StartWorkout));
        assert_eq!(parse("Next exercise"), Some(Command::NextExercise));
        assert_eq!(
            parse("set eccentric weight to five"),
            Some(Command::SetExtra { kind: Extra::Eccentric, value: 5.0, unit: None })
        );
        assert_eq!(parse("set speed to point five"), Some(Command::SetSpeed(0.5)));
        assert_eq!(parse("set speed to zero point five"), Some(Command::SetSpeed(0.5)));
        assert_eq!(parse("stop listening"), Some(Command::VoiceOff));
        assert_eq!(parse("use pounds"), Some(Command::Units(Unit::Lb)));
        assert_eq!(parse("hello there"), None);
    }
}

//! The Gym Monster motor-board protocol: decoder for the machine's frames (machine -> screen)
//! and builders for the commands Free Lift sends (screen -> machine, written by
//! machine_link.rs). See MECHANICS.md and docs/DEVELOPER_GUIDE.md §4.

pub const HEADER: [u8; 2] = [0xAA, 0xAB];
pub const TYPE_LIVE: u8 = 0x80;

/// CRC-16/X-25: poly 0x1021 reflected (0x8408), init 0xFFFF, xorout 0xFFFF.
pub fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF
}

// ---------------------------------------------------------------------------
// Commands (screen -> motor board). Decoded from the builders that call the CRC
// appender 0x1994410 and checked against frames the original logged on the tablet.
// app.rs::sync_machine decides when to send them; machine_link.rs writes them to the port.

/// Command header (pool constant List<int> [0xcc, 0xcd]).
pub const CMD_HEADER: [u8; 2] = [0xCC, 0xCD];
/// Weight on with settings (builder 0x3a9bdb0, "发送enable指令").
pub const CMD_ENABLE: u8 = 0x01;
/// 0x1bc259c "发送disable指令".
pub const CMD_DISABLE: u8 = 0x02;
/// 0x1c69708 "发送pause指令" – what the original sends to take the weight off.
pub const CMD_PAUSE: u8 = 0x03;
/// 0x3a908f0 "发送reset指令".
pub const CMD_RESET: u8 = 0x04;
/// 0x3b21378 "发送getVersionCode指令".
pub const CMD_GET_VERSION: u8 = 0x06;
/// 0x3a3f244 "发送getSn指令".
pub const CMD_GET_SN: u8 = 0x07;
/// 0x3c3bd78 "发送sleep指令".
pub const CMD_SLEEP: u8 = 0x08;
/// 0x3a8ca0c "发送wakeup指令".
pub const CMD_WAKEUP: u8 = 0x09;
/// Heartbeat (closure 0x19910bc), sent every HEARTBEAT_MS while the weight is on.
pub const CMD_HEARTBEAT: u8 = 0xFE;
/// Timer.periodic Duration 0x7a120 µs (0x1991060).
pub const HEARTBEAT_MS: u64 = 500;
/// Total length of the enable frame (byte 2).
pub const ENABLE_LEN: u8 = 18;

/// Append the CRC-16/X-25 of the frame, high byte first (0x1994410 / 0x199445c / 0x1994cb8).
/// Note: the machine's own frames carry it low byte first.
fn with_crc(mut f: Vec<u8>) -> Vec<u8> {
    let c = crc16_x25(&f);
    f.push((c >> 8) as u8);
    f.push((c & 0xFF) as u8);
    f
}

/// Six-byte command: CC CD 06 <cmd> <crc>.
pub fn cmd_simple(cmd: u8) -> Vec<u8> {
    with_crc(vec![CMD_HEADER[0], CMD_HEADER[1], 6, cmd])
}

/// Heartbeat: CC CD 07 FE <counter % 255> <crc>.
pub fn cmd_heartbeat(counter: u32) -> Vec<u8> {
    with_crc(vec![CMD_HEADER[0], CMD_HEADER[1], 7, CMD_HEARTBEAT, (counter % 255) as u8])
}

/// Device mode byte (0x3934a84: 1 标准 standard, 2 铁链 chain, 3 离心 eccentric, 4 fixed speed).
pub fn device_mode_code(mode: crate::commands::WeightMode) -> u8 {
    use crate::commands::WeightMode::*;
    match mode {
        Standard => 1,
        Chain => 2,
        Eccentric => 3,
        Isokinetic => 4,
    }
}

/// Weight on (0x3a9bdb0): CC CD 12 01 <mode> <weight f64 big-endian, lb> 00 <1 non-barbell | 2 barbell>
/// <extra lb, rounded> <crc>. `shown_lb` is the weight on screen in lb; in barbell mode it is
/// the total and the original halves it before sending.
pub fn cmd_enable(mode_code: u8, shown_lb: f64, barbell: bool, extra_lb: f64) -> Vec<u8> {
    let per_side = if barbell { shown_lb / 2.0 } else { shown_lb };
    let extra = if barbell { extra_lb / 2.0 } else { extra_lb };
    let mut f = vec![CMD_HEADER[0], CMD_HEADER[1], ENABLE_LEN, CMD_ENABLE, mode_code];
    f.extend_from_slice(&per_side.to_be_bytes());
    f.push(0); // assist ("help") – always 0 in this builder
    f.push(if barbell { 2 } else { 1 });
    f.push(extra.round().clamp(0.0, 255.0) as u8);
    with_crc(f)
}

/// Total length of the two-player (independent left/right) enable frame.
pub const ENABLE_2P_LEN: u8 = 17;
/// Two-player enable command byte for player 1 (left cable); player 2 (right) is 0xA2.
pub const CMD_ENABLE_2P_LEFT: u8 = 0xA1;
pub const CMD_ENABLE_2P_RIGHT: u8 = 0xA2;

/// Independent left/right weight on (builder 0x3acafec, "发送enable_2Player指令"; the Free Lift
/// page FreeTrainingLogic 0x1c84154 / 0x1c8445c sends player 1 = Side.left and player 2 =
/// Side.right): CC CD 11 <A1|A2> <mode> <weight f64 big-endian, lb, not halved> 00
/// <extra lb, truncated> <crc>. Decoded from code; not yet seen in the tablet's log.
pub fn cmd_enable_2player(right: bool, mode_code: u8, weight_lb: f64, extra_lb: f64) -> Vec<u8> {
    let cmd = if right { CMD_ENABLE_2P_RIGHT } else { CMD_ENABLE_2P_LEFT };
    let mut f = vec![CMD_HEADER[0], CMD_HEADER[1], ENABLE_2P_LEN, cmd, mode_code];
    f.extend_from_slice(&weight_lb.to_be_bytes());
    f.push(0);
    f.push(extra_lb.trunc().clamp(0.0, 255.0) as u8);
    with_crc(f)
}

/// The original's name for a command frame (from its "发送<name>指令" log texts).
pub fn command_name(frame: &[u8]) -> &'static str {
    match frame.get(3).copied() {
        Some(CMD_ENABLE) => "enable (weight on)",
        Some(CMD_DISABLE) => "disable",
        Some(CMD_PAUSE) => "pause (weight off)",
        Some(CMD_RESET) => "reset",
        Some(CMD_GET_VERSION) => "getVersionCode",
        Some(CMD_GET_SN) => "getSn",
        Some(CMD_SLEEP) => "sleep",
        Some(CMD_WAKEUP) => "wakeup",
        Some(CMD_HEARTBEAT) => "heartbeat",
        Some(CMD_ENABLE_2P_LEFT) => "enable_2Player left",
        Some(CMD_ENABLE_2P_RIGHT) => "enable_2Player right",
        _ => "command",
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SideData {
    /// First value (bytes 9/34). Passed unnamed to the live listener in 0x1b9c29c; its range
    /// (about ±1.6) fits cable speed in m/s (inferred).
    pub a: f64,
    /// Second value (bytes 17/42). The original stores −round(b, 4) as `distanceLeft/Right`.
    pub b: f64,
    /// Third value (bytes 25/50): `torqueLeft/Right`, in pounds (÷ 2.2 for kg).
    pub force_lb: f64,
}

impl SideData {
    /// Cable distance in metres, as the original computes it in 0x1b9c29c:
    /// the value rounded to 4 decimals (toStringAsFixed(4)) and negated.
    pub fn distance_m(&self) -> f64 {
        let rounded = (self.b * 10_000.0).round() / 10_000.0;
        0.0 - rounded
    }
    /// Torque/force in kg, as the original does in kg mode (÷ 2.2, pp+0x202fa0).
    pub fn force_kg(&self) -> f64 {
        self.force_lb / crate::mechanics::LB_PER_KG as f64
    }
}

/// Reply to a command (handler 0x1b98ae0): payload byte 0 (frame byte 4) is 1 when accepted
/// ("下位机-首次上力应答成功" the first time after enable), anything else is "下位机错误应答".
pub const TYPE_ACK: u8 = 0x83;
/// Safety stop (handler 0x1b98cdc, "下位机-保护触发"): the original cancels the heartbeat
/// (0x1994284) and shows the reason from frame byte 4.
pub const TYPE_PROTECTION: u8 = 0x82;

/// The reason texts the original shows for a safety stop (keys safe1…safe6, lockup).
pub fn protection_reason(code: u8) -> &'static str {
    match code {
        1 => "The barbell is not balanced",
        2 => "Loss of grip on the handle or barbell",
        3 => "Cable retraction too fast",
        4 => "Cable extension too fast",
        5 => "Motor temperature too high",
        6 => "The motor is locked. Please try to restore it.",
        _ => "Firmware runtime error",
    }
}

/// What each frame type is handled by in the original (dispatch in 0x1b973ec).
pub fn frame_kind_name(kind: u8) -> &'static str {
    match kind {
        0x80 => "live data (reps, distance, torque)",
        0x83 => "command reply",
        0x82 => "safety stop",
        0x8c => "rowing status",
        0x8d => "skiing status",
        0x84 => "hardware code",
        0x85 => "serial number",
        0x81 | 0x8a | 0x8b => "machine status",
        _ => "unknown",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Live {
        family: u8,
        sub_mode: u8,
        reps_left: u8,
        reps_right: u8,
        left: SideData,
        right: SideData,
    },
    Other { kind: u8, payload: Vec<u8> },
}

fn side(bytes: &[u8]) -> SideData {
    let f = |o: usize| f64::from_be_bytes(bytes[o..o + 8].try_into().unwrap());
    SideData { a: f(1), b: f(9), force_lb: f(17) }
}

/// Decode one complete frame (header .. crc). Returns None if the checksum fails.
pub fn decode(frame: &[u8]) -> Option<Frame> {
    if frame.len() < 6 || frame[0..2] != HEADER || frame[2] as usize != frame.len() {
        return None;
    }
    let n = frame.len();
    let want = u16::from_le_bytes([frame[n - 2], frame[n - 1]]);
    if crc16_x25(&frame[..n - 2]) != want {
        return None;
    }
    if frame[3] == TYPE_LIVE && n == 60 {
        Some(Frame::Live {
            family: frame[4],
            sub_mode: frame[5],
            reps_left: frame[6],
            reps_right: frame[7],
            left: side(&frame[8..33]),
            right: side(&frame[33..58]),
        })
    } else {
        Some(Frame::Other { kind: frame[3], payload: frame[4..n - 2].to_vec() })
    }
}

/// Streaming splitter: feed raw bytes, get decoded frames out.
#[derive(Default)]
pub struct Splitter {
    buf: Vec<u8>,
    pub bad_frames: u64,
}

impl Splitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let Some(start) = self.buf.windows(2).position(|w| w == HEADER) else {
                let keep = if self.buf.last() == Some(&HEADER[0]) { 1 } else { 0 };
                let drop = self.buf.len() - keep;
                self.buf.drain(..drop);
                break;
            };
            self.buf.drain(..start);
            if self.buf.len() < 3 {
                break;
            }
            let len = self.buf[2] as usize;
            if len < 6 {
                self.buf.drain(..1);
                self.bad_frames += 1;
                continue;
            }
            if self.buf.len() < len {
                break;
            }
            match decode(&self.buf[..len]) {
                Some(f) => {
                    out.push(f);
                    self.buf.drain(..len);
                }
                None => {
                    self.bad_frames += 1;
                    self.buf.drain(..1);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_live(reps: u8, force: f64) -> Vec<u8> {
        let mut f = vec![0xAA, 0xAB, 60, 0x80, 1, 0, reps, reps];
        for _ in 0..2 {
            f.push(2);
            for v in [0.25f64, -0.5, force] {
                f.extend_from_slice(&v.to_be_bytes());
            }
        }
        let c = crc16_x25(&f);
        f.extend_from_slice(&c.to_le_bytes());
        f
    }

    /// The exact frames the original logged on the tablet (logger.db, "下位机数据:发送…指令").
    #[test]
    fn commands_match_the_tablet_log() {
        assert_eq!(
            cmd_enable(3, 20.0, false, 5.0),
            vec![204, 205, 18, 1, 3, 64, 52, 0, 0, 0, 0, 0, 0, 0, 1, 5, 20, 117]
        );
        assert_eq!(
            cmd_enable(3, 60.0, false, 5.0),
            vec![204, 205, 18, 1, 3, 64, 78, 0, 0, 0, 0, 0, 0, 0, 1, 5, 10, 142]
        );
        assert_eq!(cmd_simple(CMD_PAUSE), vec![204, 205, 6, 3, 195, 157]);
        // not in the tablet log; computed independently with the same CRC (which reproduces
        // the logged pause above): the app's Sleep and wake use these
        assert_eq!(cmd_simple(CMD_DISABLE), vec![0xCC, 0xCD, 0x06, 0x02, 0xD2, 0x14]);
        assert_eq!(cmd_simple(CMD_SLEEP), vec![0xCC, 0xCD, 0x06, 0x08, 0x7D, 0x4E]);
        assert_eq!(cmd_simple(CMD_WAKEUP), vec![0xCC, 0xCD, 0x06, 0x09, 0x6C, 0xC7]);
        assert_eq!(cmd_heartbeat(256).len(), 7);
        assert_eq!(cmd_heartbeat(256)[4], 1);
        // barbell: total 40 lb is sent as 20 lb per side with code 2
        assert_eq!(&cmd_enable(1, 40.0, true, 0.0)[5..13], &20.0f64.to_be_bytes());
    }

    #[test]
    fn replies_decode_like_the_original() {
        // the 7-byte frame in the original's own recordings: AA AB 07 83 01 + crc
        let mut f = vec![0xAA, 0xAB, 7, TYPE_ACK, 1];
        let c = crc16_x25(&f);
        f.extend_from_slice(&c.to_le_bytes());
        assert_eq!(decode(&f), Some(Frame::Other { kind: TYPE_ACK, payload: vec![1] }));
        assert_eq!(protection_reason(5), "Motor temperature too high");
        assert_eq!(protection_reason(9), "Firmware runtime error");
    }

    #[test]
    fn two_player_frame_layout() {
        let f = cmd_enable_2player(true, 1, 22.0, 5.9);
        assert_eq!(f.len(), ENABLE_2P_LEN as usize);
        assert_eq!(&f[..5], &[0xCC, 0xCD, 17, 0xA2, 1]);
        assert_eq!(&f[5..13], &22.0f64.to_be_bytes());
        assert_eq!(f[13], 0);
        assert_eq!(f[14], 5); // truncated, like fcvtzs in 0x3acafec
        let c = crc16_x25(&f[..15]);
        assert_eq!(&f[15..], &[(c >> 8) as u8, c as u8]);
        assert_eq!(cmd_enable_2player(false, 1, 22.0, 0.0)[3], 0xA1);
    }

    #[test]
    fn side_values_like_the_original() {
        // distance = −round(b, 4); torque in lb, ÷ 2.2 for kg (0x1b9c29c)
        let s = SideData { a: 0.4521, b: -0.191_237, force_lb: 19.8 };
        assert!((s.distance_m() - 0.1912).abs() < 1e-9);
        assert!((s.force_kg() - 9.0).abs() < 1e-6);
        assert_eq!(frame_kind_name(0x8d), "skiing status");
    }

    #[test]
    fn crc_check_value() {
        // Standard CRC-16/X-25 check value for "123456789".
        assert_eq!(crc16_x25(b"123456789"), 0x906E);
    }

    #[test]
    fn decodes_live_frame_and_resyncs() {
        let mut s = Splitter::default();
        let mut bytes = vec![1, 2, 3, 0xAA];
        bytes.extend(build_live(4, 19.8));
        bytes.extend(build_live(5, 7.7));
        let frames = s.push(&bytes[..30]);
        assert!(frames.is_empty());
        let frames = s.push(&bytes[30..]);
        assert_eq!(frames.len(), 2);
        match &frames[0] {
            Frame::Live { reps_left, left, .. } => {
                assert_eq!(*reps_left, 4);
                assert!((left.force_lb - 19.8).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }
}

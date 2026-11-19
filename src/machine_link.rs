//! Two-way link to the Gym Monster's motor board.
//!
//! On the tablet this is `/dev/ttyS5` at 115200 baud (the port the original app holds open).
//! The link thread writes the original's commands (protocol.rs), sends the heartbeat every
//! HEARTBEAT_MS while the board is enabled (as closure 0x19910bc does), and decodes the
//! machine's live frames.

use crate::protocol::{self, Frame, Splitter};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// The motor board port on the tablet.
pub const TABLET_PORT: &str = "/dev/ttyS5";

pub enum Out {
    /// Write these bytes.
    Send(Vec<u8>),
    /// Wait this many milliseconds before the next queued item (the original sends the right
    /// cable's two-player frame 50 ms after the left one).
    Wait(u64),
    /// Start / stop the 500 ms heartbeat.
    Heartbeat(bool),
    /// Close the port. `weight_off` (or a running heartbeat): first send weight off + disable,
    /// which stops the heartbeat like the original's disable. `board_sleep` (the app's Sleep):
    /// always weight off + disable, then the board's own sleep command. See `closing_frames`.
    Close { weight_off: bool, board_sleep: bool },
}

pub enum In {
    Opened(String),
    Frame(Frame),
    Sent(Vec<u8>),
    Error(String),
    Closed,
}

pub struct MachineLink {
    pub port: String,
    tx: Sender<Out>,
    pub rx: Receiver<In>,
}

impl MachineLink {
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(Out::Send(bytes));
    }
    /// Send several frames, waiting `gap_ms` between them.
    pub fn send_spaced(&self, frames: &[Vec<u8>], gap_ms: u64) {
        for (i, f) in frames.iter().enumerate() {
            if i > 0 {
                let _ = self.tx.send(Out::Wait(gap_ms));
            }
            let _ = self.tx.send(Out::Send(f.clone()));
        }
    }
    pub fn heartbeat(&self, on: bool) {
        let _ = self.tx.send(Out::Heartbeat(on));
    }
    pub fn close(&self, weight_off: bool) {
        let _ = self.tx.send(Out::Close { weight_off, board_sleep: false });
    }
    /// The app's Sleep: weight off, disable, then put the motor board to sleep, and close.
    pub fn close_and_sleep(&self) {
        let _ = self.tx.send(Out::Close { weight_off: true, board_sleep: true });
    }
}

impl Drop for MachineLink {
    fn drop(&mut self) {
        let _ = self.tx.send(Out::Close { weight_off: false, board_sleep: false });
    }
}

/// Wait before the board's sleep command, after disable (the original sends sleep from an
/// idle machine, never straight after a weight-off; this gives the board a moment).
pub const SLEEP_AFTER_DISABLE_MS: u64 = 100;
/// Wait after the last closing frame before the port is closed, so it is out of the UART.
pub const CLOSE_DRAIN_MS: u64 = 50;

/// What is written when the port closes, in order (pause = weight off). Pinned by tests.
pub fn closing_frames(weight_off: bool, heartbeat: bool, board_sleep: bool) -> Vec<Vec<u8>> {
    let mut f = Vec::new();
    if weight_off || heartbeat || board_sleep {
        f.push(protocol::cmd_simple(protocol::CMD_PAUSE));
        f.push(protocol::cmd_simple(protocol::CMD_DISABLE));
    }
    if board_sleep {
        f.push(protocol::cmd_simple(protocol::CMD_SLEEP));
    }
    f
}

trait Port: Read + Write + Send {}
impl<T: Read + Write + Send> Port for T {}

#[cfg(unix)]
fn open_port(path: &str, _baud: u32) -> Result<Box<dyn Port>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("Couldn't open {path}: {e}"))?;
    // raw 8N1 at 115200, like the original's libserialport configuration
    unsafe {
        let fd = f.as_raw_fd();
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) == 0 {
            libc::cfmakeraw(&mut t);
            libc::cfsetispeed(&mut t, libc::B115200);
            libc::cfsetospeed(&mut t, libc::B115200);
            t.c_cflag |= libc::CLOCAL | libc::CREAD;
            libc::tcsetattr(fd, libc::TCSANOW, &t);
        }
    }
    Ok(Box::new(f))
}

#[cfg(not(unix))]
fn open_port(path: &str, _baud: u32) -> Result<Box<dyn Port>, String> {
    Err(format!("Couldn't open {path}: the machine is only reachable from the tablet"))
}

pub fn connect(port: &str, baud: u32) -> MachineLink {
    let (out_tx, out_rx) = channel::<Out>();
    let (in_tx, in_rx) = channel::<In>();
    let p = port.to_string();
    std::thread::spawn(move || run(p, baud, out_rx, in_tx));
    MachineLink { port: port.to_string(), tx: out_tx, rx: in_rx }
}

fn write_all(port: &mut Box<dyn Port>, bytes: &[u8], events: &Sender<In>) -> bool {
    let mut done = 0;
    let start = Instant::now();
    while done < bytes.len() {
        match port.write(&bytes[done..]) {
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > Duration::from_millis(200) {
                    let _ = events.send(In::Error("write timed out".into()));
                    return false;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => {
                let _ = events.send(In::Error(format!("write failed: {e}")));
                return false;
            }
        }
    }
    let _ = port.flush();
    let _ = events.send(In::Sent(bytes.to_vec()));
    true
}

fn run(path: String, baud: u32, out: Receiver<Out>, events: Sender<In>) {
    let mut port = match open_port(&path, baud) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(In::Error(e));
            let _ = events.send(In::Closed);
            return;
        }
    };
    let _ = events.send(In::Opened(path.clone()));
    let mut split = Splitter::default();
    let mut buf = [0u8; 1024];
    let mut heartbeat = false;
    let mut hb_counter: u32 = 0;
    let mut next_hb = Instant::now();
    loop {
        // outgoing
        loop {
            match out.try_recv() {
                Ok(Out::Send(b)) => {
                    write_all(&mut port, &b, &events);
                }
                Ok(Out::Wait(ms)) => {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                Ok(Out::Heartbeat(on)) => {
                    if on && !heartbeat {
                        next_hb = Instant::now() + Duration::from_millis(protocol::HEARTBEAT_MS);
                    }
                    heartbeat = on;
                }
                Ok(Out::Close { weight_off, board_sleep }) => {
                    // weight off, then disable (which stops the heartbeat in the original), then
                    // for the app's Sleep the board's own sleep
                    let frames = closing_frames(weight_off, heartbeat, board_sleep);
                    for f in &frames {
                        if f[3] == protocol::CMD_SLEEP {
                            std::thread::sleep(Duration::from_millis(SLEEP_AFTER_DISABLE_MS));
                        }
                        write_all(&mut port, f, &events);
                    }
                    if !frames.is_empty() {
                        std::thread::sleep(Duration::from_millis(CLOSE_DRAIN_MS));
                    }
                    let _ = events.send(In::Closed);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    if heartbeat {
                        write_all(&mut port, &protocol::cmd_simple(protocol::CMD_PAUSE), &events);
                        write_all(&mut port, &protocol::cmd_simple(protocol::CMD_DISABLE), &events);
                    }
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        // heartbeat
        if heartbeat && Instant::now() >= next_hb {
            let f = protocol::cmd_heartbeat(hb_counter);
            hb_counter = hb_counter.wrapping_add(1);
            if let Err(e) = port.write_all(&f) {
                let _ = events.send(In::Error(format!("heartbeat failed: {e}")));
            }
            next_hb += Duration::from_millis(protocol::HEARTBEAT_MS);
        }
        // incoming
        match port.read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(10)),
            Ok(n) => {
                for f in split.push(&buf[..n]) {
                    let _ = events.send(In::Frame(f));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = events.send(In::Error(format!("read failed: {e}")));
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::closing_frames;

    // bytes computed independently (CRC-16/X-25, checked against the tablet's logged pause)
    const PAUSE: [u8; 6] = [0xCC, 0xCD, 0x06, 0x03, 0xC3, 0x9D];
    const DISABLE: [u8; 6] = [0xCC, 0xCD, 0x06, 0x02, 0xD2, 0x14];
    const SLEEP: [u8; 6] = [0xCC, 0xCD, 0x06, 0x08, 0x7D, 0x4E];

    #[test]
    fn closing_sends_exactly_these_bytes() {
        // leaving the screen with the weight never used: nothing is sent
        assert!(closing_frames(false, false, false).is_empty());
        // leaving the screen after using the weight: weight off, then disable
        assert_eq!(closing_frames(true, false, false), vec![PAUSE.to_vec(), DISABLE.to_vec()]);
        assert_eq!(closing_frames(false, true, false), vec![PAUSE.to_vec(), DISABLE.to_vec()]);
        // the app's Sleep, whether or not the weight was used: weight off, disable, sleep
        for (w, h) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(closing_frames(w, h, true), vec![PAUSE.to_vec(), DISABLE.to_vec(), SLEEP.to_vec()]);
        }
    }
}

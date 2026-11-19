//! The tablet's serial ports (for the Machine window's port list).

/// The motor board's USB id, from UsbDriver.ScanDevices (classes7.dex):
/// vendor 0x0483, product 0x5740 (an STM32 USB serial port). On the tablet the board is
/// reached through /dev/ttyS5 instead.
#[allow(dead_code)]
pub const MACHINE_VID: u16 = 0x0483;
#[allow(dead_code)]
pub const MACHINE_PID: u16 = 0x5740;

pub struct PortInfo {
    pub name: String,
    pub is_machine: bool,
}

/// The serial devices the original app uses (`/dev/ttyS3` … `/dev/ttyS6` strings in libapp.so)
/// that exist on this device; the original holds /dev/ttyS5 (115200 baud) for the motor board.
pub fn ports() -> Vec<PortInfo> {
    ["/dev/ttyS3", "/dev/ttyS4", "/dev/ttyS5", "/dev/ttyS6", "/dev/ttyACM0"]
        .iter()
        .filter(|p| std::path::Path::new(p).exists())
        .map(|p| PortInfo { name: p.to_string(), is_machine: *p == crate::machine_link::TABLET_PORT })
        .collect()
}

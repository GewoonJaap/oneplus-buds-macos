//! OPOv1 framing used by HeyMelody-compatible earbuds over BLE GATT.
//!
//! Frame: `AA <varint len> 00 00 <cmd_lo> <cmd_hi> <seq> <len_lo> <len_hi> <payload>`
//! where `len` (varint) = TLV length + 2.
//!
//! Verified against a OnePlus Buds Pro 3 (see tests for real captures).

pub const SERVICE: &str = "0000079A-D102-11E1-9B23-00025B00A5A5";
pub const WRITE_CHAR: &str = "0100079A-D102-11E1-9B23-00025B00A5A5";
pub const NOTIFY_CHAR: &str = "0200079A-D102-11E1-9B23-00025B00A5A5";

/// Default token from public notes; the buds accepted it.
pub const DEFAULT_TOKEN: [u8; 4] = [0xB5, 0x50, 0xA0, 0x69];

pub const CMD_NOISE_REPLY: u16 = 0x810C;
pub const CMD_BATTERY_REPLY: u16 = 0x8106;
pub const CMD_EVENT: u16 = 0x0204;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Mode {
    Off,
    Transparency,
    /// Strongest fixed ANC
    High,
    Medium,
    Low,
    /// Adaptive ANC
    Adaptive,
}

impl Mode {
    pub const ALL: [Mode; 6] = [Mode::Off, Mode::Transparency, Mode::High, Mode::Medium, Mode::Low, Mode::Adaptive];

    /// Byte to send in the set-noise-control command (measured on real hardware).
    pub fn set_byte(self) -> u8 {
        match self {
            Mode::Off => 0x08,
            Mode::Transparency => 0x04,
            Mode::High => 0x10,
            Mode::Medium => 0x20,
            Mode::Low => 0x40,
            Mode::Adaptive => 0x80,
        }
    }

    /// Decode the 16-bit mode word the buds report.
    pub fn from_word(w: u16) -> Option<Mode> {
        Some(match w {
            0x0008 | 0x0001 => Mode::Off,
            0x0100 | 0x0004 | 0x0002 => Mode::Transparency,
            0x0010 => Mode::High,
            0x0020 => Mode::Medium,
            0x0040 => Mode::Low,
            0x0080 => Mode::Adaptive,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Transparency => "Transparency",
            Mode::High => "Noise cancelling: High",
            Mode::Medium => "Noise cancelling: Medium",
            Mode::Low => "Noise cancelling: Low",
            Mode::Adaptive => "Noise cancelling: Adaptive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eq {
    Balanced = 0,
    Bold = 1,
    Serenade = 2,
    BassBoost = 3,
    DynAudio = 7,
}

impl Eq {
    pub const ALL: [Eq; 5] = [Eq::Balanced, Eq::Bold, Eq::Serenade, Eq::BassBoost, Eq::DynAudio];

    pub fn from_id(id: u8) -> Option<Eq> {
        Self::ALL.into_iter().find(|e| *e as u8 == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Eq::Balanced => "Balanced",
            Eq::Bold => "Bold",
            Eq::Serenade => "Serenade",
            Eq::BassBoost => "Bass boost",
            Eq::DynAudio => "Dynaudio",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spatial {
    Off = 0,
    Fixed = 1,
    HeadTracked = 2,
}

impl Spatial {
    pub const ALL: [Spatial; 3] = [Spatial::Off, Spatial::Fixed, Spatial::HeadTracked];

    pub fn label(self) -> &'static str {
        match self {
            Spatial::Off => "Off",
            Spatial::Fixed => "Fixed",
            Spatial::HeadTracked => "Head tracked",
        }
    }
}

pub struct Builder {
    seq: u8,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self { seq: 1 }
    }

    fn next_seq(&mut self) -> u8 {
        let s = self.seq;
        self.seq = (self.seq + 1) & 0x7f;
        if self.seq == 0 {
            self.seq = 1;
        }
        s
    }

    pub fn wrap(tlv: &[u8]) -> Vec<u8> {
        let mut v = tlv.len() + 2;
        let mut out = vec![0xAA];
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                out.push(b | 0x80);
            } else {
                out.push(b);
                break;
            }
        }
        out.extend([0, 0]);
        out.extend(tlv);
        out
    }

    pub fn hello(&mut self) -> Vec<u8> {
        vec![0xAA, 0x07, 0, 0, 0x00, 0x01, self.next_seq(), 0, 0, 0x12]
    }

    pub fn register(&mut self, token: [u8; 4]) -> Vec<u8> {
        let mut v = vec![0xAA, 0x0C, 0, 0, 0x00, 0x85, self.next_seq(), 0x05, 0, 0];
        v.extend(token);
        v
    }

    pub fn battery_query(&mut self) -> Vec<u8> {
        Self::wrap(&[0x06, 0x01, self.next_seq(), 0, 0])
    }

    pub fn raw(&mut self, lo: u8, hi: u8, payload: &[u8]) -> Vec<u8> {
        let mut t = vec![lo, hi, self.next_seq(), payload.len() as u8, 0];
        t.extend(payload);
        Self::wrap(&t)
    }

    pub fn eq_query(&mut self) -> Vec<u8> {
        self.raw(0x0F, 0x01, &[])
    }

    pub fn eq_set(&mut self, e: Eq) -> Vec<u8> {
        self.raw(0x06, 0x04, &[e as u8])
    }

    pub fn spatial_set(&mut self, m: Spatial) -> Vec<u8> {
        self.raw(0x22, 0x04, &[m as u8])
    }

    pub fn noise_query(&mut self) -> Vec<u8> {
        Self::wrap(&[0x0C, 0x01, self.next_seq(), 0x02, 0, 0x01, 0x01])
    }

    pub fn noise_set(&mut self, mode: Mode) -> Vec<u8> {
        self.noise_set_raw(mode.set_byte())
    }

    pub fn noise_set_raw(&mut self, byte: u8) -> Vec<u8> {
        Self::wrap(&[0x04, 0x04, self.next_seq(), 0x03, 0, 0x01, 0x01, byte])
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub cmd: u16,
    pub seq: u8,
    pub payload: Vec<u8>,
}

pub fn parse(raw: &[u8]) -> Option<Frame> {
    if raw.len() < 4 || raw[0] != 0xAA {
        return None;
    }
    let mut i = 1;
    while i < raw.len() && raw[i] & 0x80 != 0 {
        i += 1;
    }
    let tlv = raw.get(i + 3..)?;
    if tlv.len() < 5 {
        return None;
    }
    let cmd = u16::from_le_bytes([tlv[0], tlv[1]]);
    let len = u16::from_le_bytes([tlv[3], tlv[4]]) as usize;
    Some(Frame { cmd, seq: tlv[2], payload: tlv.get(5..(5 + len).min(tlv.len()))?.to_vec() })
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Cell {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Battery {
    pub left: Option<Cell>,
    pub right: Option<Cell>,
    pub case: Option<Cell>,
}

/// Payload: `00 <count> (<dev> <level|0x80=charging>)*` with dev 1=left, 2=right, 3=case.
/// Layout confirmed on real Buds Pro 3 (left+right entries); the case entry follows the public notes.
pub fn parse_battery(p: &[u8]) -> Battery {
    let mut b = Battery::default();
    let count = p.get(1).copied().unwrap_or(0) as usize;
    for i in 0..count {
        let (Some(&dev), Some(&raw)) = (p.get(2 + i * 2), p.get(3 + i * 2)) else { break };
        let cell = Some(Cell { percent: raw & 0x7f, charging: raw & 0x80 != 0 });
        match dev {
            1 => b.left = cell,
            2 => b.right = cell,
            3 => b.case = cell,
            _ => {}
        }
    }
    b
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Current noise-control mode (reply to a query, or a pushed change).
    Noise(Option<Mode>, u16),
    Battery(Battery),
    Other(u16, Vec<u8>),
}

pub fn decode(raw: &[u8]) -> Option<Event> {
    let f = parse(raw)?;
    match f.cmd {
        CMD_NOISE_REPLY if f.payload.len() >= 5 => {
            let w = u16::from_le_bytes([f.payload[3], f.payload[4]]);
            Some(Event::Noise(Mode::from_word(w), w))
        }
        // Pushed events: payload = 03 <sub> 01 <word lo> <word hi>
        CMD_EVENT if f.payload.len() == 5 && f.payload[0] == 0x03 => {
            let w = u16::from_le_bytes([f.payload[3], f.payload[4]]);
            Some(Event::Noise(Mode::from_word(w), w))
        }
        CMD_BATTERY_REPLY => Some(Event::Battery(parse_battery(&f.payload))),
        c => Some(Event::Other(c, f.payload)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    #[test]
    fn builders_match_captured_frames() {
        let mut b = Builder::new();
        assert_eq!(b.hello(), h("aa070000000101000012"));
        assert_eq!(b.register(DEFAULT_TOKEN), h("aa0c0000008502050000b550a069"));
        assert_eq!(b.noise_query(), h("aa0900000c010302000101"));
        assert_eq!(b.battery_query(), h("aa0700000601040000"));
    }

    #[test]
    fn set_frame_shape() {
        let mut b = Builder::new();
        b.hello();
        assert_eq!(b.noise_set(Mode::Medium), h("aa0a00000404020300010120"));
    }

    #[test]
    fn decodes_real_noise_replies() {
        // after SET 08 / 10 / 20 / 40 / 80 on real Buds Pro 3
        for (frame, mode) in [
            ("aa0c00000c810605000001010800", Mode::Off),
            ("aa0c00000c810605000001011000", Mode::High),
            ("aa0c00000c810605000001012000", Mode::Medium),
            ("aa0c00000c810605000001014000", Mode::Low),
            ("aa0c00000c810605000001018000", Mode::Adaptive),
            ("aa0c00000c810605000001010001", Mode::Transparency),
        ] {
            match decode(&h(frame)) {
                Some(Event::Noise(Some(m), _)) => assert_eq!(m, mode, "{frame}"),
                other => panic!("{frame}: {other:?}"),
            }
        }
    }

    #[test]
    fn decodes_real_battery_reply() {
        match decode(&h("aa0d00000681040600000201460246")) {
            Some(Event::Battery(b)) => {
                assert_eq!(b.left, Some(Cell { percent: 70, charging: false }));
                assert_eq!(b.right, Some(Cell { percent: 70, charging: false }));
                assert_eq!(b.case, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn battery_charging_bit() {
        let b = parse_battery(&[0, 1, 3, 0x80 | 55]);
        assert_eq!(b.case, Some(Cell { percent: 55, charging: true }));
    }

    #[test]
    fn decodes_pushed_event() {
        assert!(matches!(decode(&h("aa0c000004021c05000304014000")), Some(Event::Noise(Some(Mode::Low), 0x40))));
    }

    #[test]
    fn set_byte_roundtrips_through_report_word() {
        for m in [Mode::High, Mode::Medium, Mode::Low, Mode::Adaptive, Mode::Off] {
            assert_eq!(Mode::from_word(m.set_byte() as u16), Some(m));
        }
    }
}

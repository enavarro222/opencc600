//! The BLE GATT measurements of the Bluetooth SIG cycling profiles, decoded from their notifications.

/// Services (16-bit UUIDs) and their measurement characteristics.
pub const HEART_RATE: u16 = 0x180D;
pub const HEART_RATE_MEASUREMENT: u16 = 0x2A37;
pub const SPEED_CADENCE: u16 = 0x1816;
pub const CSC_MEASUREMENT: u16 = 0x2A5B;
pub const POWER: u16 = 0x1818;
pub const POWER_MEASUREMENT: u16 = 0x2A63;

/// Heart Rate Measurement (0x2A37): the rate in bpm (flags bit 0: 16 bits); `None` if too short, 0 or above 255.
pub fn heart_rate(data: &[u8]) -> Option<u8> {
    let flags = *data.first()?;
    let bpm = if flags & 0x01 != 0 {
        u16::from_le_bytes([*data.get(1)?, *data.get(2)?])
    } else {
        *data.get(1)? as u16
    };
    u8::try_from(bpm).ok().filter(|&b| b > 0)
}

/// A revolution count and the time of its last event (1/1024 s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RevEvent {
    pub revs: u32,
    pub time: u16,
}

/// CSC Measurement (0x2A5B): wheel (u32 revolutions) and crank (u16) data, each if present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Csc {
    pub wheel: Option<RevEvent>,
    pub crank: Option<RevEvent>,
}

pub fn csc(data: &[u8]) -> Option<Csc> {
    let mut r = Reader(data);
    let flags = r.u8()?;
    let wheel = if flags & 0x01 != 0 {
        Some(RevEvent { revs: r.u32()?, time: r.u16()? })
    } else {
        None
    };
    let crank = if flags & 0x02 != 0 {
        Some(RevEvent { revs: r.u16()? as u32, time: r.u16()? })
    } else {
        None
    };
    Some(Csc { wheel, crank })
}

/// Cycling Power Measurement (0x2A63): the instant power and, if present, the crank revolutions (cadence).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Power {
    pub watts: u16,
    pub crank: Option<RevEvent>,
}

pub fn power(data: &[u8]) -> Option<Power> {
    let mut r = Reader(data);
    let flags = r.u16()?;
    // a negative power (a trainer braking) is no power
    let watts = r.u16()? as i16 as i32;
    // the optional fields before the crank data, in their order: pedal balance (u8), accumulated torque (u16),
    // wheel revolutions (u32 + u16 time)
    if flags & 0x0001 != 0 {
        r.skip(1)?;
    }
    if flags & 0x0004 != 0 {
        r.skip(2)?;
    }
    if flags & 0x0010 != 0 {
        r.skip(6)?;
    }
    let crank = if flags & 0x0020 != 0 {
        Some(RevEvent { revs: r.u16()? as u32, time: r.u16()? })
    } else {
        None
    };
    Some(Power { watts: watts.max(0) as u16, crank })
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*head)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.0 = self.0.get(n..)?;
        Some(())
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take()?))
    }
}

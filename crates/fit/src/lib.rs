//! FIT activity file writer, hardware independent, `no_std` without allocation.
//!
//! A FIT file is a 14-byte [`header`], records, then a CRC of everything before it. Records are definition
//! messages (layout of a local message type) and data messages. This crate writes one fixed layout per message
//! ([`DEFINITIONS`], written once at the start of the file) and the data messages of an activity: [`FileId`],
//! [`Event`], [`Record`], [`Lap`], [`Session`], [`Activity`].
//!
//! The data size in the header is only known at the end: the caller writes a placeholder header first, then
//! rewrites it with [`header`] and appends the [`Crc`] of the whole file when closing it.
//!
//! Units follow the FIT profile (scaled integers), field numbers are from the FIT SDK profile; the tests check
//! them by decoding with an independent decoder.

#![no_std]

/// Seconds between the Unix epoch and the FIT epoch (1989-12-31 00:00:00 UTC).
pub const FIT_EPOCH_OFFSET: u32 = 631_065_600;

/// FIT timestamp from Unix seconds.
pub const fn timestamp(unix: u32) -> u32 {
    unix - FIT_EPOCH_OFFSET
}

pub const HEADER_LEN: usize = 14;
/// FIT protocol 2.0, profile 21.40.
const PROTOCOL: u8 = 0x20;
const PROFILE: u16 = 2140;

/// File header for `data_size` bytes of records (everything between the header and the final CRC).
pub fn header(data_size: u32) -> [u8; HEADER_LEN] {
    let mut h = [0; HEADER_LEN];
    h[0] = HEADER_LEN as u8;
    h[1] = PROTOCOL;
    h[2..4].copy_from_slice(&PROFILE.to_le_bytes());
    h[4..8].copy_from_slice(&data_size.to_le_bytes());
    h[8..12].copy_from_slice(b".FIT");
    let crc = Crc::new().update(&h[..12]).value();
    h[12..14].copy_from_slice(&crc.to_le_bytes());
    h
}

/// FIT CRC (CRC-16/ARC), updated incrementally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Crc(u16);

impl Crc {
    pub const fn new() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn update(mut self, data: &[u8]) -> Self {
        const TABLE: [u16; 16] = [
            0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401, 0xA001, 0x6C00, 0x7800,
            0xB401, 0x5000, 0x9C01, 0x8801, 0x4400,
        ];
        for &b in data {
            let mut crc = self.0;
            let t = TABLE[(crc & 0xF) as usize];
            crc = (crc >> 4) & 0x0FFF;
            crc = crc ^ t ^ TABLE[(b & 0xF) as usize];
            let t = TABLE[(crc & 0xF) as usize];
            crc = (crc >> 4) & 0x0FFF;
            crc = crc ^ t ^ TABLE[((b >> 4) & 0xF) as usize];
            self.0 = crc;
        }
        self
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

/// FIT base types used here.
mod base {
    pub const ENUM: u8 = 0x00;
    pub const SINT8: u8 = 0x01;
    pub const UINT8: u8 = 0x02;
    pub const UINT16: u8 = 0x84;
    pub const SINT32: u8 = 0x85;
    pub const UINT32: u8 = 0x86;
    pub const UINT32Z: u8 = 0x8C;
}

/// Invalid values ("no data") of each base type.
pub mod invalid {
    pub const UINT8: u8 = 0xFF;
    pub const SINT8: i8 = 0x7F;
    pub const UINT16: u16 = 0xFFFF;
    pub const SINT32: i32 = 0x7FFF_FFFF;
    pub const UINT32: u32 = 0xFFFF_FFFF;
}

/// One field of a definition: field number, size in bytes, base type.
#[derive(Debug, Clone, Copy)]
pub struct Field(pub u8, pub u8, pub u8);

/// Layout of a local message type.
#[derive(Debug, Clone, Copy)]
pub struct Definition {
    pub local: u8,
    pub global: u16,
    pub fields: &'static [Field],
}

impl Definition {
    /// Size of the data message, header byte included.
    pub const fn data_len(&self) -> usize {
        let mut n = 1;
        let mut i = 0;
        while i < self.fields.len() {
            n += self.fields[i].1 as usize;
            i += 1;
        }
        n
    }

    pub const fn encoded_len(&self) -> usize {
        6 + 3 * self.fields.len()
    }

    /// Writes the definition message into `out`, returns its length.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Full> {
        let n = self.encoded_len();
        let out = out.get_mut(..n).ok_or(Full)?;
        out[0] = 0x40 | self.local;
        out[1] = 0; // reserved
        out[2] = 0; // little endian
        out[3..5].copy_from_slice(&self.global.to_le_bytes());
        out[5] = self.fields.len() as u8;
        for (i, f) in self.fields.iter().enumerate() {
            out[6 + 3 * i..9 + 3 * i].copy_from_slice(&[f.0, f.1, f.2]);
        }
        Ok(n)
    }
}

/// The output buffer is too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// Little-endian field writer for one data message (see [`Message::encode`]).
pub struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Out<'_> {
    fn put(&mut self, b: &[u8]) {
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
    }
    fn u8(&mut self, v: u8) {
        self.put(&[v]);
    }
    fn i8(&mut self, v: i8) {
        self.put(&v.to_le_bytes());
    }
    fn u16(&mut self, v: u16) {
        self.put(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.put(&v.to_le_bytes());
    }
}

/// Little-endian reader over the fields of one data message.
struct In<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl In<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let b = self.buf[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        b
    }
    fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    fn i8(&mut self) -> i8 {
        self.u8() as i8
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }
}

/// `msg` as a data message of `def` (header byte included), or None.
fn data_of<'a>(def: &Definition, msg: &'a [u8]) -> Option<In<'a>> {
    (msg.len() == def.data_len() && msg[0] == def.local).then_some(In { buf: msg, pos: 1 })
}

/// A data message with a fixed [`Definition`].
pub trait Message {
    const DEFINITION: Definition;
    fn fields(&self, out: &mut Out<'_>);

    /// Writes the data message into `out`, returns its length.
    fn encode(&self, out: &mut [u8]) -> Result<usize, Full> {
        let n = Self::DEFINITION.data_len();
        let buf = out.get_mut(..n).ok_or(Full)?;
        buf[0] = Self::DEFINITION.local;
        let mut o = Out { buf, len: 1 };
        self.fields(&mut o);
        debug_assert_eq!(o.len, n);
        Ok(n)
    }
}

/// Every definition, in local type order: written once at the start of the file.
pub const DEFINITIONS: [Definition; 6] = [
    FileId::DEFINITION,
    Event::DEFINITION,
    Record::DEFINITION,
    Lap::DEFINITION,
    Session::DEFINITION,
    Activity::DEFINITION,
];

/// Length of the record starting with header byte `header`, for a file whose definitions are [`DEFINITIONS`]:
/// a definition (`next` = its field count byte) or a data message. None for anything else.
pub fn record_len(header: u8, field_count: Option<u8>) -> Option<usize> {
    if header & 0x80 != 0 {
        return None; // compressed timestamp headers are not written
    }
    let def = DEFINITIONS.get((header & 0x0F) as usize)?;
    if header & 0x40 != 0 {
        Some(6 + 3 * field_count? as usize)
    } else if header & 0x30 == 0 {
        Some(def.data_len())
    } else {
        None
    }
}

/// Writes [`DEFINITIONS`] into `out`, returns the length.
pub fn encode_definitions(out: &mut [u8]) -> Result<usize, Full> {
    let mut n = 0;
    for d in DEFINITIONS {
        n += d.encode(out.get_mut(n..).ok_or(Full)?)?;
    }
    Ok(n)
}

/// `manufacturer` value for a device without a FIT manufacturer id.
pub const MANUFACTURER_DEVELOPMENT: u16 = 255;

/// `file_id`: first message of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    pub serial_number: u32,
    /// FIT timestamp.
    pub time_created: u32,
}

impl Message for FileId {
    const DEFINITION: Definition = Definition {
        local: 0,
        global: 0,
        fields: &[
            Field(0, 1, base::ENUM),    // type
            Field(1, 2, base::UINT16),  // manufacturer
            Field(2, 2, base::UINT16),  // product
            Field(3, 4, base::UINT32Z), // serial_number
            Field(4, 4, base::UINT32),  // time_created
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        o.u8(4); // activity
        o.u16(MANUFACTURER_DEVELOPMENT);
        o.u16(600);
        o.u32(self.serial_number);
        o.u32(self.time_created);
    }
}

/// Timer start / stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub timestamp: u32,
    pub kind: EventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Start,
    /// Pause: the timer stops, the ride goes on.
    Stop,
    /// End of the ride.
    StopAll,
}

impl Event {
    /// Reads back a data message written by [`Message::encode`].
    pub fn decode(msg: &[u8]) -> Option<Self> {
        let mut i = data_of(&Self::DEFINITION, msg)?;
        let timestamp = i.u32();
        let _event = i.u8();
        let kind = match i.u8() {
            0 => EventType::Start,
            1 => EventType::Stop,
            4 => EventType::StopAll,
            _ => return None,
        };
        Some(Self { timestamp, kind })
    }
}

impl Message for Event {
    const DEFINITION: Definition = Definition {
        local: 1,
        global: 21,
        fields: &[
            Field(253, 4, base::UINT32), // timestamp
            Field(0, 1, base::ENUM),     // event
            Field(1, 1, base::ENUM),     // event_type
            Field(4, 1, base::UINT8),    // event_group
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        o.u32(self.timestamp);
        o.u8(0); // timer
        o.u8(match self.kind {
            EventType::Start => 0,
            EventType::Stop => 1,
            EventType::StopAll => 4,
        });
        o.u8(0);
    }
}

/// One sample, normally every second. `None` fields are written as FIT invalid values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Record {
    pub timestamp: u32,
    /// Degrees × 10⁷ (as `gnss::Fix`), converted to semicircles.
    pub lat_e7: Option<i32>,
    pub lon_e7: Option<i32>,
    /// Decimetres.
    pub altitude_dm: Option<i32>,
    pub speed_mm_s: Option<u32>,
    /// Centimetres since the start.
    pub distance_cm: Option<u32>,
    pub temperature_c: Option<i8>,
    pub heart_rate: Option<u8>,
    pub cadence: Option<u8>,
    pub power_w: Option<u16>,
}

/// Degrees × 10⁷ → semicircles (2³¹ semicircles = 180°), rounded.
pub fn semicircles(deg_e7: i32) -> i32 {
    let v = deg_e7 as i64 * (1i64 << 31);
    let d = 1_800_000_000i64;
    (if v >= 0 {
        (v + d / 2) / d
    } else {
        (v - d / 2) / d
    }) as i32
}

/// Semicircles → degrees × 10⁷, rounded.
pub fn from_semicircles(sc: i32) -> i32 {
    let v = sc as i64 * 1_800_000_000;
    let d = 1i64 << 31;
    (if v >= 0 {
        (v + d / 2) / d
    } else {
        (v - d / 2) / d
    }) as i32
}

/// Decimetres → FIT `enhanced_altitude` (scale 5, offset 500 m): 0.2 m steps, rounded down.
fn altitude(dm: i32) -> u32 {
    (dm as i64 + 5000)
        .div_euclid(2)
        .clamp(0, u32::MAX as i64 - 1) as u32
}

fn valid<T: PartialEq>(v: T, invalid: T) -> Option<T> {
    (v != invalid).then_some(v)
}

impl Record {
    /// Reads back a data message written by [`Message::encode`]: what a decoder of the file sees (altitude in
    /// 0.2 m steps, positions through semicircles).
    pub fn decode(msg: &[u8]) -> Option<Self> {
        let mut i = data_of(&Self::DEFINITION, msg)?;
        Some(Self {
            timestamp: i.u32(),
            lat_e7: valid(i.i32(), invalid::SINT32).map(from_semicircles),
            lon_e7: valid(i.i32(), invalid::SINT32).map(from_semicircles),
            altitude_dm: valid(i.u32(), invalid::UINT32).map(|a| a as i32 * 2 - 5000),
            speed_mm_s: valid(i.u32(), invalid::UINT32),
            distance_cm: valid(i.u32(), invalid::UINT32),
            temperature_c: valid(i.i8(), invalid::SINT8),
            heart_rate: valid(i.u8(), invalid::UINT8),
            cadence: valid(i.u8(), invalid::UINT8),
            power_w: valid(i.u16(), invalid::UINT16),
        })
    }
}

impl Message for Record {
    const DEFINITION: Definition = Definition {
        local: 2,
        global: 20,
        fields: &[
            Field(253, 4, base::UINT32), // timestamp
            Field(0, 4, base::SINT32),   // position_lat
            Field(1, 4, base::SINT32),   // position_long
            Field(78, 4, base::UINT32),  // enhanced_altitude
            Field(73, 4, base::UINT32),  // enhanced_speed
            Field(5, 4, base::UINT32),   // distance
            Field(13, 1, base::SINT8),   // temperature
            Field(3, 1, base::UINT8),    // heart_rate
            Field(4, 1, base::UINT8),    // cadence
            Field(7, 2, base::UINT16),   // power
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        o.u32(self.timestamp);
        o.i32(self.lat_e7.map_or(invalid::SINT32, semicircles));
        o.i32(self.lon_e7.map_or(invalid::SINT32, semicircles));
        o.u32(self.altitude_dm.map_or(invalid::UINT32, altitude));
        o.u32(self.speed_mm_s.unwrap_or(invalid::UINT32));
        o.u32(self.distance_cm.unwrap_or(invalid::UINT32));
        o.i8(self.temperature_c.unwrap_or(invalid::SINT8));
        o.u8(self.heart_rate.unwrap_or(invalid::UINT8));
        o.u8(self.cadence.unwrap_or(invalid::UINT8));
        o.u16(self.power_w.unwrap_or(invalid::UINT16));
    }
}

/// Totals over a lap or the whole session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// FIT timestamps.
    pub start_time: u32,
    pub end_time: u32,
    /// Milliseconds: elapsed (wall clock) and timer (without pauses).
    pub elapsed_ms: u32,
    pub timer_ms: u32,
    pub distance_cm: u32,
    pub ascent_m: u16,
    pub descent_m: u16,
    pub max_speed_mm_s: u32,
}

impl Totals {
    /// Average moving speed, mm/s.
    pub fn avg_speed_mm_s(&self) -> u32 {
        if self.timer_ms == 0 {
            0
        } else {
            (self.distance_cm as u64 * 10_000 / self.timer_ms as u64) as u32
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lap(pub Totals);

impl Message for Lap {
    const DEFINITION: Definition = Definition {
        local: 3,
        global: 19,
        fields: &[
            Field(253, 4, base::UINT32), // timestamp
            Field(2, 4, base::UINT32),   // start_time
            Field(7, 4, base::UINT32),   // total_elapsed_time
            Field(8, 4, base::UINT32),   // total_timer_time
            Field(9, 4, base::UINT32),   // total_distance
            Field(110, 4, base::UINT32), // enhanced_avg_speed
            Field(111, 4, base::UINT32), // enhanced_max_speed
            Field(21, 2, base::UINT16),  // total_ascent
            Field(22, 2, base::UINT16),  // total_descent
            Field(0, 1, base::ENUM),     // event
            Field(1, 1, base::ENUM),     // event_type
            Field(25, 1, base::ENUM),    // sport
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        let t = &self.0;
        o.u32(t.end_time);
        o.u32(t.start_time);
        o.u32(t.elapsed_ms);
        o.u32(t.timer_ms);
        o.u32(t.distance_cm);
        o.u32(t.avg_speed_mm_s());
        o.u32(t.max_speed_mm_s);
        o.u16(t.ascent_m);
        o.u16(t.descent_m);
        o.u8(9); // lap
        o.u8(1); // stop
        o.u8(SPORT_CYCLING);
    }
}

const SPORT_CYCLING: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub totals: Totals,
    pub num_laps: u16,
}

impl Message for Session {
    const DEFINITION: Definition = Definition {
        local: 4,
        global: 18,
        fields: &[
            Field(253, 4, base::UINT32), // timestamp
            Field(2, 4, base::UINT32),   // start_time
            Field(7, 4, base::UINT32),   // total_elapsed_time
            Field(8, 4, base::UINT32),   // total_timer_time
            Field(9, 4, base::UINT32),   // total_distance
            Field(124, 4, base::UINT32), // enhanced_avg_speed
            Field(125, 4, base::UINT32), // enhanced_max_speed
            Field(22, 2, base::UINT16),  // total_ascent
            Field(23, 2, base::UINT16),  // total_descent
            Field(25, 2, base::UINT16),  // first_lap_index
            Field(26, 2, base::UINT16),  // num_laps
            Field(0, 1, base::ENUM),     // event
            Field(1, 1, base::ENUM),     // event_type
            Field(5, 1, base::ENUM),     // sport
            Field(6, 1, base::ENUM),     // sub_sport
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        let t = &self.totals;
        o.u32(t.end_time);
        o.u32(t.start_time);
        o.u32(t.elapsed_ms);
        o.u32(t.timer_ms);
        o.u32(t.distance_cm);
        o.u32(t.avg_speed_mm_s());
        o.u32(t.max_speed_mm_s);
        o.u16(t.ascent_m);
        o.u16(t.descent_m);
        o.u16(0);
        o.u16(self.num_laps);
        o.u8(8); // session
        o.u8(1); // stop
        o.u8(SPORT_CYCLING);
        o.u8(0); // generic
    }
}

/// Last message: the activity summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub timestamp: u32,
    /// Local time as a FIT timestamp (UTC + offset).
    pub local_timestamp: u32,
    pub timer_ms: u32,
}

impl Activity {
    /// True for an activity data message: the file is complete.
    pub fn is(msg: &[u8]) -> bool {
        data_of(&Self::DEFINITION, msg).is_some()
    }
}

impl Message for Activity {
    const DEFINITION: Definition = Definition {
        local: 5,
        global: 34,
        fields: &[
            Field(253, 4, base::UINT32), // timestamp
            Field(0, 4, base::UINT32),   // total_timer_time
            Field(1, 2, base::UINT16),   // num_sessions
            Field(2, 1, base::ENUM),     // type
            Field(3, 1, base::ENUM),     // event
            Field(4, 1, base::ENUM),     // event_type
            Field(5, 4, base::UINT32),   // local_timestamp
        ],
    };
    fn fields(&self, o: &mut Out<'_>) {
        o.u32(self.timestamp);
        o.u32(self.timer_ms);
        o.u16(1);
        o.u8(0); // manual
        o.u8(26); // activity
        o.u8(1); // stop
        o.u32(self.local_timestamp);
    }
}

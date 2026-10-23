//! The link with the companion app (Musette), hardware independent: what the app writes comes in ([`Session::on_write`]),
//! the answers go out ([`Session::take_output`]), in the frames of `protocol-cc600`. The firmware only moves the bytes
//! (BLE: the app writes the RX characteristic, the answers are notified on TX) and does what the [`Device`] asks.
//!
//! Handled: `Hello`, the settings (`GetSettings`, `SetSettings`, all or nothing) and files sent by the app (route,
//! map): kept in RAM while they arrive, resumed after a dropped link (not after a reboot), checked (size, CRC, then
//! the format) before the device gets them. The rides: listed, read in pieces, acknowledged once the app has them
//! (with the CRC of the whole file, when the app gives one: checked first).
//!
//! The rides are in the device's storage, which the firmware's main loop owns: the [`Device`]'s ride methods are
//! async, and so is [`Session::on_write`].

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use protocol_cc600::payload::{ActivityEntry, DeviceInfo, ReadActivity};
use protocol_cc600::transfer::{self, Begin, Chunk, Kind, Partial, Receiver};
use protocol_cc600::{
    CRC16_INIT, ErrorCode, Frame, HEADER_LEN, MAX_FRAME, MAX_PAYLOAD, Op, TRAILER_LEN, crc16_update,
};

/// File bytes the app may send before an acknowledgement: the acks, not the radio, set the speed. Each chunk takes a
/// packet of the BLE host's pool (`trouble-host`, shared with the sensors; firmware/Cargo.toml). Measured on a CC600
/// from BlueZ, 42 KB, MTU 247: 1 KB with 16 packets, 4.2 KB/s; 2 KB with 32 packets and an L2CAP queue of 16,
/// 10 KB/s; 4 KB ran the pool out (`OutOfMemory`, the writes lost and sent again: slower).
pub const WINDOW: u16 = 2048;

/// What the device does for the link.
#[allow(async_fn_in_trait)]
pub trait Device {
    fn settings(&self) -> settings::Settings;
    /// Settings changed by the app (already checked): to apply and save.
    fn set_settings(&mut self, s: settings::Settings);
    /// The largest file of this kind the device takes now, or why it takes none.
    fn max_file(&self, kind: Kind) -> Result<u32, ErrorCode>;
    /// Bytes free for rides, `None` before the storage was read.
    fn storage_free(&self) -> Option<u32>;
    /// A complete file, its size, CRC and format checked: to store and use.
    fn file_received(&mut self, kind: Kind, data: Vec<u8>) -> Result<(), ErrorCode>;
    /// The finished rides, in the order the app lists them.
    async fn rides(&mut self) -> Result<Vec<ActivityEntry>, ErrorCode>;
    /// Up to `max` bytes of ride `id` from `offset`: fewer at the end of the file, none past it.
    async fn read_ride(&mut self, id: u32, offset: u32, max: u16) -> Result<Vec<u8>, ErrorCode>;
    /// The app has ride `id`: marked as sent, the device may free it.
    async fn ride_sent(&mut self, id: u32) -> Result<(), ErrorCode>;
}

/// What happened, for the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A file is arriving: bytes held so far, of `size`.
    Receiving {
        kind: Kind,
        received: u32,
        size: u32,
    },
    Received(Kind),
    SettingsChanged,
}

/// One app at a time; it outlives the connections, so that a file resumes after a dropped link.
pub struct Session {
    input: Vec<u8>,
    output: Vec<u8>,
    receiver: Receiver,
    file: Vec<u8>,
    partial: Option<Partial>,
    events: Vec<Event>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub const fn new() -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            receiver: Receiver::new(WINDOW),
            file: Vec::new(),
            partial: None,
            events: Vec::new(),
        }
    }

    /// Bytes the app wrote (a frame may come in several writes, a write may hold several frames). Returns what
    /// happened; the answers wait in [`Self::take_output`].
    pub async fn on_write(&mut self, bytes: &[u8], device: &mut impl Device) -> Vec<Event> {
        self.input.extend_from_slice(bytes);
        loop {
            match Frame::decode(&self.input) {
                Ok((frame, n)) => {
                    let (op, seq) = (frame.op, frame.seq);
                    // the payload is copied out: answering needs `self`
                    let payload: Vec<u8> = frame.payload.into();
                    self.input.drain(..n);
                    self.handle(op, seq, &payload, device).await;
                }
                Err(protocol_cc600::Error::Incomplete { .. }) => break,
                Err(protocol_cc600::Error::UnknownOp(_)) => {
                    let seq = self.input[2];
                    self.skip_frame();
                    self.reply(Op::Error, seq, &[ErrorCode::BadRequest as u8]);
                }
                // a damaged frame: dropped, the app asks again (a lost chunk is resent)
                Err(protocol_cc600::Error::BadCrc { .. }) => self.skip_frame(),
                Err(_) => self.input.clear(),
            }
        }
        if self.input.len() > MAX_FRAME {
            self.input.clear();
        }
        core::mem::take(&mut self.events)
    }

    /// The answers, to send in order (split as the link needs: the app reassembles).
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.output)
    }

    /// The link dropped: a file being received stays, to resume.
    pub fn disconnected(&mut self) {
        self.receiver.abort();
        self.input.clear();
        self.output.clear();
    }

    /// The file being received, if any: kind, bytes held, size.
    pub fn receiving(&self) -> Option<(Kind, u32, u32)> {
        self.partial
            .map(|p| (p.begin.kind, p.received, p.begin.size))
    }

    fn skip_frame(&mut self) {
        let len =
            HEADER_LEN + u16::from_le_bytes([self.input[3], self.input[4]]) as usize + TRAILER_LEN;
        self.input.drain(..len.min(self.input.len()));
    }

    fn reply(&mut self, op: Op, seq: u8, payload: &[u8]) {
        let start = self.output.len();
        let frame = Frame::new(op, seq, &payload[..payload.len().min(MAX_PAYLOAD)]);
        self.output.resize(start + frame.encoded_len(), 0);
        let _ = frame.encode(&mut self.output[start..]);
    }

    fn error(&mut self, seq: u8, code: ErrorCode) {
        self.reply(Op::Error, seq, &[code as u8]);
    }

    async fn handle(&mut self, op: Op, seq: u8, payload: &[u8], device: &mut impl Device) {
        match op {
            Op::Hello => {
                // the room for each file, for the app to make one that fits (0: no room now); asked again before
                // each file
                let info = DeviceInfo {
                    route_max: device.max_file(Kind::Route).unwrap_or(0),
                    map_max: device.max_file(Kind::Map).unwrap_or(0),
                    free: device.storage_free().unwrap_or(DeviceInfo::UNKNOWN),
                };
                self.reply(Op::DeviceInfo, seq, &info.to_bytes())
            }
            Op::GetSettings => {
                let mut text = String::new();
                let _ = device.settings().write(&mut text);
                self.reply(Op::Settings, seq, text.as_bytes());
            }
            Op::SetSettings => {
                let Ok(text) = core::str::from_utf8(payload) else {
                    return self.error(seq, ErrorCode::BadRequest);
                };
                match device.settings().apply(text) {
                    Ok(s) => {
                        device.set_settings(s);
                        self.events.push(Event::SettingsChanged);
                        self.reply(Op::Ok, seq, &[]);
                    }
                    Err((key, _)) => {
                        let mut p = Vec::with_capacity(1 + key.len());
                        p.push(ErrorCode::BadSetting as u8);
                        p.extend_from_slice(key.as_bytes());
                        self.reply(Op::Error, seq, &p);
                    }
                }
            }
            Op::TransferBegin => self.begin(seq, payload, device),
            Op::TransferChunk => self.chunk(seq, payload),
            Op::TransferEnd => self.end(seq, device),
            Op::TransferAbort => {
                self.receiver.abort();
                self.partial = None;
                self.file = Vec::new();
                self.reply(Op::Ok, seq, &[]);
            }
            Op::ListActivities => match device.rides().await {
                Ok(rides) => {
                    for r in rides {
                        self.reply(Op::ActivityEntry, seq, &r.to_bytes());
                    }
                    self.reply(Op::EndOfList, seq, &[]);
                }
                Err(code) => self.error(seq, code),
            },
            Op::ReadActivity => {
                let Some(r) = ReadActivity::from_bytes(payload) else {
                    return self.error(seq, ErrorCode::BadRequest);
                };
                let max = r.max.min(MAX_PAYLOAD as u16);
                match device.read_ride(r.id, r.offset, max).await {
                    Ok(data) => self.reply(Op::ActivityChunk, seq, &data),
                    Err(code) => self.error(seq, code),
                }
            }
            Op::AckActivity => self.ack(seq, payload, device).await,
            // answers to the app's requests are not requests
            _ => self.error(seq, ErrorCode::BadRequest),
        }
    }

    /// `AckActivity`: id, then the CRC of the file as the app received it (0: not given, Musette's for now).
    async fn ack(&mut self, seq: u8, payload: &[u8], device: &mut impl Device) {
        let Some((id, crc)) = payload.first_chunk::<4>().map(|id| {
            (
                u32::from_le_bytes(*id),
                payload
                    .get(4..6)
                    .map_or(0, |c| u16::from_le_bytes([c[0], c[1]])),
            )
        }) else {
            return self.error(seq, ErrorCode::BadRequest);
        };
        if crc != 0 {
            match ride_crc(device, id).await {
                Ok(c) if c == crc => {}
                Ok(_) => return self.error(seq, ErrorCode::BadCrc),
                Err(code) => return self.error(seq, code),
            }
        }
        match device.ride_sent(id).await {
            Ok(()) => self.reply(Op::Ok, seq, &[]),
            Err(code) => self.error(seq, code),
        }
    }

    fn begin(&mut self, seq: u8, payload: &[u8], device: &mut impl Device) {
        let Some(begin) = Begin::from_bytes(payload) else {
            return self.error(seq, ErrorCode::BadRequest);
        };
        let max = match device.max_file(begin.kind) {
            Ok(max) => max,
            Err(code) => return self.error(seq, code),
        };
        match self.receiver.begin(begin, self.partial, max) {
            Ok(ready) => {
                if ready.next == 0 {
                    // another file: the partial one goes
                    self.file = Vec::new();
                }
                self.file.truncate(ready.next as usize);
                // the whole file at once, not grown chunk by chunk (each growth holds two copies a moment): a file
                // the memory cannot hold is refused here, not a panic halfway
                let more = (begin.size as usize).saturating_sub(self.file.len());
                if self.file.try_reserve_exact(more).is_err() {
                    // nothing held: the transfer the receiver just began goes too
                    self.file = Vec::new();
                    self.receiver = Receiver::new(WINDOW);
                    self.partial = None;
                    return self.error(seq, ErrorCode::NoSpace);
                }
                self.partial = self.receiver.progress();
                self.events.push(Event::Receiving {
                    kind: begin.kind,
                    received: ready.next,
                    size: begin.size,
                });
                self.reply(Op::TransferReady, seq, &ready.to_bytes());
            }
            Err(code) => self.error(seq, code),
        }
    }

    fn chunk(&mut self, seq: u8, payload: &[u8]) {
        let Some((offset, data)) = transfer::parse_chunk(payload) else {
            return self.error(seq, ErrorCode::BadRequest);
        };
        match self.receiver.chunk(offset, data) {
            Ok(Chunk::Accept(ack)) => {
                self.file.extend_from_slice(data);
                self.partial = self.receiver.progress();
                if let Some(ack) = ack {
                    if let Some(p) = self.partial {
                        self.events.push(Event::Receiving {
                            kind: p.begin.kind,
                            received: p.received,
                            size: p.begin.size,
                        });
                    }
                    self.reply(Op::TransferAck, seq, &ack.to_bytes());
                }
            }
            Ok(Chunk::Resend(at)) => self.reply(Op::TransferResend, seq, &at.to_bytes()),
            Ok(Chunk::Ignore) => {}
            Err(code) => self.error(seq, code),
        }
    }

    fn end(&mut self, seq: u8, device: &mut impl Device) {
        match self.receiver.end() {
            Ok(begin) => {
                self.partial = None;
                let data = core::mem::take(&mut self.file);
                let result =
                    check(begin.kind, &data).and_then(|()| device.file_received(begin.kind, data));
                match result {
                    Ok(()) => {
                        self.events.push(Event::Received(begin.kind));
                        self.reply(Op::Ok, seq, &[]);
                    }
                    Err(code) => self.error(seq, code),
                }
            }
            Err(code) => {
                if code == ErrorCode::BadCrc {
                    self.partial = None;
                    self.file = Vec::new();
                }
                self.error(seq, code)
            }
        }
    }
}

/// The CRC of the whole of ride `id`, read as the app reads it.
async fn ride_crc(device: &mut impl Device, id: u32) -> Result<u16, ErrorCode> {
    let (mut crc, mut offset) = (CRC16_INIT, 0);
    loop {
        let data = device.read_ride(id, offset, MAX_PAYLOAD as u16).await?;
        if data.is_empty() {
            return Ok(crc);
        }
        crc = crc16_update(crc, &data);
        offset += data.len() as u32;
    }
}

/// The file is one the device can read.
fn check(kind: Kind, data: &[u8]) -> Result<(), ErrorCode> {
    let readable = match kind {
        Kind::Route => route::Route::parse(data).is_ok(),
        Kind::Map => route::map::Map::parse(data).is_ok(),
    };
    readable.then_some(()).ok_or(ErrorCode::BadRequest)
}

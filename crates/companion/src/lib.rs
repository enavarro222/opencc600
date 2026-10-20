//! The link with the companion app (Musette), hardware independent: what the app writes comes in ([`Session::on_write`]),
//! the answers go out ([`Session::take_output`]), in the frames of `protocol-cc600`. The firmware only moves the bytes
//! (BLE: the app writes the RX characteristic, the answers are notified on TX) and does what the [`Device`] asks.
//!
//! Handled: `Hello`, the settings (`GetSettings`, `SetSettings`, all or nothing) and files sent by the app (route,
//! map): kept in RAM while they arrive, resumed after a dropped link (not after a reboot), checked (size, CRC, then
//! the format) before the device gets them. The rides (`ListActivities`...) are refused for now.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use protocol_cc600::transfer::{self, Begin, Chunk, Kind, Partial, Receiver};
use protocol_cc600::{ErrorCode, Frame, HEADER_LEN, MAX_FRAME, MAX_PAYLOAD, Op, TRAILER_LEN};

/// File bytes the app may send before an acknowledgement. Each chunk takes a packet of the BLE host's pool (16 of
/// 251 bytes in `trouble-host`, shared with the sensors): 4 KB in flight ran it out (`OutOfMemory`, the write lost).
pub const WINDOW: u16 = 1024;

/// What the device does for the link.
pub trait Device {
    fn settings(&self) -> settings::Settings;
    /// Settings changed by the app (already checked): to apply and save.
    fn set_settings(&mut self, s: settings::Settings);
    /// The largest file of this kind the device takes now, or why it takes none.
    fn max_file(&self, kind: Kind) -> Result<u32, ErrorCode>;
    /// A complete file, its size, CRC and format checked: to store and use.
    fn file_received(&mut self, kind: Kind, data: Vec<u8>) -> Result<(), ErrorCode>;
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
    pub fn on_write(&mut self, bytes: &[u8], device: &mut impl Device) -> Vec<Event> {
        self.input.extend_from_slice(bytes);
        loop {
            match Frame::decode(&self.input) {
                Ok((frame, n)) => {
                    let (op, seq) = (frame.op, frame.seq);
                    // the payload is copied out: answering needs `self`
                    let payload: Vec<u8> = frame.payload.into();
                    self.input.drain(..n);
                    self.handle(op, seq, &payload, device);
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

    fn handle(&mut self, op: Op, seq: u8, payload: &[u8], device: &mut impl Device) {
        match op {
            Op::Hello => self.reply(Op::DeviceInfo, seq, &[]),
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
            // rides: not yet; answers to the app's requests are not requests
            _ => self.error(seq, ErrorCode::BadRequest),
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

/// The file is one the device can read.
fn check(kind: Kind, data: &[u8]) -> Result<(), ErrorCode> {
    let readable = match kind {
        Kind::Route => route::Route::parse(data).is_ok(),
        Kind::Map => route::map::Map::parse(data).is_ok(),
    };
    readable.then_some(()).ok_or(ErrorCode::BadRequest)
}

//! What the companion app (BLE, crates/companion) reads and changes: the settings, the route, the rides; the rides
//! served by the main loop, which owns the storage.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use littlefs2::io::Error;
use log::{info, warn};
use protocol_cc600::transfer::Kind;

use alloc::vec::Vec;

use crate::config::{SETTINGS, SETTINGS_CHANGED, settings};
use crate::Fs;
use crate::route_file::{ROUTE_MAX, ROUTE_RECEIVED};

/// What the companion app (BLE, [`ble::run`]) reads and changes: the settings, the route, the rides.
#[derive(Default)]
pub(crate) struct App {
    /// A ride read ahead: id, offset, bytes. The app reads 512 bytes at a time, each request waits for the main loop.
    ahead: Option<(u32, u32, Vec<u8>)>,
}

/// Bytes of a ride read from the storage for each request of the app (the most a frame carries).
pub(crate) const READ_AHEAD: u16 = protocol_cc600::MAX_PAYLOAD as u16;

/// What the app asks of the rides, served by the main loop (it owns the storage).
pub(crate) enum RideRequest {
    List,
    Read { id: u32, offset: u32, max: u16 },
    Sent(u32),
}

pub(crate) enum RideAnswer {
    List(Vec<protocol_cc600::payload::ActivityEntry>),
    Read(Vec<u8>),
    Sent,
}

pub(crate) static RIDE_REQUEST: Signal<CriticalSectionRawMutex, RideRequest> = Signal::new();

pub(crate) static RIDE_ANSWER: Signal<CriticalSectionRawMutex, Result<RideAnswer, protocol_cc600::ErrorCode>> =
    Signal::new();

/// One request at a time: the app waits for each answer.
pub(crate) async fn ask_rides(r: RideRequest) -> Result<RideAnswer, protocol_cc600::ErrorCode> {
    RIDE_ANSWER.reset();
    RIDE_REQUEST.signal(r);
    RIDE_ANSWER.wait().await
}

pub(crate) fn serve_rides(fs: &Fs<'_>, r: RideRequest) -> Result<RideAnswer, protocol_cc600::ErrorCode> {
    use protocol_cc600::ErrorCode;
    match r {
        RideRequest::List => {
            let mut list = Vec::new();
            ride_store::finished_sent(fs, |id, size, sent| {
                list.push(protocol_cc600::payload::ActivityEntry {
                    id,
                    start_time: id.saturating_sub(fit::FIT_EPOCH_OFFSET),
                    size: size as u32,
                    synced: sent,
                })
            })
            .map_err(|_| ErrorCode::Storage)?;
            list.sort_by_key(|e| e.id);
            Ok(RideAnswer::List(list))
        }
        RideRequest::Read { id, offset, max } => {
            let mut buf = alloc::vec![0; usize::from(max)];
            match ride_store::read_at(fs, id, true, offset as usize, &mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    Ok(RideAnswer::Read(buf))
                }
                Err(Error::NO_SUCH_ENTRY) => Err(ErrorCode::BadRequest),
                Err(_) => Err(ErrorCode::Storage),
            }
        }
        RideRequest::Sent(id) => match ride_store::mark_sent(fs, id) {
            Ok(()) => {
                info!("ride {id}: sent");
                Ok(RideAnswer::Sent)
            }
            Err(Error::NO_SUCH_ENTRY) => Err(ErrorCode::BadRequest),
            Err(e) => {
                warn!("ride {id}: littlefs error {} marking it sent", e.code());
                Err(ErrorCode::Storage)
            }
        },
    }
}

impl companion::Device for App {
    fn settings(&self) -> settings::Settings {
        settings()
    }

    fn set_settings(&mut self, s: settings::Settings) {
        SETTINGS.lock(|c| c.set(Some(s)));
        SETTINGS_CHANGED.signal(());
    }

    fn max_file(&self, kind: Kind) -> Result<u32, protocol_cc600::ErrorCode> {
        match kind {
            Kind::Route => Ok(ROUTE_MAX as u32),
            // not stored yet (`bench-map` only)
            Kind::Map => Err(protocol_cc600::ErrorCode::BadRequest),
        }
    }

    /// Stored by the main loop, like a route sent over USB.
    fn file_received(&mut self, kind: Kind, data: Vec<u8>) -> Result<(), protocol_cc600::ErrorCode> {
        match kind {
            Kind::Route => {
                ROUTE_RECEIVED.signal(Some(data));
                Ok(())
            }
            Kind::Map => Err(protocol_cc600::ErrorCode::BadRequest),
        }
    }

    async fn rides(&mut self) -> Result<Vec<protocol_cc600::payload::ActivityEntry>, protocol_cc600::ErrorCode> {
        self.ahead = None;
        match ask_rides(RideRequest::List).await? {
            RideAnswer::List(list) => Ok(list),
            _ => Err(protocol_cc600::ErrorCode::Busy),
        }
    }

    async fn read_ride(&mut self, id: u32, offset: u32, max: u16) -> Result<Vec<u8>, protocol_cc600::ErrorCode> {
        let held = |a: &(u32, u32, Vec<u8>)| {
            a.0 == id && a.1 <= offset && offset + u32::from(max) <= a.1 + a.2.len() as u32
                // the end of the file: fewer bytes than asked for
                || a.0 == id && a.1 <= offset && a.2.len() < usize::from(READ_AHEAD)
        };
        if !self.ahead.as_ref().is_some_and(held) {
            self.ahead = None;
            let max = max.max(READ_AHEAD);
            match ask_rides(RideRequest::Read { id, offset, max }).await? {
                RideAnswer::Read(data) => self.ahead = Some((id, offset, data)),
                _ => return Err(protocol_cc600::ErrorCode::Busy),
            }
        }
        let (_, start, data) = self.ahead.as_ref().ok_or(protocol_cc600::ErrorCode::Busy)?;
        let from = ((offset - start) as usize).min(data.len());
        Ok(data[from..(from + usize::from(max)).min(data.len())].to_vec())
    }

    async fn ride_sent(&mut self, id: u32) -> Result<(), protocol_cc600::ErrorCode> {
        self.ahead = None;
        match ask_rides(RideRequest::Sent(id)).await? {
            RideAnswer::Sent => Ok(()),
            _ => Err(protocol_cc600::ErrorCode::Busy),
        }
    }
}

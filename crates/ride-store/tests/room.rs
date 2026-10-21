//! The rides first: the logs stay within their budget, out of the reserve, and give way to a ride short of room.

use ride_store::littlefs2::fs::{Allocation, Filesystem};
use ride_store::littlefs2::io::Error;
use ride_store::ram::{BLOCK_SIZE, RamFlash};
use ride_store::room::{self, LOG_FILE_BYTES, LOGS, LOGS_MAX_BYTES, RESERVE_BYTES};

/// 4 MiB: the logs' budget, the reserve and some rides.
type Flash = RamFlash<1024>;

fn with_fs(f: impl FnOnce(&Filesystem<'_, Flash>)) {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    f(&fs);
}

/// Appends `len` bytes to log `id`, in trip-log-sized pieces.
fn write_log(fs: &Filesystem<'_, Flash>, id: u32, len: usize) {
    if !fs.exists(LOGS) {
        fs.create_dir(LOGS).unwrap();
    }
    let chunk = [b'x'; 2048];
    let mut left = len;
    while left > 0 {
        let n = left.min(chunk.len());
        fs.open_file_with_options_and_then(
            |o| o.write(true).create(true).append(true),
            &room::log_path(id),
            |f| f.write(&chunk[..n]).map(|_| ()),
        )
        .unwrap();
        left -= n;
    }
}

/// A ride of `len` bytes, finished.
fn write_ride(fs: &Filesystem<'_, Flash>, id: u32, len: usize) {
    ride_store::start(fs, id).unwrap();
    ride_store::append(fs, id, &vec![7; len]).unwrap();
    ride_store::finish(fs, id).unwrap();
}

fn log_ids(fs: &Filesystem<'_, Flash>) -> Vec<u32> {
    let mut ids = Vec::new();
    room::logs(fs, |id, _| ids.push(id)).unwrap();
    ids.sort();
    ids
}

#[test]
fn the_logs_keep_within_their_budget_the_oldest_deleted() {
    with_fs(|fs| {
        for id in 1..=6 {
            assert!(room::for_log(fs).unwrap());
            write_log(fs, id, LOG_FILE_BYTES);
        }
        assert!(room::logs_bytes(fs).unwrap() <= LOGS_MAX_BYTES);
        assert_eq!(log_ids(fs), [3, 4, 5, 6]);
    });
}

#[test]
fn the_logs_never_write_into_the_reserve() {
    with_fs(|fs| {
        write_log(fs, 1, LOG_FILE_BYTES);
        // even without the old log, rides leave less than the reserve and a log file: the old log goes, still not
        // enough
        let free = room::free_bytes(fs).unwrap();
        write_ride(fs, 100, free - RESERVE_BYTES + LOG_FILE_BYTES / 2);
        assert!(!room::for_log(fs).unwrap());
        assert_eq!(log_ids(fs), [] as [u32; 0]);
        assert!(room::free_bytes(fs).unwrap() >= RESERVE_BYTES);
        // the rides are never touched
        let mut rides = Vec::new();
        ride_store::finished(fs, |id, _| rides.push(id)).unwrap();
        assert_eq!(rides, [100]);
    });
}

#[test]
fn a_ride_short_of_room_takes_the_logs_room() {
    with_fs(|fs| {
        for id in 1..=3 {
            write_log(fs, id, LOG_FILE_BYTES);
        }
        let free = room::free_bytes(fs).unwrap();
        write_ride(fs, 100, free - 8 * BLOCK_SIZE);
        // the ride being recorded fills what is left
        ride_store::start(fs, 200).unwrap();
        let chunk = [5u8; 340];
        let err = loop {
            if let Err(e) = ride_store::append(fs, 200, &chunk) {
                break e;
            }
        };
        assert_eq!(err, Error::NO_SPACE);
        // the two oldest logs are enough for the reserve
        assert!(room::for_ride(fs).unwrap() >= RESERVE_BYTES);
        assert_eq!(log_ids(fs), [3]);
        ride_store::append(fs, 200, &chunk).unwrap();
    });
}

#[test]
fn nothing_to_delete_on_a_storage_without_logs() {
    with_fs(|fs| {
        let free = room::free_bytes(fs).unwrap();
        assert_eq!(room::for_ride(fs).unwrap(), free);
        assert!(room::for_log(fs).unwrap());
    });
}

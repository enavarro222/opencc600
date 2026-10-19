//! "A ride is never lost" (docs/DEV_PROCESS.md): the power is cut at every flash program / erase of a scenario in
//! turn, then the flash is mounted again and checked against what the scenario had been told was done.

use std::collections::BTreeMap;

use ride_store::littlefs2::fs::{Allocation, Filesystem};
use ride_store::littlefs2::io::Result;
use ride_store::ram::RamFlash;

/// 32 blocks = 128 KiB: a few rides of 20 KB plus littlefs metadata, enough to wrap blocks around.
type Flash = RamFlash<32>;

const CHUNK: usize = 500;
const CHUNKS: usize = 40;

/// Deterministic ride content, different for each ride.
fn ride_data(id: u32) -> Vec<u8> {
    let mut x = id.wrapping_mul(2_654_435_761) | 1;
    (0..CHUNK * CHUNKS)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// What the scenario was told had been done (each step that returned Ok).
#[derive(Default, Debug)]
struct Acked {
    started: BTreeMap<u32, usize>,
    finished: Vec<u32>,
    removed: Vec<u32>,
    /// Step in progress when the power was cut.
    doing: Option<(&'static str, u32)>,
}

fn record(fs: &Filesystem<'_, Flash>, id: u32, acked: &mut Acked) -> Result<()> {
    acked.doing = Some(("start", id));
    ride_store::start(fs, id)?;
    acked.started.insert(id, 0);
    for chunk in ride_data(id).chunks(CHUNK) {
        acked.doing = Some(("append", id));
        ride_store::append(fs, id, chunk)?;
        *acked.started.get_mut(&id).unwrap() += chunk.len();
    }
    Ok(())
}

fn scenario(fs: &Filesystem<'_, Flash>, acked: &mut Acked) -> Result<()> {
    record(fs, 1, acked)?;
    acked.doing = Some(("finish", 1));
    ride_store::finish(fs, 1)?;
    acked.finished.push(1);
    record(fs, 2, acked)?;
    // ride 1 uploaded meanwhile: deleted
    acked.doing = Some(("remove", 1));
    ride_store::remove(fs, 1)?;
    acked.removed.push(1);
    acked.doing = Some(("finish", 2));
    ride_store::finish(fs, 2)?;
    acked.finished.push(2);
    acked.doing = None;
    Ok(())
}

fn content(fs: &Filesystem<'_, Flash>, id: u32, finished: bool, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    let n = ride_store::read(fs, id, finished, &mut buf).unwrap();
    assert_eq!(n, len);
    buf
}

/// Checks the remounted flash against what had been acknowledged before the cut.
fn check(fs: &Filesystem<'_, Flash>, acked: &Acked, cut: u32) {
    let (mut finished, mut unfinished) = (BTreeMap::new(), BTreeMap::new());
    ride_store::finished(fs, |id, len| {
        finished.insert(id, len);
    })
    .unwrap();
    ride_store::unfinished(fs, |id, len| {
        unfinished.insert(id, len);
    })
    .unwrap();
    let ctx = format!(
        "cut at op {cut}, acked {acked:?}, finished {finished:?}, unfinished {unfinished:?}"
    );

    for id in [1, 2] {
        let data = ride_data(id);
        let removed = acked.removed.contains(&id);
        let is_finished = acked.finished.contains(&id);
        match (finished.get(&id), unfinished.get(&id)) {
            (Some(_), Some(_)) => panic!("ride {id} both finished and not: {ctx}"),
            (None, None) => {
                // gone: only if its creation was not acknowledged yet, or its removal was done or in flight
                let removing = acked.doing == Some(("remove", id));
                assert!(
                    !acked.started.contains_key(&id) || removed || removing,
                    "ride {id} lost: {ctx}"
                );
            }
            (Some(&len), None) => {
                assert!(!removed, "removed ride {id} came back: {ctx}");
                assert!(
                    is_finished || acked.doing == Some(("finish", id)),
                    "ride {id} finished early: {ctx}"
                );
                assert_eq!(len, data.len(), "finished ride {id} truncated: {ctx}");
                assert_eq!(
                    content(fs, id, true, len),
                    data,
                    "finished ride {id} corrupted: {ctx}"
                );
            }
            (None, Some(&len)) => {
                assert!(!is_finished, "finished ride {id} back to unfinished: {ctx}");
                let acked_len = acked.started.get(&id).copied().unwrap_or(0);
                // an append is atomic: either it made it entirely, or not at all
                assert!(
                    len == acked_len || len == acked_len + CHUNK,
                    "ride {id} has {len} bytes: {ctx}"
                );
                assert_eq!(
                    content(fs, id, false, len),
                    data[..len],
                    "ride {id} corrupted: {ctx}"
                );
            }
        }
    }
}

#[test]
fn a_ride_is_never_lost() {
    let mut formatted = Flash::new();
    Filesystem::format(&mut formatted).unwrap();

    let mut cut = 1;
    loop {
        let mut flash = formatted.clone();
        flash.cut_after(cut);
        let mut acked = Acked::default();
        let done = {
            let mut alloc = Allocation::new();
            let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
            scenario(&fs, &mut acked).is_ok()
        };
        if done {
            assert!(!flash.is_off());
            break;
        }
        assert!(
            flash.is_off(),
            "cut {cut}: scenario failed without a power cut"
        );

        flash.power_on();
        let mut alloc = Allocation::new();
        let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap_or_else(|e| {
            panic!("cut at op {cut}: mount failed after power cut: {e:?}, acked {acked:?}")
        });
        check(&fs, &acked, cut);

        // the store keeps working: close what was interrupted, record a new ride
        let mut unfinished = vec![];
        ride_store::unfinished(&fs, |id, _| unfinished.push(id)).unwrap();
        for id in unfinished {
            ride_store::finish(&fs, id).unwrap();
        }
        ride_store::start(&fs, 3).unwrap();
        ride_store::append(&fs, 3, &ride_data(3)[..CHUNK]).unwrap();
        ride_store::finish(&fs, 3).unwrap();
        cut += 1;
    }
    println!(
        "scenario: {} flash operations, a power cut tested at each",
        cut - 1
    );
    assert!(cut > 100, "suspiciously few operations: {cut}");
}

#[test]
fn rides_round_trip() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    let mut acked = Acked::default();
    scenario(&fs, &mut acked).unwrap();
    let mut finished = vec![];
    ride_store::finished(&fs, |id, len| finished.push((id, len))).unwrap();
    assert_eq!(finished, [(2, CHUNK * CHUNKS)]);
    assert_eq!(content(&fs, 2, true, CHUNK * CHUNKS), ride_data(2));
}

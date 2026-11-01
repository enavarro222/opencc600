use ride_store::littlefs2::{
    fs::{Allocation, Filesystem},
    path,
};
use ride_store::ram::RamFlash;
use route::atlas::{
    self,
    build::{Feature, encode},
};

type Flash = RamFlash<32>;

fn atlas(name: &str, count: usize) -> Vec<u8> {
    let features: Vec<_> = (0..count)
        .map(|i| Feature {
            class: atlas::BOUNDARY,
            level: 0,
            label: String::new(),
            points: (0..100).map(|j| (2000 + j, 45000 + i as i32)).collect(),
        })
        .collect();
    encode(name, &features).unwrap()
}

#[test]
fn atlas_remains_when_the_route_is_replaced_or_removed_and_storage_reboots() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let expected = atlas("Europe", 40);
    {
        let mut alloc = Allocation::new();
        let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
        atlas_store::store(&fs, &expected).unwrap();
        fs.write(path!("/route"), b"first route").unwrap();
        fs.write(path!("/route"), b"second route").unwrap();
        fs.remove(path!("/route")).unwrap();
        assert!(atlas_store::store(&fs, b"corrupted atlas").is_err());
        assert_eq!(atlas_store::load(&fs).unwrap().unwrap(), expected);
    }
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    assert_eq!(atlas_store::load(&fs).unwrap().unwrap(), expected);
}

#[test]
fn a_cut_during_any_write_keeps_the_old_or_complete_new_atlas() {
    let (old, new) = (atlas("France", 40), atlas("Europe", 80));
    let mut baseline = Flash::new();
    Filesystem::format(&mut baseline).unwrap();
    {
        let mut alloc = Allocation::new();
        let fs = Filesystem::mount(&mut alloc, &mut baseline).unwrap();
        atlas_store::store(&fs, &old).unwrap();
    }
    let mut checked = 0;
    for cut in 1..2000 {
        let mut flash = baseline.clone();
        flash.cut_after(cut);
        let done = {
            let mut alloc = Allocation::new();
            let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
            atlas_store::store(&fs, &new).is_ok()
        };
        if done {
            assert!(!flash.is_off());
            break;
        }
        assert!(flash.is_off(), "failed without a cut at {cut}");
        flash.power_on();
        let mut alloc = Allocation::new();
        let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
        let kept = atlas_store::load(&fs).unwrap().unwrap();
        assert!(kept == old || kept == new, "partial atlas at cut {cut}");
        atlas_store::store(&fs, &new).unwrap();
        assert_eq!(atlas_store::load(&fs).unwrap().unwrap(), new);
        checked += 1;
    }
    assert!(checked > 20 && checked < 1999, "checked {checked} cuts");
}

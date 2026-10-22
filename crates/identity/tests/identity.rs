//! The NVS pages are built here, laid out as on the two devices (docs/CC700.md): the dumps themselves hold the MAC
//! address and the paired phone's name.

use identity::{Model, NVS_SIZE, Name, ble_address};

const PAGE: usize = 4096;

/// The vendor blob: SpuVersion, ManuID 136, SPU id, model name.
fn blob(spu_id: u16, model: &[u8]) -> Vec<u8> {
    let mut b = vec![0x0e, 0x0f, 0x81, 0x01, 0x88, 0x00];
    b.extend_from_slice(&spu_id.to_le_bytes());
    b.extend_from_slice(model);
    b.resize(17, 0);
    b
}

struct Nvs(Vec<u8>);

impl Nvs {
    fn new() -> Self {
        Nvs(vec![0xFF; NVS_SIZE])
    }

    fn page(&mut self, page: usize, state: u32) -> &mut Self {
        self.0[page * PAGE..page * PAGE + 4].copy_from_slice(&state.to_le_bytes());
        self
    }

    fn mark(&mut self, page: usize, index: usize, state: u8) {
        let at = page * PAGE + 32 + index / 4;
        let shift = (index % 4) * 2;
        self.0[at] = (self.0[at] & !(0b11 << shift)) | (state << shift);
    }

    /// Writes an entry (and the `extra` entries after it, which it spans) and marks them written.
    fn entry(
        &mut self,
        page: usize,
        index: usize,
        head: [u8; 4],
        key: &[u8],
        data: [u8; 8],
        extra: &[u8],
    ) -> &mut Self {
        let at = page * PAGE + 64 + index * 32;
        let e = &mut self.0[at..at + 32];
        e[..4].copy_from_slice(&head);
        e[8..24].fill(0);
        e[8..8 + key.len()].copy_from_slice(key);
        e[24..].copy_from_slice(&data);
        self.0[at + 32..at + 32 + extra.len()].copy_from_slice(extra);
        for i in 0..=extra.len().div_ceil(32) {
            self.mark(page, index + i, 0b10);
        }
        self
    }

    fn namespace(&mut self, page: usize, index: usize) -> &mut Self {
        self.entry(
            page,
            index,
            [0, 0x01, 1, 0xFF],
            b"nvsstorage",
            [1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            &[],
        )
    }

    /// The data chunk at `index`, its index entry two entries further.
    fn blob(&mut self, page: usize, index: usize, chunk: u8, bytes: &[u8]) -> &mut Self {
        let size = (bytes.len() as u16).to_le_bytes();
        let span = 1 + bytes.len().div_ceil(32) as u8;
        self.entry(
            page,
            index,
            [1, 0x42, span, chunk],
            b"Res1Page11",
            [size[0], size[1], 0xFF, 0xFF, 0, 0, 0, 0],
            bytes,
        );
        let count = 1;
        self.entry(
            page,
            index + 2,
            [1, 0x48, 1, 0xFF],
            b"Res1Page11",
            [size[0], size[1], 0, 0, count, chunk, 0xFF, 0xFF],
            &[],
        )
    }

    fn erase(&mut self, page: usize, index: usize) -> &mut Self {
        self.mark(page, index, 0);
        self
    }
}

const ACTIVE: u32 = 0xFFFF_FFFE;
const FULL: u32 = 0xFFFF_FFFC;

#[test]
fn a_cc700_from_its_factory_blob() {
    let mut nvs = Nvs::new();
    nvs.page(0, FULL)
        .namespace(0, 0)
        .blob(0, 104, 0, &blob(389, b"CC700"));
    nvs.page(1, ACTIVE);
    assert_eq!(Model::from_nvs(&nvs.0), Model::Cc700);
}

#[test]
fn a_cc600_from_its_factory_blob() {
    let mut nvs = Nvs::new();
    nvs.page(0, FULL)
        .namespace(0, 0)
        .blob(0, 104, 0, &blob(345, b"CC600"));
    assert_eq!(Model::from_nvs(&nvs.0), Model::Cc600);
}

#[test]
fn the_current_copy_when_the_blob_was_rewritten() {
    // as on the CC600: the first copy erased, the new one (other chunk version) on the next page
    let mut nvs = Nvs::new();
    nvs.page(0, FULL)
        .namespace(0, 0)
        .blob(0, 104, 0, &blob(345, b"CC600"));
    nvs.erase(0, 104).erase(0, 105).erase(0, 106);
    nvs.page(1, ACTIVE).blob(1, 89, 128, &blob(389, b"CC700"));
    assert_eq!(Model::from_nvs(&nvs.0), Model::Cc700);
    // the index points at its chunk version: a stale chunk left written does not count
    let mut stale = Nvs::new();
    stale
        .page(0, FULL)
        .namespace(0, 0)
        .blob(0, 104, 0, &blob(389, b"CC700"))
        .erase(0, 106);
    stale.page(1, ACTIVE).blob(1, 89, 128, &blob(345, b"CC600"));
    assert_eq!(Model::from_nvs(&stale.0), Model::Cc600);
}

#[test]
fn a_cc600_without_factory_data() {
    assert_eq!(Model::from_nvs(&Nvs::new().0), Model::Cc600);
    assert_eq!(Model::from_nvs(&vec![0; NVS_SIZE]), Model::Cc600);
    assert_eq!(Model::from_nvs(&[]), Model::Cc600);
    // a page not in use (being freed) is not read
    let mut nvs = Nvs::new();
    nvs.page(0, 0xFFFF_FFF8)
        .namespace(0, 0)
        .blob(0, 104, 0, &blob(389, b"CC700"));
    assert_eq!(Model::from_nvs(&nvs.0), Model::Cc600);
}

#[test]
fn name_and_address_from_the_mac() {
    let mac = [0x7c, 0x4f, 0xad, 0x84, 0xae, 0xa8];
    assert_eq!(Name::new(Model::Cc700, mac).as_str(), "OpenCC700 AEA8");
    assert_eq!(
        Name::new(Model::Cc600, [0, 0, 0, 0, 0x0b, 0xc4]).as_str(),
        "OpenCC600 0BC4"
    );
    assert_eq!(ble_address(mac), [0xa8, 0xae, 0x84, 0xad, 0x4f, 0xfc]);
}

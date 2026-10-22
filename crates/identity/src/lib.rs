//! Which device this is, and how it shows itself over Bluetooth.
//!
//! The CC600 and the CC700 are the same board running the same firmware (docs/CC700.md): only the factory data in the
//! ESP32's `nvs` partition tells them apart. The vendor firmware keeps there, in namespace `nvsstorage`, a 17-byte blob
//! `Res1Page11`: its SPU id at 6..8 (345 on the CC600, 389 on the CC700) and the model name from 8 (`CC600` /
//! `CC700`). We only read it; without it (erased, unreadable), the device is a CC600.
//!
//! The name and the address come from the base MAC address (efuse), unique to each device: `OpenCC700 AEA8`.

#![no_std]

/// The `nvs` partition in the vendor partition table (checked by tools/flash.py).
pub const NVS_OFFSET: u32 = 0x9000;
pub const NVS_SIZE: usize = 0x4000;

#[derive(Clone, Copy, Debug)]
pub struct Identity {
    pub model: Model,
    pub name: Name,
    pub ble_address: [u8; 6],
}

impl Identity {
    /// `nvs`: the content of the `nvs` partition (empty when unreadable); `mac`: the base MAC address.
    pub fn new(nvs: &[u8], mac: [u8; 6]) -> Self {
        let model = Model::from_nvs(nvs);
        Identity {
            model,
            name: Name::new(model, mac),
            ble_address: ble_address(mac),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    #[default]
    Cc600,
    Cc700,
}

impl Model {
    /// From the content of the `nvs` partition; a CC600 when the factory data is not found.
    pub fn from_nvs(nvs: &[u8]) -> Self {
        match nvs::find_blob(nvs, b"nvsstorage", b"Res1Page11") {
            Some(blob) if blob.get(8..13) == Some(b"CC700") => Model::Cc700,
            _ => Model::Cc600,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::Cc600 => "CC600",
            Model::Cc700 => "CC700",
        }
    }
}

/// The advertised name: `OpenCC600 XXXX`, `XXXX` the last two bytes of the MAC address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name([u8; 14]);

impl Name {
    pub fn new(model: Model, mac: [u8; 6]) -> Self {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut b = *b"OpenCC600 0000";
        b[4..9].copy_from_slice(model.name().as_bytes());
        for (i, byte) in mac[4..].iter().enumerate() {
            b[10 + 2 * i] = HEX[usize::from(byte >> 4)];
            b[11 + 2 * i] = HEX[usize::from(byte & 0xF)];
        }
        Name(b)
    }

    pub fn as_str(&self) -> &str {
        // ASCII only, built above
        core::str::from_utf8(&self.0).unwrap_or("OpenCC600")
    }
}

/// Our static random BLE address (least significant byte first, as the host stack takes it): the MAC address with the
/// two top bits set, unique to the device and the same at every boot.
pub fn ble_address(mac: [u8; 6]) -> [u8; 6] {
    let [a, b, c, d, e, f] = mac;
    [f, e, d, c, b, a | 0xC0]
}

/// Just enough of the ESP-IDF NVS format (v2, multi-page blobs) to read one blob.
mod nvs {
    const PAGE: usize = 4096;
    const ENTRY: usize = 32;
    const ENTRIES: usize = 126;
    /// Page states: in use, and full (the others are empty, being freed or corrupt).
    const ACTIVE: u32 = 0xFFFF_FFFE;
    const FULL: u32 = 0xFFFF_FFFC;
    /// Entry state in the page's bitmap (2 bits per entry): written, neither empty nor erased.
    const WRITTEN: u8 = 0b10;
    const TYPE_U8: u8 = 0x01;
    const TYPE_BLOB_DATA: u8 = 0x42;
    const TYPE_BLOB_INDEX: u8 = 0x48;
    /// Namespace of the namespace entries.
    const NS_INDEX: u8 = 0;

    struct Entry<'a> {
        ns: u8,
        kind: u8,
        span: u8,
        chunk: u8,
        key: &'a [u8],
        data: &'a [u8],
        /// The entries after it on its page, which hold a blob chunk's bytes.
        rest: &'a [u8],
    }

    fn entries(nvs: &[u8]) -> impl Iterator<Item = Entry<'_>> {
        nvs.chunks_exact(PAGE)
            .filter(|p| matches!(u32::from_le_bytes([p[0], p[1], p[2], p[3]]), ACTIVE | FULL))
            .flat_map(|p| {
                let bitmap = &p[32..64];
                (0..ENTRIES).filter_map(move |i| {
                    if (bitmap[i / 4] >> ((i % 4) * 2)) & 0b11 != WRITTEN {
                        return None;
                    }
                    let at = 64 + i * ENTRY;
                    let e = &p[at..at + ENTRY];
                    let key = &e[8..24];
                    let key = &key[..key.iter().position(|&b| b == 0).unwrap_or(key.len())];
                    Some(Entry {
                        ns: e[0],
                        kind: e[1],
                        span: e[2],
                        chunk: e[3],
                        key,
                        data: &e[24..],
                        rest: &p[at + ENTRY..],
                    })
                })
            })
    }

    /// A blob of one chunk (all the vendor's are under 4 KB): its index entry says which chunk version is current.
    pub fn find_blob<'a>(nvs: &'a [u8], namespace: &[u8], key: &[u8]) -> Option<&'a [u8]> {
        let ns = entries(nvs)
            .find(|e| e.ns == NS_INDEX && e.kind == TYPE_U8 && e.key == namespace)?
            .data[0];
        let index =
            entries(nvs).find(|e| e.ns == ns && e.kind == TYPE_BLOB_INDEX && e.key == key)?;
        let start = index.data[5];
        let chunk = entries(nvs)
            .find(|e| e.ns == ns && e.kind == TYPE_BLOB_DATA && e.key == key && e.chunk == start)?;
        let size = usize::from(u16::from_le_bytes([chunk.data[0], chunk.data[1]]));
        if size > usize::from(chunk.span).saturating_sub(1) * ENTRY {
            return None;
        }
        chunk.rest.get(..size)
    }
}

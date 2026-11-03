//! What a HID report map says about the input reports a remote sends: where its fake touch is (tip switch, X, Y)
//! and its keys (consumer, keyboard, buttons), bitmaps or arrays. Only what a remote needs: no outputs, no features,
//! no relative moves.

/// Usage pages.
pub const GENERIC_DESKTOP: u16 = 0x01;
pub const KEYBOARD: u16 = 0x07;
pub const BUTTON: u16 = 0x09;
pub const CONSUMER: u16 = 0x0C;
pub const DIGITIZER: u16 = 0x0D;

const X: u16 = 0x30;
const Y: u16 = 0x31;
const TIP_SWITCH: u16 = 0x42;

/// A key: its usage page in the high half, its usage in the low one (`0x000C_00E9`: volume up).
pub type Code = u32;

/// A value in a report: its first bit (the report ID byte not counted), its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bits {
    pub report: u8,
    pub bit: u16,
    pub size: u8,
}

impl Bits {
    /// The value in `data` (unsigned), `None` past its end.
    pub fn read(&self, data: &[u8]) -> Option<u32> {
        let (first, size) = (self.bit as usize, self.size as usize);
        if size == 0 || size > 32 || first + size > data.len() * 8 {
            return None;
        }
        let mut v = 0u64;
        for (i, byte) in data[first / 8..(first + size).div_ceil(8)].iter().enumerate() {
            v |= (*byte as u64) << (8 * i);
        }
        Some(((v >> (first % 8)) & ((1u64 << size) - 1)) as u32)
    }
}

/// The first contact of a touchscreen: pressed or not, where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Touch {
    pub tip: Bits,
    pub x: Bits,
    pub y: Bits,
    pub x_max: u32,
    pub y_max: u32,
}

/// One key of a bitmap: its bit is set while it is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBit {
    pub at: Bits,
    pub code: Code,
}

/// An array of keys: each slot holds the index of a key down (from `first`), or a value out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyArray {
    /// The first slot; the others follow, `count` of them.
    pub at: Bits,
    pub count: u8,
    pub page: u16,
    pub first: u16,
    pub last: u16,
    pub min: i32,
}

pub const KEY_BITS: usize = 64;
pub const KEY_ARRAYS: usize = 8;

/// The input reports of a report map, as far as a remote goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub touch: Option<Touch>,
    bits: [Option<KeyBit>; KEY_BITS],
    arrays: [Option<KeyArray>; KEY_ARRAYS],
}

/// An axis of the touch: where, its logical maximum.
type Axis = (Bits, u32);

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    min: i32,
    max: i32,
    size: u8,
    count: u8,
    report: u8,
}

/// Usages given in a main item's locals, at most this many kept (a bitmap of 24 consumer keys gives 24).
const USAGES: usize = 32;

impl Layout {
    pub fn parse(map: &[u8]) -> Layout {
        let mut out = Layout { touch: None, bits: [None; KEY_BITS], arrays: [None; KEY_ARRAYS] };
        let mut g = Globals::default();
        let mut stack = [Globals::default(); 4];
        let mut depth = 0;
        let mut usages = [0u16; USAGES];
        let mut n_usages = 0;
        let (mut umin, mut umax) = (None::<u16>, None::<u16>);
        // the next free bit of each report ID
        let mut next_bit = [(0u8, 0u16); 16];
        // the touch being found, in its first collection only
        let mut tip: Option<Bits> = None;
        let (mut x, mut y): (Option<Axis>, Option<Axis>) = (None, None);
        let mut i = 0;
        while i < map.len() {
            let b = map[i];
            if b == 0xFE {
                // long item: its size, its tag, its data
                let size = map.get(i + 1).copied().unwrap_or(0) as usize;
                i += 3 + size;
                continue;
            }
            let n = [0, 1, 2, 4][(b & 3) as usize];
            let Some(raw) = map.get(i + 1..i + 1 + n) else { break };
            let mut v = 0u32;
            for (k, byte) in raw.iter().enumerate() {
                v |= (*byte as u32) << (8 * k);
            }
            let signed = match n {
                1 => v as u8 as i8 as i32,
                2 => v as u16 as i16 as i32,
                _ => v as i32,
            };
            i += 1 + n;
            match b & 0xFC {
                0x04 => g.page = v as u16,
                0x14 => g.min = signed,
                // a maximum is unsigned when the minimum is not negative (`25 FF` is 255)
                0x24 => g.max = if g.min >= 0 { v as i32 } else { signed },
                0x74 => g.size = v.min(32) as u8,
                0x84 => g.report = v as u8,
                0x94 => g.count = v.min(255) as u8,
                0xA4 if depth < stack.len() => {
                    stack[depth] = g;
                    depth += 1;
                }
                0xB4 if depth > 0 => {
                    depth -= 1;
                    g = stack[depth];
                }
                0x08 => {
                    // an extended usage (4 bytes) carries its page: kept only on the current one
                    if (n < 4 || (v >> 16) as u16 == g.page) && n_usages < USAGES {
                        usages[n_usages] = v as u16;
                        n_usages += 1;
                    }
                }
                0x18 => umin = Some(v as u16),
                0x28 => umax = Some(v as u16),
                0x80 => {
                    let slot = match next_bit.iter().position(|(r, _)| *r == g.report) {
                        Some(s) => s,
                        None => {
                            let s = next_bit.iter().position(|(r, b)| *r == 0 && *b == 0).unwrap_or(15);
                            next_bit[s].0 = g.report;
                            s
                        }
                    };
                    let start = next_bit[slot].1;
                    let total = g.size as u16 * g.count as u16;
                    next_bit[slot].1 = start + total;
                    let constant = v & 1 != 0;
                    let variable = v & 2 != 0;
                    let relative = v & 4 != 0;
                    let usage_at = |k: usize| -> Option<u16> {
                        match (umin, umax) {
                            (Some(lo), Some(hi)) => lo.checked_add(k as u16).filter(|u| *u <= hi),
                            _ if n_usages > 0 => Some(usages[k.min(n_usages - 1)]),
                            _ => None,
                        }
                    };
                    let keys = matches!(g.page, CONSUMER | KEYBOARD | BUTTON);
                    // keys may be "relative" (a BLE-M3's consumer bitmap): only axes are left out then
                    if !constant && (keys || !relative) {
                        if variable {
                            for k in 0..g.count as usize {
                                let Some(u) = usage_at(k) else { continue };
                                let at = Bits { report: g.report, bit: start + (k as u16) * g.size as u16, size: g.size };
                                match (g.page, u) {
                                    (DIGITIZER, TIP_SWITCH) if g.size == 1 && tip.is_none() => tip = Some(at),
                                    (GENERIC_DESKTOP, X) if x.is_none() => x = Some((at, g.max.max(1) as u32)),
                                    (GENERIC_DESKTOP, Y) if y.is_none() => y = Some((at, g.max.max(1) as u32)),
                                    _ if keys && g.size == 1 => {
                                        if let Some(free) = out.bits.iter_mut().find(|s| s.is_none()) {
                                            *free = Some(KeyBit { at, code: (g.page as u32) << 16 | u as u32 });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        } else if keys && g.count > 0 {
                            let first = umin.unwrap_or(if n_usages > 0 { usages[0] } else { 0 });
                            let last = umax.unwrap_or(first.saturating_add((g.max - g.min).max(0) as u16));
                            if let Some(free) = out.arrays.iter_mut().find(|s| s.is_none()) {
                                *free = Some(KeyArray {
                                    at: Bits { report: g.report, bit: start, size: g.size },
                                    count: g.count,
                                    page: g.page,
                                    first,
                                    last,
                                    min: g.min,
                                });
                            }
                        }
                    }
                    n_usages = 0;
                    (umin, umax) = (None, None);
                }
                // outputs, features, collections: their locals go
                0x90 | 0xB0 | 0xA0 => {
                    n_usages = 0;
                    (umin, umax) = (None, None);
                }
                // the end of a collection closes the touch being found
                0xC0 => {
                    if out.touch.is_none()
                        && let (Some(tip), Some((x, x_max)), Some((y, y_max))) = (tip, x, y)
                        && tip.report == x.report
                        && x.report == y.report
                    {
                        out.touch = Some(Touch { tip, x, y, x_max, y_max });
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The keys down in this report, at most `N`, in the order of the map; `None` if the report has no keys.
    pub fn keys<const N: usize>(&self, report: u8, data: &[u8]) -> Option<([Code; N], usize)> {
        let mut out = [0; N];
        let mut n = 0;
        let mut any = false;
        for k in self.bits.iter().flatten().filter(|k| k.at.report == report) {
            any = true;
            if k.at.read(data) == Some(1) && n < N {
                out[n] = k.code;
                n += 1;
            }
        }
        for a in self.arrays.iter().flatten().filter(|a| a.at.report == report) {
            any = true;
            for s in 0..a.count as u16 {
                let at = Bits { bit: a.at.bit + s * a.at.size as u16, ..a.at };
                let Some(raw) = at.read(data) else { continue };
                let index = raw as i64 - a.min as i64;
                let usage = a.first as i64 + index;
                // 0 is "no key" in every page a remote uses
                if index >= 0 && usage > 0 && usage <= a.last as i64 && n < N {
                    out[n] = (a.page as u32) << 16 | usage as u32;
                    n += 1;
                }
            }
        }
        any.then_some((out, n))
    }
}

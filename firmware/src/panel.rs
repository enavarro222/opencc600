//! ST7789 240x320 panel on the LCD_CAM i8080 16-bit bus.
//!
//! Reference: c606-oss `main/lcd.c` + `main/board_c606.h` (vendor `MidLcdInit()` / `mg_panel_st7789_init()`).
//! The panel reset line is on the nRF side: an ESP32 reset leaves the panel powered with its last frame in GRAM,
//! so [`Panel::new`] clears the GRAM before turning the display on.

use alloc::boxed::Box;
use alloc::vec;
use core::convert::Infallible;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::prelude::*;
use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::dma::DmaTxBuf;
use esp_hal::lcd_cam::lcd::i8080::{Command, I8080};

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 320;
/// Lines per DMA transfer (the vendor uses 60-line LVGL buffers; 40 keeps the DMA buffer in internal RAM small).
pub const BAND_LINES: usize = 40;
pub const BAND_BYTES: usize = WIDTH * BAND_LINES * 2;

const SWRESET: u8 = 0x01;
const SLPOUT: u8 = 0x11;
const INVON: u8 = 0x21;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2A;
const RASET: u8 = 0x2B;
const RAMWR: u8 = 0x2C;

/// Vendor `mg_panel_st7789_init()` (C606), after SWRESET: (command, parameters, delay in ms).
/// DISPON is sent separately, once the GRAM is cleared.
const INIT: &[(u8, &[u8], u32)] = &[
    (SLPOUT, &[], 120),
    (0x36, &[0x00], 1), // MADCTL: RGB order
    (0x3A, &[0x55], 1), // COLMOD: 16 bpp
    (0xB2, &[0x0C, 0x0C, 0x00, 0x33, 0x33], 1),
    (0xB7, &[0x74], 1),
    (0xBB, &[0x1E], 1),
    (0xC0, &[0x2C], 1),
    (0xC2, &[0x01], 1),
    (0xC3, &[0x10], 1),
    (0xC4, &[0x20], 1),
    (0xC6, &[0x0F], 1),
    (0xD0, &[0xA4, 0xA1], 1),
    (
        0xE0,
        &[
            0xF0, 0x06, 0x0B, 0x06, 0x07, 0x25, 0x34, 0x44, 0x4A, 0x38, 0x14, 0x13, 0x2E, 0x34,
        ],
        1,
    ),
    (
        0xE1,
        &[
            0xF0, 0x0C, 0x10, 0x0A, 0x09, 0x06, 0x33, 0x43, 0x49, 0x36, 0x12, 0x14, 0x2A, 0x32,
        ],
        1,
    ),
    (0xE9, &[0x11, 0x11, 0x03], 1),
    (INVON, &[], 1),
];

pub struct Panel<'d> {
    // `I8080::send` consumes the bus and the buffer until the transfer is done.
    bus: Option<I8080<'d, Blocking>>,
    buf: Option<DmaTxBuf>,
    delay: Delay,
}

impl<'d> Panel<'d> {
    /// Runs the vendor init sequence, clears the GRAM to black, then turns the display on.
    pub fn new(bus: I8080<'d, Blocking>, buf: DmaTxBuf) -> Self {
        assert!(buf.capacity() >= BAND_BYTES);
        let mut panel = Self {
            bus: Some(bus),
            buf: Some(buf),
            delay: Delay::new(),
        };
        panel.command(SWRESET, &[]);
        panel.delay.delay_millis(20);
        for &(cmd, params, ms) in INIT {
            panel.command(cmd, params);
            panel.delay.delay_millis(ms);
        }
        panel.clear(Rgb565::BLACK);
        panel.command(DISPON, &[]);
        panel.delay.delay_millis(1);
        panel
    }

    /// Command + 8-bit parameters (each byte goes out on D0..D7 of the 16-bit bus).
    fn command(&mut self, cmd: u8, params: &[u8]) {
        self.send(Command::One(cmd), |buf| buf.fill(params));
    }

    /// Fills the DMA buffer with `fill`, sends it after `cmd`, waits for the end of the transfer.
    fn send<W: Into<u16> + Copy>(&mut self, cmd: Command<W>, fill: impl FnOnce(&mut DmaTxBuf)) {
        let mut buf = self.buf.take().unwrap();
        fill(&mut buf);
        let (res, bus, buf) = match self.bus.take().unwrap().send(cmd, 0, buf) {
            Ok(transfer) => transfer.wait(),
            Err((e, ..)) => panic!("i8080 send: {e:?}"),
        };
        res.unwrap();
        self.bus = Some(bus);
        self.buf = Some(buf);
    }

    fn set_window(&mut self, y0: usize, y1: usize) {
        let x1 = (WIDTH - 1) as u16;
        let (y0, y1) = (y0 as u16, (y1 - 1) as u16);
        self.command(CASET, &[0, 0, (x1 >> 8) as u8, x1 as u8]);
        self.command(
            RASET,
            &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8],
        );
    }

    /// Writes `lines` full-width lines from `y0`; `pixel(i)` gives the little-endian RGB565 value of pixel `i`.
    /// 16-bit transfer: one pixel per bus clock.
    fn write_lines(&mut self, y0: usize, lines: usize, pixel: impl Fn(usize) -> u16) {
        self.set_window(y0, y0 + lines);
        self.send(Command::One(RAMWR as u16), |buf| {
            buf.set_length(lines * WIDTH * 2);
            for (i, dst) in buf.as_mut_slice().chunks_exact_mut(2).enumerate() {
                dst.copy_from_slice(&pixel(i).to_le_bytes());
            }
        });
    }

    pub fn clear(&mut self, color: Rgb565) {
        let raw = RawU16::from(color).into_inner();
        for band in 0..HEIGHT / BAND_LINES {
            self.write_lines(band * BAND_LINES, BAND_LINES, |_| raw);
        }
    }

    pub fn flush(&mut self, fb: &FrameBuffer) {
        for (band, px) in fb.px.chunks(WIDTH * BAND_LINES).enumerate() {
            self.write_lines(band * BAND_LINES, px.len() / WIDTH, |i| px[i]);
        }
    }
}

/// Full-screen RGB565 framebuffer (150 KB, lands in PSRAM), drawn with `embedded-graphics`.
pub struct FrameBuffer {
    px: Box<[u16]>,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self {
            px: vec![0; WIDTH * HEIGHT].into_boxed_slice(),
        }
    }
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Infallible> {
        for Pixel(p, c) in pixels {
            if let (Ok(x @ 0..240), Ok(y @ 0..320)) = (usize::try_from(p.x), usize::try_from(p.y)) {
                self.px[y * WIDTH + x] = RawU16::from(c).into_inner();
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Rgb565) -> Result<(), Infallible> {
        self.px.fill(RawU16::from(color).into_inner());
        Ok(())
    }
}

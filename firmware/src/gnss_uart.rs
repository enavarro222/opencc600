//! GNSS receiver link: Airoha AG3352Q on UART0 (TX GPIO1, RX GPIO0), 921600 8N1, NMEA (c606-oss
//! `docs/HARDWARE.md`). Its power is switched by the nRF (`E2 02 07`, nrf-link `gps_power`).
//!
//! Received through UHCI DMA into a ring of descriptors in internal RAM: the DMA keeps writing while the CPU is
//! stalled by a flash write (sector erase ~45 ms, i.e. ~4 KB at this baud rate; the UART FIFO holds 128 bytes).
//! [`GnssUart::drain`] hands over what arrived; call it at least every couple of seconds.

use esp_hal::Blocking;
use esp_hal::dma::{DmaRxStreamBuf, DmaTxBuf};
use esp_hal::peripherals::{DMA_CH1, GPIO0, GPIO1, UART0, UHCI0};
use esp_hal::uart::uhci::{self, Uhci, UhciDmaRxTransfer, UhciTx};
use esp_hal::uart::{self, RxConfig, Uart};
use esp_hal::{dma_rx_stream_buffer, dma_tx_buffer};

pub const BAUD: u32 = 921_600;
/// ~8 s of NMEA at the module's default output (~2 KB/s).
const RING: usize = 16 * 1024;
const CHUNK: usize = 1024;

pub struct GnssUart {
    rx: UhciDmaRxTransfer<'static, Blocking, DmaRxStreamBuf>,
    /// For the `$PAIR` commands; `None` only while a write is in flight.
    tx: Option<(UhciTx<'static, Blocking>, DmaTxBuf)>,
}

/// Longest command [`GnssUart::send`] takes.
pub const MAX_COMMAND: usize = 64;

impl GnssUart {
    pub fn new(
        uart: UART0<'static>,
        tx: GPIO1<'static>,
        rx: GPIO0<'static>,
        uhci: UHCI0<'static>,
        dma: DMA_CH1<'static>,
    ) -> Self {
        let config = uart::Config::default()
            .with_baudrate(BAUD)
            .with_rx(RxConfig::default().with_fifo_full_threshold(64));
        let uart = Uart::new(uart, config).unwrap().with_tx(tx).with_rx(rx);
        let mut uhci = Uhci::new(uart, uhci, dma);
        uhci.apply_rx_config(&uhci::RxConfig::default().with_chunk_limit(CHUNK as u16))
            .unwrap();
        // Holds `uart_idle_eof_en` (in a register shared with TX): a descriptor is closed as soon as the line goes
        // idle, so the end of each 1 Hz burst is handed over at once instead of when the 1 KB chunk fills up.
        uhci.apply_tx_config(&uhci::TxConfig::default()).unwrap();
        let (uhci_rx, tx) = uhci.split();
        let ring = dma_rx_stream_buffer!(RING, CHUNK);
        let rx = uhci_rx
            .read(ring)
            .unwrap_or_else(|(e, ..)| panic!("gnss dma: {e:?}"));
        let buf = dma_tx_buffer!(MAX_COMMAND).unwrap();
        Self {
            rx,
            tx: Some((tx, buf)),
        }
    }

    /// Sends `data` (at most [`MAX_COMMAND`] bytes) and waits until it is out: ~0.5 ms for a `$PAIR` command.
    pub fn send(&mut self, data: &[u8]) {
        let (tx, mut buf) = self.tx.take().unwrap();
        buf.as_mut_slice()[..data.len()].copy_from_slice(data);
        buf.set_length(data.len());
        let (tx, buf) = match tx.write(buf) {
            Ok(transfer) => {
                let (res, tx, buf) = transfer.wait();
                if let Err(e) = res {
                    log::warn!("gnss tx: {e:?}");
                }
                (tx, buf)
            }
            Err((e, tx, buf)) => {
                log::warn!("gnss tx: {e:?}");
                (tx, buf)
            }
        };
        self.tx = Some((tx, buf));
    }

    /// Calls `f` with every byte slice received since the last call; returns the byte count.
    pub fn drain(&mut self, mut f: impl FnMut(&[u8])) -> usize {
        let mut total = 0;
        loop {
            let data = self.rx.peek();
            if data.is_empty() {
                return total;
            }
            let n = data.len();
            f(data);
            self.rx.consume(n);
            total += n;
        }
    }
}

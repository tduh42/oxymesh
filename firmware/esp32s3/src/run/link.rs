//! The link to the app: companion frames on a serial wire (USB, `<`/`>` framing, ASI:24-73)
//! and over Bluetooth (`crate::ble::Shared`); every frame from the device goes out on both
//! (`docs/spec/companion.md` §1.3).

use esp_hal::Async;
use esp_hal::uart::Uart;
use mc_node::{Device, Storage};
use mc_proto::{FRAME_MAX, FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
use rand_core::Rng;

use crate::ble::Shared;

/// A serial wire to the computer: the M5's UART0 behind its CH340, the ESP32-S3's own USB on
/// the Heltec.
#[allow(async_fn_in_trait)]
pub trait Wire {
    fn read_ready(&mut self) -> bool;
    /// Until bytes arrive; they are read into `buf`. Dropping it loses nothing.
    async fn read_async(&mut self, buf: &mut [u8]) -> usize;
    /// Bytes read, 0 on error.
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Writes everything, or gives up on an error.
    fn write_all(&mut self, data: &[u8]);
}

impl Wire for Uart<'_, Async> {
    fn read_ready(&mut self) -> bool {
        Uart::read_ready(self)
    }

    async fn read_async(&mut self, buf: &mut [u8]) -> usize {
        Uart::read_async(self, buf).await.unwrap_or(0)
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        Uart::read(self, buf).unwrap_or(0)
    }

    fn write_all(&mut self, data: &[u8]) {
        let mut sent = 0;
        while sent < data.len() {
            match Uart::write(self, data.get(sent..).unwrap_or(&[])) {
                Ok(k) => sent += k,
                Err(_) => break,
            }
        }
    }
}

pub struct AppLink<W: Wire> {
    pub wire: W,
    deframer: UsbDeframer,
    rx_bytes: [u8; 64],
    /// Bytes `wait_input` read, for the next `poll`.
    stash: [u8; 64],
    stashed: usize,
    frame_in: [u8; FRAME_MAX],
    out: [u8; USB_MAX],
}

impl<W: Wire> AppLink<W> {
    pub fn new(wire: W) -> Self {
        Self {
            wire,
            deframer: UsbDeframer::new(TO_DEVICE),
            rx_bytes: [0; 64],
            stash: [0; 64],
            stashed: 0,
            frame_in: [0; FRAME_MAX],
            out: [0; USB_MAX],
        }
    }

    /// Until the computer sends something (kept for the next `poll`); at once if something is
    /// already kept.
    pub async fn wait_input(&mut self) {
        if self.stashed == 0 {
            self.stashed = self.wire.read_async(&mut self.stash).await;
        }
    }

    fn feed(
        &mut self,
        bytes: usize,
        from_stash: bool,
        dev: &mut Device,
        now: mc_core::Instant,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        for i in 0..bytes {
            let b = if from_stash {
                self.stash[i]
            } else {
                self.rx_bytes[i]
            };
            if let Some(f) = self.deframer.push(b) {
                let len = f.len();
                self.frame_in[..len].copy_from_slice(f);
                dev.on_app_frame(now, &self.frame_in[..len], rng, st);
            }
        }
    }

    /// Frames from the app (wire, then Bluetooth) to the device, then the device's frames to
    /// both.
    pub fn poll(
        &mut self,
        dev: &mut Device,
        ble: &Shared,
        now: mc_core::Instant,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        let kept = core::mem::take(&mut self.stashed);
        self.feed(kept, true, dev, now, rng, st);
        while self.wire.read_ready() {
            let n = self.wire.read(&mut self.rx_bytes);
            if n == 0 {
                break;
            }
            self.feed(n.min(self.rx_bytes.len()), false, dev, now, rng, st);
        }
        loop {
            let n = ble.link.borrow_mut().recv(&mut self.frame_in);
            let Some(n) = n else { break };
            dev.on_app_frame(now, &self.frame_in[..n], rng, st);
        }
        // A phone's Bluetooth queue sets the pace: no frame while it is full (none is dropped),
        // and the next contact of a list only once it is empty (`BleLink::ready`). The frames
        // wait in the device; `ble::pump` wakes the loop as it frees a slot.
        loop {
            let (any, contact) = {
                let l = ble.link.borrow();
                let ms = now.as_nanos() / 1_000_000;
                (l.ready(false, ms), l.ready(true, ms))
            };
            if !any {
                break;
            }
            let Some(f) = dev.poll_app(contact) else {
                break;
            };
            if ble.link.borrow_mut().send(&f) {
                ble.outbound.signal(());
            }
            if let Ok(n) = usb_frame(FROM_DEVICE, &f, &mut self.out) {
                self.wire.write_all(&self.out[..n]);
            }
        }
    }
}

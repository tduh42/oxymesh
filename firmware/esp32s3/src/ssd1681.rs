//! SSD1681 e-paper display, 200×200 black and white (GDEY0154D67 panel of the ThinkNode M5).
//!
//! Written from the SSD1681 datasheet (Solomon Systech, rev. 1.0), without taking anything from
//! GxEPD2 (GPL, ADR-0007). Two kinds of refresh:
//! - partial ("display mode 2", ~0.5 s): drives the pixels that differ between the new image
//!   (RAM 0x24) and the old one (RAM 0x26);
//! - full, in two steps (~2.5 s): the whole panel cleared to white with the OTP mode-1
//!   waveform (it clears ghosts), then the image drawn by a partial refresh from white. Some
//!   panels' mode-1 waveform leaves a full-screen image grey, while their mode-2 transitions
//!   are deep; drawn this way every black pixel comes from a mode-2 transition, as MeshCore's
//!   driver does it (`docs/measurements.md` 2026-11-17).
//!
//! The BUSY pin stays high during a refresh: the driver does not wait for it, it exposes
//! [`Ssd1681::busy`] so that the main loop keeps running (radio, BLE), and
//! [`Ssd1681::continue_full`] for the second step of a full refresh.

use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;

pub const WIDTH: u32 = 200;
pub const HEIGHT: u32 = 200;
pub const BUF: usize = (WIDTH * HEIGHT / 8) as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    /// The display did not finish a command in time.
    Timeout,
}

/// In-memory image: 1 bit per pixel, 1 = white (SSD1681 convention). Shared with `mc-ui`, so
/// the emulator and the tests decode exactly what the panel receives.
pub use mc_ui::panel::Frame;

pub struct Ssd1681<'d> {
    spi: Spi<'d, Blocking>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    busy: Input<'d>,
    delay: Delay,
    /// The panel was cleared to white for a full refresh: the image still has to be drawn.
    from_white: bool,
}

impl<'d> Ssd1681<'d> {
    pub fn new(
        spi: Spi<'d, Blocking>,
        cs: Output<'d>,
        dc: Output<'d>,
        rst: Output<'d>,
        busy: Input<'d>,
    ) -> Self {
        Self {
            spi,
            cs,
            dc,
            rst,
            busy,
            delay: Delay::new(),
            from_white: false,
        }
    }

    /// The display is still working (BUSY high).
    pub fn busy(&self) -> bool {
        self.busy.is_high()
    }

    fn wait(&self, ms: u32) -> Result<(), Error> {
        let mut t = 0;
        while self.busy() {
            if t >= ms {
                return Err(Error::Timeout);
            }
            self.delay.delay_millis(1);
            t += 1;
        }
        Ok(())
    }

    fn command(&mut self, cmd: u8, data: &[u8]) -> Result<(), Error> {
        self.cs.set_low();
        self.dc.set_low();
        let r = self.spi.write(&[cmd]).map_err(|_| Error::Spi);
        self.dc.set_high();
        let r = r.and_then(|()| {
            if data.is_empty() {
                Ok(())
            } else {
                self.spi.write(data).map_err(|_| Error::Spi)
            }
        });
        self.cs.set_high();
        r
    }

    /// Reset and settings (datasheet §8): 200 lines, increasing addresses, full-screen window,
    /// internal temperature sensor.
    pub fn init(&mut self) -> Result<(), Error> {
        self.rst.set_low();
        self.delay.delay_millis(10);
        self.rst.set_high();
        self.delay.delay_millis(10);
        self.wait(100)?;
        self.command(0x12, &[])?; // SWRESET
        self.wait(100)?;
        self.command(0x01, &[0xC7, 0x00, 0x00])?; // Driver output control: MUX = 199
        self.command(0x11, &[0x03])?; // Data entry mode: X+, Y+
        self.command(0x44, &[0x00, 0x18])?; // RAM X: bytes 0 to 24
        self.command(0x45, &[0x00, 0x00, 0xC7, 0x00])?; // RAM Y: lines 0 to 199
        self.command(0x3C, &[0x05])?; // Border waveform
        self.command(0x18, &[0x80])?; // Internal temperature sensor
        self.from_white = false;
        Ok(())
    }

    /// A RAM filled with one byte (0xFF: white).
    fn fill_ram(&mut self, cmd: u8, byte: u8) -> Result<(), Error> {
        self.command(0x4E, &[0x00])?;
        self.command(0x4F, &[0x00, 0x00])?;
        let chunk = [byte; 100];
        self.cs.set_low();
        self.dc.set_low();
        let mut r = self.spi.write(&[cmd]).map_err(|_| Error::Spi);
        self.dc.set_high();
        for _ in 0..BUF / chunk.len() {
            r = r.and_then(|()| self.spi.write(&chunk).map_err(|_| Error::Spi));
        }
        self.cs.set_high();
        r
    }

    fn write_ram(&mut self, cmd: u8, frame: &Frame) -> Result<(), Error> {
        self.command(0x4E, &[0x00])?;
        self.command(0x4F, &[0x00, 0x00])?;
        self.command(cmd, &frame.bits)
    }

    /// Full refresh, first step: the panel cleared to white (~2 s). Returns immediately; once
    /// BUSY drops, [`Ssd1681::continue_full`] draws the image (or the next partial refresh
    /// does, from white).
    pub fn show_full(&mut self, _frame: &Frame) -> Result<(), Error> {
        self.wait(5000)?;
        // Border back to the waveform's white: a partial refresh set it to "unchanged".
        self.command(0x3C, &[0x05])?;
        self.fill_ram(0x24, 0xFF)?;
        self.fill_ram(0x26, 0xFF)?;
        self.command(0x22, &[0xF7])?; // clock, temperature, OTP LUT, mode 1
        self.command(0x20, &[])?;
        self.from_white = true;
        Ok(())
    }

    /// A full refresh is halfway: the panel is white, the image not drawn yet.
    pub fn full_pending(&self) -> bool {
        self.from_white
    }

    /// Full refresh, second step: the image drawn from white by a partial refresh (~0.5 s).
    /// Does nothing if no full refresh is halfway.
    pub fn continue_full(&mut self, frame: &Frame) -> Result<(), Error> {
        if !self.from_white {
            return Ok(());
        }
        self.show_partial(frame, frame)
    }

    /// After the first step of a full refresh, the panel is white whatever `previous` says.
    pub fn show_partial(&mut self, frame: &Frame, previous: &Frame) -> Result<(), Error> {
        self.wait(5000)?;
        self.command(0x3C, &[0x80])?; // border unchanged in partial mode
        if self.from_white {
            self.fill_ram(0x26, 0xFF)?;
            self.from_white = false;
        } else {
            self.write_ram(0x26, previous)?;
        }
        self.write_ram(0x24, frame)?;
        self.command(0x22, &[0xFF])?; // mode 2: partial
        self.command(0x20, &[])
    }

    /// Deep sleep (near-zero consumption, the image stays).
    pub fn sleep(&mut self) -> Result<(), Error> {
        self.wait(5000)?;
        self.command(0x10, &[0x01])
    }
}

//! A server on the ThinkNode M5 (`docs/spec/server.md` §6): MeshCore's `simple_repeater` or
//! `simple_room_server` (v1.18) in Rust, with status pages on the e-paper. Shared by the
//! `rpt-m5` and `room-m5` binaries.
//!
//! `mc-node` in server mode; `mc-server` serves logins, requests, the console over the air, and
//! the role's own work (neighbours and discovery, or posts). UART0 (CH340, 115,200 baud)
//! carries the C++ server's text console. Storage in `mcdata`; at first boot the C++ firmware's
//! SPIFFS is read once. The GPS (the `gps` commands) gives the position and the clock; the
//! clock chip keeps the time. No Bluetooth, like the C++ servers. Buttons: the lower one shows
//! the next page (long press acts), the top one the previous page, and wakes the board after
//! power-off.
use core::cell::RefCell;
use core::fmt::Write;

use embassy_futures::select::{Either4, select, select4};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::iso_8859_1::FONT_10X20;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{
    Event as PinEvent, Input, InputConfig, Level, Output, OutputConfig, Pull, WakeupConfig,
};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_storage::FlashStorage;
use log::info;
use mc_node::{Node, Prefs};
use mc_server::neighbours::Neighbours;
use mc_server::server::{Action, Gps as GpsState, default_node_prefs};
use mc_server::{Role, Server};
use mc_ui::input::{Button, Gestures};
use mc_ui::panel;
use mc_ui::refresh::{self, Kind, Policy};
use mc_ui::server::{Act, Pages, View};
use mc_ui::status::Screen;
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::gps;
use oxymesh_esp32s3::m5;
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::pcf8563::Pcf8563;
use oxymesh_esp32s3::run::radio::{Chip, NoFrontEnd, Radio, Tx, lora_of};
use oxymesh_esp32s3::run::server::Console;
use oxymesh_esp32s3::run::sleep::{self, Saver};
use oxymesh_esp32s3::run::{BOOT_EPOCH, BUILD_DATE, Deadline, boot, crash, mc_now, server};
use oxymesh_esp32s3::ssd1681::{Frame, Ssd1681};
use oxymesh_esp32s3::sx126x::Sx126x;
use oxymesh_esp32s3::tsens::ChipTemp;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::{ConstStaticCell, StaticCell};

const VERSION: &str = "v1.18.0-oxymesh";
const MANUFACTURER: &str = "Elecrow ThinkNode M5";
const HEAP_SIZE: usize = 24 * 1024;
const HEAP_RECLAIMED: usize = 72 * 1024;
/// The e-paper is redrawn at most this often, unless a button was pressed.
const REDRAW_MS: u64 = 30_000;

static NODE: ConstStaticCell<Node> = ConstStaticCell::new(Node::empty());
/// What was sent to the panel (current, previous) and what is drawn.
static FRAMES: StaticCell<[Frame; 3]> = StaticCell::new();

/// What the panel receives: the drawn image, inverted and/or turned half a turn.
fn to_panel(src: &Frame, dst: &mut Frame, invert: bool, rotate: bool) {
    for (i, d) in dst.bits.iter_mut().enumerate() {
        let b = if rotate {
            src.bits
                .get(panel::BUF - 1 - i)
                .copied()
                .unwrap_or(0xFF)
                .reverse_bits()
        } else {
            src.bits.get(i).copied().unwrap_or(0xFF)
        };
        *d = if invert { !b } else { b };
    }
}

/// `ADVERT_NAME` of the C++ builds, by role.
fn name_of(role: Role) -> &'static [u8] {
    match role {
        Role::Repeater => b"Thinknode M5 Repeater",
        Role::Room => b"Thinknode M5 Room Server",
    }
}

/// The server on this board: a repeater or a room server.
#[allow(clippy::large_stack_frames, reason = "the server's tables in main")]
pub async fn run(role: Role) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: HEAP_RECLAIMED);
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    crash::note();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // From here on a loop that stops turning restarts the chip (ADR-0013).
    let mut watchdog = crash::Watchdog::start(p.RTC_TIMER);
    let lpwr = p.LPWR;
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh on the M5: {}", role.name());

    let out = OutputConfig::default();
    let _lora_en = Output::new(p.GPIO46, Level::High, out);
    let i2c = I2c::new(
        p.I2C1,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C")
    .with_sda(p.GPIO48)
    .with_scl(p.GPIO47);
    let mut io = Pca9557::new(i2c).ok();
    let mut rtc_chip = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .ok()
    .map(|i2c| Pcf8563::new(i2c.with_sda(p.GPIO2).with_scl(p.GPIO1)));
    // The peripherals' rail (e-ink, GPS, LoRa) on before any of them is set up
    // (`docs/measurements.md` 2026-11-17).
    if let Some(io) = io.as_mut() {
        let _ = io.set(pin::LED_ENABLE, true);
        let _ = io.set(pin::PERIPH_POWER, true);
    }
    Timer::after(Duration::from_millis(20)).await;

    // GPS, wired by board revision (`docs/spec/m5.md` §2).
    let highs = (0..10)
        .filter(|_| {
            io.as_mut()
                .and_then(|io| io.read(pin::GPS_VERSION).ok())
                .unwrap_or(false)
        })
        .count();
    let revision = if highs > 5 {
        gps::Revision::V2
    } else {
        gps::Revision::V1
    };
    let uart = Uart::new(p.UART1, UartConfig::default().with_baudrate(9600)).ok();
    let mut gps_rx = match revision {
        gps::Revision::V1 => uart.map(|u| {
            gps::Gps::new(
                u.with_rx(p.GPIO19).with_tx(p.GPIO20),
                revision.receiver(),
                gps::Pins {
                    enable: Some(Output::new(p.GPIO11, Level::Low, out)),
                    reset: Some(Output::new(p.GPIO13, Level::Low, out)),
                    rail: None,
                },
            )
        }),
        gps::Revision::V2 => uart.map(|u| {
            gps::Gps::new(
                u.with_rx(p.GPIO20).with_tx(p.GPIO19),
                revision.receiver(),
                gps::Pins {
                    enable: None,
                    reset: None,
                    rail: Some(Output::new(p.GPIO10, Level::Low, out)),
                },
            )
        }),
    };

    // Storage: `mcdata`, and the C++ firmware's SPIFFS, read only (ADR-0006).
    let flash = RefCell::new(FlashStorage::new(p.FLASH).multicore_auto_park());
    let (data_part, cpp_part) = boot::partitions(&flash);
    let mut store = boot::open_store(&flash, data_part);
    let cpp_fs = || boot::cpp_fs(&flash, cpp_part);
    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
    // Storage at boot has no fixed bound (a first boot imports the C++ firmware's data): the
    // watchdog waits for the main loop (ADR-0013).
    watchdog.stop();
    let id = server::identity(role, &mut store, cpp_fs, &hw);
    let mut prefs = Prefs::defaults(id.pub_key());
    default_node_prefs(&mut prefs, role);
    prefs.name = [0; 32];
    let name = name_of(role);
    prefs.name[..name.len()].copy_from_slice(name);
    let node = NODE.take();
    node.reset(id, prefs, BOOT_EPOCH, mc_now());
    let mut rpt = Server::new(mc_now(), VERSION, role);
    // `MAX_NEIGHBOURS` of the C++ repeater build; the room's `ROOM_PASSWORD`.
    rpt.neighbours = Neighbours::with_limit(8);
    if role == Role::Room {
        let _ = rpt.prefs.guest_password.push_str("hello");
    }
    store.load_server(node, &mut rpt);
    watchdog.resume();
    if let Some(t) = rtc_chip.as_mut().and_then(Pcf8563::read)
        && t >= BOOT_EPOCH
    {
        node.rtc.set(mc_now(), t);
    }
    rpt.board.manufacturer = MANUFACTURER;
    rpt.board.build_date = BUILD_DATE;
    rpt.board.reset_reason = crash::restart();
    // Light sleep (`powersaving on`, P2).
    rpt.board.powersaving = true;
    rpt.board.shutdown_reason = "Not available";

    // Radio.
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .expect("SPI")
    .with_sck(p.GPIO16)
    .with_mosi(p.GPIO15)
    .with_miso(p.GPIO7);
    let tx_dbm = node.apply_regulatory(|t| t);
    let mut radio = Radio::new(
        Sx126x::new(
            spi,
            Output::new(p.GPIO17, Level::High, out),
            Output::new(p.GPIO6, Level::High, out),
            Input::new(p.GPIO5, InputConfig::default()),
            Input::new(p.GPIO4, InputConfig::default()),
        ),
        NoFrontEnd,
        Chip {
            tcxo_millivolts: m5::TCXO_MILLIVOLTS,
            dcdc: true,
            ocp_ma: m5::OCP_MA,
            rx_patch: false,
        },
        &node.prefs,
        tx_dbm,
    );

    // Display, buttons, battery.
    let espi = Spi::new(
        p.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    )
    .expect("display SPI")
    .with_sck(p.GPIO38)
    .with_mosi(p.GPIO45);
    let mut epd = Ssd1681::new(
        espi,
        Output::new(p.GPIO39, Level::High, out),
        Output::new(p.GPIO40, Level::High, out),
        Output::new(p.GPIO41, Level::High, out),
        Input::new(p.GPIO42, InputConfig::default()),
    );
    let _ = epd.init();
    let frames = FRAMES.init([Frame::new(), Frame::new(), Frame::new()]);
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    let mut btn_a = Input::new(p.GPIO21, pull_up);
    let mut btn_b = Input::new(p.GPIO14, pull_up);
    let usb_in = Input::new(p.GPIO12, InputConfig::default());
    let mut adc_cfg = AdcConfig::new();
    let mut bat_pin = adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<esp_hal::peripherals::ADC1>>(
        p.GPIO8,
        Attenuation::_11dB,
    );
    let mut adc = Adc::new(p.ADC1, adc_cfg);
    let mut bat = Smoother::new();
    let chip_temp = ChipTemp::new();

    let mut wire = Uart::new(p.UART0, UartConfig::default())
        .expect("UART0")
        .with_tx(p.GPIO43)
        .with_rx(p.GPIO44)
        .into_async();
    // Typing on the console wakes it from light sleep (the first characters are lost).
    let _ = wire.enable_wakeup(&esp_hal::uart::WakeupConfig::default());
    let mut low = LowPower::new(lpwr);
    let mut saver = Saver::new();
    let mut console = Console::new(role);
    rpt.start(node, mc_now());
    console.banner(&mut wire, node);

    let mut pages = Pages::new();
    let mut gestures = Gestures::new();
    let mut policy = Policy::new();
    let mut shown: Option<Screen> = None;
    let mut next_draw = 0u64;
    let mut next_battery = Instant::now();
    let mut battery_percent = None;
    // Until then, the buttons are sampled every 10 ms after an edge (debounce).
    let mut buttons_settle = 0u64;
    loop {
        watchdog.feed();
        let now = mc_now();
        console.poll(&mut wire, &mut rpt, node, now, &mut rng);
        if let Some(t) = radio.poll(node, now, &mut rng)
            && let Some(io) = io.as_mut()
        {
            let _ = io.set(pin::LED_NOTIFY, t == Tx::Started);
        }
        while let Some(ev) = node.poll_event() {
            rpt.on_event(node, &ev, now, &mut rng);
        }
        rpt.board.noise_floor = radio.noise_floor();

        // GPS: on as the setting says; its fix for the node, its time for the clock.
        if let Some(g) = gps_rx.as_mut() {
            let want = node.prefs.gps_enabled;
            if want != g.is_on() {
                g.power(want).await;
            }
            let (fix, t) = g.poll();
            if let Some(f) = fix {
                node.gps_pos = Some((f.lat, f.lon));
                node.gps_alt_mm = f.alt_mm;
            }
            if let Some(t) = t {
                node.rtc.set(now, t);
                if let Some(c) = rtc_chip.as_mut() {
                    let _ = c.write(t);
                }
            }
            let mut diag = heapless::String::new();
            let _ = write!(
                diag,
                "en:{} ub:{} ok:{} bad:{} sat:{} fix:{}",
                u8::from(g.is_on()),
                g.bytes,
                g.sentences,
                g.errors,
                g.satellites(),
                u8::from(fix.is_some())
            );
            rpt.board.gps = Some(GpsState {
                enabled: g.is_on(),
                active: node.prefs.gps_enabled,
                fix: fix.is_some(),
                sats: i32::from(g.satellites()),
                diag,
            });
        }
        rpt.tick(node, now, &mut rng);

        let mut power_off = false;
        while let Some(a) = rpt.poll_action() {
            if rpt.save(node, a, &mut store) {
                continue;
            }
            match a {
                Action::Reboot => {
                    // The access list's lazy save would be lost.
                    if rpt.take_pending_acl_save() {
                        rpt.save(node, Action::SaveAcl, &mut store);
                    }
                    Timer::after(Duration::from_millis(200)).await;
                    esp_hal::system::software_reset();
                }
                Action::PowerOff => power_off = true,
                Action::Erase => {
                    watchdog.stop();
                    store.erase_all();
                    watchdog.resume();
                }
                Action::RadioChanged | Action::RestoreRadio => {
                    let tx = node.apply_regulatory(|t| t);
                    radio.reconfigure(&node.prefs, tx);
                }
                Action::TempRadio(params) => {
                    let mut tp = node.prefs.clone();
                    tp.radio = params;
                    radio.retune(lora_of(&tp));
                }
                Action::Gps(on) => node.prefs.gps_enabled = on,
                Action::GpsInterval(s) => node.prefs.gps_interval = s,
                Action::ClockSet => {
                    let t = node.rtc.now(now);
                    if let Some(c) = rtc_chip.as_mut() {
                        let _ = c.write(t);
                    }
                }
                _ => {}
            }
        }

        // Battery and chip temperature every 10 s.
        if Instant::now() >= next_battery {
            next_battery = Instant::now() + Duration::from_secs(10);
            rpt.board.mcu_temp_deci = chip_temp.read();
            let raw_mv = adc.read_blocking(&mut bat_pin);
            rpt.board.battery_mv =
                bat.push((u32::from(raw_mv) * m5::BATTERY_DIVIDER_MILLI / 1000) as u16);
            rpt.board.usb_powered = Some(usb_in.is_high());
            battery_percent = (rpt.board.battery_mv > 0).then(|| percent(rpt.board.battery_mv));
        }

        // Buttons: the lower one is A, the top one B.
        let now_ms = Instant::now().as_millis();
        gestures.edge(Button::A, btn_a.is_low(), now_ms);
        gestures.edge(Button::B, btn_b.is_low(), now_ms);
        while let Some(g) = gestures.poll(now_ms) {
            policy.input(now_ms);
            next_draw = 0;
            match pages.gesture(g) {
                Some(Act::Advert) => {
                    rpt.send_advert(node, mc_now(), 0, true);
                    pages.done(Act::Advert, now_ms);
                }
                Some(Act::PowerOff) => power_off = true,
                None => {}
            }
        }
        if pages.tick(now_ms) {
            next_draw = 0;
        }

        if power_off {
            watchdog.stop();
            // The access list's lazy save would be lost.
            if rpt.take_pending_acl_save() {
                rpt.save(node, Action::SaveAcl, &mut store);
            }
            let [cur, _, draw] = &mut *frames;
            draw.clear();
            let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
            let name = core::str::from_utf8(mc_node::contacts::name_str(&node.prefs.name))
                .unwrap_or("Repeater");
            let _ = Text::new(name, Point::new(4, 30), big).draw(draw);
            let _ = Text::new("Off.", Point::new(4, 90), big).draw(draw);
            let _ = Text::new("Press the top", Point::new(4, 130), big).draw(draw);
            let _ = Text::new("button.", Point::new(4, 155), big).draw(draw);
            let u = node.prefs.ui;
            to_panel(draw, cur, u.invert, u.rotate);
            let _ = epd.show_full(cur);
            while epd.busy() {
                Timer::after(Duration::from_millis(20)).await;
            }
            let _ = epd.continue_full(cur);
            while epd.busy() {
                Timer::after(Duration::from_millis(20)).await;
            }
            let _ = epd.sleep();
            radio.sleep();
            if let Some(g) = gps_rx.as_mut() {
                g.power(false).await;
            }
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, false);
                let _ = io.set(pin::PERIPH_POWER, false);
            }
            while btn_b.is_low() {
                Timer::after(Duration::from_millis(20)).await;
            }
            btn_b.listen(PinEvent::LowLevel);
            let _ = btn_b.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
            low.sleep_deep(RtcSleepConfig::deep());
        }

        // A full refresh cleared the panel to white: now the page.
        if !epd.busy() && epd.full_pending() {
            let [cur, _, _] = &mut *frames;
            let _ = epd.continue_full(cur);
        }
        // The page, at most every 30 s unless a button was pressed.
        if !epd.busy() && now_ms >= next_draw {
            next_draw = now_ms + REDRAW_MS;
            let v = View {
                node,
                server: &rpt,
                now: mc_now(),
                battery_percent,
                charging: usb_in.is_high(),
                tz_offset_min: node.prefs.ui.tz_offset_min,
            };
            let s = pages.screen(&v);
            if shown.as_ref() != Some(&s) {
                let changed = shown
                    .as_ref()
                    .is_none_or(|o| o.lines.first() != s.lines.first());
                let [cur, prev, draw] = &mut *frames;
                mc_ui::server::render_epd(&s, draw);
                prev.bits = cur.bits;
                let u = node.prefs.ui;
                to_panel(draw, cur, u.invert, u.rotate);
                match policy.frame(refresh::hash(&cur.bits), changed) {
                    Some(Kind::Full) => {
                        let _ = epd.show_full(cur);
                    }
                    Some(Kind::Partial) => {
                        let _ = epd.show_partial(cur, prev);
                    }
                    None => {}
                }
                shown = Some(s);
            } else if let Some(Kind::Full) = policy.idle(now_ms) {
                let [cur, _, _] = &mut *frames;
                let _ = epd.show_full(cur);
            }
        }

        // Sleep until something is due or happens (P1): the radio, a button, the console, or
        // the earliest deadline (at most a second).
        let now_ms = Instant::now().as_millis();
        let mut due = Deadline::within(Duration::from_secs(1));
        due.at_mc(node.next_wakeup());
        due.at_mc(rpt.next_wakeup());
        due.at(radio.next_deadline());
        due.at(next_battery);
        due.at_ms(next_draw);
        if epd.full_pending() {
            due.at_ms(now_ms + 20);
        }
        if let Some(t) = gestures.next_deadline() {
            due.at_ms(t);
        }
        if now_ms < buttons_settle
            || btn_a.is_low() != gestures.down(Button::A)
            || btn_b.is_low() != gestures.down(Button::B)
        {
            due.at_ms(now_ms + 10);
        }
        if epd.busy() {
            due.at_ms(now_ms + 20);
        }
        if gps_rx.as_ref().is_some_and(|g| g.is_on()) {
            // UART1 at 9600 baud: its 128-byte FIFO fills in about 130 ms.
            due.at_ms(now_ms + 50);
        }
        let deadline = due.get();
        // Power saving (P2): light sleep when nothing is going on, until the server's next
        // job, the battery, the page, at most 30 s.
        let pending = node.next_wakeup().is_some()
            || radio.transmitting()
            || epd.busy()
            || epd.full_pending();
        // Only the repeater sleeps: the C++ room server keeps the setting and never does
        // (`simple_room_server/main.cpp` has no `board.sleep`).
        if role == Role::Repeater
            && saver.may_sleep(rpt.prefs.powersaving, pending, now_ms)
            && now_ms >= buttons_settle
            && btn_a.is_high()
            && btn_b.is_high()
        {
            let mut until = Deadline::within(sleep::SLEEP_MAX);
            until.at_mc(rpt.next_wakeup());
            until.at(next_battery);
            until.at_ms(next_draw);
            let person = saver.sleep(
                &mut low,
                until.get(),
                radio.irq_pin(),
                &mut [
                    (&mut btn_a, PinEvent::LowLevel),
                    (&mut btn_b, PinEvent::LowLevel),
                ],
            );
            if person {
                saver.used(Instant::now().as_millis());
                buttons_settle = Instant::now().as_millis() + 60;
            }
        } else if deadline <= Instant::now() {
            embassy_futures::yield_now().await;
        } else {
            let woke = select4(
                Timer::at(deadline),
                radio.wait_irq(),
                select(btn_a.wait_for_any_edge(), btn_b.wait_for_any_edge()),
                console.wait_input(&mut wire),
            )
            .await;
            match woke {
                Either4::Third(_) => {
                    buttons_settle = Instant::now().as_millis() + 60;
                    saver.used(Instant::now().as_millis());
                }
                Either4::Fourth(()) => saver.used(Instant::now().as_millis()),
                _ => {}
            }
        }
    }
}

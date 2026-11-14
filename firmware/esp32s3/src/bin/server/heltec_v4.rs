//! A server on the Heltec WiFi LoRa 32 V4 (`docs/spec/server.md` §6): MeshCore's
//! `simple_repeater` or `simple_room_server` (v1.18) in Rust, with status pages on the OLED.
//! Shared by the `rpt-heltec-v4` and `room-heltec-v4` binaries.
//!
//! `mc-node` in server mode; `mc-server` serves logins, requests, the console over the air, and
//! the role's own work. The USB carries the C++ server's text console (`Repeater ID: …` or
//! `Room ID: …`, then one command per line, replies as `  -> …`). Storage in `mcdata`; at first
//! boot the C++ firmware's SPIFFS is read once (a server's settings, access list and regions; a
//! companion's identity and radio). No Bluetooth, like the C++ servers. One button: click =
//! next page, double click = previous, long press = act.
use core::cell::RefCell;

use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant, Timer};
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
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use esp_storage::FlashStorage;
use log::info;
use mc_node::{Node, Prefs};
use mc_server::neighbours::Neighbours;
use mc_server::server::{Action, default_node_prefs};
use mc_server::{Role, Server};
use mc_ui::input::{Button, Gestures};
use mc_ui::server::{Act, Pages, View};
use mc_ui::status::{Oled, Screen};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::heltec_v4::{self as board, Fem, UsbWire};
use oxymesh_esp32s3::run::radio::{Chip, Radio, Tx, lora_of};
use oxymesh_esp32s3::run::server::Console;
use oxymesh_esp32s3::run::sleep::{self, Saver};
use oxymesh_esp32s3::run::{Deadline, boot, mc_now, server};
use oxymesh_esp32s3::ssd1306::Ssd1306;
use oxymesh_esp32s3::sx126x::Sx126x;
use oxymesh_esp32s3::tsens::ChipTemp;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::{ConstStaticCell, StaticCell};

const VERSION: &str = "v1.18.0-oxymesh";
const BUILD_DATE: &str = "10-Nov-2026";
/// Heap for the status pages (text, a few KB).
const HEAP_SIZE: usize = 24 * 1024;
const HEAP_RECLAIMED: usize = 72 * 1024;
/// The clock at boot until an admin sets it (no clock chip on this board).
const BOOT_EPOCH: u32 = 1_793_900_000;
const UI_TICK_MS: u64 = 500;
const SCREEN_TIMEOUT_MS: u64 = 60_000;

static NODE: ConstStaticCell<Node> = ConstStaticCell::new(Node::empty());
static OLED: StaticCell<Oled> = StaticCell::new();

/// `ADVERT_NAME` of the C++ builds, by role.
fn name_of(role: Role) -> &'static [u8] {
    match role {
        Role::Repeater => b"Heltec Repeater",
        Role::Room => b"Heltec Room",
    }
}

/// The server on this board: a repeater or a room server.
#[allow(clippy::large_stack_frames, reason = "the server's tables in main")]
pub async fn run(role: Role) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: HEAP_RECLAIMED);
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let lpwr = p.LPWR;
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh on the Heltec V4: {}", role.name());

    let out = OutputConfig::default();
    // Vext (active low) powers the OLED.
    let _vext = Output::new(p.GPIO36, Level::Low, out);
    Timer::after(Duration::from_millis(5)).await;
    let mut oled_reset = Output::new(p.GPIO21, Level::Low, out);
    Timer::after(Duration::from_millis(2)).await;
    oled_reset.set_high();
    Timer::after(Duration::from_millis(2)).await;
    let mut oled = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .ok()
    .map(|i2c| Ssd1306::new(i2c.with_sda(p.GPIO17).with_scl(p.GPIO18)));
    if let Some(o) = oled.as_mut()
        && o.init().is_err()
    {
        oled = None;
    }

    // Storage: `mcdata`, and the C++ firmware's SPIFFS, read only (ADR-0006).
    let flash = RefCell::new(FlashStorage::new(p.FLASH).multicore_auto_park());
    let (data_part, cpp_part) = boot::partitions(&flash);
    let mut store = boot::open_store(&flash, data_part);
    let cpp_fs = || boot::cpp_fs(&flash, cpp_part);
    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
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
    rpt.neighbours = Neighbours::with_limit(50);
    if role == Role::Room {
        let _ = rpt.prefs.guest_password.push_str("hello");
    }
    store.load_server(node, &mut rpt);

    // Front end, then radio.
    let fem = Fem::new(p.GPIO7, p.GPIO2, p.GPIO5, p.GPIO46, node.prefs.fem_rxgain);
    let front_end = fem.kind;
    rpt.board.manufacturer = fem.board_name();
    rpt.board.build_date = BUILD_DATE;
    rpt.board.front_end = front_end;
    rpt.board.reset_reason = "Not available";
    // Light sleep (`powersaving on`, P2).
    rpt.board.powersaving = true;
    rpt.board.shutdown_reason = "Not available";
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .expect("SPI")
    .with_sck(p.GPIO9)
    .with_mosi(p.GPIO10)
    .with_miso(p.GPIO11);
    let tx_dbm = node.apply_regulatory(|t| front_end.gain_db(t));
    let mut radio = Radio::new(
        Sx126x::new(
            spi,
            Output::new(p.GPIO8, Level::High, out),
            Output::new(p.GPIO12, Level::High, out),
            Input::new(p.GPIO13, InputConfig::default()),
            Input::new(p.GPIO14, InputConfig::default()),
        ),
        fem,
        Chip {
            tcxo_millivolts: board::TCXO_MILLIVOLTS,
            dcdc: true,
            ocp_ma: board::OCP_MA,
            rx_patch: true,
        },
        &node.prefs,
        tx_dbm,
    );

    // Button, LED, battery.
    let mut button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    let mut led = Output::new(p.GPIO35, Level::Low, out);
    let mut adc_ctrl = Output::new(p.GPIO37, Level::Low, out);
    let mut adc_cfg = AdcConfig::new();
    let mut bat_pin = adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<esp_hal::peripherals::ADC1>>(
        p.GPIO1,
        Attenuation::_2p5dB,
    );
    let mut adc = Adc::new(p.ADC1, adc_cfg);
    let mut bat = Smoother::new();
    let chip_temp = ChipTemp::new();

    let mut wire = UsbWire {
        usb: UsbSerialJtag::new(p.USB_DEVICE).into_async(),
    };
    let mut console = Console::new(role);
    rpt.start(node, mc_now());
    console.banner(&mut wire, node);

    let frame = OLED.init(Oled::new());
    let mut pages = Pages::new();
    let mut gestures = Gestures::new();
    let mut shown: Option<Screen> = None;
    let mut screen_on = true;
    let mut screen_until = Instant::now().as_millis() + SCREEN_TIMEOUT_MS;
    let mut next_ui_tick = 0u64;
    let mut next_battery = Instant::now();
    let mut battery_percent = None;
    // Until then, the button is sampled every 10 ms after an edge (debounce).
    let mut button_settle = 0u64;
    let mut low = LowPower::new(lpwr);
    let mut saver = Saver::new();
    loop {
        let now = mc_now();
        console.poll(&mut wire, &mut rpt, node, now, &mut rng);
        match radio.poll(node, now, &mut rng) {
            Some(Tx::Started) => led.set_high(),
            Some(Tx::Done) => led.set_low(),
            None => {}
        }
        while let Some(ev) = node.poll_event() {
            rpt.on_event(node, &ev, now, &mut rng);
        }
        rpt.board.noise_floor = radio.noise_floor();
        rpt.tick(node, now, &mut rng);

        let mut power_off = false;
        while let Some(a) = rpt.poll_action() {
            if rpt.save(node, a, &mut store) {
                continue;
            }
            match a {
                Action::Reboot => {
                    Timer::after(Duration::from_millis(200)).await;
                    esp_hal::system::software_reset();
                }
                Action::PowerOff => power_off = true,
                Action::Erase => store.erase_all(),
                Action::RadioChanged | Action::RestoreRadio => {
                    let tx = node.apply_regulatory(|t| front_end.gain_db(t));
                    radio.reconfigure(&node.prefs, tx);
                }
                Action::TempRadio(params) => {
                    let mut tp = node.prefs.clone();
                    tp.radio = params;
                    radio.retune(lora_of(&tp));
                }
                // No GPS on this board; saves are done above.
                _ => {}
            }
        }

        // Battery and chip temperature every 10 s.
        if Instant::now() >= next_battery {
            next_battery = Instant::now() + Duration::from_secs(10);
            rpt.board.mcu_temp_deci = chip_temp.read();
            adc_ctrl.set_high();
            Timer::after(Duration::from_millis(10)).await;
            let raw_mv = adc.read_blocking(&mut bat_pin);
            adc_ctrl.set_low();
            rpt.board.battery_mv =
                bat.push((u32::from(raw_mv) * board::BATTERY_MULT_MILLI / 1000) as u16);
            battery_percent = (rpt.board.battery_mv > 0).then(|| percent(rpt.board.battery_mv));
        }

        // The button: the first press of a dark screen only lights it.
        let now_ms = Instant::now().as_millis();
        gestures.edge(Button::A, button.is_low(), now_ms);
        while let Some(g) = gestures.poll(now_ms) {
            screen_until = now_ms + SCREEN_TIMEOUT_MS;
            if !screen_on {
                screen_on = true;
                if let Some(o) = oled.as_mut() {
                    let _ = o.power(true);
                }
                shown = None;
                continue;
            }
            match pages.gesture(g) {
                Some(Act::Advert) => {
                    rpt.send_advert(node, mc_now(), 0, true);
                    pages.done(Act::Advert, now_ms);
                }
                Some(Act::PowerOff) => power_off = true,
                None => {}
            }
        }
        if power_off {
            if let Some(o) = oled.as_mut() {
                let _ = o.power(false);
            }
            radio.sleep();
            led.set_low();
            while button.is_low() {
                Timer::after(Duration::from_millis(20)).await;
            }
            button.listen(PinEvent::LowLevel);
            let _ = button.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
            low.sleep_deep(RtcSleepConfig::deep());
        }

        if screen_on && now_ms >= screen_until {
            screen_on = false;
            if let Some(o) = oled.as_mut() {
                let _ = o.power(false);
            }
        }
        if now_ms >= next_ui_tick {
            next_ui_tick = now_ms + UI_TICK_MS;
            pages.tick(now_ms);
            if screen_on && let Some(o) = oled.as_mut() {
                let v = View {
                    node,
                    server: &rpt,
                    now: mc_now(),
                    battery_percent,
                    charging: false,
                    tz_offset_min: node.prefs.ui.tz_offset_min,
                };
                let s = pages.screen(&v);
                if shown.as_ref() != Some(&s) {
                    mc_ui::status::render(&s, frame);
                    let _ = o.show(frame);
                    shown = Some(s);
                }
            }
        }

        // Sleep until something is due or happens (P1): the radio, the button, the console,
        // or the earliest deadline (at most a second).
        let now_ms = Instant::now().as_millis();
        let mut due = Deadline::within(Duration::from_secs(1));
        due.at_mc(node.next_wakeup());
        due.at_mc(rpt.next_wakeup());
        due.at(radio.next_deadline());
        due.at(next_battery);
        if screen_on {
            due.at_ms(next_ui_tick);
            due.at_ms(screen_until);
        }
        if let Some(t) = gestures.next_deadline() {
            due.at_ms(t);
        }
        if now_ms < button_settle || button.is_low() != gestures.down(Button::A) {
            due.at_ms(now_ms + 10);
        }
        let deadline = due.get();
        // Power saving (P2): light sleep when nothing is going on and the screen is dark,
        // until the server's next job or the battery, at most 30 s. The USB console is gone
        // while asleep (the chip's own USB sleeps too).
        let pending = node.next_wakeup().is_some() || radio.transmitting();
        // Only the repeater sleeps: the C++ room server keeps the setting and never does
        // (`simple_room_server/main.cpp` has no `board.sleep`).
        if role == Role::Repeater
            && saver.may_sleep(rpt.prefs.powersaving, pending, now_ms)
            && !screen_on
            && now_ms >= button_settle
            && button.is_high()
        {
            let mut until = Deadline::within(sleep::SLEEP_MAX);
            until.at_mc(rpt.next_wakeup());
            until.at(next_battery);
            let person = saver.sleep(
                &mut low,
                until.get(),
                radio.irq_pin(),
                &mut [(&mut button, PinEvent::LowLevel)],
            );
            if person {
                saver.used(Instant::now().as_millis());
                button_settle = Instant::now().as_millis() + 60;
            }
        } else if deadline <= Instant::now() {
            embassy_futures::yield_now().await;
        } else {
            let woke = select4(
                Timer::at(deadline),
                radio.wait_irq(),
                button.wait_for_any_edge(),
                console.wait_input(&mut wire),
            )
            .await;
            match woke {
                Either4::Third(_) => {
                    button_settle = Instant::now().as_millis() + 60;
                    saver.used(Instant::now().as_millis());
                }
                Either4::Fourth(()) => saver.used(Instant::now().as_millis()),
                _ => {}
            }
        }
    }
}

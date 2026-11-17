//! `mc`: tools around a MeshCore companion connected over USB.

mod airtest;
mod ble;
mod describe;
mod duo;
mod png;
mod port;
mod stop;
mod ui;

use clap::{Parser, Subcommand};
use mc_air::crypto::ChannelSecret;
use mc_proto::codec::APP_VER;
use mc_proto::{Command, Response};

use crate::describe::{Keys, describe};
use crate::port::Port;

#[derive(Parser)]
#[command(name = "mc", about = "oxymesh tools for MeshCore companions")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Every `port` is a serial path, or `ble:<name>` for a companion over Bluetooth (the name with
/// or without its `MeshCore-` prefix; `ble:` alone takes the first one found).
#[derive(Subcommand)]
enum Cmd {
    /// Query the device: version, settings, scope.
    Info { port: String },
    /// Companions advertising over Bluetooth nearby.
    BleScan {
        #[arg(long, default_value_t = 5)]
        secs: u64,
        /// Every device heard, not only companions.
        #[arg(long)]
        all: bool,
    },
    /// Print, decoded, the frames the device hears (LOG_RX_DATA).
    Sniff {
        port: String,
        /// Duration in seconds (endless by default).
        #[arg(long)]
        secs: Option<u64>,
        /// Channel to decrypt in addition to the Public channel: `#name` or `name=hex secret (16 bytes)`.
        #[arg(long)]
        channel: Vec<String>,
    },
    /// B sends A a burst of direct messages; counts deliveries and what A received.
    Burst {
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        #[arg(long, default_value_t = 30)]
        count: u32,
        /// Milliseconds between two messages.
        #[arg(long, default_value_t = 700)]
        gap: u64,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Send a direct message to a contact, by the hex prefix of its key (6 bytes or more).
    Send {
        port: String,
        to: String,
        text: String,
    },
    /// Command round trips to the device (GET_DEVICE_TIME), to spot a slow main loop.
    Latency {
        port: String,
        #[arg(long, default_value_t = 20)]
        count: u32,
    },
    /// Fetch and print the messages waiting on the device (SYNC_NEXT_MESSAGE until empty).
    Inbox { port: String },
    /// What the C++ firmware left in a SPIFFS image (the M5's partition at 0x290000, read with
    /// `espflash read-flash 0x290000 0x160000 <file>`): files and what an import would bring.
    /// Prints no secret; the image itself holds the private key, keep it out of the repository.
    Spiffs { image: std::path::PathBuf },
    /// The device's custom variables (`GET_CUSTOM_VARS`), or set one: `gps:1`, `gps_interval:60`.
    Var { port: String, set: Option<String> },
    /// The text a `debug-log` firmware prints on its serial port, for `secs` seconds, without
    /// resetting the device (unlike `espflash monitor`).
    Log {
        port: String,
        #[arg(long, default_value_t = 20)]
        secs: u64,
    },
    /// A repeater or room server over the air, through a companion: logs in with
    /// `password` (empty: as a known client or a guest), then for each line `status` (the
    /// stats, decoded), `telemetry` (Cayenne LPP, raw) or a console command, whose reply is
    /// printed. Console replies are read from the companion's message queue: messages
    /// already waiting there are taken (and printed) too.
    Remote {
        port: String,
        /// The server's key, or a prefix of it (hex), as in the companion's contacts.
        key: String,
        password: String,
        lines: Vec<String>,
    },
    /// The text console of a repeater (`rpt-m5`, `rpt-heltec-v4`, or a C++ repeater): sends
    /// each line, prints what comes back (`  -> reply`). Without lines, prints for `--wait`
    /// seconds (the `Repeater ID:` banner after a restart).
    Term {
        port: String,
        lines: Vec<String>,
        /// Seconds to wait for a reply to each line.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Radio, core and packet counters (`GET_STATS` 1, 0, 2): noise floor, airtime, errors.
    /// `--every N` repeats every N seconds, `--count` times.
    Stats {
        port: String,
        #[arg(long)]
        every: Option<u64>,
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Device console commands (`RUN_CLI_COMMAND`), one reply per line: `mc cli $M5 "get radio" ver`.
    Cli {
        port: String,
        #[arg(required = true)]
        commands: Vec<String>,
    },
    /// Set the device's clock to this computer's (SET_DEVICE_TIME), nothing sent over the air.
    Time { port: String },
    /// When the device last heard each contact (their `lastmod`), as counts per hour; no names.
    Heard { port: String },
    /// The device's telemetry permissions byte (SELF_INFO), or set it (SET_OTHER_PARAMS, the
    /// other fields kept): bits 0-1 base, 2-3 location, 4-5 environment; 0 nobody, 1
    /// favourites, 2 everyone. The value is decimal, or hex with `0x`.
    Telemetry { port: String, value: Option<String> },
    /// The device's transmit power (SELF_INFO), or set it in dBm (SET_RADIO_TX_POWER).
    TxPower {
        port: String,
        #[arg(allow_hyphen_values = true)]
        dbm: Option<i8>,
    },
    /// Make the device send its advert (zero hop unless `--flood`).
    Advert {
        port: String,
        #[arg(long)]
        flood: bool,
    },
    /// Make the device transmit a raw frame (SEND_RAW_PACKET).
    Inject {
        port: String,
        hex: String,
        #[arg(long, default_value_t = 0)]
        prio: u8,
    },
    /// Test mc-air against a C++ firmware: `--ref` transmits and listens, `--dut` is under test.
    Airtest {
        #[arg(long = "ref")]
        ref_port: String,
        #[arg(long)]
        dut: String,
        #[arg(short, long)]
        verbose: bool,
    },
    /// The device's interface in a terminal, on a simulated device (`--png DIR`: panel images).
    Ui {
        /// Compact text (33×20) instead of large (25×15).
        #[arg(long)]
        compact: bool,
        /// Render every screen of the tour as PNG files in this directory, then exit.
        #[arg(long)]
        png: Option<std::path::PathBuf>,
        /// The board: `m5` (e-paper, two buttons) or `heltec-v4` (OLED status pages, one button).
        #[arg(long, default_value = "m5")]
        board: String,
    },
    /// A long soak between A (device under test) and B that never reads a message queue:
    /// adverts both ways, a DM from B to A (acknowledged), a ping each way; A's restarts counted.
    Soak {
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        #[arg(long, default_value_t = 100)]
        runs: u32,
        /// Seconds between rounds.
        #[arg(long, default_value_t = 0)]
        gap: u64,
        /// Adverts every this many rounds (0: never).
        #[arg(long, default_value_t = 1)]
        advert_every: u32,
        /// A DM B → A every this many rounds (0: never).
        #[arg(long, default_value_t = 1)]
        dm_every: u32,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Two companions talk to each other (adverts, DMs, channel, pings), zero-hop unless `--flood`.
    Duo {
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        /// Also run the checks that flood: channel text and route learning.
        #[arg(long)]
        flood: bool,
        /// Telemetry pings in each direction.
        #[arg(long, default_value_t = 3)]
        pings: u32,
        /// Also reboot A with a DM waiting and check it is still there (oxymesh on A).
        #[arg(long)]
        reboot: bool,
        #[arg(short, long)]
        verbose: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Info { port } => info(&port),
        Cmd::BleScan { secs, all } => ble::scan(secs, all).map(|found| {
            for f in &found {
                match (f.system, f.rssi) {
                    (true, _) => println!("{}  connected to this computer", f.name),
                    (false, Some(r)) => println!("{}  {r} dBm", f.name),
                    (false, None) => println!("{}", f.name),
                }
            }
            println!("{} companion(s)", found.len());
        }),
        Cmd::Sniff {
            port,
            secs,
            channel,
        } => sniff(&port, secs, &channel),
        Cmd::Inject { port, hex, prio } => inject(&port, &hex, prio),
        Cmd::Time { port } => set_time(&port),
        Cmd::Heard { port } => heard_times(&port),
        Cmd::TxPower { port, dbm } => tx_power(&port, dbm),
        Cmd::Telemetry { port, value } => telemetry(&port, value.as_deref()),
        Cmd::Advert { port, flood } => advert(&port, flood),
        Cmd::Inbox { port } => inbox(&port),
        Cmd::Spiffs { image } => spiffs(&image),
        Cmd::Var { port, set } => var(&port, set.as_deref()),
        Cmd::Cli { port, commands } => console(&port, &commands),
        Cmd::Stats { port, every, count } => stats(&port, every, count),
        Cmd::Log { port, secs } => log(&port, secs),
        Cmd::Term { port, lines, wait } => term(&port, &lines, wait),
        Cmd::Remote {
            port,
            key,
            password,
            lines,
        } => remote(&port, &key, &password, &lines),
        Cmd::Latency { port, count } => latency(&port, count),
        Cmd::Burst {
            a,
            b,
            count,
            gap,
            verbose,
        } => duo::burst(&a, &b, count, gap, verbose),
        Cmd::Send { port, to, text } => send(&port, &to, &text),
        Cmd::Airtest {
            ref_port,
            dut,
            verbose,
        } => airtest::run(&ref_port, &dut, verbose),
        Cmd::Ui {
            compact,
            png,
            board,
        } => ui::run(compact, png.as_deref(), &board),
        Cmd::Soak {
            a,
            b,
            runs,
            gap,
            advert_every,
            dm_every,
            verbose,
        } => duo::soak(
            &a,
            &b,
            runs,
            duo::Pace {
                gap_s: gap,
                advert_every,
                dm_every,
            },
            verbose,
        ),
        Cmd::Duo {
            a,
            b,
            flood,
            pings,
            reboot,
            verbose,
        } => duo::run(&a, &b, flood, pings, reboot, verbose),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn info(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    })?;
    p.ask(&Command::GetDefaultFloodScope)?;
    p.ask(&Command::GetDeviceTime)?;
    Ok(())
}

fn sniff(path: &str, secs: Option<u64>, channels: &[String]) -> Result<(), String> {
    let mut keys = Keys::default();
    keys.channels.push(("Public".into(), ChannelSecret::PUBLIC));
    for c in channels {
        if let Some((name, hex)) = c.split_once('=') {
            let mut k = [0u8; 16];
            let n = mc_core::hex::decode_into(hex, &mut k).ok_or("invalid hex secret")?;
            if n != 16 {
                return Err("the secret must be 16 bytes".into());
            }
            keys.channels
                .push((name.into(), ChannelSecret::from_psk16(&k)));
        } else {
            keys.channels
                .push((c.clone(), ChannelSecret::from_hashtag(c)));
        }
    }
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let end = secs.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    while end.is_none_or(|e| std::time::Instant::now() < e) {
        for f in p.drain()? {
            if let Ok(Response::LogRxData { snr4, rssi, raw }) = Response::decode(&f) {
                println!(
                    "snr={:>5.2} rssi={rssi:>4} {}",
                    f32::from(snr4) / 4.0,
                    describe(raw, &keys)
                );
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

fn send(path: &str, to: &str, text: &str) -> Result<(), String> {
    let mut prefix = [0u8; 32];
    let n = mc_core::hex::decode_into(to, &mut prefix).ok_or("invalid hex prefix")?;
    if n < 6 {
        return Err("the prefix needs at least 6 bytes (12 hex digits)".into());
    }
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let now = crate::port::now_unix();
    let sent = p.ask(&Command::SendTxtMsg {
        txt_type: 0,
        attempt: 0,
        timestamp: now,
        prefix: prefix[..6].try_into().unwrap_or([0; 6]),
        text: text.as_bytes(),
    })?;
    if let Ok(Response::Sent { ack, .. }) = Response::decode(&sent) {
        let got = p.wait(20, |r| match r {
            Response::SendConfirmed { ack: a, rtt_ms } if *a == ack => Some(*rtt_ms),
            _ => None,
        });
        match got {
            Ok(rtt) => println!("delivered, round trip {rtt} ms"),
            Err(_) => println!("no delivery confirmation within 20 s"),
        }
    }
    Ok(())
}

fn latency(path: &str, count: u32) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let mut ms: Vec<u128> = Vec::new();
    for _ in 0..count {
        let t = std::time::Instant::now();
        p.ask(&Command::GetDeviceTime)?;
        ms.push(t.elapsed().as_millis());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let slow = ms.iter().filter(|&&m| m > 100).count();
    ms.sort_unstable();
    let (Some(min), Some(median), Some(max)) = (ms.first(), ms.get(ms.len() / 2), ms.last()) else {
        return Err("--count 0: nothing to measure".into());
    };
    println!(
        "{count} round trips: min {min} ms, median {median} ms, max {max} ms; {slow} over 100 ms"
    );
    Ok(())
}

fn inbox(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let mut n = 0;
    loop {
        let f = p.ask(&Command::SyncNextMessage)?;
        match Response::decode(&f) {
            Ok(Response::NoMoreMessages) => break,
            Ok(Response::ContactMsg {
                prefix,
                text,
                timestamp,
                path_len,
                snr4,
                ..
            }) => println!(
                "DM from {} at {timestamp} path_len {path_len:#04x} SNR {:.2}: {}",
                mc_core::Hex(&prefix),
                f32::from(snr4) / 4.0,
                String::from_utf8_lossy(text)
            ),
            Ok(Response::ChannelMsg { idx, text, .. }) => {
                println!("channel {idx}: {}", String::from_utf8_lossy(text))
            }
            Ok(other) => println!("{other}"),
            Err(e) => println!("unreadable frame ({e:?})"),
        }
        n += 1;
    }
    println!("{n} message(s)");
    Ok(())
}

fn advert(path: &str, flood: bool) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    // The device refuses a time before its own: the advert still goes out.
    if let Err(e) = p.ask_ok(&Command::SetDeviceTime {
        unix_s: crate::port::now_unix(),
    }) {
        println!("clock not changed: {e}");
    }
    p.ask_ok(&Command::SendSelfAdvert { flood: Some(flood) })?;
    Ok(())
}

fn telemetry(path: &str, value: Option<&str>) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let f = p.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"mc",
    })?;
    let Ok(Response::SelfInfo(si)) = Response::decode(&f) else {
        return Err("no SELF_INFO".into());
    };
    if let Some(v) = value {
        let t = byte(v).map_err(|e| format!("telemetry: {e}"))?;
        p.ask_ok(&Command::SetOtherParams {
            manual_add: si.manual_add,
            telemetry: Some(t),
            advert_loc_policy: Some(si.advert_loc_policy),
            multi_acks: Some(si.multi_acks),
        })?;
        p.ask(&Command::AppStart {
            reserved: [0; 7],
            app_name: b"mc",
        })?;
    }
    Ok(())
}

fn tx_power(path: &str, dbm: Option<i8>) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    if let Some(d) = dbm {
        p.ask_ok(&Command::SetRadioTxPower { dbm: d })?;
    }
    p.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"mc",
    })?;
    Ok(())
}

fn heard_times(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.send(&Command::GetContacts { since: None })?;
    let mut times: Vec<(u8, u32)> = Vec::new();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut done = false;
    while !done && std::time::Instant::now() < end {
        for f in p.drain()? {
            match Response::decode(&f) {
                Ok(Response::Contact(c)) => times.push((c.kind, c.lastmod)),
                Ok(Response::EndOfContacts { .. }) => done = true,
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let now = crate::port::now_unix();
    println!("{} contacts; now {now}", times.len());
    let mut by_hour: std::collections::BTreeMap<u32, [u32; 5]> = Default::default();
    for (kind, t) in &times {
        if *t + 48 * 3600 >= now && *t <= now + 600 {
            by_hour.entry(t / 3600).or_default()[usize::from((*kind).min(4))] += 1;
        }
    }
    for (h, k) in by_hour {
        let hh = (h % 24, h * 3600);
        println!(
            "{:>10} {:02}h UTC: {} heard (chat {}, repeater {}, room {}, sensor {})",
            hh.1,
            hh.0,
            k.iter().sum::<u32>(),
            k[1],
            k[2],
            k[3],
            k[4]
        );
    }
    Ok(())
}

fn set_time(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.ask_ok(&Command::SetDeviceTime {
        unix_s: crate::port::now_unix(),
    })
    .map_err(|e| format!("{e} (a device refuses a time before its own clock's)"))?;
    Ok(())
}

fn var(path: &str, set: Option<&str>) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    match set {
        Some(v) => p.ask_ok(&Command::SetCustomVar { var: v.as_bytes() })?,
        None => p.ask(&Command::GetCustomVars)?,
    };
    Ok(())
}

fn log(path: &str, secs: u64) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut port = serialport::new(path, 115_200)
        .timeout(std::time::Duration::from_millis(200))
        .open()
        .map_err(|e| format!("{path}: {e}"))?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut buf = [0u8; 512];
    let mut out = std::io::stdout();
    while std::time::Instant::now() < end {
        match port.read(&mut buf) {
            Ok(n) => {
                let _ =
                    out.write_all(&String::from_utf8_lossy(&buf[..n]).into_owned().into_bytes());
                let _ = out.flush();
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    Ok(())
}

/// Sends a command whose answer is `SENT` (or `OK`).
fn ask_ok(p: &mut Port, c: &Command<'_>) -> Result<(), String> {
    p.ask(c).map(|_| ())
}

/// Logs in to a server through a companion, then status requests or console commands.
fn remote(path: &str, to: &str, password: &str, lines: &[String]) -> Result<(), String> {
    let mut prefix = [0u8; 32];
    let n = mc_core::hex::decode_into(to, &mut prefix).ok_or("invalid hex prefix")?;
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.send(&Command::GetContacts { since: None })?;
    let mut key = None;
    p.wait(15, |r| match r {
        Response::Contact(c) => {
            if c.key[..n] == prefix[..n] {
                key = Some(c.key);
            }
            None
        }
        Response::EndOfContacts { .. } => Some(()),
        _ => None,
    })?;
    let _ = p.drain();
    let key: [u8; 32] =
        key.ok_or("no such contact: wait for the server's advert (or send one with `advert`)")?;
    println!("server {}…", mc_core::Hex(&key[..8]));
    p.ask(&Command::SendLogin {
        key,
        password: password.as_bytes(),
    })?;
    let login = p.wait(30, |r| match r {
        Response::LoginSuccess {
            permissions,
            server,
            ..
        } => Some(Ok((*permissions, *server))),
        Response::LoginFail { .. } => Some(Err(())),
        _ => None,
    });
    // Firmware level 1: a v1.18 room server (52-byte status); 2: a repeater (56 bytes).
    let mut room = false;
    match login {
        Ok(Ok((admin, Some(s)))) => {
            room = s.fw_level == 1;
            println!(
                "logged in: admin {admin}, permissions {}, firmware level {}",
                s.acl, s.fw_level
            );
        }
        Ok(Ok((admin, None))) => println!("logged in: admin {admin}"),
        Ok(Err(())) => return Err("login refused".into()),
        Err(_) => return Err("no answer to the login within 30 s".into()),
    }
    for line in lines {
        if line == "telemetry" {
            ask_ok(
                &mut p,
                &Command::SendTelemetryReq {
                    reserved: [0; 3],
                    key: Some(key),
                },
            )?;
            let lpp = p.wait(20, |r| match r {
                Response::TelemetryResponse { lpp, .. } => Some(lpp.to_vec()),
                _ => None,
            })?;
            println!("telemetry (LPP) {}", mc_core::Hex(&lpp));
            continue;
        }
        if line == "status" {
            p.ask(&Command::SendStatusReq { key })?;
            let data = p.wait(20, |r| match r {
                Response::StatusResponse { data, .. } => Some(data.to_vec()),
                _ => None,
            })?;
            print_status(&data, room);
            continue;
        }
        p.ask(&Command::SendTxtMsg {
            txt_type: 1,
            attempt: 0,
            timestamp: crate::port::now_unix(),
            prefix: key[..6].try_into().unwrap_or([0; 6]),
            text: line.as_bytes(),
        })?;
        println!("> {line}");
        let got = p.wait(20, |r| matches!(r, Response::MsgWaiting).then_some(()));
        if got.is_err() {
            println!("  (no reply within 20 s)");
            continue;
        }
        loop {
            let f = p.ask(&Command::SyncNextMessage)?;
            match Response::decode(&f) {
                Ok(Response::ContactMsg { text, .. }) => {
                    println!("  -> {}", String::from_utf8_lossy(text));
                    break;
                }
                Ok(Response::NoMoreMessages) => break,
                _ => {}
            }
        }
    }
    Ok(())
}

/// `RepeaterStats` (56 bytes, `docs/spec/server.md` §2.4) or the room's `ServerStats` (52
/// bytes, §3): the same first 48 bytes.
fn print_status(d: &[u8], room: bool) {
    let want = if room { 52 } else { 56 };
    if d.len() < want {
        println!("status: {} bytes, expected {want}", d.len());
        return;
    }
    let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    println!(
        "battery {} mV, queue {}, noise {} dBm, last RSSI {} dBm, last SNR {} dB",
        u16_at(0),
        u16_at(2),
        u16_at(4) as i16,
        u16_at(6) as i16,
        f64::from(u16_at(42) as i16) / 4.0
    );
    println!(
        "received {} (flood {}, direct {}), sent {} (flood {}, direct {}), duplicates flood {} direct {}",
        u32_at(8),
        u32_at(32),
        u32_at(36),
        u32_at(12),
        u32_at(24),
        u32_at(28),
        u16_at(46),
        u16_at(44)
    );
    if room {
        println!(
            "airtime TX {} s, uptime {} s, errors {:#x}, posts {}, pushes {}",
            u32_at(16),
            u32_at(20),
            u16_at(40),
            u16_at(48),
            u16_at(50)
        );
    } else {
        println!(
            "airtime TX {} s, RX {} s, uptime {} s, errors {:#x}, receive errors {}",
            u32_at(16),
            u32_at(48),
            u32_at(20),
            u16_at(40),
            u32_at(52)
        );
    }
}

/// Sends console lines (ended by `\r`, as a terminal does) and prints the echo and replies.
fn term(path: &str, lines: &[String], wait: u64) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let mut port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| format!("{path}: {e}"))?;
    let mut out = std::io::stdout();
    let mut read_until = |port: &mut Box<dyn serialport::SerialPort>, end: Instant| {
        let mut seen = Vec::new();
        let mut buf = [0u8; 512];
        while Instant::now() < end {
            match port.read(&mut buf) {
                Ok(n) => {
                    seen.extend_from_slice(&buf[..n]);
                    let _ = out.write_all(&buf[..n]);
                    let _ = out.flush();
                    // A reply line is complete.
                    let text = String::from_utf8_lossy(&seen);
                    if let Some(i) = text.find("  -> ")
                        && text[i..].contains('\n')
                    {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return,
            }
        }
    };
    if lines.is_empty() {
        read_until(&mut port, Instant::now() + Duration::from_secs(wait));
        return Ok(());
    }
    for l in lines {
        port.write_all(format!("{l}\r").as_bytes())
            .map_err(|e| format!("{path}: {e}"))?;
        read_until(&mut port, Instant::now() + Duration::from_secs(wait));
    }
    Ok(())
}

fn stats(path: &str, every: Option<u64>, count: u32) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    for i in 0..count.max(1) {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_secs(every.unwrap_or(1)));
        }
        for kind in [1, 0, 2] {
            let f = p.ask(&Command::GetStats { kind })?;
            match Response::decode(&f) {
                Ok(r) => println!("{r}"),
                Err(e) => println!("unreadable: {e:?}"),
            }
        }
    }
    Ok(())
}

fn console(path: &str, commands: &[String]) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    for c in commands {
        match p.ask(&Command::RunCliCommand { cmd: c.as_bytes() }) {
            Ok(f) => match Response::decode(&f) {
                Ok(Response::CliReply { text }) => {
                    println!("{c:<28} {:?}", String::from_utf8_lossy(text));
                }
                Ok(r) => println!("{c:<28} unexpected: {r}"),
                Err(e) => println!("{c:<28} unreadable: {e:?}"),
            },
            // `reboot` and `poweroff` are never answered.
            Err(_) => println!("{c:<28} (no reply)"),
        }
    }
    Ok(())
}

fn inject(path: &str, hex: &str, prio: u8) -> Result<(), String> {
    let mut raw = [0u8; 255];
    let n = mc_core::hex::decode_into(hex, &mut raw).ok_or("invalid hex")?;
    mc_air::Packet::decode(&raw[..n]).map_err(|e| format!("invalid frame: {e:?}"))?;
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask_ok(&Command::SendRawPacket {
        priority: prio,
        raw: &raw[..n],
    })?;
    Ok(())
}

/// A byte from the command line: decimal, or hex with `0x`.
fn byte(s: &str) -> Result<u8, String> {
    let s = s.trim();
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u8::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|e| format!("{s}: {e}"))
}

fn spiffs(path: &std::path::Path) -> Result<(), String> {
    use mc_spiffs::{Spiffs, cpp};
    let img = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut fs = Spiffs::new(&img[..], img.len() as u32).map_err(|e| format!("{e:?}"))?;
    let mut files = Vec::new();
    fs.list(|n, size| files.push((String::from_utf8_lossy(n).into_owned(), size)))
        .map_err(|e| format!("{e:?}"))?;
    files.sort();
    println!("{} files:", files.len());
    for (n, size) in &files {
        println!("  {n:<24} {size:>6} B");
    }
    match cpp::summary(&mut fs) {
        Ok(s) => {
            let name = String::from_utf8_lossy(&s.name);
            println!("node      {}", name.trim_end_matches('\0'));
            println!("key       {}…", mc_core::Hex(&s.key[..8]));
            match s.radio {
                Some(r) => println!(
                    "radio     {}.{:03} MHz, {} kHz, SF{}, CR {}, {} dBm",
                    r.freq_khz / 1000,
                    r.freq_khz % 1000,
                    r.bw_hz as f64 / 1000.0,
                    r.sf,
                    r.cr,
                    r.tx_power_dbm
                ),
                None => println!("radio     (no readable /prefs.json: defaults)"),
            }
            if mc_spiffs::repeater::is_server(&mut fs).map_err(|e| format!("{e:?}"))? {
                let r = mc_spiffs::repeater::summary(&mut fs).map_err(|e| format!("{e:?}"))?;
                println!("role      repeater (or room server)");
                println!("clients   {}", r.clients);
                println!("regions   {}", r.regions);
            } else {
                println!("contacts  {}", s.contacts);
                println!("channels  {}", s.channels);
            }
            Ok(())
        }
        Err(e) => Err(format!("nothing to import: {e:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::byte;

    #[test]
    fn a_byte_is_decimal_or_hex_with_0x() {
        assert_eq!(byte("2"), Ok(2));
        assert_eq!(byte("10"), Ok(10));
        assert_eq!(byte("0x10"), Ok(16));
        assert_eq!(byte(" 0X2a "), Ok(42));
        assert!(byte("256").is_err());
        assert!(byte("2a").is_err(), "hex needs its 0x");
        assert!(byte("").is_err());
    }
}

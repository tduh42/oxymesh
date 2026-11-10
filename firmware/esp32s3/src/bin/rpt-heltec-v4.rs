//! oxymesh as a repeater on the Heltec V4 (`src/bin/server/heltec_v4.rs`).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

extern crate alloc;

use esp_backtrace as _;

#[path = "server/heltec_v4.rs"]
mod board;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    board::run(mc_server::Role::Repeater).await
}

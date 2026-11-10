//! oxymesh as a room server on the ThinkNode M5 (`src/bin/server/m5.rs`).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

extern crate alloc;

use esp_backtrace as _;

#[path = "server/m5.rs"]
mod board;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    board::run(mc_server::Role::Room).await
}

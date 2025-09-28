#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::{timestamp, unwrap};
use embassy_executor::Spawner;
use embassy_time::Instant;
use esp_alloc as _;
use esp_hal::gpio::{Level, Output, OutputConfig, Pin};
use esp_hal::peripherals::{self};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::{clock::CpuClock, timer::timg::TimerGroup};
use esp_println as _;
use esp_radio::ble::controller::BleConnector;
use static_cell::StaticCell;
use trouble_host::prelude::*;

use crate::tasks::gpio::DrumNote;
use crate::tasks::{ble, gpio};

mod tasks;
mod trouble_midi;

type BluetoothController = ExternalController<BleConnector<'static>, 20>;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // Turn on the on-board LED when panicking to signal something went wrong.

    // SAFETY: we're panicking so we should be safe as the last and only one to use the pin.
    let led_pin = unsafe { peripherals::GPIO8::steal() };
    let _ = Output::new(led_pin, Level::Low, OutputConfig::default());

    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

timestamp!("[{=u64:us}]", Instant::now().as_micros());

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    {
        let pins_notes_map = [
            (peripherals.GPIO0.degrade(), DrumNote::HighTom),
            (peripherals.GPIO1.degrade(), DrumNote::PedalHiHat),
            (peripherals.GPIO3.degrade(), DrumNote::OpenHiHat),
            (peripherals.GPIO4.degrade(), DrumNote::CrashCymbal1),
            (peripherals.GPIO5.degrade(), DrumNote::CrashCymbal2),
            (peripherals.GPIO6.degrade(), DrumNote::RideCymbal),
            (peripherals.GPIO7.degrade(), DrumNote::FloorTom),
            (peripherals.GPIO10.degrade(), DrumNote::LowTom),
            (peripherals.GPIO20.degrade(), DrumNote::BassDrum),
            (peripherals.GPIO21.degrade(), DrumNote::Snare),
        ];

        for (pin, note) in pins_notes_map {
            spawner.must_spawn(gpio::watch_gpio_task(pin, note));
        }
    }

    {
        esp_alloc::heap_allocator!(size: 72 * 1024);
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        esp_preempt::start(timg0.timer0);

        static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
        let radio = RADIO.init(unwrap!(esp_radio::init()));

        let systimer = SystemTimer::new(peripherals.SYSTIMER);
        esp_hal_embassy::init(systimer.alarm0);

        let bluetooth = peripherals.BT;
        let connector = BleConnector::new(radio, bluetooth);
        let controller = BluetoothController::new(connector);

        ble::peripheral_run(controller).await;
    }
}

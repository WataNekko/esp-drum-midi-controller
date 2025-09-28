use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{AnyPin, Input, InputConfig};
use midi_types::Note;

#[derive(defmt::Format)]
#[repr(u8)]
pub enum DrumNote {
    BassDrum = 36,
    Snare = 38,
    ClosedHiHat = 42,
    PedalHiHat = 44,
    OpenHiHat = 46,
    FloorTom = 43,
    LowTom = 45,
    HighTom = 48,
    CrashCymbal1 = 49,
    CrashCymbal2 = 57,
    RideCymbal = 51,
}

impl From<DrumNote> for Note {
    fn from(value: DrumNote) -> Self {
        Self::new(value as u8)
    }
}

#[embassy_executor::task(pool_size = 10)]
pub async fn watch_gpio_task(pin: AnyPin<'static>, note: DrumNote) {
    let mut input = Input::new(pin, InputConfig::default());

    const HIT_DEBOUNCE_TIME: Duration = Duration::from_millis(20);
    const UNHIT_DEBOUNCE_TIME: Duration = Duration::from_micros(300);

    loop {
        input.wait_for_high().await;
        let timestamp = Instant::now();
        Timer::at(timestamp + UNHIT_DEBOUNCE_TIME).await;

        input.wait_for_low().await;
        let timestamp = Instant::now();
        defmt::warn!("{}", note);
        Timer::at(timestamp + HIT_DEBOUNCE_TIME).await;
    }
}

use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    signal::Signal,
};
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

#[derive(PartialEq, defmt::Format)]
pub enum SensorsStatus {
    On,
    Off,
}
pub type SensorsStatusSignal = Signal<NoopRawMutex, SensorsStatus>;

#[embassy_executor::task(pool_size = 10)]
pub async fn watch_gpio_task(
    pin: AnyPin<'static>,
    note: DrumNote,
    status_signal: &'static SensorsStatusSignal,
) {
    let mut input = Input::new(pin, InputConfig::default());

    const INITIAL_SENSORS_STABILIZE_TIME: Duration = Duration::from_millis(500);
    Timer::after(INITIAL_SENSORS_STABILIZE_TIME).await;

    static mut PIN_HIGH_COUNT: u8 = 0;

    const HIT_DEBOUNCE_TIME: Duration = Duration::from_millis(20);
    const UNHIT_DEBOUNCE_TIME: Duration = Duration::from_micros(300);
    const STATUS_CHANGED_DEBOUNCE_TIME: Duration = Duration::from_secs(1);

    loop {
        {
            input.wait_for_high().await;
            let timestamp = Instant::now();

            // SAFETY: This task is only run on a single threaded executor, so it's safe because
            // only one task at a time touch this counter. To remove unsafe, we could use a
            // Mutex<CriticalSectionRawMutex, u8> or an AtomicU8 (that uses critical_section under
            // the hood) but that's overkill cuz interrupt services don't touch this. Or we can use
            // a RefCell but that would introduce unnecessary runtime check, since we know only one
            // task mutate this counter at a time. This is defined as long as this task stay on a
            // single executor on a single core.
            let status_changed = unsafe {
                let status_changed = if PIN_HIGH_COUNT == 0 {
                    status_signal.signal(SensorsStatus::On);
                    true
                } else {
                    false
                };
                PIN_HIGH_COUNT += 1;
                status_changed
            };

            let debounce_time = if status_changed {
                STATUS_CHANGED_DEBOUNCE_TIME
            } else {
                UNHIT_DEBOUNCE_TIME
            };
            Timer::at(timestamp + debounce_time).await;
        }

        {
            input.wait_for_low().await;
            let timestamp = Instant::now();
            defmt::warn!("{}", note);

            // SAFETY: Like above
            let status_changed = unsafe {
                PIN_HIGH_COUNT -= 1;
                if PIN_HIGH_COUNT == 0 {
                    status_signal.signal(SensorsStatus::Off);
                    true
                } else {
                    false
                }
            };

            let debounce_time = if status_changed {
                STATUS_CHANGED_DEBOUNCE_TIME
            } else {
                HIT_DEBOUNCE_TIME
            };
            Timer::at(timestamp + debounce_time).await;
        }
    }
}

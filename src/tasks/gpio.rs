use core::pin::pin;
use embassy_futures::select::select_slice;
use embassy_sync::{
    blocking_mutex::raw::{NoopRawMutex, RawMutex},
    channel::{Channel, Receiver, TrySendError},
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{AnyPin, Input, InputConfig};
use heapless::Vec;
use midi_types::Note;

#[derive(Copy, Clone, PartialEq, defmt::Format)]
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

pub type HitEventsChannel = Channel<NoopRawMutex, (Instant, DrumNote), 16>;
pub type HitEventsReceiver<'ch> = Receiver<'ch, NoopRawMutex, (Instant, DrumNote), 16>;

#[embassy_executor::task]
pub async fn watch_gpios_task(
    pins_notes_map: [(AnyPin<'static>, DrumNote); 10],
    status_signal: &'static SensorsStatusSignal,
    hit_events: &'static HitEventsChannel,
) {
    let mut pins_notes_map =
        pins_notes_map.map(|(pin, note)| (Input::new(pin, InputConfig::default()), note));

    const INITIAL_SENSORS_STABILIZE_TIME: Duration = Duration::from_millis(200);
    Timer::after(INITIAL_SENSORS_STABILIZE_TIME).await;

    loop {
        select_slice(pin!(
            pins_notes_map
                .iter_mut()
                .map(|(pin, ..)| pin.wait_for_high())
                .collect::<Vec<_, 10>>()
                .as_mut_slice()
        ))
        .await;
        status_signal.signal(SensorsStatus::On);

        select_slice(pin!(
            pins_notes_map
                .iter_mut()
                .map(|(pin, note)| watch_pin_for_hits(pin, *note, hit_events))
                .collect::<Vec<_, 10>>()
                .as_mut_slice()
        ))
        .await;
        status_signal.signal(SensorsStatus::Off);

        const SWITCH_OFF_SENSORS_STABILIZE_TIME: Duration = Duration::from_millis(200);
        Timer::after(SWITCH_OFF_SENSORS_STABILIZE_TIME).await;
    }
}

async fn watch_pin_for_hits(
    pin: &mut Input<'_>,
    mut note: DrumNote,
    hit_events: &HitEventsChannel,
) {
    static mut PIN_HIGH_COUNT: u8 = 0;
    static mut IS_HIHAT_PEDAL_HOLD: bool = false;

    loop {
        {
            pin.wait_for_high().await;
            let timestamp = Instant::now();

            // SAFETY: This task is only run on a single threaded executor, so it's safe because
            // only one task at a time touch this counter. To remove unsafe, we could use a
            // Mutex<CriticalSectionRawMutex, u8> or an AtomicU8 (that uses critical_section under
            // the hood) but that's overkill cuz interrupt services don't touch this. Or we can use
            // a RefCell but that would introduce unnecessary runtime check, since we know only one
            // task mutate this counter at a time. This is defined as long as this task stay on a
            // single executor on a single core.
            unsafe {
                PIN_HIGH_COUNT += 1;
            };

            if note == DrumNote::PedalHiHat {
                // SAFETY: Like above
                unsafe {
                    IS_HIHAT_PEDAL_HOLD = false;
                }
            }

            const UNHIT_DEBOUNCE_TIME: Duration = Duration::from_micros(300);
            Timer::at(timestamp + UNHIT_DEBOUNCE_TIME).await;
        }

        {
            pin.wait_for_low().await;
            let timestamp = Instant::now();

            // SAFETY: Like above
            unsafe {
                PIN_HIGH_COUNT -= 1;
                if PIN_HIGH_COUNT == 0 {
                    break;
                }
            };

            match note {
                // SAFETY: Like above
                DrumNote::PedalHiHat => unsafe {
                    IS_HIHAT_PEDAL_HOLD = true;
                },
                // SAFETY: Like above
                DrumNote::OpenHiHat if unsafe { IS_HIHAT_PEDAL_HOLD } => {
                    note = DrumNote::ClosedHiHat;
                }
                _ => {}
            }

            hit_events.force_send((timestamp, note));

            const HIT_DEBOUNCE_TIME: Duration = Duration::from_millis(20);
            Timer::at(timestamp + HIT_DEBOUNCE_TIME).await;
        }
    }
}

trait ForceSend<T> {
    /// Force to send the message. Overwrite old if full.
    fn force_send(&self, message: T);
}

impl<M, T, const N: usize> ForceSend<T> for Channel<M, T, N>
where
    M: RawMutex,
{
    fn force_send(&self, mut message: T) {
        while let Err(e) = self.try_send(message) {
            match e {
                TrySendError::Full(m) => {
                    message = m;
                    let _ = self.try_receive();
                }
            }
        }
    }
}

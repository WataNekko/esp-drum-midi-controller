use core::{cell::Cell, pin::pin};
use defmt::{debug, trace};
use embassy_futures::select::select_slice;
use embassy_sync::{
    blocking_mutex::raw::{NoopRawMutex, RawMutex},
    channel::{Channel, Receiver, TrySendError},
    signal::Signal,
};
use embassy_time::{Duration, Instant, TimeoutError, Timer, with_timeout};
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

        let shared_state = SharedPinsState {
            pin_high_count: Cell::new(0),
            is_pedal_hi_hat_pressed: Cell::new(false),
        };

        select_slice(pin!(
            pins_notes_map
                .iter_mut()
                .map(|(pin, note)| watch_pin_for_hits(pin, *note, &shared_state, hit_events))
                .collect::<Vec<_, 10>>()
                .as_mut_slice()
        ))
        .await;
        status_signal.signal(SensorsStatus::Off);

        const SWITCH_OFF_SENSORS_STABILIZE_TIME: Duration = Duration::from_millis(200);
        Timer::after(SWITCH_OFF_SENSORS_STABILIZE_TIME).await;
    }
}

struct SharedPinsState {
    pin_high_count: Cell<u8>,
    is_pedal_hi_hat_pressed: Cell<bool>,
}

async fn watch_pin_for_hits(
    pin: &mut Input<'_>,
    note: DrumNote,
    state: &SharedPinsState,
    hit_events: &HitEventsChannel,
) {
    loop {
        {
            pin.wait_for_stable_high().await;

            state.pin_high_count.update(|c| c + 1);

            if note == DrumNote::PedalHiHat {
                state.is_pedal_hi_hat_pressed.set(false);
            }

            trace!("Unhit {}", note);
        }

        {
            pin.wait_for_stable_low().await;
            let timestamp = Instant::now();

            state.pin_high_count.update(|c| c - 1);
            if state.pin_high_count.get() == 0 {
                // All pins are low. Probably sensors are turned off, so we're exiting.
                // (It's unlikely that all pads are hit at the same instance.)
                break;
            }

            if note == DrumNote::PedalHiHat {
                state.is_pedal_hi_hat_pressed.set(true);
            }

            let note = if note == DrumNote::OpenHiHat && state.is_pedal_hi_hat_pressed.get() {
                DrumNote::ClosedHiHat
            } else {
                note
            };
            let hit_event = (timestamp, note);

            hit_events.force_send(hit_event);
            debug!("Hit {}", hit_event);

            const HIT_DEBOUNCE_TIME: Duration = Duration::from_millis(30);
            Timer::at(timestamp + HIT_DEBOUNCE_TIME).await;
        }
    }
}

trait WaitForStable {
    /// Minimum duration the input level is unchanged to be considered stable.
    const STABLE_DURATION: Duration;

    /// Wait until the pin is high, accounting for noise when the input level is stabilizing.
    async fn wait_for_stable_high(&mut self);
    /// Wait until the pin is low, accounting for noise when the input level is stabilizing.
    async fn wait_for_stable_low(&mut self);
}

impl WaitForStable for Input<'_> {
    const STABLE_DURATION: Duration = Duration::from_micros(150);

    async fn wait_for_stable_high(&mut self) {
        loop {
            self.wait_for_high().await;

            if with_timeout(Self::STABLE_DURATION, self.wait_for_low()).await == Err(TimeoutError) {
                // Unchanged for the STABLE_DURATION.
                break;
            }
        }
    }

    async fn wait_for_stable_low(&mut self) {
        loop {
            self.wait_for_low().await;

            if with_timeout(Self::STABLE_DURATION, self.wait_for_high()).await == Err(TimeoutError)
            {
                // Unchanged for the STABLE_DURATION.
                break;
            }
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

use defmt::{error, info, unwrap, warn};
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_time::{Duration, with_timeout};
use esp_hal::gpio::{AnyPin, Level, Output, OutputConfig};
use midi_types::{Channel, MidiMessage, Value7};
use trouble_host::prelude::*;

use crate::{
    BluetoothController,
    tasks::gpio::{HitEventsReceiver, SensorsStatus, SensorsStatusSignal, blink},
    trouble_midi::MidiService,
};

const BLE_SERVICE_NAME: &str = "ESP MIDI Controller";

#[gatt_server]
struct GattServer {
    midi_service: MidiService,
}

pub async fn peripheral_run(
    controller: BluetoothController,
    status_signal: &SensorsStatusSignal,
    status_led: AnyPin<'_>,
    hit_events: HitEventsReceiver<'_>,
) {
    let mut resources: HostResources<DefaultPacketPool, 1, 0> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources);
    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    let server = unwrap!(GattServer::new_with_config(GapConfig::Peripheral(
        PeripheralConfig {
            name: BLE_SERVICE_NAME,
            appearance: &appearance::MEDIA_PLAYER,
        }
    )));

    let mut status_led = Output::new(status_led, Level::High, OutputConfig::default());

    let wait_for_status = async |status: SensorsStatus| {
        while status_signal.wait().await != status {}
        info!("Sensors switched {}", status);
    };

    join(host_runner_task(runner), async {
        loop {
            wait_for_status(SensorsStatus::On).await;

            select(
                midi_service_task(
                    BLE_SERVICE_NAME,
                    &mut peripheral,
                    &server,
                    &mut status_led,
                    hit_events,
                ),
                wait_for_status(SensorsStatus::Off),
            )
            .await;
        }
    })
    .await;
}

async fn host_runner_task<'a>(mut runner: Runner<'a, BluetoothController, DefaultPacketPool>) -> ! {
    loop {
        unwrap!(runner.run().await);
    }
}

async fn midi_service_task<'a>(
    service_name: &str,
    peripheral: &mut Peripheral<'a, BluetoothController, DefaultPacketPool>,
    server: &GattServer<'a>,
    status_led: &mut Output<'_>,
    hit_events: HitEventsReceiver<'_>,
) {
    info!("Starting advertising and GATT service");

    while let Ok(Either::First(res)) = with_timeout(
        Duration::from_secs(60),
        select(
            advertise_and_connect(service_name, peripheral, server),
            blink(status_led, Duration::from_millis(1000)),
        ),
    )
    .await
    {
        let conn = unwrap!(res);

        let connected_led_blink_task = with_timeout(
            Duration::from_secs(1),
            blink(status_led, Duration::from_millis(100)),
        );

        let connection_service_tasks = select(
            gatt_events_task(&conn),
            notify_midi_events_task(server, &conn, hit_events),
        ); // Either task finishes means we're disconnected.

        let _ = join(connected_led_blink_task, connection_service_tasks).await;
    }

    warn!("[adv] Timeout. Not connected.");
}

async fn advertise_and_connect<'a, 's, C: Controller>(
    name: &str,
    peripheral: &mut Peripheral<'a, C, DefaultPacketPool>,
    server: &'s GattServer<'a>,
) -> Result<GattConnection<'a, 's, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut advertiser_data = [0; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids16(&[[0x0f, 0x18]]),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut advertiser_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertiser_data[..len],
                scan_data: &[],
            },
        )
        .await?;
    info!("[adv] advertising");
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    info!("[adv] connection established");
    Ok(conn)
}

async fn gatt_events_task<P: PacketPool>(conn: &GattConnection<'_, '_, P>) {
    // FIXME: Fix connection with iOS not maintained.
    // TODO: Bonding? (Auto-reconnect?)
    let reason = loop {
        if let GattConnectionEvent::Disconnected { reason } = conn.next().await {
            break reason;
        }
    };
    info!("[gatt] disconnected: {:?}", reason);
}

async fn notify_midi_events_task(
    server: &GattServer<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    hit_events: HitEventsReceiver<'_>,
) {
    let midi = &server.midi_service.midi_event;
    hit_events.clear();

    loop {
        let (timestamp, note) = hit_events.receive().await;

        const MIDI_CHANNEL: Channel = Channel::new(9);
        const MIDI_VELOCITY: Value7 = Value7::new(100);
        let packet = (
            timestamp,
            MidiMessage::NoteOn(MIDI_CHANNEL, note.into(), MIDI_VELOCITY),
        )
            .into();

        if midi.notify(conn, &packet).await.is_err() {
            error!("[notify_midi_events_task] error notifying connection");
            break;
        };
    }
}

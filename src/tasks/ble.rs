use defmt::{error, info, unwrap};
use embassy_futures::{join::join, select::select};
use midi_types::{Channel, MidiMessage, Value7};
use trouble_host::prelude::*;

use crate::{
    BluetoothController,
    tasks::gpio::{HitEventsReceiver, SensorsStatus, SensorsStatusSignal},
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

    let wait_for_status = async |status: SensorsStatus| {
        while status_signal.wait().await != status {}
        info!("Sensors switched {}", status);
    };

    join(host_runner_task(runner), async {
        loop {
            wait_for_status(SensorsStatus::On).await;

            select(
                midi_service_task(BLE_SERVICE_NAME, &mut peripheral, &server, hit_events),
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
    hit_events: HitEventsReceiver<'_>,
) -> ! {
    info!("Starting advertising and GATT service");
    loop {
        let conn = unwrap!(advertise_and_connect(service_name, peripheral, server).await);
        select(
            gatt_events_task(&conn),
            notify_midi_events_task(server, &conn, hit_events),
        )
        .await; // Either task finishes means we're disconnected.
    }
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

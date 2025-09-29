use defmt::{info, unwrap};
use embassy_futures::{join::join, select::select};
use embassy_time::Timer;
use midi_types::{Channel, MidiMessage, Note, Value7};
use trouble_host::prelude::*;

use crate::{
    BluetoothController,
    tasks::gpio::{SensorsStatus, SensorsStatusSignal},
    trouble_midi::MidiService,
};

const BLE_SERVICE_NAME: &str = "ESP MIDI Controller";

#[gatt_server]
struct GattServer {
    midi_service: MidiService,
}

pub async fn peripheral_run(controller: BluetoothController, status_signal: &SensorsStatusSignal) {
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
                midi_service_task(BLE_SERVICE_NAME, &mut peripheral, &server, &stack),
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
    stack: &Stack<'_, BluetoothController, DefaultPacketPool>,
) -> ! {
    info!("Starting advertising and GATT service");
    loop {
        let conn = unwrap!(advertise_and_connect(service_name, peripheral, server).await);
        select(gatt_events_task(&conn), custom_task(server, &conn, stack)).await;
        // Either task finishes means we're disconnected.
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

async fn custom_task(
    server: &GattServer<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    stack: &Stack<'_, BluetoothController, DefaultPacketPool>,
) {
    let midi = &server.midi_service.midi_event;
    loop {
        let packet = MidiMessage::NoteOn(Channel::C10, Note::new(36), Value7::new(100)).into();
        if midi.notify(conn, &packet).await.is_err() {
            info!("[custom_task] error notifying connection");
            break;
        };
        // read RSSI (Received Signal Strength Indicator) of the connection.
        if let Ok(rssi) = conn.raw().rssi(stack).await {
            info!("[custom_task] RSSI: {:?}", rssi);
        } else {
            info!("[custom_task] error getting RSSI");
            break;
        };
        Timer::after_secs(2).await;
    }
}

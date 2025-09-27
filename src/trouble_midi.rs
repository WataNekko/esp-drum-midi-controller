use embassy_time::Instant;
use midi_convert::render_slice::MidiRenderSlice;
use midi_types::MidiMessage;
use trouble_host::{prelude::*, types::gatt_traits::FromGattError};

#[gatt_service(uuid = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700")]
pub struct MidiService {
    #[characteristic(uuid = "7772E5DB-3868-4112-A1A9-F2669D106BF3", read, write_without_response, notify, value = MidiMessage::Reset.into())]
    pub midi_event: BleMidiPacket<251>,
}

pub trait AsTimestamp {
    fn as_timestamp(&self) -> u16;
}

impl<T: AsTimestamp> AsTimestamp for &T {
    fn as_timestamp(&self) -> u16 {
        T::as_timestamp(self)
    }
}

impl AsTimestamp for u16 {
    fn as_timestamp(&self) -> u16 {
        *self
    }
}

impl AsTimestamp for Instant {
    fn as_timestamp(&self) -> u16 {
        self.as_millis() as u16
    }
}

pub struct BleMidiPacket<const SIZE: usize> {
    buffer: [u8; SIZE],
    len: usize,
}

fn is_system_msg_status_byte(status: u8) -> bool {
    status & 0xF0 == 0xF0
}

impl<const SIZE: usize> BleMidiPacket<SIZE> {
    const MIN_SIZE: usize = 3; // Header + Timestamp + Single MIDI status byte
    const _OK: () = assert!(SIZE >= Self::MIN_SIZE);

    pub fn add_timestamped(
        timestamp: impl AsTimestamp,
        msg: MidiMessage,
    ) -> BleMidiPacketBuilder<SIZE> {
        let millis = timestamp.as_timestamp();
        let header = 0x80 | ((millis >> 7) as u8 & 0x3F);
        let timestamp = 0x80 | (millis as u8 & 0x7F);

        let mut buffer = [0; SIZE];
        buffer[0] = header;
        buffer[1] = timestamp;

        let len = msg.render_slice(&mut buffer[2..]);
        let packet = Self {
            buffer,
            len: 2 + len,
        };

        let running_status = if is_system_msg_status_byte(buffer[2]) {
            None
        } else {
            Some(buffer[2])
        };

        BleMidiPacketBuilder {
            packet,
            running_status,
            timestamp_byte: timestamp,
        }
    }
}

impl<Ts: AsTimestamp, const SIZE: usize> From<(Ts, MidiMessage)> for BleMidiPacket<SIZE> {
    fn from((timestamp, msg): (Ts, MidiMessage)) -> Self {
        Self::add_timestamped(timestamp, msg).build()
    }
}

impl<const SIZE: usize> From<MidiMessage> for BleMidiPacket<SIZE> {
    fn from(msg: MidiMessage) -> Self {
        Self::add_timestamped(Instant::now(), msg).build()
    }
}

impl<const SIZE: usize> AsGatt for BleMidiPacket<SIZE> {
    const MIN_SIZE: usize = Self::MIN_SIZE;
    const MAX_SIZE: usize = SIZE;

    fn as_gatt(&self) -> &[u8] {
        &self.buffer[..self.len]
    }
}

impl<const SIZE: usize> FromGatt for BleMidiPacket<SIZE> {
    fn from_gatt(data: &[u8]) -> Result<Self, FromGattError> {
        if data.len() < Self::MIN_SIZE || data.len() > Self::MAX_SIZE {
            Err(FromGattError::InvalidLength)
        } else {
            let mut buffer = [0; SIZE];
            let len = data.len();
            buffer[..len].copy_from_slice(data);
            // Copy data directly without parsing. Provide some way to get the data from the packet
            // later if we need it?

            Ok(Self { buffer, len })
        }
    }
}

#[allow(unused)]
pub struct BleMidiPacketBuilder<const SIZE: usize> {
    packet: BleMidiPacket<SIZE>,
    running_status: Option<u8>,
    timestamp_byte: u8,
}

impl<const SIZE: usize> BleMidiPacketBuilder<SIZE> {
    pub fn build(self) -> BleMidiPacket<SIZE> {
        self.packet
    }
}

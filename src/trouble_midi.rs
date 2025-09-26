use embassy_time::Instant;
use midi_convert::render_slice::MidiRenderSlice;
use midi_types::MidiMessage;
use trouble_host::{prelude::*, types::gatt_traits::FromGattError};

#[gatt_service(uuid = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700")]
pub struct MidiService {
    #[characteristic(uuid = "7772E5DB-3868-4112-A1A9-F2669D106BF3", read, write_without_response, notify, value = MidiMessage::Reset.into())]
    pub midi_event: BleMidiPacket,
}

pub struct BleMidiPacket {
    buffer: [u8; Self::MAX_SIZE],
    len: usize,
}

const HEADER_AND_TIMESTAMP_SIZE: usize = 2;

impl AsGatt for BleMidiPacket {
    const MIN_SIZE: usize = HEADER_AND_TIMESTAMP_SIZE + 1;
    const MAX_SIZE: usize = HEADER_AND_TIMESTAMP_SIZE + 3;

    fn as_gatt(&self) -> &[u8] {
        &self.buffer[..self.len]
    }
}

impl From<(u16, MidiMessage)> for BleMidiPacket {
    fn from((millis, msg): (u16, MidiMessage)) -> Self {
        let header = 0x80 | ((millis >> 7) as u8 & 0x3F);
        let timestamp = 0x80 | (millis as u8 & 0x7F);

        let mut buffer = [0; Self::MAX_SIZE];
        buffer[0] = header;
        buffer[1] = timestamp;

        let len = msg.render_slice(&mut buffer[HEADER_AND_TIMESTAMP_SIZE..]);

        Self {
            buffer,
            len: HEADER_AND_TIMESTAMP_SIZE + len,
        }
    }
}

impl From<(Instant, MidiMessage)> for BleMidiPacket {
    fn from((instant, msg): (Instant, MidiMessage)) -> Self {
        Self::from((instant.as_millis() as u16, msg))
    }
}

impl From<MidiMessage> for BleMidiPacket {
    fn from(msg: MidiMessage) -> Self {
        Self::from((Instant::now(), msg))
    }
}

impl FromGatt for BleMidiPacket {
    fn from_gatt(data: &[u8]) -> Result<Self, FromGattError> {
        if data.len() < Self::MIN_SIZE || data.len() > Self::MAX_SIZE {
            Err(FromGattError::InvalidLength)
        } else {
            let mut buffer = [0; Self::MAX_SIZE];
            let len = data.len();
            buffer[..len].copy_from_slice(data);
            // Copy data directly without parsing. Provide some way to get the data from the packet
            // later if we need it?

            Ok(Self { buffer, len })
        }
    }
}

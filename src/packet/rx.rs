use crate::{
    header::FixedHeader,
    io::{
        err::DecodeError,
        reader::{PacketDecoder, InsufficientLen},
    },
    packet::Packet,
};

pub trait RxPacket<'p>: Packet + Sized {
    /// Decodes a packet. Must check the fixed header for correctness.
    fn receive(header: &FixedHeader, reader: PacketDecoder<'p>) -> Result<Self, RxError>;
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RxError {
    /// Constant space somewhere is not enough, e.g. `Vec<ReasonCode, MAX_TOPIC_FILTERS>` in SUBACK
    InsufficientConstSpace,

    MalformedPacket,
    ProtocolError,
}

impl From<InsufficientLen> for RxError {
    fn from(_: InsufficientLen) -> Self {
        Self::MalformedPacket
    }
}

impl From<DecodeError> for RxError {
    fn from(e: DecodeError) -> Self {
        match e {
            DecodeError::MalformedPacket => Self::MalformedPacket,
            DecodeError::ProtocolError => Self::ProtocolError,
        }
    }
}

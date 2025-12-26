use crate::io::reader::InsufficientLen;

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DecodeError {
    MalformedPacket,
    ProtocolError,
}

impl From<InsufficientLen> for DecodeError {
    fn from(_: InsufficientLen) -> Self {
        Self::MalformedPacket
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum WriteError<E> {
    WriteZero,
    Write(E),
}

impl<E> From<E> for WriteError<E> {
    fn from(e: E) -> Self {
        Self::Write(e)
    }
}

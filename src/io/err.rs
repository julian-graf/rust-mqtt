#[cfg(feature = "discrete")]
use core::fmt;

#[cfg(feature = "discrete")]
use crate::io::body::BodyReadError;

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ReadError<E> {
    Read(E),
    UnexpectedEOF,
    MalformedPacket,
    ProtocolError,
    InvalidTopicName,
}

#[cfg(feature = "discrete")]
impl<E, B: fmt::Debug> From<BodyReadError<E, B>> for ReadError<BodyReadError<E, B>> {
    fn from(e: BodyReadError<E, B>) -> Self {
        match e {
            e @ BodyReadError::InsufficientRemainingLen => Self::Read(e),
            e @ BodyReadError::Read(_) => Self::Read(e),
            e @ BodyReadError::Buffer(_) => Self::Read(e),
            BodyReadError::UnexpectedEOF => Self::UnexpectedEOF,
            BodyReadError::MalformedPacket => Self::MalformedPacket,
            BodyReadError::ProtocolError => Self::ProtocolError,
            BodyReadError::InvalidTopicName => Self::InvalidTopicName,
        }
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

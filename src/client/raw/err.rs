use crate::{
    client::raw::NetStateError,
    eio::{self, ErrorKind},
    io::reader::ReaderError,
    packet::{RxError, TxError},
    types::ReasonCode,
};

/// The main error returned by `Raw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// An outgoing packet is too long to encode its length with the variable byte integer
    TxPacketTooLong,

    /// An incoming packet is too long to be read into the buffer.
    RxBufferExceeded,

    /// The underlying Read/Write method returned an error.
    Network(ErrorKind),

    /// The network is in a faulty state.
    Disconnected,

    /// A generic constant such as `MAX_PROPERTIES` is too small.
    ConstSpace,

    /// Malformed packet or Protocol Error.
    Server,
}

impl<E: eio::Error> From<TxError<E>> for (Error, Option<ReasonCode>) {
    fn from(e: TxError<E>) -> Self {
        match e {
            TxError::WriteZero => (Error::Network(ErrorKind::WriteZero), None),
            TxError::Write(e) => (Error::Network(e.kind()), None),
            TxError::RemainingLenExceeded => (Error::TxPacketTooLong, None),
        }
    }
}

impl From<ReaderError> for (Error, Option<ReasonCode>) {
    fn from(e: ReaderError) -> Self {
        match e {
            ReaderError::Read(e) => (Error::Network(e), None),
            ReaderError::EOF => (Error::Network(ErrorKind::NotConnected), None),
            ReaderError::InvalidPacketType | ReaderError::InvalidVarByteInt => {
                (Error::Server, Some(ReasonCode::MalformedPacket))
            }
            ReaderError::BufferExceeded => (
                Error::RxBufferExceeded,
                Some(ReasonCode::ImplementationSpecificError),
            ),
        }
    }
}

impl From<RxError> for (Error, Option<ReasonCode>) {
    fn from(e: RxError) -> Self {
        match e {
            RxError::InsufficientConstSpace => (
                Error::ConstSpace,
                Some(ReasonCode::ImplementationSpecificError),
            ),
            RxError::MalformedPacket => (Error::Server, Some(ReasonCode::MalformedPacket)),
            RxError::ProtocolError => (Error::Server, Some(ReasonCode::ProtocolError)),
        }
    }
}


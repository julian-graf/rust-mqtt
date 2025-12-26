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

impl<E: eio::Error> From<TxError<E>> for Error {
    fn from(e: TxError<E>) -> Self {
        match e {
            TxError::WriteZero => Self::Network(ErrorKind::WriteZero),
            TxError::Write(e) => Self::Network(e.kind()),
            TxError::RemainingLenExceeded => Self::TxPacketTooLong,
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

impl From<NetStateError> for Error {
    fn from(_: NetStateError) -> Self {
        Self::Disconnected
    }
}

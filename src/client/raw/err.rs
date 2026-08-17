use crate::{
    client::raw::NetStateError,
    eio::{self, ErrorKind},
    packet::{RxError, TxError},
    types::ReasonCode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum Error<B> {
    /// The underlying Read/Write method returned an error.
    Network(ErrorKind),

    /// The network is in a faulty state.
    Disconnected,

    /// A buffer provision by the [`BufferProvider`] failed.
    ///
    /// [`BufferProvider`]: crate::buffer::BufferProvider
    Alloc(B),

    /// Malformed packet or Protocol Error.
    Server,
}

impl<E: eio::Error, B> From<TxError<E>> for Error<B> {
    fn from(e: TxError<E>) -> Self {
        match e {
            TxError::WriteZero => Self::Network(ErrorKind::WriteZero),
            TxError::Write(e) => Self::Network(e.kind()),
        }
    }
}

impl<E: eio::Error, B> From<RxError<E, B>> for (Error<B>, Option<ReasonCode>) {
    fn from(e: RxError<E, B>) -> Self {
        match e {
            RxError::Read(e) => (Error::Network(e.kind()), None),
            RxError::Buffer(b) => (
                Error::Alloc(b),
                Some(ReasonCode::ImplementationSpecificError),
            ),
            RxError::UnexpectedEOF => (Error::Network(ErrorKind::NotConnected), None),
            RxError::MalformedPacket => (Error::Server, Some(ReasonCode::MalformedPacket)),
            RxError::ProtocolError => (Error::Server, Some(ReasonCode::ProtocolError)),
            RxError::InvalidTopicName => (Error::Server, Some(ReasonCode::TopicNameInvalid)),
        }
    }
}

impl<B> From<NetStateError> for Error<B> {
    fn from(_: NetStateError) -> Self {
        Self::Disconnected
    }
}

/// The reason why a call to [`Client::abort`] failed.
///
/// [`Client::abort`]: crate::client::Client::abort
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AbortError {
    /// The network connection and the MQTT protocol-level connection with the server is healthy.
    /// [`Client::abort`] may not be called on a healthy connection, in this state
    /// [`Client::disconnect`] can be used for a clean disconnection
    ///
    /// [`Client::abort`]: crate::client::Client::abort
    /// [`Client::disconnect`]: crate::client::Client::disconnect
    Connected,

    /// No network connection is currently available for the client because no [`Client::connect`]
    /// call has been made since the last call to either [`Client::abort`] or [`Client::disconnect`]
    /// or no [`Client::connect`] call has been made yet in the lifetime of the client.
    Terminated,
}

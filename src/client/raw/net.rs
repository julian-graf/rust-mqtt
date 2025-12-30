use core::mem;
use std::process::Output;

use crate::{
    client::raw::RawError,
    io::{
        net::Transport,
        reader::{PacketDecodeToken, PacketReceiver, ReaderError},
    },
    packet::TxPacket,
    types::ReasonCode,
};

/// Represents a network connection with different variants for handling failures in the connection gracefully.
#[derive(Debug, Default)]
pub enum NetState<N: Transport> {
    /// The connection and dialogue with the server is ok
    Ok(N),

    /// The connection is ok but some protocol specific error (e.g. MalformedPacket or ProtocolError occured)
    ///
    /// Sending messages can be considered, but the session should ultimately be closed according to spec
    Faulted(N, ReasonCode),

    /// The connection is closed
    #[default]
    Terminated,
}

/// A network connection state related error.
pub enum Error<E> {
    /// An error has occurred in the application MQTT communication.
    /// The underlying network is still open and should be used for sending a DISCONNECT packet.
    Faulted,

    /// An error has occurred in the application MQTT communication.
    /// The underlying network has already been closed because either a DISCONNECT packet has already been sent or shouldn't be sent at all.
    Terminated,

    Inner(E),
}

impl<N: Transport> NetState<N> {
    pub(super) fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
    pub(super) fn replace(&mut self, net: N) {
        *self = Self::Ok(net);
    }
    pub fn get(&mut self) -> Result<&mut N, Error<()>> {
        match self {
            Self::Ok(n) => Ok(n),
            Self::Faulted(_, _) => Err(Error::Faulted),
            Self::Terminated => Err(Error::Terminated),
        }
    }
    pub(super) fn get_reason_code(&mut self) -> Option<ReasonCode> {
        match self {
            Self::Ok(_) => None,
            Self::Faulted(_, r) => Some(*r),
            Self::Terminated => None,
        }
    }

    pub fn fail(&mut self, reason_code: ReasonCode) {
        *self = match mem::take(self) {
            Self::Ok(n) | Self::Faulted(n, _) => Self::Faulted(n, reason_code),
            Self::Terminated => Self::Terminated,
        }
    }

    pub fn terminate(&mut self) -> Option<N> {
        match mem::take(self) {
            Self::Ok(n) => Some(n),
            Self::Faulted(n, _) => Some(n),
            Self::Terminated => None,
        }
    }

    pub fn close_with(&mut self, reason_code: Option<ReasonCode>) {
        match reason_code {
            Some(reason_code) => self.fail(reason_code),
            None => {
                self.terminate();
            }
        }
    }

    //-----------------------------------------------------

    pub async fn with_net<'a, 'n, F, Fut, T, E, M>(&'n mut self, f: F, m: M) -> Result<T, RawError>
    where
        N: 'n,
        'n: 'a,
        F: FnOnce(&mut N) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        M: FnOnce(E) -> (RawError, Option<ReasonCode>),
    {
        let net = self.get().map_err(|_| RawError::Disconnected)?;

        let t = f(net).await;

        match t {
            Ok(t) => Ok(t),
            Err(e) => {
                let (e, r) = m(e);
                match r {
                    Some(r) => self.fail(r),
                    None => {
                        self.terminate();
                    }
                }
                Err(e)
            }
        }
    }
}

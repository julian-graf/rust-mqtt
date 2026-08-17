use core::{matches, mem};

use crate::{fmt::debug_assert, io::Transport, types::ReasonCode};

/// Represents a network connection with different variants for handling failures in the connection gracefully.
#[derive(Debug, Default)]
pub(crate) enum NetState<N: Transport> {
    /// The network connection and the MQTT protocol-level connection with the server is healthy.
    Ok(N),

    /// The network connection is healthy but some protocol specific error (e.g. `MalformedPacket` or
    /// `ProtocolError`) occured, which means the next action must be to send a DISCONNECT packet with
    /// the specified [`ReasonCode`]
    DueDisconnect(N, ReasonCode),

    /// The MQTT protocol-level connection failed and the network connection is healthy and no
    /// DISCONNECT must be sent or the network connection caused an error and is therefore not healthy.
    /// At this point, the handle is only kept to be returned to the user eventually who should close
    /// the network connection gracefully and can potentially reuse the `N` instance.
    Inactive(N),

    /// No network connection is currently available for the client.
    #[default]
    Terminated,
}

pub enum Error {
    Faulted,
    Inactive,
    Terminated,
}

impl<N: Transport> NetState<N> {
    /// Returns `true` if the net state is [`Ok`].
    ///
    /// [`Ok`]: NetState::Ok
    #[must_use]
    pub(crate) fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
    /// Returns `true` if the net state is [`Terminated`].
    ///
    /// [`Terminated`]: NetState::Terminated
    #[must_use]
    pub(crate) fn is_terminated(&self) -> bool {
        matches!(self, Self::Terminated)
    }

    pub fn replace(&mut self, net: N) {
        debug_assert!(self.is_terminated());

        *self = Self::Ok(net);
    }
    pub fn get(&mut self) -> Result<&mut N, Error> {
        match self {
            Self::Ok(n) => Ok(n),
            Self::DueDisconnect(_, _) => Err(Error::Faulted),
            Self::Inactive(_) => Err(Error::Inactive),
            Self::Terminated => Err(Error::Terminated),
        }
    }

    pub fn fail(&mut self, reason_code: ReasonCode) {
        *self = match mem::take(self) {
            Self::Ok(n) | Self::DueDisconnect(n, _) => Self::DueDisconnect(n, reason_code),
            Self::Inactive(n) => Self::Inactive(n),
            Self::Terminated => Self::Terminated,
        }
    }

    pub fn deactivate(&mut self) {
        *self = match mem::take(self) {
            Self::Ok(n) | Self::DueDisconnect(n, _) | Self::Inactive(n) => Self::Inactive(n),
            Self::Terminated => Self::Terminated,
        }
    }

    pub fn terminate(&mut self) -> Self {
        mem::take(self)
    }
}

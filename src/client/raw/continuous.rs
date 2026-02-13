use core::marker::PhantomData;

use crate::{
    client::raw::{RawError, net::NetState}, eio::{Error, ErrorKind}, header::FixedHeader, io::{err::WriteError, net::Transport}, packet::{TxError, TxPacket}, types::ReasonCode, v5::packet::DisconnectPacket
};

/// An MQTT Client offering a low level api for sending and receiving packets
/// abstracting over the underlying transport and buffer.
#[derive(Debug)]
pub(crate) struct Raw<'b, N: Transport> {
    n: NetState<N>,

    buf_start: *mut u8,
    buf_len: usize,

    phantom_data: PhantomData<&'b ()>,
}

impl<'b, N: Transport> Raw<'b, N> {
    pub fn new_disconnected(buf: &'b mut [u8]) -> Self {
        let range = buf.as_mut_ptr_range();

        Self {
            n: NetState::Terminated,
            buf_start: range.start,
            buf_len: range.end - range.start,
            phantom_data: PhantomData,
        }
    }

    pub fn set_net(&mut self, net: N) {
        debug_assert!(
            !self.n.is_ok(),
            "Network must not be in Ok() state to replace it."
        );
        self.n.replace(net);
    }

    pub fn close_with(&mut self, reason_code: Option<ReasonCode>) {
        match reason_code {
            Some(r) => self.n.fail(r),
            None => {
                self.n.terminate();
            }
        }
    }

    /// Disconnect handler after an error occured.
    ///
    /// This expects the network to not be in Ok() state
    pub async fn abort(&mut self) -> Result<(), RawError> {
        debug_assert!(
            !self.n.is_ok(),
            "Network must not be in Ok() state to disconnect due to an error."
        );

        let r = self.n.get_reason_code();
        let mut n = self.n.terminate();

        match (&mut n, r) {
            (Some(n), Some(r)) => {
                let packet = DisconnectPacket::new(r);

                // Don't check whether length exceeds servers maximum packet size because we don't
                // add a reason string to the DISCONNECT packet -> length is always in the 4..=6 range in bytes.
                // The server really shouldn't reject this.
                packet.send(n).await.map_err(|e| match e {
                    TxError::Write(e) => RawError::Network(Error::kind(&e)),
                    TxError::WriteZero => RawError::Network(ErrorKind::WriteZero),
                })
            }
            (None, Some(_)) => unreachable!(
                "Netstate never contains a reason code when terminated and therefore not holding a network connection"
            ),
            (_, _) => Ok(()),
        }
    }

    fn handle_rx<E: Into<(RawError, Option<ReasonCode>)>>(
        &mut self,
        e: E,
    ) -> RawError {
        let (e, r) = e.into();

        match r {
            Some(r) => self.n.fail(r),
            None => {
                self.n.terminate();
            }
        }

        e
    }
    fn handle_tx<E: Into<RawError>>(
        &mut self,
        e: E,
    ) -> RawError {
        // Terminate right away because if send fails, sending another (DISCONNECT) packet doesn't make sense
        self.n.terminate();

        e.into()
    }

    pub async fn send<P: TxPacket>(
        &mut self,
        packet: &P,
    ) -> Result<(), RawError> {
        let net = self.n.get()?;

        packet.send(net).await.map_err(|e| self.handle_tx(e))
    }

    /// Cancel-safe if N::flush() is cancel-safe
    pub async fn flush(&mut self) -> Result<(), RawError> {
        self.n.get()?.flush().await.map_err(|e| {
            let e: WriteError<_> = e.into();
            let e: TxError<_> = e.into();
            self.handle_tx(e)
        })
    }
}

//! Implements primitives for handling connections along with sending and receiving packets.

mod err;
mod net;

use core::ops::{Deref, DerefMut};

use embedded_io_async::Error;
pub use err::Error as RawError;
pub use net::Error as NetStateError;

use crate::{
    client::raw::net::NetState,
    eio::{self, ErrorKind},
    fmt::{debug_assert, panic, unreachable},
    io::{
        err::WriteError,
        net::Transport,
        reader::{PacketDecodeToken, PacketDecoder, PacketReceiver},
    },
    packet::{RxPacket, TxError, TxPacket},
    types::ReasonCode,
    v5::packet::DisconnectPacket,
};

// Skip formatting to keep comma before closing > (see https://github.com/rust-lang/rust/issues/150163)
/// An MQTT Client offering a low level api for sending and receiving packets
#[derive(Debug)]
pub struct Raw<'b, N: Transport> {
    n: NetState<N>,
    receiver: PacketReceiver<'b>,
}

pub struct RawHandle<'h, N: Transport> {
    n: &'h mut NetState<N>,
}
impl<'h, N: Transport> Deref for RawHandle<'h, N> {
    type Target = NetState<N>;

    fn deref(&self) -> &Self::Target {
        self.n
    }
}
impl<'h, N: Transport> DerefMut for RawHandle<'h, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.n
    }
}

impl<'b, N: Transport> Raw<'b, N> {
    /// `buf.len()` must be greater or equal to 5 to allow safe operation
    pub fn new_disconnected(rx_buffer: &'b mut [u8]) -> Self {
        debug_assert!(rx_buffer.len() >= 5);

        Self {
            n: NetState::Terminated,
            receiver: PacketReceiver::new(rx_buffer),
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
        self.n.close_with(reason_code);
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

                packet.send(n).await.map_err(|e| match e {
                    TxError::Write(e) => RawError::Network(eio::Error::kind(&e)),
                    TxError::WriteZero => RawError::Network(ErrorKind::WriteZero),
                    TxError::RemainingLenExceeded => panic!("DISCONNECT never exceeds max length"),
                })
            }
            (None, Some(_)) => unreachable!(
                "Netstate never contains a reason code when terminated and therefore not holding a network connection"
            ),
            (_, _) => Ok(()),
        }
    }

    /// Cancel-safe method to receive a packet
    pub async fn recv(&mut self) -> Result<PacketDecodeToken, RawError> {
        let net = self.n.get()?;

        // self.receiver
        //     .poll(net)
        //     .await
        //     .map_err(|e| Self::handle_rx(&mut self.n, e))
        todo!()
    }

    pub fn decode<'p, P: RxPacket<'p>>(
        &'p mut self,
        token: PacketDecodeToken,
    ) -> Result<(P, RawHandle<'p, N>), RawError> {
        let decoder: PacketDecoder<'_> = self.receiver.into_decoder(token);

        // let p = P::decode(decoder).map_err(|e| Self::handle_rx(&mut self.n, e))?;

        // Ok((p, RawHandle { n: &mut self.n }))
        todo!()
    }

    /// Not cancel-safe
    pub async fn send<P: TxPacket>(&mut self, packet: &P) -> Result<(), RawError> {
        self.n.with_net(|n| packet.send(n), Into::into).await
    }

    /// Cancel-safe if N::flush() is cancel-safe
    pub async fn flush(&mut self) -> Result<(), RawError> {
        self.n
            .with_net(|n| n.flush(), |e| TxError::Write(e).into())
            .await
    }
}

#[cfg(test)]
mod unit {
    use core::time::Duration;

    use embedded_io_adapters::tokio_1::FromTokio;
    use tokio::{
        io::{AsyncWriteExt, duplex},
        join,
        sync::oneshot::channel,
        time::{sleep, timeout},
    };
    use tokio_test::{assert_err, assert_ok};

    #[cfg(feature = "alloc")]
    use crate::buffer::AllocBuffer;
    #[cfg(feature = "bump")]
    use crate::buffer::BumpBuffer;

    use crate::{
        client::raw::Raw,
        header::{FixedHeader, PacketType},
        types::VarByteInt,
    };

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_simple() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(s.write_all(&[0x10, 0x00, 0x24]).await);
        };
        let rx = async {
            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(PacketType::Connect, 0x00, VarByteInt::from(0u8))
            );
        };

        join!(rx, tx);
    }

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_with_pause() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(s.write_u8(0xE0).await);
            sleep(Duration::from_millis(100)).await;
            assert_ok!(s.write_u8(0x80).await);
            sleep(Duration::from_millis(100)).await;
            assert_ok!(s.write_u8(0x80).await);
            sleep(Duration::from_millis(100)).await;
            assert_ok!(s.write_u8(0x01).await);
        };
        let rx = async {
            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(PacketType::Disconnect, 0x00, VarByteInt::from(16_384u16))
            );
        };

        join!(rx, tx);
    }

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_cancel_no_progres() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);
        let (rx_ready, tx_ready) = channel();

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(tx_ready.await);
            assert_ok!(s.write_all(&[0xE0, 0x00]).await);
        };
        let rx = async {
            assert_err!(timeout(Duration::from_millis(100), c.recv_header()).await);
            assert_ok!(rx_ready.send(()));

            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(PacketType::Disconnect, 0x00, VarByteInt::from(0u8))
            );
        };

        join!(rx, tx);
    }

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_cancel_type_and_flags_byte() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);
        let (rx_ready, tx_ready) = channel();

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(s.write_u8(0xA0).await);
            assert_ok!(tx_ready.await);
            assert_ok!(s.write_all(&[0x80, 0x80, 0x80, 0x01]).await);
        };
        let rx = async {
            assert_err!(timeout(Duration::from_millis(100), c.recv_header()).await);
            assert_ok!(rx_ready.send(()));

            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(
                    PacketType::Unsubscribe,
                    0x00,
                    VarByteInt::try_from(2_097_152u32).unwrap()
                )
            );
        };

        join!(rx, tx);
    }

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_cancel_single_length_byte() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);
        let (rx_ready, tx_ready) = channel();

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(s.write_all(&[0xD7, 0xFF]).await);
            assert_ok!(tx_ready.await);
            assert_ok!(s.write_all(&[0xFF, 0xFF, 0x7F]).await);
        };
        let rx = async {
            assert_err!(timeout(Duration::from_millis(100), c.recv_header()).await);
            assert_ok!(rx_ready.send(()));

            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(
                    PacketType::Pingresp,
                    0x07,
                    VarByteInt::try_from(VarByteInt::MAX_ENCODABLE).unwrap()
                )
            );
        };

        join!(rx, tx);
    }

    #[tokio::test]
    #[test_log::test]
    async fn recv_header_cancel_multi() {
        #[cfg(feature = "alloc")]
        let mut b = AllocBuffer;
        #[cfg(feature = "bump")]
        let mut b = [0; 64];
        #[cfg(feature = "bump")]
        let mut b = BumpBuffer::new(&mut b);
        let (c, mut s) = duplex(64);
        let r = FromTokio::new(c);
        let (rx_ready1, tx_ready1) = channel();
        let (rx_ready2, tx_ready2) = channel();
        let (rx_ready3, tx_ready3) = channel();

        let mut c = Raw::new_disconnected(&mut b);
        c.set_net(r);

        let tx = async {
            assert_ok!(s.write_u8(0x68).await);
            assert_ok!(tx_ready1.await);
            assert_ok!(s.write_u8(0xFF).await);
            assert_ok!(tx_ready2.await);
            assert_ok!(s.write_u8(0xFF).await);
            assert_ok!(tx_ready3.await);
            assert_ok!(s.write_u8(0x7F).await);
        };
        let rx = async {
            assert_err!(timeout(Duration::from_millis(50), c.recv_header()).await);
            assert_ok!(rx_ready1.send(()));
            assert_err!(timeout(Duration::from_millis(50), c.recv_header()).await);
            assert_ok!(rx_ready2.send(()));
            assert_err!(timeout(Duration::from_millis(50), c.recv_header()).await);
            assert_ok!(rx_ready3.send(()));

            let h = assert_ok!(c.recv_header().await);
            assert_eq!(
                h,
                FixedHeader::new(
                    PacketType::Pubrel,
                    0x08,
                    VarByteInt::try_from(2_097_151u32).unwrap()
                )
            );
        };

        join!(rx, tx);
    }
}

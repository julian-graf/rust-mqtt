//! Contains utilities for session management.

use core::cmp::min;
use heapless::Vec;

use crate::{
    client::options::AckMode,
    session::handle::{FreeHandle, InboundHandle, OutboundHandle, SubHandle, UnsubHandle},
    types::{IdentifiedQoS, PacketIdentifier, ReasonCode},
};

pub(crate) mod handle;

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// client.
///
/// In case of "manual" flows, where acknowledgements are sent manually by the user, the "manual"
/// portion only applies to the first packet of its kind within a flow. This "half-manual"
/// behaviour occurs when a reconnection is at play. If the user has sent a manual PUBREL and
/// a reconnection occurs before the PUBCOMP is received, the retransmitted PUBREL is sent
/// automatically by the client when it handles the retransmission of all PUBREL packets.
/// However, a PUBREL packet that has
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocalPublishState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet must be resent after a reconnection. The
    /// specification demands a retransmission in this case, not following it is a protocol
    /// violation.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    DuePublishAtLeastOnce,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet must be resent after a reconnection. The
    /// specification demands a retransmission in this case, not following it is a protocol
    /// violation. Whether the PUBREL packet associated with this flow must be sent manually
    /// by the user is determined by the contained [`AckMode`].
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    DuePublishExactlyOnce(AckMode),
    /// A [`QoS::AtLeastOnce`] PUBLISH packet has been sent. The final and next step in the
    /// handshake is the server sending a PUBACK packet.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitAck,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been sent. The next step in the handshake is
    /// the server sending a PUBREC packet. Whether this packet must be sent manually by the
    /// user is determined by the contained [`AckMode`].
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitRec(AckMode),
    /// A PUBREC packet has been received or a reconnection has occured with a PUBREL packet
    /// having been sent before. The next step in the handshake is the client (re-)sending a
    /// PUBREL packet. Whether this packet must be sent manually by the user is determined by
    /// the contained [`AckMode`].
    DueRel(AckMode),
    /// A PUBREL packet has been sent. The final and next step in the handshake is the server
    /// sending a PUBCOMP packet.
    AwaitComp(AckMode),
}

impl LocalPublishState {
    pub(crate) fn reconnected(self) -> Self {
        match self {
            Self::DuePublishAtLeastOnce => Self::DuePublishAtLeastOnce,
            Self::AwaitAck => Self::DuePublishAtLeastOnce,

            Self::DuePublishExactlyOnce(mode) => Self::DuePublishExactlyOnce(mode),
            Self::AwaitRec(mode) => Self::DuePublishExactlyOnce(mode),
            Self::DueRel(mode) => Self::DueRel(mode),
            Self::AwaitComp(mode) => Self::DueRel(mode),
        }
    }
}

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// server.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PeerPublishState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet must be resent by the server after a reconnection.
    /// The specification demands a retransmission in this case, not following it is a protocol
    /// violation. The subsequent PUBACK packet must be sent manually by the user, as this state
    /// can only arise from a previous [`Self::DueAck`] state, which is always associated to a
    /// manual flow.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitPublishAtLeastOnce,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet must be resent by the server after a reconnection.
    /// The specification demands a retransmission in this case, not following it is a protocol
    /// violation. Whether the subsequent PUBREC and PUBCOMP packets must be sent manually by
    /// the user is determined by the contained [`AckMode`].
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    //
    // TODO: not yet clear whether this state is actually required. This state is only reached
    // after a PUBREC has been sent (or failed to send it) and the reconnection occured after.
    // This means that the peer may have received the PUBREC and also already sent the PUBREL.
    // Therefore, the peer may also send a PUBREL packet as the next packet of the handshake,
    // which we have to accept.
    // Answer: This state is required. If it were not present, the AwaitRel state must serve
    // as this state and therefore allow PUBLISH packets. This includes allowing a PUBLISH
    // packet in a handshake that has not yet been interrupted by a reconnection. A retransmit-
    // ted PUBLISH packet within a continuous network connection is a protocol violation
    // however according to [MQTT-4.4.0-1].
    AwaitPublishExactlyOnce(AckMode),
    /// A [`QoS::AtLeastOnce`] PUBLISH packet has been received. The final and next step in the
    /// handshake is the client sending a PUBACK packet. This packet must be sent manually by
    /// the user.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    DueAck,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been received. The next step in the handshake
    /// is the client sending a PUBREC packet. This packet as well as the later PUBCOMP packet
    /// must be sent manually by the user.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    DueRec,
    /// A PUBREC packet has been sent. The next step in the handshake is the server sending a
    /// PUBREL packet. Whether the subsequent PUBCOMP packet must be sent manually by the client
    /// is determined by the contained [`AckMode`].
    AwaitRel(AckMode),
    /// A PUBREL packet has been received. The final and next step in the handshake is the
    /// client sending a PUBCOMP packet. This packet must be sent manually by the user.
    DueComp,
}

impl PeerPublishState {
    pub(crate) fn reconnected(self) -> Self {
        match self {
            Self::AwaitPublishAtLeastOnce => Self::AwaitPublishAtLeastOnce,
            Self::DueAck => Self::AwaitPublishAtLeastOnce,

            Self::AwaitPublishExactlyOnce(mode) => Self::AwaitPublishExactlyOnce(mode),
            Self::DueRec => Self::AwaitPublishExactlyOnce(AckMode::Manual),
            Self::AwaitRel(mode) => Self::AwaitPublishExactlyOnce(mode),
            Self::DueComp => Self::AwaitRel(AckMode::Manual),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum Response {
    None,
    Acknowledge(ReasonCode),
    Receive(ReasonCode),
    Release(ReasonCode),
    Complete(ReasonCode),
    Disconnect(ReasonCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum StateError {
    NoCapacity,
    UnusedPacketIdentifier,
    MismatchedQoS,
    MismatchedHandshakeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum Event {
    Publish,
    Duplicate(AckMode),
    Ignored,

    Aborted,
    Rejected,
    Acknowledged(AckMode),
    Received(AckMode),
    Released(AckMode),
    Completed(AckMode),

    ServerError,
}

/// Session-associated information
///
/// Client identifier is not stored here as it would lead to inconsistencies with the underyling allocation system.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Session<
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    /// The currently in-flight subscriptions.
    pub subs: Vec<PacketIdentifier, SUBSCRIBE_MAXIMUM>,
    /// The currently in-flight unsubscriptions.
    pub unsubs: Vec<PacketIdentifier, SUBSCRIBE_MAXIMUM>,

    /// The currently in-flight incoming publications.
    pub inbound_publishes: Vec<(PacketIdentifier, PeerPublishState), RECEIVE_MAXIMUM>,
    /// The currently in-flight outgoing publications.
    pub outbound_publishes: Vec<(PacketIdentifier, LocalPublishState), SEND_MAXIMUM>,
}

impl<const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    /// Creates a handle to an unused packet identifier for usage in a new SUBSCRIBE, UNSUBSCRIBE
    /// or PUBLISH packet. Whether the specific type of category actually has free buffer space
    /// is not stated by the existence of the [`FreeHandle`], as a single full buffer should not
    /// block other packet types from being sent.
    pub(crate) fn free_handle(
        &mut self,
    ) -> Option<FreeHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        // TODO this can be a better search with a stack bitset / larger window of PIDs

        let mut packet_identifier = PacketIdentifier::ONE;

        if self.outbound_handle(packet_identifier).is_some() {
            packet_identifier = packet_identifier.next();
            while self.outbound_handle(packet_identifier).is_some()
                && packet_identifier != PacketIdentifier::ONE
            {
                packet_identifier = packet_identifier.next();
            }
        }

        Some(FreeHandle {
            session: self,
            packet_identifier,
        })
    }

    /// Obtains a handle to a packet identifier used for a currently in-flight SUBSCRIBE packet.
    pub(crate) fn sub_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<SubHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.subs
            .iter()
            .position(|&p| p == packet_identifier)
            .map(|i| SubHandle { session: self, i })
    }

    /// Obtains a handle to a packet identifier used for a currently in-flight UNSUBSCRIBE packet.
    pub(crate) fn unsub_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<UnsubHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.unsubs
            .iter()
            .position(|&p| p == packet_identifier)
            .map(|i| UnsubHandle { session: self, i })
    }
    /// Obtains a handle to a packet identifier used for an incoming, currently in-flight PUBLISH
    /// packet / handshake.
    pub(crate) fn inbound_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<InboundHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.inbound_publishes
            .iter()
            .copied()
            .enumerate()
            .find(|(_, e)| e.0 == packet_identifier)
            .map(|(i, (_, state))| InboundHandle {
                session: self,
                i,
                state,
            })
    }
    /// Obtains a handle to a packet identifier used for an outgoing, currently in-flight PUBLISH
    /// packet / handshake.
    pub(crate) fn outbound_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<OutboundHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.outbound_publishes
            .iter()
            .copied()
            .enumerate()
            .find(|(_, e)| e.0 == packet_identifier)
            .map(|(i, (_, state))| OutboundHandle {
                session: self,
                i,
                packet_identifier,
                state,
            })
    }

    /// Obtains a handle to the *first* packet identifier used for an outgoing, currently in-flight
    /// PUBLISH packet / handshake, where *first* means that subsequent calls to
    /// [`OutboundHandle::next`] allow iteration over all packet identifiers belonging to an outgoing,
    /// currently in-flight PUBLISH packet / handshake.
    pub(crate) fn outbound_iter(
        &mut self,
    ) -> Option<OutboundHandle<'_, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.outbound_publishes
            .first()
            .map(|(p, s)| (*p, *s))
            .map(|(packet_identifier, state)| OutboundHandle {
                session: self,
                i: 0,
                packet_identifier,
                state,
            })
    }

    fn active_inbound_publishes(&self) -> u16 {
        debug_assert!(u16::try_from(self.inbound_publishes.len()).is_ok());

        self.inbound_publishes.len() as u16
    }
    /// Returns the amount of currently in-flight outgoing publications.
    pub(crate) fn active_outbound_publishes(&self) -> u16 {
        debug_assert!(u16::try_from(self.outbound_publishes.len()).is_ok());

        self.outbound_publishes.len() as u16
    }

    fn available_inbound_publish_capacity(&self) -> bool {
        let capacity = min(self.inbound_publishes.capacity(), usize::from(u16::MAX)) as u16;

        capacity > self.active_inbound_publishes()
    }
    fn available_outbound_publish_capacity(&self) -> bool {
        let capacity = min(self.outbound_publishes.capacity(), usize::from(u16::MAX)) as u16;

        capacity > self.active_outbound_publishes()
    }

    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet
    /// for an incoming/server publication. Requires that the packet identifier
    /// has no entry currently.
    fn schedule_inbound(&mut self, packet_identifier: PacketIdentifier, state: PeerPublishState) {
        debug_assert!(self.available_inbound_publish_capacity());
        debug_assert!(self.inbound_handle(packet_identifier).is_none());

        self.inbound_publishes
            .push((packet_identifier, state))
            .unwrap();
    }
    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet
    /// for an outgoing/client publication. Requires that the packet identifier
    /// has no entry currently.
    pub(crate) fn schedule_outbound(
        &mut self,
        packet_identifier: PacketIdentifier,
        state: LocalPublishState,
    ) {
        debug_assert!(self.available_outbound_publish_capacity());
        debug_assert!(self.outbound_handle(packet_identifier).is_none());

        self.outbound_publishes
            .push((packet_identifier, state))
            .unwrap();
    }

    pub(crate) fn clear(&mut self) {
        self.subs.clear();
        self.unsubs.clear();
        self.inbound_publishes.clear();
        self.outbound_publishes.clear();
    }
}

impl<const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn reconnect(&mut self) {
        self.subs.clear();
        self.unsubs.clear();
        for (_, s) in self.inbound_publishes.iter_mut() {
            *s = s.reconnected();
        }
        for (_, s) in self.outbound_publishes.iter_mut() {
            *s = s.reconnected();
        }
    }

    pub(crate) fn inbound_publish(
        &mut self,
        identified_qos: IdentifiedQoS,
        ack_mode: AckMode,
    ) -> (Response, Event) {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => (Response::None, Event::Publish),
            IdentifiedQoS::AtLeastOnce(pid) | IdentifiedQoS::ExactlyOnce(pid) => self
                .inbound_handle(pid)
                .map(|mut h| h.inbound_publish(identified_qos.into()))
                .unwrap_or_else(|| {
                    if self.available_inbound_publish_capacity() {
                        match identified_qos {
                            IdentifiedQoS::AtMostOnce => unreachable!(),
                            IdentifiedQoS::AtLeastOnce(_) if ack_mode.is_manual() => {
                                self.schedule_inbound(pid, PeerPublishState::DueAck);
                                (Response::None, Event::Publish)
                            }
                            IdentifiedQoS::AtLeastOnce(_) => {
                                (Response::Acknowledge(ReasonCode::Success), Event::Publish)
                            }
                            IdentifiedQoS::ExactlyOnce(_) if ack_mode.is_manual() => {
                                self.schedule_inbound(pid, PeerPublishState::DueRec);
                                (Response::None, Event::Publish)
                            }
                            IdentifiedQoS::ExactlyOnce(_) => {
                                self.schedule_inbound(
                                    pid,
                                    PeerPublishState::AwaitRel(AckMode::Automatic),
                                );
                                (Response::Receive(ReasonCode::Success), Event::Publish)
                            }
                        }
                    } else {
                        (
                            Response::Disconnect(ReasonCode::QuotaExceeded),
                            Event::ServerError,
                        )
                    }
                }),
        }
    }

    /// The PUBACK's [`ReasonCode`] may be successful or erroneous, this doesn't matter
    /// for the state machine as this packet identifier is removed from the session
    /// state in either case.
    pub(crate) fn outbound_puback(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_puback())
            .unwrap_or(Err(StateError::UnusedPacketIdentifier))
    }

    pub(crate) fn outbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_pubrec(reason_code))
            .unwrap_or(Err(StateError::UnusedPacketIdentifier))
    }

    pub(crate) fn inbound_pubrel(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        self.inbound_handle(packet_identifier)
            .map(|h| h.inbound_pubrel(reason_code))
            .unwrap_or_else (||
                // The reason code in this case can only be PacketIdentifierNotFound
                if reason_code.is_erroneous() {
                    (Response::None, Event::Ignored)
                } else {
                    (
                        Response::Complete(ReasonCode::PacketIdentifierNotFound),
                        Event::Ignored,
                    )
                },
            )
    }

    pub(crate) fn outbound_pubcomp(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_pubcomp())
            .unwrap_or(Err(StateError::UnusedPacketIdentifier))
    }
}

impl<const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn outbound_publish(
        &mut self,
        identified_qos: IdentifiedQoS,
    ) -> Result<(), StateError> {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => Ok(()),
            IdentifiedQoS::AtLeastOnce(pid) | IdentifiedQoS::ExactlyOnce(pid) => self
                .outbound_handle(pid)
                .map(|mut h| h.outbound_publish(identified_qos.into()))
                .unwrap_or(Err(StateError::UnusedPacketIdentifier)),
        }
    }

    pub(crate) fn inbound_puback(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        self.outbound_handle(packet_identifier)
            .map(|h| h.inbound_puback(reason_code))
            .unwrap_or((Response::None, Event::Ignored))
    }

    pub(crate) fn inbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        self.outbound_handle(packet_identifier)
            .map(|h| h.inbound_pubrec(reason_code))
            .unwrap_or((
                Response::Release(ReasonCode::PacketIdentifierNotFound),
                Event::Ignored,
            ))
    }

    pub(crate) fn outbound_pubrel(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.outbound_handle(packet_identifier)
            .map(|mut h| h.outbound_pubrel())
            .unwrap_or(Err(StateError::UnusedPacketIdentifier))
    }

    pub(crate) fn inbound_pubcomp(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        self.outbound_handle(packet_identifier)
            .map(|h| h.inbound_pubcomp(reason_code))
            .unwrap_or((Response::None, Event::Ignored))
    }
}

#[cfg(test)]
mod unit {
    use std::vec::Vec;

    use crate::{
        client::options::AckMode,
        session::{Event, Response, Session, StateError},
        types::{IdentifiedQoS, PacketIdentifier, QoS, ReasonCode},
    };

    macro_rules! sm_test {
        (
            $test_name:ident,
            [ $($steps:tt)* ]
        ) => {
            #[test]
            fn $test_name() {
                let mut sm = crate::session::Session::<10, 10, 10>::default();
                let pid = crate::types::PacketIdentifier::ONE;

                sm_test!(@munch, 1, sm, pid, $($steps)*);
            }
        };
        (@munch, $i:expr, $sm:ident, $pid:ident, reconnect() $( $rest:tt )* ) => {
            $sm.reconnect();
            sm_test!(@munch, $i + 1, $sm, $pid $( $rest )*)
        };
        (@munch, $i:expr, $sm:ident, $pid:ident, $meth:ident ( $($args:tt)* ) => $expected:ident $expected_args:tt $( $rest:tt )*) => {
            let left = sm_test!(@dispatch, $sm, $pid, $meth ( $($args)* ));
            let right = sm_test!(@expected, $expected, $expected_args);
            assert!(
                left == right,
                "Step {} failed: {}({}) => {}{}  !=  {}{:?}",
                $i,
                stringify!($meth),
                stringify!($($args)*),
                stringify!($expected),
                stringify!($expected_args),
                stringify!($expected),
                left,
            );
            sm_test!(@munch, $i + 1, $sm, $pid $($rest)*)
        };
        (@munch, $i:expr, $sm:ident, $pid:ident $(,)? ) => {};
        (@expected, ok, ()) => {
            Ok(())
        };
        (@expected, err, ($variant:ident)) => {
            Err($crate::session::StateError::$variant)
        };
        (@expected, res, ($response:ident $( ($reason_code:ident) )?, $event:ident)) => {
            (
                $crate::session::Response:: $response $( ($crate::types::ReasonCode:: $reason_code ) )?,
                $crate::session::Event:: $event
            )
        };

        (@dispatch, $sm:ident, $pid:ident, in_pub(AtMostOnce, $ack_mode:ident)) => {
            $sm.inbound_publish($crate::types::IdentifiedQoS::AtMostOnce, $crate::client::options::AckMode::$ack_mode)
        };
        (@dispatch, $sm:ident, $pid:ident, in_pub($qos:ident, $ack_mode:ident)) => {
            $sm.inbound_publish($crate::types::IdentifiedQoS::$qos($pid), $crate::client::options::AckMode::$ack_mode)
        };
        (@dispatch, $sm:ident, $pid:ident, out_ack()) => {
            $sm.outbound_puback($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, out_rec($rc:ident)) => {
            $sm.outbound_pubrec($pid, $crate::types::ReasonCode::$rc)
        };
        (@dispatch, $sm:ident, $pid:ident, in_rel($rc:ident)) => {
            $sm.inbound_pubrel($pid, $crate::types::ReasonCode::$rc)
        };
        (@dispatch, $sm:ident, $pid:ident, out_comp()) => {
            $sm.outbound_pubcomp($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, out_pub(AtMostOnce, $ack_mode:ident)) => {
            $sm.outbound_publish($crate::types::IdentifiedQoS::AtMostOnce, $crate::client::options::AckMode::$ack_mode)
        };
        (@dispatch, $sm:ident, $pid:ident, out_pub($qos:ident, $ack_mode:ident)) => {
            $sm.outbound_publish($crate::types::IdentifiedQoS::$qos($pid), $crate::client::options::AckMode$ack_mode)
        };
        (@dispatch, $sm:ident, $pid:ident, in_ack($rc:ident)) => {
            $sm.inbound_puback($pid, $crate::types::ReasonCode::$rc)
        };
        (@dispatch, $sm:ident, $pid:ident, in_rec($rc:ident)) => {
            $sm.inbound_pubrec($pid, $crate::types::ReasonCode::$rc)
        };
        (@dispatch, $sm:ident, $pid:ident, out_rel()) => {
            $sm.outbound_pubrel($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, in_comp($rc:ident)) => {
            $sm.inbound_pubcomp($pid, $crate::types::ReasonCode::$rc)
        };
    }

    sm_test!(
        pid_not_in_use,
        [
            out_ack() => err(UnusedPacketIdentifier),
            out_rec(Success) => err(UnusedPacketIdentifier),
            out_rel() => err(UnusedPacketIdentifier),
            out_comp() => err(UnusedPacketIdentifier),
            in_ack(Success) => res(None, Ignored),
            in_ack(TopicNameInvalid) => res(None, Ignored),
            in_rec(Success) => res(Release(PacketIdentifierNotFound), Ignored),
            in_rec(TopicNameInvalid) => res(Release(PacketIdentifierNotFound), Ignored),    // This is matching state, server doesn't know this PID and neither do we
            in_rel(Success) => res(Complete(PacketIdentifierNotFound), Ignored),
            in_rel(PacketIdentifierNotFound) => res(None, Ignored),
            in_comp(Success) => res(None, Ignored),
            in_comp(TopicNameInvalid) => res(None, Ignored),
        ]
    );

    #[test_log::test]
    #[test]
    fn inbound_qos1() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), AckMode::Manual);

            if r == Response::Disconnect(ReasonCode::QuotaExceeded) {
                assert_eq!(e, Event::ServerError);
            } else {
                pids.push(pid);

                assert_eq!(r, Response::None);
                assert_eq!(e, Event::Publish);
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
        }
        // Invalid server actions should not be allowed
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Republish should lead to duplicate deliveries
        sm.reconnect();
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), AckMode::Manual);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Publish);
        }

        // Complete the publications
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Ok(()));
        }

        // Packet identifiers are not in the session anymore
        assert!(sm.inbound_publishes.is_empty());
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), AckMode::Automatic);
            assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }

        assert!(sm.inbound_publishes.is_empty());
    }

    sm_test!(
        inbound_qos1_auto_full_macro,
        [
            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
            out_ack() => err(UnusedPacketIdentifier),
            out_rec(Success) => err(UnusedPacketIdentifier),
            out_rel() => err(UnusedPacketIdentifier),
            out_comp() => err(UnusedPacketIdentifier),
            in_ack(Success) => res(None, Ignored),
            in_ack(TopicNameInvalid) => res(None, Ignored),
            in_rec(Success) => res(Release(PacketIdentifierNotFound), Ignored),
            in_rec(TopicNameInvalid) => res(Release(PacketIdentifierNotFound), Ignored),
            in_rel(Success) => res(Complete(PacketIdentifierNotFound), Ignored),
            in_rel(PacketIdentifierNotFound) => res(None, Ignored),
            in_comp(Success) => res(None, Ignored),
            in_comp(PacketIdentifierNotFound) => res(None, Ignored),

            // Republish is allowed and should lead to duplicate delivery
            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),

            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            reconnect(),
            in_pub(AtLeastOnce, Automatic) => res(None, Publish),

            out_ack() => ok(),

            out_ack() => err(UnusedPacketIdentifier),
            out_rec(Success) => err(UnusedPacketIdentifier),
            out_rel() => err(UnusedPacketIdentifier),
            out_comp() => err(UnusedPacketIdentifier),
            in_ack(Success) => res(None, Ignored),
            in_ack(TopicNameInvalid) => res(None, Ignored),
            in_rec(Success) => res(Release(PacketIdentifierNotFound), Ignored),
            in_rec(TopicNameInvalid) => res(Release(PacketIdentifierNotFound), Ignored),
            in_rel(Success) => res(Complete(PacketIdentifierNotFound), Ignored),
            in_rel(PacketIdentifierNotFound) => res(None, Ignored),
            in_comp(Success) => res(None, Ignored),
            in_comp(PacketIdentifierNotFound) => res(None, Ignored),
        ]
    );

    #[test_log::test]
    #[test]
    fn inbound_qos1_auto_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 1 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound); // This is matching state, server doesn't know this PID and neither do we
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // A republish with manual set to false should use the old manual setting
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound); // This is matching state, server doesn't know this PID and neither do we
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn inbound_qos1_manual_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 1 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.outbound_publishes.is_empty());

        // Complete the handshake
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Ok(()));
        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_auto() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Automatic);

            if r == Response::Disconnect(ReasonCode::QuotaExceeded) {
                assert_eq!(e, Event::ServerError);
            } else {
                pids.push(pid);

                assert_eq!(r, Response::Receive(ReasonCode::Success));
                assert_eq!(e, Event::Publish);
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Republish should lead to duplicate deliveries
        sm.reconnect();
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Automatic);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Duplicate(AckMode::Automatic));
        }

        // Invalid client actions shouldn't be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Complete the QoS 2 publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::Success));
            assert_eq!(e, Event::Released(AckMode::Automatic));
        }

        // Packet identifiers are not in the session anymore
        assert!(sm.inbound_publishes.is_empty());
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Automatic);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_manual() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Manual);

            if r == Response::Disconnect(ReasonCode::QuotaExceeded) {
                assert_eq!(e, Event::ServerError);
            } else {
                pids.push(pid);

                assert_eq!(r, Response::None);
                assert_eq!(e, Event::Publish);
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Republish should lead to duplicate deliveries
        sm.reconnect();
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Manual);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Duplicate(AckMode::Manual));
        }

        // Sending the PUBREL before PUBREC is received by the server is an error
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Proceed to the next handshake state
        for pid in pids.iter().copied() {
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Ok(()));
        }

        // Now sending a PUBREC shouldn't be allowed either
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Released(AckMode::Manual));
        }

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Ok(()));
        }

        // Packet identifiers are not in the session anymore
        assert!(sm.inbound_publishes.is_empty());
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), AckMode::Automatic);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_auto_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 2 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Receive(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Receive(ReasonCode::Success));
        assert_eq!(e, Event::Duplicate(AckMode::Automatic));

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::Success));
        assert_eq!(e, Event::Released(AckMode::Automatic));

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_manual_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 2 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Duplicate(AckMode::Manual));

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Ok(()));

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Duplicate(AckMode::Manual));

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Ok(()));
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Released(AckMode::Manual));

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Release(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish should not be allowed now
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Duplicate PUBREL should be allowed
        sm.reconnect();
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Released(AckMode::Manual));

        assert!(sm.outbound_publishes.is_empty());

        // Complete the handshake
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_error_abort() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Manual);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Ok(()));

        // Handle this relatively lax by removing the state
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Aborted);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());

        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), AckMode::Automatic);
        assert_eq!(r, Response::Receive(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        // Handle this relatively lax by removing the state
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Aborted);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos1() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            if let Some(h) = sm.free_handle() {
                let pid = h.packet_identifier;
                if let Err(e) = h.outbound_publish(QoS::AtLeastOnce, AckMode::Automatic) {
                    assert_eq!(e, StateError::NoCapacity);
                    break;
                } else {
                    pids.push(pid);
                }
            } else {
                break;
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Republish should be allowed
        sm.reconnect();
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(pid));
            assert_eq!(r, Ok(()));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Acknowledged(AckMode::Automatic));
        }

        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos1_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm
            .free_handle()
            .unwrap()
            .outbound_publish(QoS::AtLeastOnce, AckMode::Automatic);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        // Complete the handshake
        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Acknowledged(AckMode::Automatic));

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos1_error_reject() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm
            .free_handle()
            .unwrap()
            .outbound_publish(QoS::AtLeastOnce, AckMode::Automatic);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Reject the publication
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Rejected);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_auto() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pids = Vec::new();

        loop {
            if let Some(h) = sm.free_handle() {
                let pid = h.packet_identifier;
                if let Err(e) = h.outbound_publish(QoS::ExactlyOnce, AckMode::Automatic) {
                    assert_eq!(e, StateError::NoCapacity);
                    break;
                } else {
                    pids.push(pid);
                }
            } else {
                break;
            }
        }

        // Republish should be allowed
        sm.reconnect();
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Ok(()));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Move to next stage
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::Release(ReasonCode::Success));
            assert_eq!(e, Event::Received(AckMode::Automatic));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Completed(AckMode::Automatic));
        }

        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_manual() {
        let mut sm = Session::<10, 10, 10>::default();

        let mut pids = Vec::new();

        loop {
            if let Some(h) = sm.free_handle() {
                let pid = h.packet_identifier;
                if let Err(e) = h.outbound_publish(QoS::ExactlyOnce, AckMode::Manual) {
                    assert_eq!(e, StateError::NoCapacity);
                    break;
                } else {
                    pids.push(pid);
                }
            } else {
                break;
            }
        }

        // Republish should be allowed
        sm.reconnect();
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Ok(()));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Move to next stage
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Received(AckMode::Manual));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Move to next stage
        for pid in pids.iter().copied() {
            let r = sm.outbound_pubrel(pid);
            assert_eq!(r, Ok(()));
        }

        // Invalid server actions should lead to disconnect
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_puback(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
            let (r, e) = sm.inbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Completed(AckMode::Manual));
        }

        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_auto_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm
            .free_handle()
            .unwrap()
            .outbound_publish(QoS::ExactlyOnce, AckMode::Automatic);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.inbound_publishes.is_empty());

        // Republish should be allowed
        sm.reconnect();
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID));
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.inbound_publishes.is_empty());

        // Proceed to the next handshake state
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Release(ReasonCode::Success));
        assert_eq!(e, Event::Received(AckMode::Automatic));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.inbound_publishes.is_empty());

        // Republish should not be allowed after sending PUBREL
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID));
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        assert!(sm.inbound_publishes.is_empty());

        // Rerelease should be allowed
        sm.reconnect();
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.inbound_publishes.is_empty());

        // Complete the handshake
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Completed(AckMode::Automatic));

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_manual_full() {
        let mut sm = Session::<10, 10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm
            .free_handle()
            .unwrap()
            .outbound_publish(QoS::ExactlyOnce, AckMode::Manual);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.inbound_publishes.is_empty());

        // Republish should be allowed
        sm.reconnect();
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID));
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.inbound_publishes.is_empty());

        // Proceed to the next handshake state
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Received(AckMode::Manual));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.inbound_publishes.is_empty());

        assert!(sm.inbound_publishes.is_empty());

        // Republish should not be allowed after sending PUBREL
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID));
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        assert!(sm.inbound_publishes.is_empty());

        // Rerelease should be allowed
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubcomp(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

        let (r, e) = sm.inbound_puback(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::Success);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_pubrec(PID, ReasonCode::TopicNameInvalid);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
        assert_eq!(e, Event::Ignored);
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Ignored);

        assert!(sm.inbound_publishes.is_empty());

        // Complete the handshake
        let (r, e) = sm.inbound_pubcomp(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Completed(AckMode::Manual));

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[ignore]
    #[test_log::test]
    #[test]
    fn inbound_outbound_pid_collision_allowed() {}
}

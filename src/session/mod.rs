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
    /// A PUBREC packet has been received. The next step in the handshake is the client
    /// sending a PUBREL packet. This packet must be sent manually by the user.
    DueRel,
    /// A reconnection has occured with a PUBREL packet having been sent before. The next step
    /// in the handshake is the client resending a PUBREL packet. Whether this packet must be
    /// sent manually by the user is determined by the contained [`AckMode`].
    DueReRel(AckMode),
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
            Self::DueRel => Self::DueRel,
            Self::DueReRel(mode) => Self::DueReRel(mode),
            Self::AwaitComp(mode) => Self::DueReRel(mode),
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
    /// A PUBREL packet has been received before a reconnection occurred. The next step in the
    /// handshake is the server sending a PUBREL packet. The subsequent PUBCOMP packet
    /// must be sent manually by the client.
    AwaitReRel,
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
            Self::AwaitReRel => Self::AwaitReRel,
            Self::DueComp => Self::AwaitReRel,
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
    PacketIdentifierUnused,
    QoSMismatched,
    HandshakeStateMismatched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum Event {
    Publish,
    Duplicate(AckMode),
    Ignored,

    Acknowledged,
    Received(AckMode),
    Released(AckMode),
    Completed,

    Aborted,
    Rejected,

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
                .map(|h| h.inbound_republish(identified_qos.into()))
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
            .unwrap_or(Err(StateError::PacketIdentifierUnused))
    }

    pub(crate) fn outbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_pubrec(reason_code))
            .unwrap_or(Err(StateError::PacketIdentifierUnused))
    }

    pub(crate) fn inbound_pubrel(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        self.inbound_handle(packet_identifier)
            .map(|h| h.inbound_pubrel(reason_code))
            .unwrap_or_else(|| {
                (
                    Response::Complete(ReasonCode::PacketIdentifierNotFound),
                    Event::Ignored,
                )
            })
    }

    pub(crate) fn outbound_pubcomp(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_pubcomp())
            .unwrap_or(Err(StateError::PacketIdentifierUnused))
    }
}

impl<const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn outbound_republish(
        &mut self,
        identified_qos: IdentifiedQoS,
    ) -> Result<(), StateError> {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => Ok(()),
            IdentifiedQoS::AtLeastOnce(pid) | IdentifiedQoS::ExactlyOnce(pid) => self
                .outbound_handle(pid)
                .map(|mut h| h.outbound_republish(identified_qos.into()))
                .unwrap_or(Err(StateError::PacketIdentifierUnused)),
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
            .unwrap_or(Err(StateError::PacketIdentifierUnused))
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
                "Step {} failed: {}({}) => {}{}  !=  {:?}",
                $i,
                stringify!($meth),
                stringify!($($args)*),
                stringify!($expected),
                stringify!($expected_args),
                left
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
        (@expected, res, ($response:ident $( ($reason_code:ident) )?, $event:ident $( ($ack_mode:ident) )? )) => {
            (
                $crate::session::Response:: $response $( ($crate::types::ReasonCode:: $reason_code ) )?,
                $crate::session::Event:: $event $( ($crate::client::options::AckMode:: $ack_mode ) )?
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
            compile_error!("QoS 1 is not tracked and the session does not provide any methods for it")
        };
        (@dispatch, $sm:ident, $pid:ident, out_pub($qos:ident, $ack_mode:ident)) => {
            {
                let handle = $sm.free_handle().expect("no free packet identifier or space in the session");
                assert_eq!(handle.packet_identifier, $pid, "sm_test! can only handle a single packet identifier at a time");
                handle.outbound_publish($crate::types::QoS::$qos, $crate::client::options::AckMode::$ack_mode)
            }
        };
        (@dispatch, $sm:ident, $pid:ident, out_repub($qos:ident)) => {
            $sm.outbound_republish($crate::types::IdentifiedQoS::$qos($pid))
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

    // Allgemein gilt:
    // - Ein einziges negatives PUBX paket (egal wann) reicht, um den eintrag zu löschen
    // - Im manual mode trotzdem automatische PUBX response pakete, wenn
    //   - mit dem eingehenden Paket aktuell nicht gerechnet wird, man
    //     aber trotzdem eine response schicken muss gemäß spec und das
    //     ausgehende Paket nicht schon manuell ausstehend (due) ist.
    //   - ausgehende PUBX fehlerhafte reason codes benötigen
    //
    // Bei QoS Mismatch (Anderes QoS im PUBLISH):
    //
    // Bei QoS Mismatch (PUBACK bzw PUBREC/PUBREL/PUBCOMP) auf anderes QoS entry:
    //
    // Bei State Mismatch (vor allem QoS 2):
    //

    mod spec {
        mod qos1 {
            mod sender {
                sm_test!(
                    must_resend_unacknowledged,
                    [
                        out_pub(AtLeastOnce, Automatic) => ok(),
                        reconnect(),
                        out_repub(AtLeastOnce) => ok(),
                        reconnect(),
                        out_repub(AtLeastOnce) => ok(),
                    ]
                );
                sm_test!(
                    must_not_resend_same_connection,
                    [
                        out_pub(AtLeastOnce, Automatic) => ok(),
                        out_repub(AtLeastOnce) => err(HandshakeStateMismatched),
                        reconnect(),
                        out_repub(AtLeastOnce) => ok(),
                        out_repub(AtLeastOnce) => err(HandshakeStateMismatched),
                    ]
                );
                sm_test!(
                    must_not_resend_acknowledged,
                    [
                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(Success) => res(None, Acknowledged),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),

                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(Success) => res(None, Acknowledged),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),

                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(Success) => res(None, Acknowledged),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),
                    ]
                );
                sm_test!(
                    must_not_resend_negative_acknowledged,
                    [
                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(UnspecifiedError) => res(None, Rejected),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),

                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(UnspecifiedError) => res(None, Rejected),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),

                        out_pub(AtLeastOnce, Automatic) => ok(),
                        in_ack(UnspecifiedError) => res(None, Rejected),
                        reconnect(),
                        out_repub(AtLeastOnce) => err(PacketIdentifierUnused),
                    ]
                );
            }

            mod receiver {
                mod automatic {
                    sm_test!(
                        must_acknowledge,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_not_resend_puback,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                            out_ack() => err(PacketIdentifierUnused),
                            reconnect(),
                            out_ack() => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_1,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_2,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_1,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_2,
                        [
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                }

                mod manual {
                    sm_test!(
                        must_acknowledge,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),

                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_resend_puback,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                            out_ack() => err(PacketIdentifierUnused),
                            reconnect(),
                            out_ack() => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_1,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_2,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_1,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_2,
                        [
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                }
            }
        }
        mod qos2 {
            mod sender {
                mod automatic {
                    sm_test!(
                        must_resend_unacknowledged_publish,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_same_connection,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_acknowledged_publish_1,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_acknowledged_publish_2,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_negative_acknowledged_publish,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_release,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                        ]
                    );
                    // The following two test is deliberately not named "must" because the spec doesn't state that the sender
                    // must not send a PUBREL, however this test case can be derived from this receiver section:
                    // | If it has sent a PUBREC with a Reason Code of 0x80 or greater, the receiver MUST treat any subsequent
                    // | PUBLISH packet that contains that Packet Identifier as being a new Application Message [MQTT-4.3.3-9].
                    sm_test!(
                        no_release_negative_acknowledgement,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                        ]
                    );
                    sm_test!(
                        must_release_entry_1,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            in_rec(Success) => res(Release(Success), Ignored),
                        ]
                    );
                    sm_test!(
                        must_release_entry_2,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            reconnect(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_release_entry_3,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            reconnect(),
                            in_rec(Success) => res(Release(Success), Ignored),
                        ]
                    );
                    sm_test!(
                        must_release_no_entry_1,
                        [
                            in_rec(Success) => res(Release(PacketIdentifierNotFound), Ignored),
                        ]
                    );

                    sm_test!(
                        must_resend_unacknowledged_pubrel,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            reconnect(),
                            out_rel() => ok(),
                            reconnect(),
                            out_rel() => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_resend_pubrel_same_connection,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            out_rel() => err(HandshakeStateMismatched),
                            reconnect(),
                            out_rel() => ok(),
                            out_rel() => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_acknowledged_pubrel,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            in_comp(Success) => res(None, Completed),
                            reconnect(),
                            out_rel() => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            in_comp(Success) => res(None, Completed),
                            out_rel() => err(PacketIdentifierUnused),
                            reconnect(),
                            out_rel() => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_1,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_2,
                        [
                            out_pub(ExactlyOnce, Automatic) => ok(),
                            in_rec(Success) => res(Release(Success), Received(Automatic)),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                }

                mod manual {
                    sm_test!(
                        must_resend_unacknowledged_publish,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                        ]
                    );

                    sm_test!(
                        must_not_resend_publish_same_connection,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );

                    sm_test!(
                        must_not_resend_acknowledged_publish_1,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_acknowledged_publish_2,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_negative_acknowledged_publish,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_release,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                        ]
                    );
                    // The following two test is deliberately not named "must" because the spec doesn't state that the sender
                    // must not send a PUBREL, however this test case can be derived from this receiver section:
                    // | If it has sent a PUBREC with a Reason Code of 0x80 or greater, the receiver MUST treat any subsequent
                    // | PUBLISH packet that contains that Packet Identifier as being a new Application Message [MQTT-4.3.3-9].
                    sm_test!(
                        no_release_negative_acknowledgement,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(UnspecifiedError) => res(None, Rejected),
                            out_rel() => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_release_entry_1,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            in_rec(Success) => res(Release(Success), Ignored),
                        ]
                    );
                    sm_test!(
                        must_release_entry_2,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            reconnect(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                        ]
                    );
                    sm_test!(
                        must_release_entry_3,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            reconnect(),
                            in_rec(Success) => res(None, Ignored),
                            out_rel() => ok(),
                        ]
                    );
                    sm_test!(
                        must_resend_unacknowledged_pubrel,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            reconnect(),
                            out_rel() => ok(),
                            reconnect(),
                            out_rel() => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_resend_pubrel_same_connection,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            out_rel() => err(HandshakeStateMismatched),
                            reconnect(),
                            out_rel() => ok(),
                            out_rel() => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_acknowledged_pubrel,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            in_comp(Success) => res(None, Completed),
                            reconnect(),
                            out_rel() => err(PacketIdentifierUnused),

                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            in_comp(Success) => res(None, Completed),
                            out_rel() => err(PacketIdentifierUnused),
                            reconnect(),
                            out_rel() => err(PacketIdentifierUnused),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_1,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_2,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            out_rel() => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_3,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            reconnect(),
                            out_rel() => ok(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_not_resend_publish_after_pubrel_4,
                        [
                            out_pub(ExactlyOnce, Manual) => ok(),
                            in_rec(Success) => res(None, Received(Manual)),
                            reconnect(),
                            out_rel() => ok(),
                            reconnect(),
                            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
                        ]
                    );
                }
            }

            mod receiver {
                mod automatic {
                    sm_test!(
                        must_acknowledge_publish,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_not_resend_pubrec,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            out_rec(Success) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_rec(Success) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_3,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_4,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_3,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_4,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_reconnect_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_reconnect_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_pubrel_regular_entry,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),

                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_negative_pubrel_entry,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(PacketIdentifierNotFound) => res(Complete(Success), Aborted),

                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(PacketIdentifierNotFound) => res(Complete(Success), Aborted),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_pubrel_regular_no_entry,
                        [
                            in_rel(Success) => res(Complete(PacketIdentifierNotFound), Ignored),

                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_negative_pubrel_no_entry,
                        [
                            in_rel(PacketIdentifierNotFound) => res(Complete(PacketIdentifierNotFound), Ignored),

                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_3,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_4,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_5,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            reconnect(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_6,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_1,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_2,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_3,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_4,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            reconnect(),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_5,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_6,
                        [
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                }

                mod manual {
                    sm_test!(
                        must_acknowledge_publish_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_resend_pubrec,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            out_rec(Success) => err(HandshakeStateMismatched),
                            reconnect(),
                            out_rec(Success) => err(HandshakeStateMismatched),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_erroneous_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(UnspecifiedError) => ok(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_erroneous_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(UnspecifiedError) => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(UnspecifiedError) => ok(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(UnspecifiedError) => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_wrong_qos_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_wrong_qos_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_wrong_qos_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_erroneous_pubrec_wrong_qos_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_before_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_before_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_before_pubrec_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_before_pubrec_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_after_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_after_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_after_pubrec_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_publish_before_pubrel_after_pubrec_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            out_rec(Success) => ok(),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_before_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_before_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_before_pubrec_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_before_pubrec_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_after_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_after_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Manual) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_after_pubrec_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_regular_after_pubrec_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Manual)),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(None, Duplicate(Manual)),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_before_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_before_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_after_pubrec_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_after_pubrec_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_before_pubrec_after_reconnect_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_before_pubrec_after_reconnect_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            reconnect(),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_after_pubrec_after_reconnect_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_not_cause_duplicate_qos1_after_pubrec_after_reconnect_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_pub(AtLeastOnce, Automatic) => res(Receive(PacketIdentifierInUse), Aborted),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_pubrel_regular_entry,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),

                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_negative_pubrel_entry,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(PacketIdentifierNotFound) => res(Complete(Success), Aborted),

                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(PacketIdentifierNotFound) => res(Complete(Success), Aborted),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_pubrel_regular_no_entry,
                        [
                            in_rel(Success) => res(Complete(PacketIdentifierNotFound), Ignored),

                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_acknowledge_negative_pubrel_no_entry,
                        [
                            in_rel(PacketIdentifierNotFound) => res(Complete(PacketIdentifierNotFound), Ignored),

                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_5,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_6,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_7,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            reconnect(),
                            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos1_message_8,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            reconnect(),
                            in_pub(AtLeastOnce, Manual) => res(None, Publish),
                            out_ack() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_1,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_2,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_3,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_4,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_5,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_6,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            reconnect(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                    sm_test!(
                        must_accept_new_qos2_message_7,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            reconnect(),
                            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
                            in_rel(Success) => res(Complete(Success), Released(Automatic)),
                        ]
                    );
                    sm_test!(must_accept_new_qos2_message_8,
                        [
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                            reconnect(),
                            in_pub(ExactlyOnce, Manual) => res(None, Publish),
                            out_rec(Success) => ok(),
                            in_rel(Success) => res(None, Released(Manual)),
                            out_comp() => ok(),
                        ]
                    );
                }
            }
        }
    }
    mod local_error_prevention {
        // No QoS criss-cross
        // Buffer exceedance
    }
    mod remote_error_detection {
        // No QoS criss-cross
        // Buffer exceedance
    }

    sm_test!(
        // All other cases but this one should be plentily covered by the spec tests
        republish_cant_change_ack_mode,
        [
            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            in_pub(AtLeastOnce, Automatic) => res(None, Publish),
            reconnect(),
            in_pub(AtLeastOnce, Automatic) => res(None, Publish),
            out_ack() => ok(),
        ]
    );

    sm_test!(
        receiver_not_accept_publish_after_pubrel_1,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            in_rel(Success) => res(None, Released(Manual)),
            in_pub(ExactlyOnce, Manual) => res(Disconnect(ProtocolError), ServerError),
        ]
    );
    sm_test!(
        receiver_not_accept_publish_after_pubrel_2,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            in_rel(Success) => res(None, Released(Manual)),
            in_pub(ExactlyOnce, Automatic) => res(Disconnect(ProtocolError), ServerError),
        ]
    );

    sm_test!(
        no_acks_out_of_thin_air,
        [
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );

    sm_test!(
        sender_automatic_no_qos1_publish_retransmit,
        [
            out_pub(AtLeastOnce, Automatic) => ok(),
            out_repub(AtLeastOnce) => err(HandshakeStateMismatched),
            reconnect(),
            out_repub(AtLeastOnce) => ok(),
            out_repub(AtLeastOnce) => err(HandshakeStateMismatched),
            in_ack(Success) => res(None, Acknowledged),
            out_repub(AtLeastOnce) => err(PacketIdentifierUnused),

            out_pub(AtLeastOnce, Automatic) => ok(),
            in_ack(UnspecifiedError) => res(None, Rejected),
            out_repub(AtLeastOnce) => err(PacketIdentifierUnused),
        ]
    );

    sm_test!(
        sender_automatic_no_qos2_publish_retransmit,
        [
            out_pub(ExactlyOnce, Automatic) => ok(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            reconnect(),
            out_repub(ExactlyOnce) => ok(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            in_rec(Success) => res(Release(Success), Received(Automatic)),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            reconnect(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            in_comp(Success) => res(None, Completed),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

            out_pub(ExactlyOnce, Automatic) => ok(),
            in_rec(UnspecifiedError) => res(None, Rejected),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

            out_pub(ExactlyOnce, Automatic) => ok(),
            in_rec(Success) => res(Release(Success), Received(Automatic)),
            reconnect(),
            in_comp(Success) => res(None, Completed),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        sender_manual_no_qos2_publish_retransmit,
        [
            out_pub(ExactlyOnce, Manual) => ok(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            reconnect(),
            out_repub(ExactlyOnce) => ok(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            in_rec(Success) => res(None, Received(Manual)),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            reconnect(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            out_rel() => ok(),
            out_repub(ExactlyOnce) => err(HandshakeStateMismatched),
            in_comp(Success) => res(None, Completed),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

            out_pub(ExactlyOnce, Manual) => ok(),
            in_rec(UnspecifiedError) => res(None, Rejected),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),

            out_pub(ExactlyOnce, Manual) => ok(),
            in_rec(Success) => res(None, Received(Manual)),
            out_rel() => ok(),
            reconnect(),
            in_comp(Success) => res(None, Completed),
            out_repub(ExactlyOnce) => err(PacketIdentifierUnused),
        ]
    );

    sm_test!(
        sender_automatic_no_pubrel_retransmit,
        [
            out_pub(ExactlyOnce, Automatic) => ok(),
            in_rec(Success) => res(Release(Success), Received(Automatic)),
            out_rel() => err(HandshakeStateMismatched),
            reconnect(),
            out_rel() => ok(),
            out_rel() => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        sender_manual_no_pubrel_retransmit,
        [
            out_pub(ExactlyOnce, Manual) => ok(),
            in_rec(Success) => res(None, Received(Manual)),
            out_rel() => ok(),
            out_rel() => err(HandshakeStateMismatched),
            reconnect(),
            out_rel() => ok(),
            out_rel() => err(HandshakeStateMismatched),
        ]
    );

    sm_test!(
        receiver_automatic_no_puback_retransmit,
        [
            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
            out_ack() => err(PacketIdentifierUnused),

            in_pub(AtLeastOnce, Automatic) => res(Acknowledge(Success), Publish),
            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_manual_no_puback_retransmit,
        [
            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            out_ack() => ok(),
            out_ack() => err(PacketIdentifierUnused),

            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            out_ack() => ok(),
            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
        ]
    );

    sm_test!(
        receiver_automatic_no_pubrec_retransmit_1,
        [
            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        receiver_automatic_no_pubrec_retransmit_2,
        [
            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
            reconnect(),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        receiver_manual_no_erroneous_pubrec_retransmit,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(UnspecifiedError) => ok(),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),

            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(UnspecifiedError) => ok(),
            reconnect(),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_manual_no_pubrec_retransmit_1,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            out_rec(Success) => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        receiver_manual_no_pubrec_retransmit_2,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            reconnect(),
            out_rec(Success) => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        receiver_manual_no_pubrec_reconnect_retransmit,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            reconnect(),
            out_rec(Success) => err(HandshakeStateMismatched),
        ]
    );
    sm_test!(
        receiver_manual_no_erroneous_pubrec_reconnect_retransmit,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            reconnect(),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
        ]
    );

    sm_test!(
        receiver_automatic_no_pubcomp_retransmit,
        [
            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
            in_rel(Success) => res(Complete(Success), Released(Automatic)),
            out_comp() => err(PacketIdentifierUnused),

            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
            in_rel(Success) => res(Complete(Success), Released(Automatic)),
            reconnect(),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_manual_no_pubcomp_retransmit,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            in_rel(Success) => res(None, Released(Manual)),
            out_comp() => ok(),
            out_comp() => err(PacketIdentifierUnused),

            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            in_rel(Success) => res(None, Released(Manual)),
            out_comp() => ok(),
            reconnect(),
            out_comp() => err(PacketIdentifierUnused),

            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_rec(Success) => ok(),
            in_rel(Success) => res(None, Released(Manual)),
            reconnect(),
            out_comp() => err(HandshakeStateMismatched),
        ]
    );

    sm_test!(
        sender_no_incorrect_acknowledgement_qos1,
        [
            out_pub(AtLeastOnce, Automatic) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(QoSMismatched),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(QoSMismatched),
            out_comp() => err(PacketIdentifierUnused),

            out_repub(AtLeastOnce) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(QoSMismatched),
            out_comp() => err(PacketIdentifierUnused),

            in_ack(Success) => res(None, Acknowledged),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        sender_automatic_no_incorrect_acknowledgement_qos2,
        [
            out_pub(ExactlyOnce, Automatic) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            out_repub(ExactlyOnce) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            in_rec(Success) => res(Release(Success), Received(Automatic)),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),

            out_rel() => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            in_comp(Success) => res(None, Completed),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        sender_manual_no_incorrect_acknowledgement_qos2,
        [
            out_pub(ExactlyOnce, Manual) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            out_repub(ExactlyOnce) => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            in_rec(Success) => res(None, Received(Manual)),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),

            out_rel() => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),

            out_rel() => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(HandshakeStateMismatched),
            out_comp() => err(PacketIdentifierUnused),

            in_comp(Success) => res(None, Completed),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_manual_no_incorrect_acknowledgement_qos1,
        [
            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            out_rec(Success) => err(QoSMismatched),
            out_rec(UnspecifiedError) => err(QoSMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(QoSMismatched),

            reconnect(),
            out_ack() => err(HandshakeStateMismatched),
            out_rec(Success) => err(QoSMismatched),
            out_rec(UnspecifiedError) => err(QoSMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(QoSMismatched),

            in_pub(AtLeastOnce, Manual) => res(None, Publish),
            out_ack() => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_automatic_no_incorrect_acknowledgement_qos2,
        [
            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Publish),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            reconnect(),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            in_pub(ExactlyOnce, Automatic) => res(Receive(Success), Duplicate(Automatic)),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            in_rel(Success) => res(Complete(Success), Released(Automatic)),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
        ]
    );
    sm_test!(
        receiver_manual_no_incorrect_acknowledgement_qos2,
        [
            in_pub(ExactlyOnce, Manual) => res(None, Publish),
            out_ack() => err(QoSMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            reconnect(),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            in_pub(ExactlyOnce, Manual) => res(None, Duplicate(Manual)),
            out_ack() => err(QoSMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            out_rec(Success) => ok(),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            in_rel(Success) => res(None, Released(Manual)),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),

            reconnect(),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(HandshakeStateMismatched),

            in_rel(Success) => res(None, Released(Manual)),
            out_ack() => err(QoSMismatched),
            out_rec(Success) => err(HandshakeStateMismatched),
            out_rec(UnspecifiedError) => err(HandshakeStateMismatched),
            out_rel() => err(PacketIdentifierUnused),

            out_comp() => ok(),
            out_ack() => err(PacketIdentifierUnused),
            out_rec(Success) => err(PacketIdentifierUnused),
            out_rec(UnspecifiedError) => err(PacketIdentifierUnused),
            out_rel() => err(PacketIdentifierUnused),
            out_comp() => err(PacketIdentifierUnused),
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
            assert_eq!(r, Err(StateError::QoSMismatched));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::QoSMismatched));
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
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));

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
            assert_eq!(r, Err(StateError::QoSMismatched));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
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
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));

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

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::QoSMismatched));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
        }

        // Proceed to the next handshake state
        for pid in pids.iter().copied() {
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Ok(()));
        }

        // Now sending a PUBREC shouldn't be allowed either
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::QoSMismatched));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
        }

        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Released(AckMode::Manual));
        }

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::QoSMismatched));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
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
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubrec(pid, ReasonCode::Success);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::PacketIdentifierUnused));

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
            let r = sm.outbound_republish(IdentifiedQoS::AtLeastOnce(pid));
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
            assert_eq!(e, Event::Acknowledged);
        }

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
            let r = sm.outbound_republish(IdentifiedQoS::ExactlyOnce(pid));
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
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_republish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Completed);
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
            let r = sm.outbound_republish(IdentifiedQoS::ExactlyOnce(pid));
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
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
            assert_eq!(e, Event::ServerError);
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_republish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
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
        }

        // Republishing at this stage should be disallowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_republish(IdentifiedQoS::ExactlyOnce(pid));
            assert_eq!(r, Err(StateError::HandshakeStateMismatched));
        }

        // Complete the publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubcomp(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Completed);
        }

        assert!(sm.outbound_publishes.is_empty());
    }

    #[ignore]
    #[test_log::test]
    #[test]
    fn inbound_outbound_pid_collision_allowed() {}
}

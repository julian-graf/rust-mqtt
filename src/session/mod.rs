//! Contains utilities for session management.

use core::cmp::min;
use heapless::Vec;

use crate::{
    session::{
        handle::{FreeHandle, InboundHandle, OutboundHandle},
        state::{LocalPublishState, PeerPublishState},
        state_machine::{Event, Response, StateError},
    },
    types::{
        IdentifiedQoS, PacketIdentifier,
        ReasonCode,
    },
};

pub(crate) mod handle;
pub mod state;
pub(crate) mod state_machine;

/// Session-associated information
///
/// Client identifier is not stored here as it would lead to inconsistencies with the underyling allocation system.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Session<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    /// The currently in-flight incoming publications.
    pub inbound_publishes: Vec<(PacketIdentifier, PeerPublishState), RECEIVE_MAXIMUM>,
    /// The currently in-flight outgoing publications.
    pub outbound_publishes: Vec<(PacketIdentifier, LocalPublishState), SEND_MAXIMUM>,
}

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub fn inbound_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<InboundHandle<'_, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        self.inbound_publishes
            .iter()
            .copied()
            .enumerate()
            .find(|(_, e)| e.0 == packet_identifier)
            .map(|(i, (_, state))| InboundHandle {
                session: self,
                i,
                packet_identifier,
                state,
            })
    }
    pub fn outbound_handle(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<OutboundHandle<'_, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
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
    pub fn handle_sub() {}
    pub fn handle_unsub() {}
    pub fn free_handle(&mut self) -> Option<FreeHandle<'_, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
        if RECEIVE_MAXIMUM == usize::from(u16::MAX) && !self.available_outbound_capacity() {
            return None;
        }

        // TODO this can be a better search with a stack bitset / larger window of PIDs

        let mut packet_identifier = PacketIdentifier::ONE;

        while self.outbound_handle(packet_identifier).is_some() {
            packet_identifier = packet_identifier.next();
        }

        return Some(FreeHandle {
            session: self,
            packet_identifier,
        });
    }

    pub(crate) fn outbound_iter(
        &mut self,
    ) -> Option<OutboundHandle<'_, RECEIVE_MAXIMUM, SEND_MAXIMUM>> {
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

    /// Returns the amount of currently in-flight incoming publications.
    #[must_use]
    pub fn active_inbound_publishes(&self) -> u16 {
        debug_assert!(u16::try_from(self.inbound_publishes.len()).is_ok());

        self.inbound_publishes.len() as u16
    }
    /// Returns the amount of slots for incoming publications.
    #[must_use]
    pub fn available_inbound_capacity(&self) -> bool {
        let capacity = min(self.inbound_publishes.capacity(), usize::from(u16::MAX)) as u16;

        capacity > self.active_inbound_publishes()
    }

    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet
    /// for an incoming/server publication. Assumes the packet identifier has no
    /// entry currently.
    ///
    /// # Safety
    /// `self.pending_server_publishes` has free capacity.
    pub(crate) fn schedule_inbound(
        &mut self,
        packet_identifier: PacketIdentifier,
        state: PeerPublishState,
    ) {
        debug_assert!(self.available_inbound_capacity());
        debug_assert!(self.inbound_handle(packet_identifier).is_none());

        self.inbound_publishes
            .push((packet_identifier, state))
            .unwrap();
    }

    /// Returns the amount of currently in-flight outgoing publications.
    #[must_use]
    pub fn active_outbound_publishes(&self) -> u16 {
        debug_assert!(u16::try_from(self.outbound_publishes.len()).is_ok());

        self.outbound_publishes.len() as u16
    }
    pub fn available_outbound_capacity(&self) -> bool {
        let capacity = min(self.outbound_publishes.capacity(), usize::from(u16::MAX)) as u16;

        capacity > self.active_outbound_publishes()
    }

    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet.
    /// for an outgoing/client publication. Assumes the packet identifier has no
    /// entry currently.
    ///
    /// # Safety
    /// `self.pending_client_publishes` has free capacity.
    pub(crate) fn schedule_outbound(
        &mut self,
        packet_identifier: PacketIdentifier,
        state: LocalPublishState,
    ) {
        debug_assert!(self.available_outbound_capacity());
        debug_assert!(self.outbound_handle(packet_identifier).is_none());

        self.outbound_publishes
            .push((packet_identifier, state))
            .unwrap();
    }

    pub(crate) fn remove_inbound_publish(&mut self, packet_identifier: PacketIdentifier) {
        self.inbound_handle(packet_identifier)
            .map(InboundHandle::remove);
    }
    pub(crate) fn remove_outbound_publish(&mut self, packet_identifier: PacketIdentifier) {
        self.outbound_handle(packet_identifier)
            .map(OutboundHandle::remove);
    }

    pub fn clear(&mut self) {
        self.inbound_publishes.clear();
        self.outbound_publishes.clear();
    }
}

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn reconnect(&mut self) {
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
        manual_ack: bool,
    ) -> (Response, Event) {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => (Response::None, Event::Publish),
            IdentifiedQoS::AtLeastOnce(pid) | IdentifiedQoS::ExactlyOnce(pid) => self
                .inbound_handle(pid)
                .map(|mut h| h.inbound_publish(identified_qos.into()))
                .unwrap_or_else(|| {
                    if self.available_inbound_capacity() {
                        match identified_qos {
                            IdentifiedQoS::AtMostOnce => unreachable!(),
                            IdentifiedQoS::AtLeastOnce(_) if manual_ack => {
                                self.schedule_inbound(pid, PeerPublishState::DueAck);
                                (Response::None, Event::Publish)
                            }
                            IdentifiedQoS::AtLeastOnce(_) => {
                                (Response::Acknowledge(ReasonCode::Success), Event::Publish)
                            }
                            IdentifiedQoS::ExactlyOnce(_) if manual_ack => {
                                self.schedule_inbound(pid, PeerPublishState::DueRec);
                                (Response::None, Event::Publish)
                            }
                            IdentifiedQoS::ExactlyOnce(_) => {
                                self.schedule_inbound(pid, PeerPublishState::AwaitRel(false));
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

    /// The PUBACK's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_puback(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|h| h.outbound_puback())
            .unwrap_or(Err(StateError::UnusedPacketIdentifier))
    }

    /// The PUBREC's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        self.inbound_handle(packet_identifier)
            .map(|mut h| h.outbound_pubrec())
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

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn outbound_publish(
        &mut self,
        identified_qos: IdentifiedQoS,
        manual: bool,
    ) -> Result<(), StateError> {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => Ok(()),
            IdentifiedQoS::AtLeastOnce(pid) | IdentifiedQoS::ExactlyOnce(pid) => self
                .outbound_handle(pid)
                .map(|mut h| h.outbound_publish(identified_qos.into(), manual))
                .unwrap_or_else(|| {
                    if self.available_outbound_capacity() {
                        match identified_qos {
                            IdentifiedQoS::AtMostOnce => unreachable!(),
                            IdentifiedQoS::AtLeastOnce(_) => {
                                self.schedule_outbound(pid, LocalPublishState::AwaitAck)
                            }
                            IdentifiedQoS::ExactlyOnce(_) => {
                                self.schedule_outbound(pid, LocalPublishState::AwaitRec(manual))
                            }
                        }
                        Ok(())
                    } else {
                        Err(StateError::NoCapacity)
                    }
                }),
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
        session::{Event, Response, Session, state_machine::StateError},
        types::{IdentifiedQoS, PacketIdentifier, ReasonCode},
    };

    macro_rules! sm_test {
        (
            $test_name:ident,
            [ $($steps:tt)* ]
        ) => {
            #[test]
            fn $test_name() {
                let mut sm = crate::session::Session::<10, 10>::default();
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
            Err($crate::session::state_machine::StateError::$variant)
        };
        (@expected, res, ($response:ident $( ($reason_code:ident) )?, $event:ident)) => {
            (
                $crate::session::state_machine::Response:: $response $( ($crate::types::ReasonCode:: $reason_code ) )?,
                $crate::session::state_machine::Event:: $event
            )
        };

        (@dispatch, $sm:ident, $pid:ident, in_pub(AtMostOnce, $manual:expr)) => {
            $sm.inbound_publish($crate::types::IdentifiedQoS::AtMostOnce, $manual)
        };
        (@dispatch, $sm:ident, $pid:ident, in_pub($qos:ident, $manual:expr)) => {
            $sm.inbound_publish($crate::types::IdentifiedQoS::$qos($pid), $manual)
        };
        (@dispatch, $sm:ident, $pid:ident, out_ack()) => {
            $sm.outbound_puback($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, out_rec()) => {
            $sm.outbound_pubrec($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, in_rel($rc:ident)) => {
            $sm.inbound_pubrel($pid, $crate::types::ReasonCode::$rc)
        };
        (@dispatch, $sm:ident, $pid:ident, out_comp()) => {
            $sm.outbound_pubcomp($pid)
        };
        (@dispatch, $sm:ident, $pid:ident, out_pub(AtMostOnce, $manual:expr)) => {
            $sm.outbound_publish($crate::types::IdentifiedQoS::AtMostOnce, $manual)
        };
        (@dispatch, $sm:ident, $pid:ident, out_pub($qos:ident, $manual:expr)) => {
            $sm.outbound_publish($crate::types::IdentifiedQoS::$qos($pid), $manual)
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
            out_rec() => err(UnusedPacketIdentifier),
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
        let mut sm = Session::<10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), true);

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
            let r = sm.outbound_pubrec(pid);
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
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), true);
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
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(pid), false);
            assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }

        assert!(sm.inbound_publishes.is_empty());
    }

    sm_test!(
        inbound_qos1_auto_full_macro,
        [
            in_pub(AtLeastOnce, false) => res(Acknowledge(Success), Publish),
            out_ack() => err(UnusedPacketIdentifier),
            out_rec() => err(UnusedPacketIdentifier),
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
            in_pub(AtLeastOnce, false) => res(Acknowledge(Success), Publish),

            in_pub(AtLeastOnce, true) => res(None, Publish),
            reconnect(),
            in_pub(AtLeastOnce, false) => res(None, Publish),

            out_ack() => ok(),

            out_ack() => err(UnusedPacketIdentifier),
            out_rec() => err(UnusedPacketIdentifier),
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
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 1 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Acknowledge(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // A republish with manual set to false should use the old manual setting
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.outbound_publishes.is_empty());
        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 1 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_pubrec(PID);
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
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_pubrec(PID);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_auto() {
        let mut sm = Session::<10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);

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
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Duplicate);
        }

        // Invalid client actions shouldn't be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        // Complete the QoS 2 publication
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::Success));
            assert_eq!(e, Event::Released);
        }

        // Packet identifiers are not in the session anymore
        assert!(sm.inbound_publishes.is_empty());
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_manual() {
        let mut sm = Session::<10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);

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
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Duplicate);
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
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Ok(()));
        }

        // Now sending a PUBREC shouldn't be allowed either
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
        }

        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Released);
        }

        // Invalid client actions should not be allowed
        for pid in pids.iter().copied() {
            let r = sm.outbound_puback(pid);
            assert_eq!(r, Err(StateError::MismatchedQoS));
            let r = sm.outbound_pubrec(pid);
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
            let r = sm.outbound_pubrec(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubcomp(pid);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));

            let (r, e) = sm.inbound_pubrel(pid, ReasonCode::Success);
            assert_eq!(r, Response::Complete(ReasonCode::PacketIdentifierNotFound));
            assert_eq!(e, Event::Ignored);
        }

        // PIDs are not in use anymore and should be treated as new messages
        for pid in pids.iter().copied() {
            let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);
            assert_eq!(r, Response::Receive(ReasonCode::Success));
            assert_eq!(e, Event::Publish);
        }
    }

    #[test_log::test]
    #[test]
    fn inbound_qos2_auto_full() {
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 2 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Response::Receive(ReasonCode::Success));
        assert_eq!(e, Event::Publish);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Response::Receive(ReasonCode::Success));
        assert_eq!(e, Event::Duplicate);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::Complete(ReasonCode::Success));
        assert_eq!(e, Event::Released);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        // Receive the QoS 2 PUBLISH
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Duplicate);

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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let r = sm.outbound_pubrec(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish is allowed and should lead to duplicate delivery
        sm.reconnect();
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Duplicate);

        assert!(sm.outbound_publishes.is_empty());

        // Proceed to the next handshake state
        let r = sm.outbound_pubrec(PID);
        assert_eq!(r, Ok(()));
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Released);

        assert!(sm.outbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::MismatchedQoS));
        let r = sm.outbound_pubrec(PID);
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

        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::AtLeastOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Republish should not be allowed now
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);
        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Response::Disconnect(ReasonCode::ProtocolError));
        assert_eq!(e, Event::ServerError);

        assert!(sm.outbound_publishes.is_empty());

        // Duplicate PUBREL should be allowed
        sm.reconnect();
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::Success);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Released);

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
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Publish);

        let r = sm.outbound_pubrec(PID);
        assert_eq!(r, Ok(()));

        // Handle this relatively lax by removing the state
        let (r, e) = sm.inbound_pubrel(PID, ReasonCode::PacketIdentifierNotFound);
        assert_eq!(r, Response::None);
        assert_eq!(e, Event::Aborted);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());

        let (r, e) = sm.inbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
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
        fn helper(manual: bool) {
            let mut sm = Session::<10, 10>::default();

            let mut pid = PacketIdentifier::ONE;
            let mut pids = Vec::new();

            loop {
                let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(pid), manual);

                if let Err(e) = r {
                    assert_eq!(e, StateError::NoCapacity);
                } else {
                    pids.push(pid);
                }

                pid = pid.next();
                if pid == PacketIdentifier::ONE {
                    break;
                }
            }

            // Republish should be allowed
            sm.reconnect();
            for pid in pids.iter().copied() {
                let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(pid), manual);
                assert_eq!(r, Ok(()));
            }

            // Republish with other manual setting should be allowed
            sm.reconnect();
            for pid in pids.iter().copied() {
                let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(pid), !manual);
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

        // Outbound At Most Once should work identically whether manual is set or not as PUBACK is only received
        helper(true);
        helper(false);
    }

    #[test_log::test]
    #[test]
    fn outbound_qos1_full() {
        fn helper(manual: bool) {
            let mut sm = Session::<10, 10>::default();

            const PID: PacketIdentifier = PacketIdentifier::ONE;

            let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(PID), manual);
            assert_eq!(r, Ok(()));

            assert!(sm.inbound_publishes.is_empty());

            // Invalid client & server actions should not be allowed
            let r = sm.outbound_puback(PID);
            assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
            let r = sm.outbound_pubrec(PID);
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
            assert_eq!(e, Event::Acknowledged);

            assert!(sm.inbound_publishes.is_empty());
            assert!(sm.outbound_publishes.is_empty());
        }

        helper(false);
        helper(true);
    }

    #[test_log::test]
    #[test]
    fn outbound_qos1_error_reject() {
        fn helper(manual: bool) {
            let mut sm = Session::<10, 10>::default();

            const PID: PacketIdentifier = PacketIdentifier::ONE;

            let r = sm.outbound_publish(IdentifiedQoS::AtLeastOnce(PID), manual);
            assert_eq!(r, Ok(()));

            assert!(sm.inbound_publishes.is_empty());

            // Reject the publication
            let (r, e) = sm.inbound_puback(PID, ReasonCode::TopicNameInvalid);
            assert_eq!(r, Response::None);
            assert_eq!(e, Event::Rejected);

            assert!(sm.inbound_publishes.is_empty());
            assert!(sm.outbound_publishes.is_empty());
        }

        helper(false);
        helper(true);
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_auto() {
        let mut sm = Session::<10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);

            if let Err(e) = r {
                assert_eq!(e, StateError::NoCapacity);
            } else {
                pids.push(pid);
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Republish should be allowed
        sm.reconnect();
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);
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
            assert_eq!(e, Event::Received);
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
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), false);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
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
        let mut sm = Session::<10, 10>::default();

        let mut pid = PacketIdentifier::ONE;
        let mut pids = Vec::new();

        loop {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);

            if let Err(e) = r {
                assert_eq!(e, StateError::NoCapacity);
            } else {
                pids.push(pid);
            }

            pid = pid.next();
            if pid == PacketIdentifier::ONE {
                break;
            }
        }

        // Republish should be allowed
        sm.reconnect();
        for pid in pids.iter().copied() {
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);
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
            assert_eq!(e, Event::Received);
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
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);
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
            let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(pid), true);
            assert_eq!(r, Err(StateError::MismatchedHandshakeState));
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
    fn outbound_qos2_auto_full() {
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        assert_eq!(e, Event::Received);

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
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
        let r = sm.outbound_pubrec(PID);
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
        assert_eq!(e, Event::Completed);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[test_log::test]
    #[test]
    fn outbound_qos2_manual_full() {
        let mut sm = Session::<10, 10>::default();

        const PID: PacketIdentifier = PacketIdentifier::ONE;

        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), true);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        assert_eq!(e, Event::Received);

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        let r = sm.outbound_publish(IdentifiedQoS::ExactlyOnce(PID), false);
        assert_eq!(r, Err(StateError::MismatchedHandshakeState));

        assert!(sm.inbound_publishes.is_empty());

        // Rerelease should be allowed
        let r = sm.outbound_pubrel(PID);
        assert_eq!(r, Ok(()));

        assert!(sm.inbound_publishes.is_empty());

        // Invalid client & server actions should not be allowed
        let r = sm.outbound_puback(PID);
        assert_eq!(r, Err(StateError::UnusedPacketIdentifier));
        let r = sm.outbound_pubrec(PID);
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
        assert_eq!(e, Event::Completed);

        assert!(sm.inbound_publishes.is_empty());
        assert!(sm.outbound_publishes.is_empty());
    }

    #[ignore]
    #[test_log::test]
    #[test]
    fn inbound_outbound_pid_collision_allowed() {}
}

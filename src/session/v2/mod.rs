//! Contains utilities for session management.

use core::cmp::min;
use heapless::LinearMap;

use crate::{
    session::{
        state::{LocalPublishState, PeerPublishState},
        state_machine::{Event, Response, StateError},
    },
    types::{IdentifiedQoS, PacketIdentifier, ReasonCode},
};

pub mod state;
pub(crate) mod state_machine;
// mod v1;

/// Session-associated information
///
/// Client identifier is not stored here as it would lead to inconsistencies with the underyling allocation system.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Session<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    /// The currently in-flight incoming publications.
    pub inbound_publishes: LinearMap<PacketIdentifier, PeerPublishState, RECEIVE_MAXIMUM>,
    /// The currently in-flight outgoing publications.
    pub outbound_publishes: LinearMap<PacketIdentifier, LocalPublishState, SEND_MAXIMUM>,
}

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    /// Returns the state of the publication of the packet identifier if the packet identifier is in-flight in an incoming publication.
    #[must_use]
    pub fn server_publish_state(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> Option<PeerPublishState> {
        self.inbound_publishes.get(&packet_identifier).copied()
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

        let r = self.inbound_publishes.insert(packet_identifier, state);

        debug_assert!(r.is_ok_and(|o| o.is_none()));
    }

    pub(crate) fn remove_inbound_publish(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<PeerPublishState> {
        self.inbound_publishes.remove(&packet_identifier)
    }

    /// Returns the state of the publication of the packet identifier if the packet identifier is in-flight in an outgoing publication.
    #[must_use]
    pub fn client_publish_state(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> Option<LocalPublishState> {
        self.outbound_publishes.get(&packet_identifier).copied()
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

        let r = self.outbound_publishes.insert(packet_identifier, state);

        debug_assert!(r.is_ok_and(|o| o.is_none()));
    }

    pub(crate) fn remove_outbound_publish(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<LocalPublishState> {
        self.outbound_publishes.remove(&packet_identifier)
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
            IdentifiedQoS::AtLeastOnce(pid) => {
                if let Some(s) = self.remove_inbound_publish(pid) {
                    match s {
                        PeerPublishState::DueAck
                        | PeerPublishState::AwaitPublishExactlyOnce(_)
                        | PeerPublishState::DueRec
                        | PeerPublishState::AwaitRel(_)
                        | PeerPublishState::DueComp => {
                            self.schedule_inbound(pid, s);

                            (
                                Response::Disconnect(ReasonCode::ProtocolError),
                                Event::ServerError,
                            )
                        }
                        PeerPublishState::AwaitPublishAtLeastOnce => {
                            self.schedule_inbound(pid, PeerPublishState::DueAck);

                            (Response::None, Event::Publish)
                        }
                    }
                } else if self.available_inbound_capacity() {
                    if manual_ack {
                        self.schedule_inbound(pid, PeerPublishState::DueAck);
                        (Response::None, Event::Publish)
                    } else {
                        (Response::Acknowledge(ReasonCode::Success), Event::Publish)
                    }
                } else {
                    (
                        Response::Disconnect(ReasonCode::QuotaExceeded),
                        Event::ServerError,
                    )
                }
            }
            IdentifiedQoS::ExactlyOnce(pid) => {
                if let Some(s) = self.remove_inbound_publish(pid) {
                    match s {
                        PeerPublishState::AwaitPublishAtLeastOnce
                        | PeerPublishState::DueAck
                        | PeerPublishState::DueRec
                        | PeerPublishState::AwaitRel(_)
                        | PeerPublishState::DueComp => {
                            self.schedule_inbound(pid, s);

                            (
                                Response::Disconnect(ReasonCode::ProtocolError),
                                Event::ServerError,
                            )
                        }
                        PeerPublishState::AwaitPublishExactlyOnce(manual) if manual => {
                            self.schedule_inbound(pid, PeerPublishState::DueRec);

                            (Response::None, Event::Duplicate)
                        }
                        PeerPublishState::AwaitPublishExactlyOnce(_) => {
                            self.schedule_inbound(pid, PeerPublishState::AwaitRel(false));

                            (Response::Receive(ReasonCode::Success), Event::Duplicate)
                        }
                    }
                } else if self.available_inbound_capacity() {
                    if manual_ack {
                        self.schedule_inbound(pid, PeerPublishState::DueRec);

                        return (Response::None, Event::Publish);
                    } else {
                        self.schedule_inbound(pid, PeerPublishState::AwaitRel(false));

                        return (Response::Receive(ReasonCode::Success), Event::Publish);
                    }
                } else {
                    (
                        Response::Disconnect(ReasonCode::QuotaExceeded),
                        Event::ServerError,
                    )
                }
            }
        }
    }

    /// The PUBACK's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_puback(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        let Some(s) = self.remove_inbound_publish(packet_identifier) else {
            return Err(StateError::UnusedPacketIdentifier);
        };

        match s {
            PeerPublishState::AwaitPublishAtLeastOnce => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedHandshakeState)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::DueComp => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::DueAck => Ok(()),
        }
    }

    /// The PUBREC's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        let Some(s) = self.remove_inbound_publish(packet_identifier) else {
            return Err(StateError::UnusedPacketIdentifier);
        };

        match s {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::DueComp => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedHandshakeState)
            }
            PeerPublishState::DueRec => {
                self.schedule_inbound(packet_identifier, PeerPublishState::AwaitRel(true));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubrel(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        let Some(s) = self.remove_inbound_publish(packet_identifier) else {
            // The reason code in this case can only be PacketIdentifierNotFound
            return if reason_code.is_erroneous() {
                // We didn't find the PID, server hasn't found it
                // -> treat it as matching session state.
                (Response::None, Event::Ignored)
            } else {
                (
                    Response::Complete(ReasonCode::PacketIdentifierNotFound),
                    Event::Ignored,
                )
            };
        };

        match s {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                // QoS mismatch -> the spec doesn't state what to do here nor
                // is there a fitting reason code we could use in a PUBCOMP
                self.schedule_inbound(packet_identifier, s);
                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            PeerPublishState::DueRec | PeerPublishState::DueComp => {
                // Handshake state mismatch -> the spec doesn't state what to do here
                // nor is there a fitting reason code we could use in a PUBCOMP
                self.schedule_inbound(packet_identifier, s);
                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            PeerPublishState::AwaitPublishExactlyOnce(manual)
            | PeerPublishState::AwaitRel(manual) => {
                // The reason code in this case can only be PacketIdentifierNotFound
                if reason_code.is_erroneous() {
                    // The server hasn't found the PID of a PUBREC packet of ours.
                    // This means it doesn't track its original PUBLISH anymore.
                    // -> remove this session state
                    (Response::None, Event::Aborted)
                } else if manual {
                    self.schedule_inbound(packet_identifier, PeerPublishState::DueComp);
                    (Response::None, Event::Released)
                } else {
                    (Response::Complete(ReasonCode::Success), Event::Released)
                }
            }
        }
    }

    pub(crate) fn outbound_pubcomp(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        let Some(s) = self.remove_inbound_publish(packet_identifier) else {
            return Err(StateError::UnusedPacketIdentifier);
        };

        match s {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_) => {
                self.schedule_inbound(packet_identifier, s);
                Err(StateError::MismatchedHandshakeState)
            }
            PeerPublishState::DueComp => Ok(()),
        }
    }
}

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn outbound_publish(
        &mut self,
        identified_qos: IdentifiedQoS,
        manual_ack: bool,
    ) -> Result<(), StateError> {
        match identified_qos {
            IdentifiedQoS::AtMostOnce => Ok(()),
            IdentifiedQoS::AtLeastOnce(pid) => {
                // Check whether this is a republish or a new publication
                if let Some(s) = self.client_publish_state(pid) {
                    match s {
                        LocalPublishState::DuePublishAtLeastOnce => {
                            self.remove_outbound_publish(pid);
                            self.schedule_outbound(pid, LocalPublishState::AwaitAck);
                            Ok(())
                        }
                        LocalPublishState::AwaitAck => Err(StateError::MismatchedHandshakeState),
                        LocalPublishState::DuePublishExactlyOnce(_)
                        | LocalPublishState::AwaitRec(_)
                        | LocalPublishState::DueRel(_)
                        | LocalPublishState::AwaitComp(_) => Err(StateError::MismatchedQoS),
                    }
                } else if self.available_outbound_capacity() {
                    self.schedule_outbound(pid, LocalPublishState::AwaitAck);
                    Ok(())
                } else {
                    Err(StateError::NoCapacity)
                }
            }
            IdentifiedQoS::ExactlyOnce(pid) => {
                // Check whether this is a republish or a new publication
                if let Some(s) = self.client_publish_state(pid) {
                    match s {
                        LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                            Err(StateError::MismatchedQoS)
                        }
                        LocalPublishState::AwaitRec(_)
                        | LocalPublishState::DueRel(_)
                        | LocalPublishState::AwaitComp(_) => {
                            Err(StateError::MismatchedHandshakeState)
                        }
                        LocalPublishState::DuePublishExactlyOnce(manual) => {
                            self.remove_outbound_publish(pid);
                            self.schedule_outbound(pid, LocalPublishState::AwaitRec(manual));
                            Ok(())
                        }
                    }
                } else if self.available_outbound_capacity() {
                    self.schedule_outbound(pid, LocalPublishState::AwaitRec(manual_ack));
                    Ok(())
                } else {
                    Err(StateError::NoCapacity)
                }
            }
        }
    }

    pub(crate) fn inbound_puback(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        let Some(s) = self.remove_outbound_publish(packet_identifier) else {
            return (Response::None, Event::Ignored);
        };

        match s {
            LocalPublishState::AwaitAck => {
                let e = if reason_code.is_success() {
                    Event::Acknowledged
                } else {
                    Event::Rejected
                };
                (Response::None, e)
            }
            LocalPublishState::DuePublishAtLeastOnce
            | LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::DueRel(_)
            | LocalPublishState::AwaitComp(_) => {
                self.schedule_outbound(packet_identifier, s);
                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
        }
    }

    pub(crate) fn inbound_pubrec(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        let Some(s) = self.remove_outbound_publish(packet_identifier) else {
            return (
                Response::Release(ReasonCode::PacketIdentifierNotFound),
                Event::Ignored,
            );
        };

        match s {
            LocalPublishState::DuePublishAtLeastOnce
            | LocalPublishState::AwaitAck
            | LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::DueRel(_)
            | LocalPublishState::AwaitComp(_) => {
                self.schedule_outbound(packet_identifier, s);

                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            LocalPublishState::AwaitRec(manual) => {
                if reason_code.is_success() {
                    let r = if manual {
                        self.schedule_outbound(packet_identifier, LocalPublishState::DueRel(true));
                        Response::None
                    } else {
                        self.schedule_outbound(
                            packet_identifier,
                            LocalPublishState::AwaitComp(false),
                        );
                        Response::Release(ReasonCode::Success)
                    };
                    (r, Event::Received)
                } else {
                    (Response::None, Event::Rejected)
                }
            }
        }
    }

    pub(crate) fn outbound_pubrel(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Result<(), StateError> {
        let Some(s) = self.remove_outbound_publish(packet_identifier) else {
            return Err(StateError::UnusedPacketIdentifier);
        };

        match s {
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                self.schedule_outbound(packet_identifier, s);
                Err(StateError::MismatchedQoS)
            }
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::AwaitComp(_) => {
                self.schedule_outbound(packet_identifier, s);
                Err(StateError::MismatchedHandshakeState)
            }
            LocalPublishState::DueRel(manual) => {
                self.schedule_outbound(packet_identifier, LocalPublishState::AwaitComp(manual));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubcomp(
        &mut self,
        packet_identifier: PacketIdentifier,
        reason_code: ReasonCode,
    ) -> (Response, Event) {
        let Some(s) = self.remove_outbound_publish(packet_identifier) else {
            return (Response::None, Event::Ignored);
        };

        match s {
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                self.schedule_outbound(packet_identifier, s);
                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::DueRel(_) => {
                self.schedule_outbound(packet_identifier, s);

                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            LocalPublishState::AwaitComp(_) => {
                if reason_code.is_success() {
                    (Response::None, Event::Completed)
                } else {
                    // TODO this mirrors the previous behaviour, but perhaps we should be trying to fix the state
                    (Response::None, Event::Rejected)
                }
            }
        }
    }
}

#[cfg(test)]
mod unit {
    use std::vec::Vec;

    use heapless::LinearMap;

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
            in_rec(Success) => res(Release(PacketIdentifierNotFound), Ignored),             // This is matching state, server doesn't know this PID and neither do we
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
        let mut sm = Session::<10, 10> {
            inbound_publishes: LinearMap::new(),
            outbound_publishes: LinearMap::new(),
        };

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

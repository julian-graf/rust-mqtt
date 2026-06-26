use crate::{
    session::{
        Session,
        state::{LocalPublishState, PeerPublishState},
        state_machine::{Event, Response, StateError},
    },
    types::{PacketIdentifier, QoS, ReasonCode},
};

pub struct FreeHandle<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    pub session: &'a mut Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub packet_identifier: PacketIdentifier,
}

impl<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    FreeHandle<'a, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub fn outbound_publish(self, qos: QoS, manual: bool) -> Result<(), StateError> {
        assert_ne!(qos, QoS::AtMostOnce);

        let initial_state = match qos {
            QoS::AtMostOnce => panic!(),
            QoS::AtLeastOnce if manual => panic!(),
            QoS::AtLeastOnce => LocalPublishState::AwaitAck,
            QoS::ExactlyOnce => LocalPublishState::AwaitRec(manual),
        };

        self.session
            .outbound_publishes
            .push((self.packet_identifier, initial_state))
            .map_err(|_| StateError::NoCapacity)
    }
}

pub struct InboundHandle<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    pub session: &'a mut Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
    pub packet_identifier: PacketIdentifier,
    pub state: PeerPublishState,
}
pub struct OutboundHandle<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    pub session: &'a mut Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
    pub packet_identifier: PacketIdentifier,
    pub state: LocalPublishState,
}

impl<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    InboundHandle<'a, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn set(&mut self, state: PeerPublishState) {
        self.state = state;
        self.session.inbound_publishes.get_mut(self.i).unwrap().1 = self.state;
    }
    pub(crate) fn remove(self) {
        self.session.inbound_publishes.swap_remove(self.i);
    }

    pub(crate) fn inbound_publish(&mut self, qos: QoS) -> (Response, Event) {
        match qos {
            QoS::AtMostOnce => panic!("QoS 1 has no packet identifier, so this call is incorrect"),
            QoS::AtLeastOnce => match self.state {
                PeerPublishState::DueAck
                | PeerPublishState::AwaitPublishExactlyOnce(_)
                | PeerPublishState::DueRec
                | PeerPublishState::AwaitRel(_)
                | PeerPublishState::DueComp => (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                ),
                PeerPublishState::AwaitPublishAtLeastOnce => {
                    self.set(PeerPublishState::DueAck);

                    (Response::None, Event::Publish)
                }
            },
            QoS::ExactlyOnce => match self.state {
                PeerPublishState::AwaitPublishAtLeastOnce
                | PeerPublishState::DueAck
                | PeerPublishState::DueRec
                | PeerPublishState::AwaitRel(_)
                | PeerPublishState::DueComp => (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                ),
                PeerPublishState::AwaitPublishExactlyOnce(manual) if manual => {
                    self.set(PeerPublishState::DueRec);

                    (Response::None, Event::Duplicate)
                }
                PeerPublishState::AwaitPublishExactlyOnce(_) => {
                    self.set(PeerPublishState::AwaitRel(false));

                    (Response::Receive(ReasonCode::Success), Event::Duplicate)
                }
            },
        }
    }

    /// The PUBACK's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_puback(self) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce => Err(StateError::MismatchedHandshakeState),
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::DueComp => Err(StateError::MismatchedQoS),
            PeerPublishState::DueAck => {
                self.remove();
                Ok(())
            }
        }
    }

    /// The PUBREC's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_pubrec(&mut self) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::DueComp => Err(StateError::MismatchedHandshakeState),
            PeerPublishState::DueRec => {
                self.set(PeerPublishState::AwaitRel(true));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubrel(mut self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                // QoS mismatch -> the spec doesn't state what to do here nor
                // is there a fitting reason code we could use in a PUBCOMP
                (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                )
            }
            PeerPublishState::DueRec | PeerPublishState::DueComp => {
                // Handshake state mismatch -> the spec doesn't state what to do here
                // nor is there a fitting reason code we could use in a PUBCOMP
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
                    self.remove();

                    (Response::None, Event::Aborted)
                } else if manual {
                    self.set(PeerPublishState::DueComp);
                    (Response::None, Event::Released)
                } else {
                    self.remove();
                    (Response::Complete(ReasonCode::Success), Event::Released)
                }
            }
        }
    }

    /// The PUBCOMP's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_pubcomp(self) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_) => Err(StateError::MismatchedHandshakeState),
            PeerPublishState::DueComp => {
                self.remove();
                Ok(())
            }
        }
    }
}

impl<'a, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    OutboundHandle<'a, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn set(&mut self, state: LocalPublishState) {
        self.state = state;
        self.session.outbound_publishes.get_mut(self.i).unwrap().1 = self.state;
    }
    pub(crate) fn remove(self) {
        self.session.outbound_publishes.swap_remove(self.i);
    }
    pub(crate) fn next(self) -> Option<Self> {
        let i = self.i + 1;

        self.session
            .outbound_publishes
            .get(i)
            .map(|(p, s)| (*p, *s))
            .map(|(packet_identifier, state)| Self {
                session: self.session,
                i,
                packet_identifier,
                state,
            })
    }

    pub(crate) fn packet_identifier(&self) -> PacketIdentifier {
        self.packet_identifier
    }
    pub(crate) fn state(&self) -> LocalPublishState {
        self.state
    }

    pub(crate) fn outbound_publish(
        &mut self,
        qos: QoS,
        manual_ack: bool,
    ) -> Result<(), StateError> {
        match qos {
            QoS::AtMostOnce => {
                panic!("QoS 1 has no packet identifier, so this call is incorrect")
            }
            QoS::AtLeastOnce => match self.state {
                LocalPublishState::DuePublishAtLeastOnce => {
                    self.set(LocalPublishState::AwaitAck);
                    Ok(())
                }
                LocalPublishState::AwaitAck => Err(StateError::MismatchedHandshakeState),
                LocalPublishState::DuePublishExactlyOnce(_)
                | LocalPublishState::AwaitRec(_)
                | LocalPublishState::DueRel(_)
                | LocalPublishState::AwaitComp(_) => Err(StateError::MismatchedQoS),
            },
            QoS::ExactlyOnce => match self.state {
                LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                    Err(StateError::MismatchedQoS)
                }
                LocalPublishState::AwaitRec(_)
                | LocalPublishState::DueRel(_)
                | LocalPublishState::AwaitComp(_) => Err(StateError::MismatchedHandshakeState),
                LocalPublishState::DuePublishExactlyOnce(manual) => {
                    self.set(LocalPublishState::AwaitRec(manual));
                    Ok(())
                }
            },
        }
    }

    pub(crate) fn inbound_puback(self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            LocalPublishState::AwaitAck => {
                self.remove();

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
            | LocalPublishState::AwaitComp(_) => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),
        }
    }

    pub(crate) fn inbound_pubrec(mut self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            LocalPublishState::DuePublishAtLeastOnce
            | LocalPublishState::AwaitAck
            | LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::DueRel(_)
            | LocalPublishState::AwaitComp(_) => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),
            LocalPublishState::AwaitRec(manual) => {
                if reason_code.is_success() {
                    let r = if manual {
                        self.set(LocalPublishState::DueRel(true));
                        Response::None
                    } else {
                        self.set(LocalPublishState::AwaitComp(false));
                        Response::Release(ReasonCode::Success)
                    };
                    (r, Event::Received)
                } else {
                    self.remove();
                    (Response::None, Event::Rejected)
                }
            }
        }
    }

    pub(crate) fn outbound_pubrel(&mut self) -> Result<(), StateError> {
        match self.state {
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                Err(StateError::MismatchedQoS)
            }
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::AwaitComp(_) => Err(StateError::MismatchedHandshakeState),
            LocalPublishState::DueRel(manual) => {
                self.set(LocalPublishState::AwaitComp(manual));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubcomp(self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::DueRel(_) => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),
            LocalPublishState::AwaitComp(_) => {
                self.remove();
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

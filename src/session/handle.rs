use crate::{
    client::options::AckMode,
    session::{Event, LocalPublishState, PeerPublishState, Response, Session, StateError},
    types::{PacketIdentifier, QoS, ReasonCode},
};

pub struct FreeHandle<
    'a,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    pub session: &'a mut Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub packet_identifier: PacketIdentifier,
}

impl<'a, const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    FreeHandle<'a, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub fn outbound_sub(self) -> Result<(), StateError> {
        self.session
            .subs
            .push(self.packet_identifier)
            .map_err(|_| StateError::NoCapacity)
    }
    pub fn outbound_unsub(self) -> Result<(), StateError> {
        self.session
            .unsubs
            .push(self.packet_identifier)
            .map_err(|_| StateError::NoCapacity)
    }
    pub fn outbound_publish(self, qos: QoS, ack_mode: AckMode) -> Result<(), StateError> {
        assert_ne!(qos, QoS::AtMostOnce, "QoS 0 is not to be tracked");
        assert!(
            !(qos == QoS::AtLeastOnce && ack_mode.is_manual()),
            "QoS 1 is not to be tracked"
        );

        if self.session.available_outbound_publish_capacity() {
            let initial_state = match qos {
                QoS::AtMostOnce => unreachable!(),
                QoS::AtLeastOnce if ack_mode.is_manual() => unreachable!(),
                QoS::AtLeastOnce => LocalPublishState::AwaitAck,
                QoS::ExactlyOnce => LocalPublishState::AwaitRec(ack_mode),
            };

            self.session
                .schedule_outbound(self.packet_identifier, initial_state);

            Ok(())
        } else {
            Err(StateError::NoCapacity)
        }
    }
}

pub struct SubHandle<
    'a,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    pub session: &'a mut Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
}

impl<'a, const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    SubHandle<'a, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn remove(self) {
        self.session.subs.swap_remove(self.i);
    }
}

pub struct UnsubHandle<
    'a,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    pub session: &'a mut Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
}

impl<'a, const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    UnsubHandle<'a, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    pub(crate) fn remove(self) {
        self.session.unsubs.swap_remove(self.i);
    }
}

pub struct InboundHandle<
    'a,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    pub session: &'a mut Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
    pub state: PeerPublishState,
}

impl<'a, const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    InboundHandle<'a, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    fn set(&mut self, state: PeerPublishState) {
        self.state = state;
        self.session.inbound_publishes.get_mut(self.i).unwrap().1 = self.state;
    }
    fn remove(self) {
        self.session.inbound_publishes.swap_remove(self.i);
    }

    pub(crate) fn inbound_republish(&mut self, qos: QoS) -> (Response, Event) {
        match qos {
            QoS::AtMostOnce => panic!("QoS 0 has no packet identifier, so this call is incorrect"),
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
                PeerPublishState::AwaitPublishExactlyOnce(AckMode::Manual) => {
                    self.set(PeerPublishState::DueRec);

                    (Response::None, Event::Duplicate(AckMode::Manual))
                }
                PeerPublishState::AwaitPublishExactlyOnce(AckMode::Automatic) => {
                    self.set(PeerPublishState::AwaitRel(AckMode::Automatic));

                    (
                        Response::Receive(ReasonCode::Success),
                        Event::Duplicate(AckMode::Automatic),
                    )
                }
            },
        }
    }

    /// The PUBACK's [`ReasonCode`] may be successful or erroneous, this doesn't matter
    /// for the state machine as this packet identifier is removed from the session
    /// state in either case.
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

    pub(crate) fn outbound_pubrec(mut self, reason_code: ReasonCode) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                Err(StateError::MismatchedQoS)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::DueComp => Err(StateError::MismatchedHandshakeState),
            PeerPublishState::DueRec if reason_code.is_success() => {
                self.set(PeerPublishState::AwaitRel(AckMode::Manual));
                Ok(())
            }
            PeerPublishState::DueRec => {
                self.remove();
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
            PeerPublishState::AwaitPublishExactlyOnce(mode) | PeerPublishState::AwaitRel(mode) => {
                // The reason code in this case can only be PacketIdentifierNotFound
                if reason_code.is_erroneous() {
                    // The server hasn't found the PID of a PUBREC packet of ours.
                    // This means it doesn't track its original PUBLISH anymore.
                    // -> remove this session state
                    self.remove();

                    (Response::None, Event::Aborted)
                } else if mode.is_manual() {
                    self.set(PeerPublishState::DueComp);
                    (Response::None, Event::Released(mode))
                } else {
                    self.remove();
                    (
                        Response::Complete(ReasonCode::Success),
                        Event::Released(mode),
                    )
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

pub struct OutboundHandle<
    'a,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
> {
    pub session: &'a mut Session<SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>,
    pub i: usize,
    pub packet_identifier: PacketIdentifier,
    pub state: LocalPublishState,
}

impl<'a, const SUBSCRIBE_MAXIMUM: usize, const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    OutboundHandle<'a, SUBSCRIBE_MAXIMUM, RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    fn set(&mut self, state: LocalPublishState) {
        self.state = state;
        self.session.outbound_publishes.get_mut(self.i).unwrap().1 = self.state;
    }
    fn remove(self) {
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

    pub(crate) fn outbound_republish(&mut self, qos: QoS) -> Result<(), StateError> {
        match qos {
            QoS::AtMostOnce => {
                panic!("QoS 0 has no packet identifier, so this call is incorrect")
            },
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
                LocalPublishState::DuePublishExactlyOnce(mode) => {
                    self.set(LocalPublishState::AwaitRec(mode));
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
            LocalPublishState::AwaitRec(mode) => {
                if reason_code.is_success() {
                    let r = if mode.is_manual() {
                        self.set(LocalPublishState::DueRel(AckMode::Manual));
                        Response::None
                    } else {
                        self.set(LocalPublishState::AwaitComp(AckMode::Automatic));
                        Response::Release(ReasonCode::Success)
                    };
                    (r, Event::Received(mode))
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
            LocalPublishState::DueRel(mode) => {
                self.set(LocalPublishState::AwaitComp(mode));
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

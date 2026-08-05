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
            "outbound QoS 1 does not have acknowledgements"
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

    pub(crate) fn inbound_republish(mut self, qos: QoS) -> (Response, Event) {
        match qos {
            QoS::AtMostOnce => panic!("QoS 0 has no packet identifier, so this call is incorrect"),
            QoS::AtLeastOnce => match self.state {
                // In the case of `DueComp` we have received the PUBREL already, so MQTT-4.3.3-10 does
                // not apply here:
                // | Until it has received the corresponding PUBREL packet, the receiver MUST acknowledge
                // | any subsequent PUBLISH packet with the same Packet Identifier by sending a PUBREC.
                //
                // However, we still send the same erroneous PUBREC in that case to tell the peer about
                // the mismatch.
                PeerPublishState::AwaitPublishExactlyOnce(_)
                | PeerPublishState::DueRec
                | PeerPublishState::AwaitRel(_)
                | PeerPublishState::AwaitReRel
                | PeerPublishState::DueComp => {
                    self.remove();
                    (
                        Response::Receive(ReasonCode::PacketIdentifierInUse),
                        Event::Aborted,
                    )
                }

                // The user has not yet sent the manual PUBACK so we leave it up to them.
                // We could consider emitting a duplicate event here because we haven't sent
                // the PUBACK yet undermining the condition of the following normative statement:
                // | After it has sent a PUBACK packet the receiver MUST treat any incoming PUBLISH
                // | packet that contains the same Packet Identifier as being a new Application Message,
                // | irrespective of the setting of its DUP flag [MQTT-4.3.2-5].
                //
                // However, we can still let this message through, which we do.
                PeerPublishState::DueAck => (Response::None, Event::Publish),
                PeerPublishState::AwaitPublishAtLeastOnce => {
                    self.set(PeerPublishState::DueAck);

                    (Response::None, Event::Publish)
                }
            },
            QoS::ExactlyOnce => match self.state {
                PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                    self.remove();
                    (
                        Response::Acknowledge(ReasonCode::PacketIdentifierInUse),
                        Event::Aborted,
                    )
                }

                // This state is reached after a reconnection with both possibilities of
                // - the peer resending a PUBLISH
                // - the peer (re-)sending a PUBREL
                //
                // We (the user) has not yet sent a PUBREC in this connection so it is their responsibility.
                PeerPublishState::AwaitPublishExactlyOnce(AckMode::Automatic) => {
                    self.set(PeerPublishState::AwaitRel(AckMode::Automatic));

                    (Response::Receive(ReasonCode::Success), Event::Duplicate(AckMode::Automatic))
                }
                PeerPublishState::AwaitPublishExactlyOnce(AckMode::Manual) => {
                    self.set(PeerPublishState::DueRec);

                    (Response::None, Event::Duplicate(AckMode::Manual))
                }

                // The user has not yet sent the manual PUBREC so we leave it up to them.
                // The PUBLISH has not driven the state forward, so an ignored event would
                // be fitting, but duplicate matches better here.
                PeerPublishState::DueRec => (Response::None, Event::Duplicate(AckMode::Manual)),

                // The user has already sent a PUBREC packet, so we send this PUBREC
                // automatically.
                PeerPublishState::AwaitRel(mode) => (
                    Response::Receive(ReasonCode::Success),
                    Event::Duplicate(mode),
                ),

                // The peer has already sent a PUBREL packet so it must not resend the PUBLISH
                // packet. We have not yet sent a PUBCOMP packet so this PUBLISH also can't be
                // a new application message that reuses the same packet identifier. This is a
                // clear protocol error.
                PeerPublishState::AwaitReRel | PeerPublishState::DueComp => (
                    Response::Disconnect(ReasonCode::ProtocolError),
                    Event::ServerError,
                ),
            },
        }
    }

    /// The PUBACK's [`ReasonCode`] may be successful or erroneous, this doesn't matter
    /// for the state machine as this packet identifier is removed from the session
    /// state in either case.
    pub(crate) fn outbound_puback(self) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce => Err(StateError::HandshakeStateMismatched),
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::AwaitReRel
            | PeerPublishState::DueComp => Err(StateError::QoSMismatched),
            PeerPublishState::DueAck => {
                self.remove();
                Ok(())
            }
        }
    }

    pub(crate) fn outbound_pubrec(mut self, reason_code: ReasonCode) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                Err(StateError::QoSMismatched)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::AwaitReRel
            | PeerPublishState::DueComp => Err(StateError::HandshakeStateMismatched),
            PeerPublishState::DueRec if reason_code.is_erroneous() => {
                self.remove();
                Ok(())
            }
            PeerPublishState::DueRec => {
                self.set(PeerPublishState::AwaitRel(AckMode::Manual));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubrel(mut self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            // QoS mismatch, the spec doesn't state what to do here. We could
            // - disconnect due to a protocol error (ReasonCode::PacketIdentifierInUse is not allowed for DISCONNECT packets)
            // - send a PUBCOMP, but the only allowed ReasonCode::PacketIdentifierNotFound is not fitting
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),

            PeerPublishState::AwaitPublishExactlyOnce(_) if reason_code.is_erroneous() => {
                self.remove();
                (Response::Complete(ReasonCode::Success), Event::Aborted)
            }
            PeerPublishState::AwaitPublishExactlyOnce(AckMode::Automatic) => {
                self.remove();
                (
                    Response::Complete(ReasonCode::Success),
                    Event::Released(AckMode::Automatic),
                )
            }
            PeerPublishState::AwaitPublishExactlyOnce(AckMode::Manual) => {
                self.set(PeerPublishState::DueComp);
                (Response::None, Event::Released(AckMode::Manual))
            }

            // This state means we have never sent a PUBREC and therefore the peer should
            // not have sent a PUBREL. We have two options:
            // - Accept the release and move the session state forward despite having
            //   skipped the PUBREC. After all, we already delivered the message.
            // - Disconnect due to a protocol error. This risks that the session entry on
            //   our end becomes stale (especially if the reason code of this PUBREL is
            //   negative) because the peer has removed their entry and won't resend the
            //   PUBLISH or PUBREL packet
            PeerPublishState::DueRec if reason_code.is_erroneous() => {
                self.remove();
                (Response::Complete(ReasonCode::Success), Event::Aborted)
            }
            PeerPublishState::DueRec => {
                self.set(PeerPublishState::DueComp);
                (Response::None, Event::Released(AckMode::Manual))
            }

            PeerPublishState::AwaitRel(_) if reason_code.is_erroneous() => {
                self.remove();
                (Response::Complete(ReasonCode::Success), Event::Aborted)
            }
            PeerPublishState::AwaitRel(AckMode::Automatic) => {
                self.remove();
                (
                    Response::Complete(ReasonCode::Success),
                    Event::Released(AckMode::Automatic),
                )
            }
            PeerPublishState::AwaitRel(AckMode::Manual) => {
                self.set(PeerPublishState::DueComp);
                (Response::None, Event::Released(AckMode::Manual))
            }

            PeerPublishState::AwaitReRel => {
                self.set(PeerPublishState::DueComp);
                (
                    Response::None,
                    Event::Released(AckMode::Manual),
                )
            },

            PeerPublishState::DueComp if reason_code.is_erroneous() => {
                self.remove();
                (Response::Complete(ReasonCode::Success), Event::Aborted)
            }
            PeerPublishState::DueComp => (Response::None, Event::Ignored),
        }
    }

    /// The PUBCOMP's [`ReasonCode`] is assumed to be successful.
    pub(crate) fn outbound_pubcomp(self) -> Result<(), StateError> {
        match self.state {
            PeerPublishState::AwaitPublishAtLeastOnce | PeerPublishState::DueAck => {
                Err(StateError::QoSMismatched)
            }
            PeerPublishState::AwaitPublishExactlyOnce(_)
            | PeerPublishState::DueRec
            | PeerPublishState::AwaitRel(_)
            | PeerPublishState::AwaitReRel => Err(StateError::HandshakeStateMismatched),
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
                panic!("QoS 0 has no packet identifier, so this call is incorrect");
            }
            QoS::AtLeastOnce => match self.state {
                LocalPublishState::DuePublishAtLeastOnce => {
                    self.set(LocalPublishState::AwaitAck);
                    Ok(())
                }
                LocalPublishState::AwaitAck => Err(StateError::HandshakeStateMismatched),
                LocalPublishState::DuePublishExactlyOnce(_)
                | LocalPublishState::AwaitRec(_)
                | LocalPublishState::DueRel
                | LocalPublishState::DueReRel(_)
                | LocalPublishState::AwaitComp(_) => Err(StateError::QoSMismatched),
            },
            QoS::ExactlyOnce => match self.state {
                LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                    Err(StateError::QoSMismatched)
                }
                LocalPublishState::AwaitRec(_)
                | LocalPublishState::DueRel
                | LocalPublishState::DueReRel(_)
                | LocalPublishState::AwaitComp(_) => Err(StateError::HandshakeStateMismatched),
                LocalPublishState::DuePublishExactlyOnce(mode) => {
                    self.set(LocalPublishState::AwaitRec(mode));
                    Ok(())
                }
            },
        }
    }

    pub(crate) fn inbound_puback(self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            // According to the spec, we MUST retransmit our PUBLISH packet on reconnect,
            // however, for QoS 2 we also accept a PUBREC in the counterpart of this state.
            //
            // The peer should not have sent a PUBACK on reconnect, but our priority is
            // not remote spec enforcement but reliable delivery. The receival of this
            // PUBACK proves that the peer took ownership of the message and delivered it.
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                self.remove();

                let e = if reason_code.is_success() {
                    Event::Acknowledged
                } else {
                    Event::Rejected
                };
                (Response::None, e)
            }

            // Mismatched QoS
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::DueRel
            | LocalPublishState::DueReRel(_)
            | LocalPublishState::AwaitComp(_) => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),
        }
    }

    pub(crate) fn inbound_pubrec(mut self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            // Mismatched QoS
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),

            // Ideally, this state doesn't exist when a PUBREC is received because
            // - on reconnection, all PUBLISH packets should be resent immediately
            // and
            // - the peer must not send a PUBREC packet "out of thin air" after a
            //   reconnection.
            //
            // So we act to stay as conform to the spec as possible. Relevant
            // normative statements:
            // | The sender MUST send a PUBREL packet when it receives a PUBREC packet from the receiver with a Reason Code value less than 0x80 [MQTT-4.3.3-4].
            // | The sender MUST NOT re-send the PUBLISH once it has sent the corresponding PUBREL packet [MQTT-4.3.3-6].
            // | On reconnection, the sender MUST resend any unacknowledged PUBLISH packets [MQTT-4.4.0-1].
            //
            // In our case, after having sent the mandatory PUBREL packet, we treat
            // the PUBLISH packet as acknowledged, which means we don't need to
            // (and must not) resend the PUBLISH packet, which we wouldn't be
            // allowed to anyway because we have sent the PUBREL already.
            LocalPublishState::DuePublishExactlyOnce(_) if reason_code.is_erroneous() => {
                self.remove();
                (Response::None, Event::Rejected)
            }
            LocalPublishState::DuePublishExactlyOnce(AckMode::Automatic) => {
                self.set(LocalPublishState::AwaitComp(AckMode::Automatic));
                (
                    Response::Release(ReasonCode::Success),
                    Event::Received(AckMode::Automatic),
                )
            }
            LocalPublishState::DuePublishExactlyOnce(AckMode::Manual) => {
                self.set(LocalPublishState::DueRel);
                (Response::None, Event::Received(AckMode::Manual))
            }

            // Ideally, this state doesn't exist when a PUBREC is received because
            // - the peer must not send a PUBREC packet "out of thin air" after a
            //   reconnection and must not retransmit it during a connection
            // and either of
            // - on reconnection, all PUBREL packets should be resent immediately
            // - in manual ack mode, the PUBREL packet should be sent immediately
            //   after receiving the PUBREC
            LocalPublishState::DueRel | LocalPublishState::DueReRel(_)
                if reason_code.is_erroneous() =>
            {
                self.remove();
                (Response::None, Event::Rejected)
            }
            LocalPublishState::DueReRel(AckMode::Automatic) => {
                (Response::Release(ReasonCode::Success), Event::Ignored)
            }
            LocalPublishState::DueReRel(AckMode::Manual) | LocalPublishState::DueRel => {
                // The user has not yet sent the manual PUBREL so we leave it up to them.
                // We do not emit the `Received` event because the user has already seen
                // that event and this PUBREC has not driven the state forward.
                (Response::None, Event::Ignored)
            }

            LocalPublishState::AwaitComp(_) => {
                // If the ack mode is manual, the user has already sent the PUBREL so we
                // do it automatically now.
                (Response::Release(ReasonCode::Success), Event::Ignored)
            }

            LocalPublishState::AwaitRec(_) if reason_code.is_erroneous() => {
                self.remove();
                (Response::None, Event::Rejected)
            }
            LocalPublishState::AwaitRec(AckMode::Automatic) => {
                self.set(LocalPublishState::AwaitComp(AckMode::Automatic));
                (
                    Response::Release(ReasonCode::Success),
                    Event::Received(AckMode::Automatic),
                )
            }
            LocalPublishState::AwaitRec(AckMode::Manual) => {
                self.set(LocalPublishState::DueRel);
                (Response::None, Event::Received(AckMode::Manual))
            }
        }
    }

    pub(crate) fn outbound_pubrel(&mut self) -> Result<(), StateError> {
        match self.state {
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => {
                Err(StateError::QoSMismatched)
            }
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::AwaitComp(_) => Err(StateError::HandshakeStateMismatched),
            LocalPublishState::DueRel => {
                self.set(LocalPublishState::AwaitComp(AckMode::Manual));
                Ok(())
            }
            LocalPublishState::DueReRel(mode) => {
                self.set(LocalPublishState::AwaitComp(mode));
                Ok(())
            }
        }
    }

    pub(crate) fn inbound_pubcomp(self, reason_code: ReasonCode) -> (Response, Event) {
        match self.state {
            // Mismatched QoS
            LocalPublishState::DuePublishAtLeastOnce | LocalPublishState::AwaitAck => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),

            // We have not sent any PUBREL yet at all, the peer skipped this handshake step.
            LocalPublishState::DuePublishExactlyOnce(_)
            | LocalPublishState::AwaitRec(_)
            | LocalPublishState::DueRel => (
                Response::Disconnect(ReasonCode::ProtocolError),
                Event::ServerError,
            ),

            LocalPublishState::DueReRel(_) | LocalPublishState::AwaitComp(_) => {
                self.remove();
                if reason_code.is_success() {
                    (Response::None, Event::Completed)
                } else {
                    (Response::None, Event::Rejected)
                }
            }
        }
    }
}

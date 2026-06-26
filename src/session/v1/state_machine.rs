use crate::types::ReasonCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Response {
    None,
    Acknowledge(ReasonCode),
    Receive(ReasonCode),
    Release(ReasonCode),
    Complete(ReasonCode),
    Disconnect(ReasonCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum StateError {
    NoCapacity,
    UnusedPacketIdentifier,
    MismatchedQoS,
    MismatchedHandshakeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event {
    Publish,
    Duplicate,
    Ignored,

    Aborted,
    Rejected,
    Acknowledged,
    Received,
    Released,
    Completed,

    ServerError,
}

// pub trait StateMachine: InboundPublishStateMachine + OutboundPublishStateMachine {}

// impl<T> StateMachine for T where T: InboundPublishStateMachine + OutboundPublishStateMachine {}

// pub trait InboundPublishStateMachine {
//     fn inbound_publish(
//         &mut self,
//         identified_qos: IdentifiedQoS,
//         manual_ack: bool,
//     ) -> (Response, Event);
//     fn outbound_puback(&mut self, packet_identifier: PacketIdentifier) -> Result<(), StateError>;
//     fn outbound_pubrec(&mut self, packet_identifier: PacketIdentifier) -> Result<(), StateError>;
//     fn inbound_pubrel(
//         &mut self,
//         packet_identifier: PacketIdentifier,
//         reason_code: ReasonCode,
//     ) -> (Response, Event);
//     fn outbound_pubcomp(&mut self, packet_identifier: PacketIdentifier) -> Result<(), StateError>;
// }

// pub trait OutboundPublishStateMachine {
//     fn outbound_publish(
//         &mut self,
//         identified_qos: IdentifiedQoS,
//         manual_ack: bool,
//     ) -> Result<(), StateError>;
//     fn inbound_puback(
//         &mut self,
//         packet_identifier: PacketIdentifier,
//         reason_code: ReasonCode,
//     ) -> (Response, Event);
//     fn inbound_pubrec(
//         &mut self,
//         packet_identifier: PacketIdentifier,
//         reason_code: ReasonCode,
//     ) -> (Response, Event);
//     fn outbound_pubrel(&mut self, packet_identifier: PacketIdentifier) -> Result<(), StateError>;
//     fn inbound_pubcomp(
//         &mut self,
//         packet_identifier: PacketIdentifier,
//         reason_code: ReasonCode,
//     ) -> (Response, Event);
// }

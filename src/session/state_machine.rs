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

use crate::types::{MqttString, MqttStringPair};

/// Options for an acknowledgement to the server with a PUBACK, PUBREC, PUBREL or PUBCOMP packet.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Options<'a> {
    /// The reason string property of the PUBACK, PUBREC, PUBREL or PUBCOMP packet.
    pub reason_string: Option<MqttString<'a>>,

    /// Arbitrary key-value pairs of strings sent as the user property entries of the PUBACK, PUBREC,
    /// PUBREL or PUBCOMP packet. Note that this slice's length must be less than [`Client`]'s const
    /// generic parameter `MAX_USER_PROPERTIES`.
    ///
    /// [`Client`]: crate::client::Client
    pub user_properties: &'a [MqttStringPair<'a>],
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'d> Options<'d> {
    /// Creates new acknowledgement options without properties.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            reason_string: None,
            user_properties: &[],
        }
    }
}

/// The mode with which acknowledgements of a publication flow for a given packet
/// identifier and an incoming/outgoing direction are sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Mode {
    /// Acknowledgements are sent automatically by the client [`ReasonCode::Success`].
    /// In case of a reconnection, PUBLISH packets have to be resent manually, but all
    /// required PUBREL packets are resent automatically.
    ///
    /// [`ReasonCode::Success`]: crate::types::ReasonCode::Success
    #[default]
    Automatic,
    /// Acknowledgements must be sent manually by the user. In case of a reconnection,
    /// any PUBLISH and PUBREL packets must be resent automatically.
    Manual,
}

impl Mode {
    /// Returns `true` if the ack mode is [`Automatic`].
    ///
    /// [`Automatic`]: AckMode::Automatic
    #[must_use]
    pub fn is_automatic(&self) -> bool {
        matches!(self, Self::Automatic)
    }

    /// Returns `true` if the ack mode is [`Manual`].
    ///
    /// [`Manual`]: AckMode::Manual
    #[must_use]
    pub fn is_manual(&self) -> bool {
        matches!(self, Self::Manual)
    }
}

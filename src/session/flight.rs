use crate::types::PacketIdentifier;

/// An incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct InFlightPublish<S> {
    /// The packet identifier of the publication process.
    pub packet_identifier: PacketIdentifier,
    /// The state of the publication process.
    pub state: S,
    /// The outgoing PUBACK/PUBREC/PUBREL/PUBCOMP packets are sent manually.
    // pub manual: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClientPublishState {
    /// PUBLISH has been sent, now waiting for PUBACK.
    AwaitAck,
    /// PUBLISH has been sent, now waiting for PUBREC.
    AwaitRec,
    /// PUBREC has been received, PUBREL will be sent manually. In non-manual flows, this step is skipped.
    DueRel,
    /// PUBREL has been sent, now waiting for PUBCOMP.
    AwaitComp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ServerPublishState {
    /// PUBLISH has been received. PUBACK will be sent manually. In non-manual flows, this step is skipped.
    DueAck,
    /// PUBLISH has been received. PUBREC will be sent manually. In non-manual flows, this step is skipped.
    DueRec,
    /// PUBREC has been sent, now waiting for PUBREL.
    AwaitRel,
    /// PUBREL has been received, PUBCOMP will be sent manually. In non-manual flows, this step is skipped.
    DueComp,
}

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the client.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum CPublishFlightState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet has been sent.
    /// The next step in the handshake is the server sending a PUBACK packet.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitingPuback,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been sent.
    /// The next step in the handshake is the server sending a PUBREC packet.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitingPubrec,
    /// A PUBREC packet has been received and responded to with a PUBREL packet.
    /// The last step in the handshake is the server sending a PUBCOMP packet.
    AwaitingPubcomp,
}

/// The state of an incomplete [`QoS::ExactlyOnce`] publication by the server.
///
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SPublishFlightState {
    /// A [`QoS::ExactlyOnce`] packet has been received and responded to with a PUBREC packet.
    /// The next step in the handshake is the server sending a PUBREL packet.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitingPubrel,
}

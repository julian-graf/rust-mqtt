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
}

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// client.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClientPublishState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet has been sent. The final and next step in the
    /// handshake is the server sending a PUBACK packet.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitAck,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been sent. The next step in the handshake is
    /// the server sending a PUBREC packet. The subsequent PUBREL packet will be sent
    /// automatically by the client.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitRec,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been sent. The next step in the handshake is
    /// the server sending a PUBREC packet. The subsequent PUBREL packet must be sent manually
    /// by the user.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitRecManual,
    /// A PUBREC packet has been received. The next step in the handshake is the client sending
    /// a PUBREL packet. This packet must be sent manually by the user.
    DueRel,
    /// A PUBREL packet has been sent. The final and next step in the handshake is the server
    /// sending a PUBCOMP packet.
    AwaitComp,
}

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// server.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ServerPublishState {
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
    /// PUBREL packet. The subsequent PUBCOMP packet will be sent automatically.
    AwaitRel,
    /// A PUBREC packet has been sent. The next step in the handshake is the server sending a
    /// PUBREL packet. The subsequent PUBCOMP packet must be sent manually by the client.
    AwaitRelManual,
    /// A PUBREL packet has been received. The final and next step in the handshake is the
    /// client sending a PUBCOMP packet. This packet must be sent manually by the user.
    DueComp,
}

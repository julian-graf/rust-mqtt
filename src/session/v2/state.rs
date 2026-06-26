/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// client.
///
/// In case of "manual" flows, where acknowledgements are sent manually by the user, the "manual"
/// portion only applies to the first packet of its kind within a flow. This "half-manual"
/// behaviour occurs when a reconnection is at play. If the user has sent a manual PUBREL and
/// a reconnection occurs before the PUBCOMP is received, the retransmitted PUBREL is sent
/// automatically by the client when it handles the retransmission of all PUBREL packets.
/// However, a PUBREL packet that has
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocalPublishState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet must be resent after a reconnection. The
    /// specification demands a retransmission in this case, not following it is a protocol
    /// violation.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    DuePublishAtLeastOnce,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet must be resent after a reconnection. The
    /// specification demands a retransmission in this case, not following it is a protocol
    /// violation. Whether the PUBREL packet associated with this flow must be sent manually
    /// by the user is determined by the boolean flag.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    DuePublishExactlyOnce(bool),
    /// A [`QoS::AtLeastOnce`] PUBLISH packet has been sent. The final and next step in the
    /// handshake is the server sending a PUBACK packet.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitAck,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet has been sent. The next step in the handshake is
    /// the server sending a PUBREC packet. Whether this packet must be sent manually by the
    /// user is determined by the boolean flag.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    AwaitRec(bool),
    /// A PUBREC packet has been received or a reconnection has occured with a PUBREL packet
    /// having been sent before. The next step in the handshake is the client (re-)sending a
    /// PUBREL packet. Whether this packet must be sent manually by the user is determined by
    /// the boolean flag.
    DueRel(bool),
    /// A PUBREL packet has been sent. The final and next step in the handshake is the server
    /// sending a PUBCOMP packet.
    AwaitComp(bool),
}

impl LocalPublishState {
    pub fn manual(self) -> bool {
        match self {
            Self::DuePublishAtLeastOnce => false,
            Self::AwaitAck => false,

            Self::DuePublishExactlyOnce(manual) => manual,
            Self::AwaitRec(manual) => manual,
            Self::DueRel(manual) => manual,
            Self::AwaitComp(manual) => manual,
        }
    }

    pub fn reconnected(self) -> Self {
        match self {
            Self::DuePublishAtLeastOnce => Self::DuePublishAtLeastOnce,
            Self::AwaitAck => Self::DuePublishAtLeastOnce,

            Self::DuePublishExactlyOnce(manual) => Self::DuePublishExactlyOnce(manual),
            Self::AwaitRec(manual) => Self::DuePublishExactlyOnce(manual),
            Self::DueRel(manual) => Self::DueRel(manual),
            Self::AwaitComp(manual) => Self::DueRel(manual),
        }
    }
}

/// The state of an incomplete [`QoS::AtLeastOnce`] or [`QoS::ExactlyOnce`] publication by the
/// server.
///
/// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
/// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PeerPublishState {
    /// A [`QoS::AtLeastOnce`] PUBLISH packet must be resent by the server after a reconnection.
    /// The specification demands a retransmission in this case, not following it is a protocol
    /// violation. The subsequent PUBACK packet must be sent manually by the user, as this state
    /// can only arise from a previous [`Self::DueAck`] state, which is always associated to a
    /// manual flow.
    ///
    /// [`QoS::AtLeastOnce`]: crate::types::QoS::AtLeastOnce
    AwaitPublishAtLeastOnce,
    /// A [`QoS::ExactlyOnce`] PUBLISH packet must be resent by the server after a reconnection.
    /// The specification demands a retransmission in this case, not following it is a protocol
    /// violation. Whether the subsequent PUBREC and PUBCOMP packets must be sent manually by
    /// the user is determined by the boolean flag.
    ///
    /// [`QoS::ExactlyOnce`]: crate::types::QoS::ExactlyOnce
    //
    // TODO: not yet clear whether this state is actually required. This state is only reached
    // after a PUBREC has been sent (or failed to send it) and the reconnection occured after.
    // This means that the peer may have received the PUBREC and also already sent the PUBREL.
    // Therefore, the peer may also send a PUBREL packet as the next packet of the handshake,
    // which we have to accept.
    // Answer: This state is required. If it were not present, the AwaitRel state must serve
    // as this state and therefore allow PUBLISH packets. This includes allowing a PUBLISH
    // packet in a handshake that has not yet been interrupted by a reconnection. A retransmit-
    // ted PUBLISH packet within a continuous network connection is a protocol violation
    // however according to [MQTT-4.4.0-1].
    AwaitPublishExactlyOnce(bool),
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
    /// PUBREL packet. Whether the subsequent PUBCOMP packet must be sent manually by the client
    /// is determined by the boolean flag.
    AwaitRel(bool),
    /// A PUBREL packet has been received. The final and next step in the handshake is the
    /// client sending a PUBCOMP packet. This packet must be sent manually by the user.
    DueComp,
}

impl PeerPublishState {
    pub fn manual(self) -> bool {
        match self {
            Self::AwaitPublishAtLeastOnce => true,
            Self::DueAck => true,

            Self::AwaitPublishExactlyOnce(manual) => manual,
            Self::DueRec => true,
            Self::AwaitRel(manual) => manual,
            Self::DueComp => true,
        }
    }

    pub fn reconnected(self) -> Self {
        match self {
            Self::AwaitPublishAtLeastOnce => Self::AwaitPublishAtLeastOnce,
            Self::DueAck => Self::AwaitPublishAtLeastOnce,

            Self::AwaitPublishExactlyOnce(manual) => Self::AwaitPublishExactlyOnce(manual),
            Self::DueRec => Self::AwaitPublishExactlyOnce(true),
            Self::AwaitRel(manual) => Self::AwaitPublishExactlyOnce(manual),
            Self::DueComp => Self::AwaitRel(true),
        }
    }
}

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
pub enum ClientPublishState {
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
    AwaitRec,
    AwaitRecManual,
    /// A PUBREC packet has been received or a reconnection has occured with a PUBREL packet
    /// having been sent before. The next step in the handshake is the client (re-)sending a
    /// PUBREL packet. Whether this packet must be sent manually by the user is determined by
    /// the boolean flag.
    DueRel,
    /// A PUBREL packet has been sent. The final and next step in the handshake is the server
    /// sending a PUBCOMP packet.
    AwaitComp,
}

impl ClientPublishState {
    pub fn manual(self) -> bool {
        match self {
            Self::AwaitAck => false,
            Self::AwaitRec => false,
            Self::AwaitRecManual => true,
            Self::DueRel => true,
            Self::AwaitComp => false,
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

impl ServerPublishState {
    pub fn manual(self) -> bool {
        match self {
            Self::DueAck => true,
            Self::DueRec => true,
            Self::AwaitRel => false,
            Self::AwaitRelManual => true,
            Self::DueComp => true,
        }
    }
}

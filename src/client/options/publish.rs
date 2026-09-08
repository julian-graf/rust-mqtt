use core::num::NonZero;

use const_fn::const_fn;

use crate::{
    client::AckMode,
    types::{MqttBinary, MqttString, MqttStringPair, QoS, TopicName},
};

/// Options for a publication.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Options<'p> {
    /// In case of [`QoS::ExactlyOnce`], whether the PUBREL acknowledgement
    /// packet of this flow is sent automatically by the client or must be
    /// sent manually by the user with [`Client::manual_release`]. In case
    /// of [`QoS::AtMostOnce`] or [`QoS::AtLeastOnce`] this value being set
    /// to [`AckMode::Manual`] has no effect and causes a recoverable error
    /// by the client.
    ///
    /// [`Client::manual_release`]: crate::client::Client::manual_release
    pub ack_mode: AckMode,

    /// The quality of service that the message is published with to the server.
    /// The quality of service level used by the server to send this publication
    /// to subscribed clients is the minimum of this value and the quality of service
    /// value of the receiving client's subscription.
    ///
    /// Must be less than or equal to the server's maximum quality of service level
    /// (can be checked via [`Client::server_config`]). The client will not publish
    /// if a violation occurs but prevent the protocol error and return an error.
    ///
    /// [`Client::server_config`]: crate::client::Client::server_config
    pub qos: QoS,

    /// Depicts the value of the retain flag in the PUBLISH packet.
    /// If set to 1, the server should retain the message on this topic.
    /// Retained messages with quality of service 0 can be discarded
    /// at any time by the server.
    ///
    /// Must be false when the server does not support retain (can be checked via
    /// [`Client::server_config`]). The client will not publish if a violation occurs
    /// but prevent the protocol error and return an error.
    ///
    /// [`Client::server_config`]: crate::client::Client::server_config
    pub retain: bool,

    /// The topic that the message is published on. The topic can be referenced over
    /// an existing topic alias mapping or by specifying the topic name and optionally
    /// mapping a topic alias to it.
    ///
    /// If an alias is used, it must be less than or equal to the server's maximum topic
    /// alias (can be checked via [`Client::server_config`]). The client will not publish
    /// if a violation occurs but prevent the protocol error and return an error.
    ///
    /// [`Client::server_config`]: crate::client::Client::server_config
    pub topic: TopicReference<'p>,

    /// Indicates whether the message is valid UTF-8. If [`None`], there is no statement
    /// about the UTF-8 character of the message.
    pub payload_format_indicator: Option<bool>,

    /// The message expiry interval in seconds of this application message. After this
    /// interval has passed, the server cannot publish this message onward to subscribers.
    /// If set to [`None`], the message does not expire and the message expiry interval
    /// property is omitted on the network.
    pub message_expiry_interval: Option<u32>,

    /// The topic on which the receiver should publish the response.
    pub response_topic: Option<TopicName<'p>>,

    /// Arbitrary binary data which the receiver should attach in the response to associate
    /// their response with this request.
    pub correlation_data: Option<MqttBinary<'p>>,

    /// Arbitrary key-value pairs of strings. Note that this slice's length must be less than
    /// [`Client`]'s const generic parameter `MAX_USER_PROPERTIES`.
    ///
    /// [`Client`]: crate::client::Client
    pub user_properties: &'p [MqttStringPair<'p>],

    /// The custom content type of the message.
    pub content_type: Option<MqttString<'p>>,
}

impl<'p> Options<'p> {
    /// Creates options with values coherent to the [`Default`] implementations of the fields and
    /// [`QoS::AtMostOnce`].
    #[must_use]
    pub const fn new(topic: TopicReference<'p>) -> Options<'p> {
        Options {
            ack_mode: AckMode::Automatic,
            qos: QoS::AtMostOnce,
            retain: false,
            topic,
            payload_format_indicator: None,
            message_expiry_interval: None,
            response_topic: None,
            correlation_data: None,
            user_properties: &[],
            content_type: None,
        }
    }

    /// Sets the acknowledgement mode to manual acknowledgements.
    #[must_use]
    pub const fn ack_manually(mut self) -> Self {
        self.ack_mode = AckMode::Manual;
        self
    }
    /// Sets the Quality of Service level.
    ///
    /// Note that this level must be less than or equal to the server's
    /// maximum quality of service level.
    #[must_use]
    pub const fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }
    /// Sets the Quality of Service level to 1 ([`QoS::AtLeastOnce`]).
    ///
    /// Note that this is only allowed if the server's maximum quality
    /// of service is 1 or 2.
    #[must_use]
    pub const fn at_least_once(self) -> Self {
        self.qos(QoS::AtLeastOnce)
    }
    /// Sets the Quality of Service level to 2 ([`QoS::ExactlyOnce`]).
    ///
    /// Note that this is only allowed if the server's maximum quality
    /// of service is 2.
    #[must_use]
    pub const fn exactly_once(self) -> Self {
        self.qos(QoS::ExactlyOnce)
    }
    /// Sets the retain flag to true.
    ///
    /// Note that this is only allowed if the server supports retain.
    #[must_use]
    pub const fn retain(mut self) -> Self {
        self.retain = true;
        self
    }
    /// Sets the payload format indicator property.
    #[must_use]
    pub const fn payload_format_indicator(mut self, is_payload_utf8: bool) -> Self {
        self.payload_format_indicator = Some(is_payload_utf8);
        self
    }
    /// Sets the message expiry interval in seconds.
    #[must_use]
    pub const fn message_expiry_interval(mut self, seconds: u32) -> Self {
        self.message_expiry_interval = Some(seconds);
        self
    }
    /// Marks the publication as a request by setting the response topic property.
    #[const_fn(cfg(not(feature = "alloc")))]
    #[must_use]
    pub const fn response_topic(mut self, topic: TopicName<'p>) -> Self {
        self.response_topic = Some(topic);
        self
    }
    /// Sets the correlation data property in the request.
    #[const_fn(cfg(not(feature = "alloc")))]
    #[must_use]
    pub const fn correlation_data(mut self, data: MqttBinary<'p>) -> Self {
        self.correlation_data = Some(data);
        self
    }
    /// Sets the user properties. Note that this slice's length must be less than [`Client`]'s
    /// const generic parameter `MAX_USER_PROPERTIES`.
    ///
    /// [`Client`]: crate::client::Client
    #[must_use]
    pub const fn user_properties(mut self, user_properties: &'p [MqttStringPair<'p>]) -> Self {
        self.user_properties = user_properties;
        self
    }
    /// Sets the content type property.
    #[const_fn(cfg(not(feature = "alloc")))]
    #[must_use]
    pub const fn content_type(mut self, content_type: MqttString<'p>) -> Self {
        self.content_type = Some(content_type);
        self
    }
}

/// The options for specifying which topic to publish to. Topic aliases only last for the
/// duration of a single network connection and not necessarily until the session end.
///
/// Topic aliases must not be 0
#[derive(Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TopicReference<'t, T = &'t [u8]> {
    /// Publish to the inner topic name without creating an alias.
    Name(TopicName<'t, T>),

    /// Publish to an already mapped topic alias. The alias must have been defined earlier
    /// in the network connection.
    Alias(NonZero<u16>),

    /// Create a new topic alias or replace an existing topic alias.
    /// The alias lasts until the end of the network connection.
    Mapping(TopicName<'t, T>, NonZero<u16>),
}

impl<'t, T: AsRef<[u8]>> core::fmt::Debug for TopicReference<'t, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Name(arg0) => f.debug_tuple("Name").field(arg0).finish(),
            Self::Alias(arg0) => f.debug_tuple("Alias").field(arg0).finish(),
            Self::Mapping(arg0, arg1) => f.debug_tuple("Mapping").field(arg0).field(arg1).finish(),
        }
    }
}
#[cfg(feature = "defmt")]
impl<'a, T: AsRef<[u8]>> defmt::Format for TopicReference<'a, T> {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            Self::Name(arg0) => defmt::write!(fmt, "Name({:?})", arg0.as_ref()),
            Self::Alias(arg0) => defmt::write!(fmt, "Alias({:?})", arg0.as_ref()),
            Self::Mapping(arg0, arg1) => {
                defmt::write!(fmt, "Mapping({:?}, {:?})", arg0.as_ref(), arg1.as_ref())
            }
        }
    }
}

impl<'t, T: AsRef<[u8]>> PartialEq for TopicReference<'t, T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Name(l0), Self::Name(r0)) => l0 == r0,
            (Self::Alias(l0), Self::Alias(r0)) => l0 == r0,
            (Self::Mapping(l0, l1), Self::Mapping(r0, r1)) => l0 == r0 && l1 == r1,
            _ => false,
        }
    }
}
impl<'t, T: AsRef<[u8]>> Eq for TopicReference<'t, T> {}

impl<'t, T> TopicReference<'t, T> {
    pub(crate) fn alias(&self) -> Option<NonZero<u16>> {
        match self {
            Self::Name(_) => None,
            Self::Alias(alias) => Some(*alias),
            Self::Mapping(_, alias) => Some(*alias),
        }
    }
    pub(crate) fn topic_name(&self) -> Option<&TopicName<'t, T>> {
        match self {
            Self::Name(topic_name) => Some(topic_name),
            Self::Alias(_) => None,
            Self::Mapping(topic_name, _) => Some(topic_name),
        }
    }
}

impl<'t, T: AsRef<[u8]>> TopicReference<'t, T> {
    /// Delegates to [`Bytes::as_borrowed`].
    ///
    /// [`Bytes::as_borrowed`]: crate::Bytes::as_borrowed
    #[must_use]
    pub fn as_borrowed(&'t self) -> TopicReference<'t> {
        match self {
            Self::Name(topic_name) => TopicReference::Name(topic_name.as_borrowed()),
            Self::Alias(alias) => TopicReference::Alias(*alias),
            Self::Mapping(topic_name, alias) => {
                TopicReference::Mapping(topic_name.as_borrowed(), *alias)
            }
        }
    }
}

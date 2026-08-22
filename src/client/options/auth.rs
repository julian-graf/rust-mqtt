use const_fn::const_fn;

use crate::types::{MqttBinary, MqttString};

/// Options for enhanced authentication for the CONNECT packet.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Options<'m, 'd> {
    /// The authentication method property of the CONNECT packet and all AUTH packets in the same
    /// network connection.
    pub authentication_method: Option<MqttString<'m>>,

    /// The authentication data property of the CONNECT packet.
    pub authentication_data: Option<MqttBinary<'d>>,
}

impl<'m, 'd> Options<'m, 'd> {
    /// Creates new authentication options without an authentication method and authentication data.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            authentication_method: None,
            authentication_data: None,
        }
    }

    /// Sets the authentication method property.
    #[const_fn(cfg(not(feature = "alloc")))]
    #[must_use]
    pub const fn authentication_method(mut self, authentication_method: MqttString<'m>) -> Self {
        self.authentication_method = Some(authentication_method);
        self
    }
    /// Sets the authentication data property.
    #[const_fn(cfg(not(feature = "alloc")))]
    #[must_use]
    pub const fn authentication_data(mut self, authentication_data: MqttBinary<'d>) -> Self {
        self.authentication_data = Some(authentication_data);
        self
    }
}

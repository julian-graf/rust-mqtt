use heapless::Vec;

use crate::types::{MqttBinary, MqttString, MqttStringPair};

pub struct Auth<'a, const MAX_USER_PROPERTIES: usize> {
    pub authentication_data: Option<MqttBinary<'a>>,
    pub reason_string: Option<MqttString<'a>>,
    pub user_properties: Vec<MqttStringPair<'a>, MAX_USER_PROPERTIES>,
}

pub trait AuthProvider {
    type Error;

    fn kontinue<const MAX_USER_PROPERTIES: usize>(
        &mut self,
        auth: &Auth<'_, MAX_USER_PROPERTIES>,
    ) -> Result<Auth<'_, MAX_USER_PROPERTIES>, Self::Error>;
    fn success<const MAX_USER_PROPERTIES: usize>(
        &mut self,
        auth: &Auth<'_, MAX_USER_PROPERTIES>,
    ) -> Result<(), Self::Error>;
}

use crate::io::{err::DecodeError, read::Readable, reader::PacketDecoder};

mod types;
mod values;

pub use types::PropertyType;
pub use values::*;

/// This implementation serves as both an enabler trait for a generic Readable and Writable implementation for the Property as well as a marker trait for the following qualities of the Readable and Writable impls:
///
/// * Writable writes both its property type's identifier and its content
/// * Readable reads only the property's content
pub trait Property {
    const TYPE: PropertyType;
    type Inner;

    fn into_inner(self) -> Self::Inner;
}

/// Helper trait to read optional, but at max once properties into a packet
pub trait AtMostOnceProperty<'p, T: Property> {
    fn try_set(&mut self, read: &mut PacketDecoder<'p>) -> Result<(), DecodeError>;
}

impl<'p, T: Property + Readable<'p>> AtMostOnceProperty<'p, T> for Option<T> {
    fn try_set(&mut self, read: &mut PacketDecoder<'p>) -> Result<(), DecodeError> {
        if self.is_some() {
            Err(DecodeError::ProtocolError)
        } else {
            let value = T::read(read)?;

            self.replace(value);
            Ok(())
        }
    }
}

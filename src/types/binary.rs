#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

use crate::{
    bytes::Bytes,
    fmt::const_debug_assert,
    types::{MqttString, TooLargeToEncode},
};

/// Arbitrary binary data with a length less than or equal to [`MqttBinary::MAX_LENGTH`] ([`u16::MAX`]).
/// Exceeding this size ultimately leads to malformed packets.
///
/// # Examples
///
/// ```rust
/// use rust_mqtt::Bytes;
/// use rust_mqtt::types::{MqttBinary, MqttString, TooLargeToEncode};
///
/// let slice = [0x00; MqttBinary::MAX_LENGTH];
/// let too_long = [0x00; MqttBinary::MAX_LENGTH + 1];
///
/// let b = MqttBinary::from_slice(&slice)?;
/// assert_eq!(b.as_bytes(), &slice);
/// assert!(MqttBinary::from_slice(&too_long).is_err());
///
/// let b = MqttBinary::from_bytes(Bytes::from(&slice[..]))?;
/// assert_eq!(b.as_bytes(), &slice);
/// assert!(MqttBinary::from_bytes(Bytes::from(&too_long[..])).is_err());
///
/// let from_slice_unchecked = MqttBinary::from_slice_unchecked(&slice);
/// assert_eq!(from_slice_unchecked.as_bytes(), &slice);
///
/// let from_bytes_unchecked = MqttBinary::from_bytes_unchecked(Bytes::from(&slice[..]));
/// assert_eq!(from_bytes_unchecked.as_bytes(), &slice);
///
/// let from_vec = MqttBinary::try_from(vec![0, 1, 2])?;
/// assert_eq!(from_vec.as_bytes(), &[0, 1, 2]);
///
/// let from_boxed_slice = MqttBinary::try_from(vec![3, 4, 5].into_boxed_slice())?;
/// assert_eq!(from_boxed_slice.as_bytes(), &[3, 4, 5]);
///
/// # Ok::<(), TooLargeToEncode>(())
/// ```
#[derive(Clone)]
pub struct MqttBinary<'b, B = &'b [u8]>(pub(crate) Bytes<'b, B>);

impl<'b, B: AsRef<[u8]>> PartialEq for MqttBinary<'b, B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<'b, B: AsRef<[u8]>> Eq for MqttBinary<'b, B> {}

impl<'b> Default for MqttBinary<'b> {
    fn default() -> Self {
        Self(Bytes::default())
    }
}

impl<B: AsRef<[u8]>> core::fmt::Debug for MqttBinary<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("MqttBinary").field(&self.as_ref()).finish()
    }
}

#[cfg(feature = "defmt")]
impl<'a, B: AsRef<[u8]>> defmt::Format for MqttBinary<'a, B> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "MqttBinary({:?})", self.as_ref());
    }
}

impl<'b> TryFrom<&'b [u8]> for MqttBinary<'b> {
    type Error = TooLargeToEncode;

    fn try_from(value: &'b [u8]) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl<'b> TryFrom<&'b str> for MqttBinary<'b> {
    type Error = TooLargeToEncode;

    fn try_from(value: &'b str) -> Result<Self, Self::Error> {
        Self::new(value.as_bytes())
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<Vec<u8>> for MqttBinary<'static, Box<[u8]>> {
    type Error = TooLargeToEncode;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.into_boxed_slice())
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<Box<[u8]>> for MqttBinary<'static, Box<[u8]>> {
    type Error = TooLargeToEncode;

    fn try_from(value: Box<[u8]>) -> Result<Self, Self::Error> {
        Self::new(Bytes::from(value))
    }
}

impl<'b, B: AsRef<[u8]>> From<MqttString<'b, B>> for MqttBinary<'b, B> {
    fn from(value: MqttString<'b, B>) -> Self {
        Self(value.0.0)
    }
}

impl<B: AsRef<[u8]>> AsRef<[u8]> for MqttBinary<'_, B> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<'b, B: AsRef<[u8]>> MqttBinary<'b, B> {
    /// The maximum length of binary data so that it can be encoded. This value is limited by the 2-byte length field.
    pub const MAX_LENGTH: usize = u16::MAX as usize;

    /// Converts a slice into [`MqttBinary`] by cloning the reference and checking for the max
    /// length of [`MqttBinary::MAX_LENGTH`].
    ///
    /// # Errors
    ///
    /// Returns [`TooLargeToEncode`] if `slice`'s length exceeds [`MqttBinary::MAX_LENGTH`].
    pub fn new(b: impl Into<Bytes<'b, B>>) -> Result<Self, TooLargeToEncode> {
        let bytes = b.into();

        match bytes.len() {
            ..=Self::MAX_LENGTH => Ok(Self::new_unchecked(bytes)),
            _ => Err(TooLargeToEncode),
        }
    }

    /// Converts a `B` into [`MqttBinary`] without checking for the max length of
    /// [`MqttBinary::MAX_LENGTH`].
    ///
    /// # Invariants
    ///
    /// The length of the slice parameter in bytes is less than or equal to
    /// [`MqttBinary::MAX_LENGTH`].
    ///
    /// # Panics
    ///
    /// In debug builds, this function will panic if the slice's length is greater than
    /// [`MqttBinary::MAX_LENGTH`].
    #[must_use]
    pub fn new_unchecked(b: impl Into<Bytes<'b, B>>) -> Self {
        let bytes = b.into();

        const_debug_assert!(
            bytes.len() <= Self::MAX_LENGTH,
            "the bytes' length exceeds MAX_LENGTH"
        );

        Self(bytes)
    }

    /// Returns the length of the underlying data.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u16 {
        self.0.len() as u16
    }

    /// Returns whether the underlying data is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the underlying bytes as `&[u8]`
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Delegates to [`Bytes::as_borrowed`].
    #[inline]
    #[must_use]
    pub fn as_borrowed(&'b self) -> MqttBinary<'b> {
        MqttBinary(self.0.as_borrowed())
    }
}

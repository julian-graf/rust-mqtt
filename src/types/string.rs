#[cfg(feature = "alloc")]
use alloc::{boxed::Box, string::String, vec::Vec};
use core::str::{Utf8Error, from_utf8, from_utf8_unchecked};

use crate::{
    fmt::const_debug_assert,
    types::{MqttBinary, TooLargeToEncode},
};

/// Error returned when creating [`MqttString`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttStringError {
    /// The passed data is not valid UTF-8.
    Utf8Error(Utf8Error),

    /// The passed data contains at least one null character.
    NullCharacter,

    /// The passed data exceeds the max length of [`MqttString::MAX_LENGTH`].
    TooLargeToEncode,
}

#[cfg(feature = "defmt")]
impl defmt::Format for MqttStringError {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            Self::Utf8Error(e) => defmt::write!(
                fmt,
                "Utf8Error(Utf8Error {{ valid_up_to: {:?}, error_len: {:?} }})",
                e.valid_up_to(),
                e.error_len()
            ),
            Self::NullCharacter => defmt::write!(fmt, "NullCharacter"),
            Self::TooLargeToEncode => defmt::write!(fmt, "TooLargeToEncode"),
        }
    }
}
impl From<Utf8Error> for MqttStringError {
    fn from(e: Utf8Error) -> Self {
        Self::Utf8Error(e)
    }
}
impl From<TooLargeToEncode> for MqttStringError {
    fn from(_: TooLargeToEncode) -> Self {
        Self::TooLargeToEncode
    }
}

/// Arbitrary UTF-8 encoded string with a length in bytes less than or equal to
/// [`MqttString::MAX_LENGTH`] ([`u16::MAX`]) and no null characters.
/// Exceeding this size ultimately leads to malformed packets.
///
/// # Examples
///
/// ```rust
/// use rust_mqtt::types::{MqttBinary, MqttString, MqttStringError};
///
/// let bytes = [b'a'; MqttString::MAX_LENGTH];
/// let too_long = [b'a'; MqttString::MAX_LENGTH + 1];
/// let null_character = "hi\0there";
///
/// let slice = core::str::from_utf8(&bytes)?;
/// let too_long = core::str::from_utf8(&too_long)?;
///
/// let b = MqttBinary::from_slice(&bytes)?;
/// let s = MqttString::from_utf8_binary(b)?;
/// assert_eq!(s.as_str(), slice);
/// let b = MqttBinary::from_slice(null_character.as_bytes())?;
/// assert_eq!(MqttString::from_utf8_binary(b).unwrap_err(), MqttStringError::NullCharacter);
///
/// let s = MqttString::from_str(slice)?;
/// assert_eq!(s.as_str(), slice);
/// assert_eq!(MqttString::from_str(too_long).unwrap_err(), MqttStringError::TooLargeToEncode);
/// assert_eq!(MqttString::from_str(&null_character).unwrap_err(), MqttStringError::NullCharacter);
///
/// let s = MqttString::from_str_unchecked(slice);
/// assert_eq!(s.as_str(), slice);
///
/// let b = MqttBinary::from_slice_unchecked(slice.as_bytes());
/// let s = unsafe { MqttString::from_utf8_binary_unchecked(b) };
/// assert_eq!(s.as_str(), slice);
///
/// let from_string = MqttString::try_from("abc".to_string())?;
/// assert_eq!(from_string.as_str(), "abc");
///
/// let from_vec = MqttString::try_from(vec![b'd', b'e', b'f'])?;
/// assert_eq!(from_vec.as_str(), "def");
///
/// let from_boxed_str = MqttString::try_from(Box::<str>::from("ghi"))?;
/// assert_eq!(from_boxed_str.as_str(), "ghi");
///
/// let from_boxed_byte_slice = MqttString::try_from(vec![b'j', b'k', b'l'].into_boxed_slice())?;
/// assert_eq!(from_boxed_byte_slice.as_str(), "jkl");
///
/// # Ok::<(), MqttStringError>(())
/// ```
#[derive(Clone)]
pub struct MqttString<'s, B = &'s [u8]>(pub(crate) MqttBinary<'s, B>);

impl<'s, B: AsRef<[u8]>> PartialEq for MqttString<'s, B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<'s, B: AsRef<[u8]>> Eq for MqttString<'s, B> {}

impl<'s> Default for MqttString<'s> {
    fn default() -> Self {
        Self(MqttBinary::default())
    }
}

impl<B: AsRef<[u8]>> core::fmt::Debug for MqttString<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("MqttString").field(&self.as_ref()).finish()
    }
}

#[cfg(feature = "defmt")]
impl<'a, B: AsRef<[u8]>> defmt::Format for MqttString<'a, B> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "MqttString({:?})", self.as_ref());
    }
}

impl<'s, B: AsRef<[u8]>> TryFrom<MqttBinary<'s, B>> for MqttString<'s, B> {
    type Error = MqttStringError;

    fn try_from(value: MqttBinary<'s, B>) -> Result<Self, Self::Error> {
        Self::from_utf8_binary(value)
    }
}
impl<'s> TryFrom<&'s str> for MqttString<'s> {
    type Error = MqttStringError;

    fn try_from(value: &'s str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<String> for MqttString<'static, Box<[u8]>> {
    type Error = MqttStringError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        MqttBinary::try_from(value.into_bytes())
            .map_err(|_| MqttStringError::TooLargeToEncode)
            .and_then(|b| {
                (!b.as_bytes().contains(&0))
                    // Safety: String contains valid UTF-8
                    // Invariants: we checked for null characters
                    .then(|| unsafe { MqttString::from_utf8_binary_unchecked(b) })
                    .ok_or(MqttStringError::NullCharacter)
            })
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<Vec<u8>> for MqttString<'static, Box<[u8]>> {
    type Error = MqttStringError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        String::try_from(value)
            .map_err(|e| MqttStringError::Utf8Error(e.utf8_error()))
            .and_then(Self::try_from)
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<Box<str>> for MqttString<'static, Box<[u8]>> {
    type Error = MqttStringError;

    fn try_from(value: Box<str>) -> Result<Self, Self::Error> {
        Self::try_from(value.into_string())
    }
}
#[cfg(feature = "alloc")]
impl TryFrom<Box<[u8]>> for MqttString<'static, Box<[u8]>> {
    type Error = MqttStringError;

    fn try_from(value: Box<[u8]>) -> Result<Self, Self::Error> {
        Self::try_from(value.into_vec())
    }
}

impl<B: AsRef<[u8]>> AsRef<str> for MqttString<'_, B> {
    fn as_ref(&self) -> &str {
        // Safety: MqttString contains valid UTF-8
        unsafe { from_utf8_unchecked(self.0.as_ref()) }
    }
}

impl<'s, B: AsRef<[u8]>> MqttString<'s, B> {
    /// The maximum length of a string in bytes so that it can be encoded.
    /// This value is limited by the 2-byte length field.
    pub const MAX_LENGTH: usize = MqttBinary::<&[u8]>::MAX_LENGTH;

    /// Converts [`MqttBinary`] into [`MqttString`] by checking for null characters and valid UTF-8.
    /// Valid length is guaranteed by [`MqttBinary`]'s invariant.
    ///
    /// # Errors
    ///
    /// * [`MqttStringError::Utf8Error`] if `b` is not valid UTF-8.
    /// * [`MqttStringError::NullCharacter`] if `b` contains an ASCII `\0` character.
    /// * [`MqttStringError::TooLargeToEncode`] if `b`'s length exceeds [`MqttString::MAX_LENGTH`].
    pub fn from_utf8_binary(b: MqttBinary<'s, B>) -> Result<MqttString<'s, B>, MqttStringError> {
        let mut i = 0;
        while i < b.as_bytes().len() {
            if b.as_bytes()[i] == 0 {
                return Err(MqttStringError::NullCharacter);
            }
            i += 1;
        }

        match from_utf8(b.as_bytes()) {
            Ok(_) => Ok(Self(b)),
            Err(e) => Err(MqttStringError::Utf8Error(e)),
        }
    }

    /// Converts [`MqttBinary`] into [`MqttString`] without checking for null characters or valid UTF-8.
    /// Valid length is guaranteed by [`MqttBinary`]'s invariant.
    ///
    /// # Safety
    ///
    /// The binary passed in must be valid UTF-8.
    ///
    /// # Invariants
    ///
    /// The binary data does not contain any null characters.
    ///
    /// # Panics
    ///
    /// In debug builds, this function will panic if the binary contains a null character or is not
    /// valid UTF-8.
    #[must_use]
    pub unsafe fn from_utf8_binary_unchecked(b: MqttBinary<'s, B>) -> Self {
        if cfg!(debug_assertions) {
            let mut i = 0;
            while i < b.as_bytes().len() {
                const_debug_assert!(b.as_bytes()[i] != 0);
                i += 1;
            }
        }
        const_debug_assert!(from_utf8(b.as_bytes()).is_ok());

        Self(b)
    }

    /// Converts a string slice into [`MqttString`] by checking for null characters and the max
    /// length of [`MqttString::MAX_LENGTH`].
    ///
    /// # Errors
    ///
    /// * [`MqttStringError::NullCharacter`] if `s` contains an ASCII `\0` character.
    /// * [`MqttStringError::TooLargeToEncode`] if `s`' length exceeds [`MqttString::MAX_LENGTH`].
    #[expect(clippy::should_implement_trait)]   // cannot implement FromStr due to lifetime constraints
    pub fn from_str(s: &'s str) -> Result<MqttString<'s>, MqttStringError> {
        let mut i = 0;
        while i < s.len() {
            if s.as_bytes()[i] == 0 {
                return Err(MqttStringError::NullCharacter);
            }
            i += 1;
        }

        match s.len() {
            ..=Self::MAX_LENGTH => Ok(MqttString(MqttBinary::new_unchecked(s.as_bytes()))),
            _ => Err(MqttStringError::TooLargeToEncode),
        }
    }

    /// Converts a string slice into [`MqttString`] without checking for null characters or the max
    /// length of [`MqttString::MAX_LENGTH`].
    ///
    /// # Invariants
    ///
    /// The length of the string slice must be less than or equal to [`MqttString::MAX_LENGTH`]. The
    /// string must not contain any null characters.
    ///
    /// # Panics
    ///
    /// In debug builds, this function will panic if the slice contains a null character or its length is greater
    /// than [`MqttString::MAX_LENGTH`].
    #[must_use]
    pub fn from_str_unchecked(s: &'s str) -> MqttString<'s> {
        if cfg!(debug_assertions) {
            let mut i = 0;
            while i < s.len() {
                const_debug_assert!(s.as_bytes()[i] != 0);
                i += 1;
            }
        }

        MqttString(MqttBinary::new_unchecked(s.as_bytes()))
    }

    /// Returns the length of the underlying data in bytes.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u16 {
        self.0.len()
    }

    /// Returns whether the underlying data is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the underlying string as `&str`
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Safety: MqttString contains valid UTF-8
        unsafe { from_utf8_unchecked(self.0.as_bytes()) }
    }

    /// Delegates to [`Bytes::as_borrowed`].
    ///
    /// [`Bytes::as_borrowed`]: crate::Bytes::as_borrowed
    #[inline]
    #[must_use]
    pub fn as_borrowed(&'s self) -> MqttString<'s> {
        MqttString(self.0.as_borrowed())
    }
}

/// A name-value pair of two [`MqttString`]'s.
#[derive(Clone)]
pub struct MqttStringPair<'s, S = &'s [u8]> {
    /// The name part of the string pair.
    pub name: MqttString<'s, S>,

    /// The value part of the string pair.
    pub value: MqttString<'s, S>,
}

// TODO Default impl for B = &[u8]

impl<'s, S: AsRef<[u8]>> PartialEq for MqttStringPair<'s, S> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}
impl<'s, S: AsRef<[u8]>> Eq for MqttStringPair<'s, S> {}

impl<'s, S: AsRef<[u8]>> core::fmt::Debug for MqttStringPair<'s, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MqttStringPair")
            .field("name", &self.name.as_str())
            .field("value", &self.value.as_str())
            .finish()
    }
}

#[cfg(feature = "defmt")]
impl<'s, S: AsRef<[u8]>> defmt::Format for MqttStringPair<'s, S> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "MqttStringPair {{ name: {:?}, value: {:?} }}",
            self.name.as_str(),
            self.value.as_str()
        );
    }
}

impl<'s, B: AsRef<[u8]>> MqttStringPair<'s, B> {
    /// Creates a new [`MqttStringPair`]
    #[must_use]
    pub const fn new(name: MqttString<'s, B>, value: MqttString<'s, B>) -> Self {
        Self { name, value }
    }

    /// Delegates to [`Bytes::as_borrowed`].
    ///
    /// [`Bytes::as_borrowed`]: crate::Bytes::as_borrowed
    #[inline]
    #[must_use]
    pub fn as_borrowed(&'s self) -> MqttStringPair<'s> {
        MqttStringPair::new(self.name.as_borrowed(), self.value.as_borrowed())
    }
}

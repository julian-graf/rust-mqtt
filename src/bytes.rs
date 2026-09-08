#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};
use core::{borrow::Borrow, marker::PhantomData, ops::Deref};

/// Contiguous bytes in memory. Is either a [`u8`] slice or (with crate feature "alloc") an owned
/// [`Box`]<[u8]>.
///
/// It is recommended to almost always use owned [`Bytes`] instead of a reference to [`Bytes`],
/// as it makes this type compatible for code designed for both owned and borrowed variants.
///
/// Important: Cloning this will clone the underlying Box if it is an owned variant.
/// You can however borrow another owned [`Bytes`] by calling [`Bytes::as_borrowed`].
/// The [`Bytes::as_borrowed`] method is passed on through wrapper types, for example
/// [`MqttString`].
///
/// [`MqttString`]: crate::types::MqttString
#[derive(Clone)]
pub struct Bytes<'a, B = &'a [u8]> {
    b: B,
    _lt: PhantomData<&'a ()>,
}

impl<B> Bytes<'_, B> {
    pub const fn new(bytes: B) -> Self {
        Self {
            b: bytes,
            _lt: PhantomData,
        }
    }
}

impl<B: AsRef<[u8]>> Bytes<'_, B> {
    /// Returns the underlying data as `&[u8]`.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.as_ref()
    }

    /// Borrows `self` with its full lifetime to create another owned [`Self`] instance.
    #[inline]
    #[must_use]
    pub fn as_borrowed<'a>(&'a self) -> Bytes<'a, &'a [u8]> {
        Bytes::new(self.as_ref())
    }

    /// Returns the number of bytes.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_ref().len()
    }

    /// Returns whether the underlying data has a length of 0.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl<B: AsRef<[u8]>> AsRef<[u8]> for Bytes<'_, B> {
    fn as_ref(&self) -> &[u8] {
        self.b.as_ref()
    }
}

// impl<B> From<Bytes<B>> for B {
//     fn from(bytes: Bytes<B>) -> Self {
//         bytes.0
//     }
// }
impl<B> From<B> for Bytes<'_, B> {
    fn from(bytes: B) -> Self {
        Self::new(bytes)
    }
}

impl Default for Bytes<'_, &'_ [u8]> {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl<B: AsRef<[u8]>> core::fmt::Debug for Bytes<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        self.as_ref().fmt(f)
    }
}

#[cfg(feature = "defmt")]
impl<B: AsRef<[u8]>> defmt::Format for Bytes<'_, B> {
    fn format(&self, fmt: defmt::Formatter) {
        self.as_ref().format(fmt)
    }
}

impl<B: AsRef<[u8]>> PartialEq for Bytes<'_, B> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}
impl<B: AsRef<[u8]>> Eq for Bytes<'_, B> {}

impl<'a> From<&'a mut str> for Bytes<'_, &'a [u8]> {
    fn from(value: &'a mut str) -> Self {
        Self::new(value.as_bytes())
    }
}
impl<'a> From<&'a str> for Bytes<'_, &'a [u8]> {
    fn from(value: &'a str) -> Self {
        Self::new(value.as_bytes())
    }
}

// #[cfg(feature = "alloc")]
// impl From<Box<[u8]>> for Bytes<'_> {
//     fn from(value: Box<[u8]>) -> Self {
//         Self::Owned(value)
//     }
// }
// #[cfg(feature = "alloc")]
// impl From<Vec<u8>> for Bytes<'_> {
//     fn from(value: Vec<u8>) -> Self {
//         Self::Owned(value.into_boxed_slice())
//     }
// }

// impl Deref for Bytes<'_> {
//     type Target = [u8];

//     fn deref(&self) -> &Self::Target {
//         self.as_bytes()
//     }
// }

// impl Borrow<[u8]> for Bytes<'_> {
//     fn borrow(&self) -> &[u8] {
//         self
//     }
// }

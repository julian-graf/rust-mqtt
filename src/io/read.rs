use crate::{
    io::{err::DecodeError, reader::PacketDecoder},
    types::{MqttBinary, MqttString},
};

pub trait Readable<'r>: Sized + 'r {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError>;
}

impl<'r, const N: usize> Readable<'r> for [u8; N] {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        let mut array = [0; N];
        array.copy_from_slice(read.take_bytes(N)?);

        Ok(array)
    }
}
impl<'r> Readable<'r> for u8 {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        <[u8; 1]>::read(read).map(Self::from_be_bytes)
    }
}
impl<'r> Readable<'r> for u16 {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        <[u8; 2]>::read(read).map(Self::from_be_bytes)
    }
}
impl<'r> Readable<'r> for u32 {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        <[u8; 4]>::read(read).map(Self::from_be_bytes)
    }
}
impl<'r> Readable<'r> for bool {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        match u8::read(read)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::MalformedPacket),
        }
    }
}
impl<'r> Readable<'r> for MqttBinary<'r> {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        let len = u16::read(read)?;

        // Safety: `len` is u16 and therefore always greater than or equal to MqttBinary::MAX_ENCODABLE, which is u16::MAX
        Ok(unsafe { MqttBinary::from_slice_unchecked(read.take_bytes(len as usize)?) })
    }
}
impl<'r> Readable<'r> for MqttString<'r> {
    fn read(read: &mut PacketDecoder<'r>) -> Result<Self, DecodeError> {
        MqttBinary::read(read)?
            .try_into()
            .map_err(|_| DecodeError::MalformedPacket)
    }
}

#[cfg(test)]
mod unit {
    mod readable {
        use tokio_test::{assert_err, assert_ok};

        use crate::{io::err::DecodeError, io::read::Readable, test::read::SliceReader};

        #[tokio::test]
        #[test_log::test]
        async fn read_array() {
            let mut r = SliceReader::new(b"abcdefghijklmnopqrstuvwxyz");
            let a = assert_ok!(<[u8; 26]>::read(&mut r).await);
            assert_eq!(&a, b"abcdefghijklmnopqrstuvwxyz");
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u8() {
            let mut r = SliceReader::new(b"\x12");
            let v = assert_ok!(u8::read(&mut r).await);
            assert_eq!(v, 0x12);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u16() {
            let mut r = SliceReader::new(b"\x01\x02");
            let v = assert_ok!(u16::read(&mut r).await);
            assert_eq!(v, 0x0102);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u32() {
            let mut r = SliceReader::new(b"\x01\x02\x03\x04");
            let v = assert_ok!(u32::read(&mut r).await);
            assert_eq!(v, 0x01020304);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_bool() {
            let mut r_false = SliceReader::new(b"\x00");
            let b = assert_ok!(bool::read(&mut r_false).await);
            assert!(!b);

            let mut r_true = SliceReader::new(b"\x01");
            let b = assert_ok!(bool::read(&mut r_true).await);
            assert!(b);

            let mut r_bad = SliceReader::new(b"\x02");
            let res = bool::read(&mut r_bad).await;
            assert!(matches!(res, Err(DecodeError::MalformedPacket)));
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_eof() {
            let mut r = SliceReader::new(b"abcdefghijklmno");
            let e = assert_err!(<[u8; 16]>::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut r = SliceReader::new(b"");
            let e = assert_err!(u8::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut r = SliceReader::new(b"\x00");
            let e = assert_err!(u16::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut r = SliceReader::new(b"\x00\x00\x00");
            let e = assert_err!(u32::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);
        }
    }

    mod body_reader {
        use tokio_test::{assert_err, assert_ok};

        #[cfg(feature = "alloc")]
        use crate::buffer::AllocBuffer;
        #[cfg(feature = "bump")]
        use crate::buffer::BumpBuffer;

        use crate::{
            io::{
                err::{BodyReadError, DecodeError},
                read::{BodyReader, Readable},
            },
            test::read::SliceReader,
            types::{MqttBinary, MqttString},
        };

        #[tokio::test]
        #[test_log::test]
        async fn read_array() {
            let mut s = SliceReader::new(b"abcdefghijklmnopqrstuvwxyz");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 26);
            let a = assert_ok!(<[u8; 26]>::read(&mut r).await);
            assert_eq!(&a, b"abcdefghijklmnopqrstuvwxyz");
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u8() {
            let mut s = SliceReader::new(b"\x12");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let v = assert_ok!(u8::read(&mut r).await);
            assert_eq!(v, 0x12);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u16() {
            let mut s = SliceReader::new(b"\x01\x02");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let v = assert_ok!(u16::read(&mut r).await);
            assert_eq!(v, 0x0102);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_u32() {
            let mut s = SliceReader::new(b"\x01\x02\x03\x04");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 4);
            let v = assert_ok!(u32::read(&mut r).await);
            assert_eq!(v, 0x01020304);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_bool() {
            let mut s_false = SliceReader::new(b"\x00");
            #[cfg(feature = "alloc")]
            let mut b_false = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b_false = [0; 64];
            #[cfg(feature = "bump")]
            let mut b_false = BumpBuffer::new(&mut b_false);

            let mut r_false = BodyReader::new(&mut s_false, &mut b_false, 1);
            let v = assert_ok!(bool::read(&mut r_false).await);
            assert!(!v);

            let mut s_true = SliceReader::new(b"\x01");
            #[cfg(feature = "alloc")]
            let mut b_true = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b_true = [0; 64];
            #[cfg(feature = "bump")]
            let mut b_true = BumpBuffer::new(&mut b_true);

            let mut r_true = BodyReader::new(&mut s_true, &mut b_true, 1);
            let v = assert_ok!(bool::read(&mut r_true).await);
            assert!(v);

            let mut s_bad = SliceReader::new(b"\x02");
            #[cfg(feature = "alloc")]
            let mut b_bad = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b_bad = [0; 64];
            #[cfg(feature = "bump")]
            let mut b_bad = BumpBuffer::new(&mut b_bad);

            let mut r_bad = BodyReader::new(&mut s_bad, &mut b_bad, 1);
            let res = bool::read(&mut r_bad).await;
            assert!(matches!(res, Err(DecodeError::MalformedPacket)));
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_binary() {
            let mut s = SliceReader::new(&[0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0xFF]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 7);
            let v = assert_ok!(MqttBinary::read(&mut r).await);
            assert_eq!(v.as_ref(), &[0x01, 0x02, 0x03, 0x04, 0xFF]);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_string() {
            let mut s = SliceReader::new(&[
                0x00, 0x09, b'r', b'u', b's', b't', b'-', b'm', b'q', b't', b't',
            ]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 11);
            let v = assert_ok!(MqttString::read(&mut r).await);
            assert_eq!(v.as_ref(), "rust-mqtt");
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_stream() {
            #[rustfmt::skip]
            let mut s = SliceReader::new(
                &[
                    0x42,                                    // u8
                    0x01, 0x02,                              // u16
                    0x01,                                    // bool (true)
                    0xDE, 0xAD, 0xBE, 0xEF,                  // u32
                    0x00, 0x03, 0xAA, 0xBB, 0xCC,            // binary
                    0x00, 0x04, b't', b'e', b's', b't',      // string
                    0x11, 0x22, 0x33,                        // array[3]
                ]
            );
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 22);

            let v_u8 = assert_ok!(u8::read(&mut r).await);
            assert_eq!(v_u8, 0x42);

            let v_u16 = assert_ok!(u16::read(&mut r).await);
            assert_eq!(v_u16, 0x0102);

            let v_bool = assert_ok!(bool::read(&mut r).await);
            assert!(v_bool);

            let v_u32 = assert_ok!(u32::read(&mut r).await);
            assert_eq!(v_u32, 0xDEADBEEF);

            let v_binary = assert_ok!(MqttBinary::read(&mut r).await);
            assert_eq!(v_binary.as_ref(), &[0xAA, 0xBB, 0xCC]);

            let v_string = assert_ok!(MqttString::read(&mut r).await);
            assert_eq!(v_string.as_ref(), "test");

            let v_array = assert_ok!(<[u8; 3]>::read(&mut r).await);
            assert_eq!(v_array, [0x11, 0x22, 0x33]);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_eof() {
            let mut s = SliceReader::new(b"abcdefghijklmno");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 16);
            let e = assert_err!(<[u8; 16]>::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut s = SliceReader::new(b"");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let e = assert_err!(u8::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut s = SliceReader::new(b"\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let e = assert_err!(u16::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            let mut s = SliceReader::new(b"\x00\x00\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 4);
            let e = assert_err!(u32::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            // MqttBinary - EOF when reading length
            let mut s = SliceReader::new(b"\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let e = assert_err!(MqttBinary::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            // MqttBinary - EOF when reading data
            let mut s = SliceReader::new(&[0x00, 0x05, 0x01, 0x02]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 7);
            let e = assert_err!(MqttBinary::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            // MqttString - EOF when reading length
            let mut s = SliceReader::new(b"\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let e = assert_err!(MqttString::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);

            // MqttString - EOF when reading data
            let mut s = SliceReader::new(&[0x00, 0x04, b't', b'e']);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 6);
            let e = assert_err!(MqttString::read(&mut r).await);
            assert_eq!(e, DecodeError::UnexpectedEOF);
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_array() {
            let mut s = SliceReader::new(b"abcdefghijklmno");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 15);
            let e = assert_err!(<[u8; 16]>::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            let mut s = SliceReader::new(b"abcdefghijklmnop");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 15);
            let e = assert_err!(<[u8; 16]>::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }
        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_u8() {
            let mut s = SliceReader::new(b"");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 0);
            let e = assert_err!(u8::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            let mut s = SliceReader::new(b"");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 0);
            let e = assert_err!(u8::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }
        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_u16() {
            let mut s = SliceReader::new(b"\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let e = assert_err!(u16::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            let mut s = SliceReader::new(b"\x00\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let e = assert_err!(u16::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }
        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_u32() {
            let mut s = SliceReader::new(b"\x00\x00\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 3);
            let e = assert_err!(u32::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            let mut s = SliceReader::new(b"\x00\x00\x00\x00");
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 3);
            let e = assert_err!(u32::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_binary() {
            // Insufficient remaining length when reading length prefix
            let mut s = SliceReader::new(&[0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0xFF]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let e = assert_err!(MqttBinary::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            // Insufficient remaining length when reading data
            let mut s = SliceReader::new(&[0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0xFF]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 6);
            let e = assert_err!(MqttBinary::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            // Insufficient remaining length with exact length prefix bytes but not enough for data
            let mut s = SliceReader::new(&[0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0xFF]);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let e = assert_err!(MqttBinary::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }

        #[tokio::test]
        #[test_log::test]
        async fn read_insufficient_remaining_len_string() {
            // Insufficient remaining length when reading length prefix
            let mut s = SliceReader::new(&[0x00, 0x04, b't', b'e', b's', b't']);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 1);
            let e = assert_err!(MqttString::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            // Insufficient remaining length when reading data
            let mut s = SliceReader::new(&[0x00, 0x04, b't', b'e', b's', b't']);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 5);
            let e = assert_err!(MqttString::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));

            // Insufficient remaining length with exact length prefix bytes but not enough for data
            let mut s = SliceReader::new(&[0x00, 0x04, b't', b'e', b's', b't']);
            #[cfg(feature = "alloc")]
            let mut b = AllocBuffer;
            #[cfg(feature = "bump")]
            let mut b = [0; 64];
            #[cfg(feature = "bump")]
            let mut b = BumpBuffer::new(&mut b);

            let mut r = BodyReader::new(&mut s, &mut b, 2);
            let e = assert_err!(MqttString::read(&mut r).await);
            assert_eq!(e, DecodeError::Read(BodyReadError::InsufficientRemainingLen));
        }
    }
}

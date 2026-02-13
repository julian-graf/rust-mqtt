use core::hint::unreachable_unchecked;

use crate::{
    bytes::Bytes,
    eio::Read,
    fmt::trace,
    io::err::ReadError,
    types::{MqttBinary, MqttString, TopicName, VarByteInt},
};

pub trait Readable<R: Read>: Sized {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>>;
}

pub trait Store<'a>: Read {
    async fn read_and_store(&mut self, len: usize) -> Result<Bytes<'a>, ReadError<Self::Error>>;
}

impl<R: Read, const N: usize> Readable<R> for [u8; N] {
    async fn read(read: &mut R) -> Result<Self, ReadError<<R>::Error>> {
        trace!("reading array of {} bytes", N);

        let mut array = [0; N];
        let mut slice = &mut array[..];
        while !slice.is_empty() {
            match read.read(slice).await.map_err(ReadError::Read)? {
                0 => return Err(ReadError::UnexpectedEOF),
                n => slice = &mut slice[n..],
            }
        }
        Ok(array)
    }
}
impl<R: Read> Readable<R> for u8 {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        <[u8; 1]>::read(read).await.map(Self::from_be_bytes)
    }
}
impl<R: Read> Readable<R> for u16 {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        <[u8; 2]>::read(read).await.map(Self::from_be_bytes)
    }
}
impl<R: Read> Readable<R> for u32 {
    async fn read(read: &mut R) -> Result<Self, ReadError<<R>::Error>> {
        <[u8; 4]>::read(read).await.map(Self::from_be_bytes)
    }
}
impl<R: Read> Readable<R> for bool {
    async fn read(read: &mut R) -> Result<Self, ReadError<<R>::Error>> {
        match u8::read(read).await? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ReadError::MalformedPacket),
        }
    }
}
impl<R: Read> Readable<R> for VarByteInt {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        let mut i = 0;
        let mut buffer = [0; 4];

        loop {
            match read.read(&mut buffer[i..(i + 1)]).await {
                Ok(0) => return Err(ReadError::UnexpectedEOF),
                Ok(1) => {}
                Ok(_) => unsafe { unreachable_unchecked() },
                Err(e) => return Err(ReadError::Read(e)),
            }

            let is_continuation_byte = buffer[i] >= 128;
            if is_continuation_byte {
                if i == 3 {
                    return Err(ReadError::MalformedPacket);
                }
                i += 1;
            } else {
                let slice = &buffer[0..=i];

                // Invariant: We checked that the slice is within the valid length range and
                // that the last byte matches the end condition of the variable byte integer encoding
                break Ok(VarByteInt::from_slice_unchecked(slice));
            }
        }
    }
}
impl<'b, R: Read + Store<'b>> Readable<R> for MqttBinary<'b> {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        let len = u16::read(read).await? as usize;

        trace!("reading slice of {} bytes", len);

        Ok(MqttBinary(read.read_and_store(len).await?))
    }
}
impl<'s, R: Read + Store<'s>> Readable<R> for MqttString<'s> {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        MqttBinary::read(read)
            .await?
            .try_into()
            .map_err(|_| ReadError::MalformedPacket)
    }
}
impl<'s, R: Read + Store<'s>> Readable<R> for TopicName<'s> {
    async fn read(read: &mut R) -> Result<Self, ReadError<R::Error>> {
        let str = MqttString::read(read).await?;

        TopicName::new(str).ok_or(ReadError::InvalidTopicName)
    }
}

#[cfg(test)]
mod unit {
    use tokio_test::{assert_err, assert_ok};

    use crate::{
        io::err::ReadError, io::read::Readable, test::read::SliceReader, types::VarByteInt,
    };

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
        assert!(matches!(res, Err(ReadError::MalformedPacket)));
    }

    #[tokio::test]
    #[test_log::test]
    async fn read_var_byte_int() {
        let mut r = SliceReader::new(&[0x00]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 0);

        let mut r = SliceReader::new(&[0x7F]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 127);

        let mut r = SliceReader::new(&[0x80, 0x01]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 128);

        let mut r = SliceReader::new(&[0xFF, 0x7F]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 16_383);

        let mut r = SliceReader::new(&[0x80, 0x80, 0x01]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 16_384);

        let mut r = SliceReader::new(&[0xFF, 0xFF, 0x7F]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 2_097_151);

        let mut r = SliceReader::new(&[0x80, 0x80, 0x80, 0x01]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), 2_097_152);

        let mut r = SliceReader::new(&[0xFF, 0xFF, 0xFF, 0x7F]);
        let v = assert_ok!(VarByteInt::read(&mut r).await);
        assert_eq!(v.value(), VarByteInt::MAX_ENCODABLE);

        let mut r = SliceReader::new(&[0x80, 0x80, 0x80, 0x80, 0x01]);
        let res = VarByteInt::read(&mut r).await;
        assert!(matches!(res, Err(ReadError::MalformedPacket)));

        let mut r = SliceReader::new(&[0x80, 0x80, 0x80, 0x80]);
        let res = VarByteInt::read(&mut r).await;
        assert!(matches!(res, Err(ReadError::MalformedPacket)));
    }

    #[tokio::test]
    #[test_log::test]
    async fn read_eof() {
        let mut r = SliceReader::new(b"abcdefghijklmno");
        let e = assert_err!(<[u8; 16]>::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);

        let mut r = SliceReader::new(b"");
        let e = assert_err!(u8::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);

        let mut r = SliceReader::new(b"\x00");
        let e = assert_err!(u16::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);

        let mut r = SliceReader::new(b"\x00\x00\x00");
        let e = assert_err!(u32::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);

        let mut r = SliceReader::new(&[]);
        let e = assert_err!(VarByteInt::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);

        let mut r = SliceReader::new(&[0x80]);
        let e = assert_err!(VarByteInt::read(&mut r).await);
        assert_eq!(e, ReadError::UnexpectedEOF);
    }
}

use core::hint::unreachable_unchecked;

use crate::{
    eio::{Error, ErrorKind, Read},
    fmt::trace,
    header::{FixedHeader, PacketType},
    types::VarByteInt,
};

#[derive(Debug, Clone, Copy)]
pub enum ReaderError {
    Read(ErrorKind),
    EOF,
    InvalidPacketType,
    InvalidVarByteInt,
    BufferExceeded,
}

impl<E: Error> From<E> for ReaderError {
    fn from(e: E) -> Self {
        Self::Read(e.kind())
    }
}

#[derive(Debug)]
pub struct PacketReceiver<'b> {
    header: Option<FixedHeader>,
    buf: &'b mut [u8],
    initialized: usize,
}

impl<'b> PacketReceiver<'b> {
    pub fn new(buf: &'b mut [u8]) -> Self {
        Self {
            header: None,
            buf,
            initialized: 0,
        }
    }

    pub fn into_decoder(&mut self, token: PacketDecodeToken) -> PacketDecoder {
        let PacketDecodeToken(header) = token;

        debug_assert_eq!(header, self.header.unwrap());
        debug_assert_eq!(header.remaining_len.size(), self.initialized);

        let buf = &self.buf[..self.initialized];

        PacketDecoder::new(token, buf)
    }

    pub async fn poll<R: Read>(&mut self, read: &mut R) -> Result<PacketDecodeToken, ReaderError> {
        if let None = self.header {
            self.try_read_header(read).await?;
        }

        let header = unsafe { self.header.unwrap_unchecked() };
        let remaining_len = header.remaining_len.size();

        if remaining_len > self.buf.len() {
            return Err(ReaderError::BufferExceeded);
        }

        while self.initialized != remaining_len {
            let buf = &mut self.buf[self.initialized..remaining_len];

            let read = read.read(buf).await?;
            self.initialized += read;
            if read == 0 {
                return Err(ReaderError::EOF);
            }
        }

        Ok(PacketDecodeToken::new(header))
    }

    async fn try_read_header<R: Read>(&mut self, r: &mut R) -> Result<(), ReaderError> {
        loop {
            let i = self.initialized as usize;
            if i > 4 {
                // Safety: `self.read` gets reset to 0 when reaching 5
                unsafe { unreachable_unchecked() }
            }

            // Since i is in the 0..=4 range, we can safely index into `self.buffer`

            trace!("receiving byte {} of header", i);

            let read = r
                .read(&mut self.buf[i..(i + 1)])
                .await
                .map_err(|e| e.kind())
                .map_err(ReaderError::Read)? as u8;

            match read {
                0 => return Err(ReaderError::EOF),
                1 => self.initialized += 1,
                // Safety: `Read` can never return a value greater then the length of the slice.
                _ => unsafe { unreachable_unchecked() },
            }

            trace!("received {} header byte(s) in total", self.initialized);

            let byte = self.buf[i];

            if i == 0 {
                if PacketType::from_type_and_flags(byte).is_err() {
                    self.initialized = 0;
                    return Err(ReaderError::InvalidPacketType);
                } else {
                    continue;
                };
            }

            let is_continuation_byte = byte >= 128;

            if is_continuation_byte {
                if i == 4 {
                    self.initialized = 0;
                    return Err(ReaderError::InvalidVarByteInt);
                } else {
                    continue;
                }
            } else {
                let slice = &self.buf[1..=i];

                // Safety: We checked that the slice is within the valid range and
                // that the last byte matches the end condition of the variable byte integer encoding
                let remaining_len = unsafe { VarByteInt::from_slice_unchecked(slice) };

                self.initialized = 0;
                self.header = Some(FixedHeader {
                    type_and_flags: self.buf[0],
                    remaining_len,
                });
            }
        }
    }
}

pub struct PacketDecodeToken(FixedHeader);

impl PacketDecodeToken {
    pub fn new(header: FixedHeader) -> Self {
        Self(header)
    }
    pub fn header(&self) -> &FixedHeader {
        &self.0
    }
}

pub struct PacketDecoder<'d> {
    header: FixedHeader,
    buf: &'d [u8],
    pos: usize,
}

pub struct InsufficientLen;

impl<'d> PacketDecoder<'d> {
    pub fn new(token: PacketDecodeToken, buf: &'d [u8]) -> Self {
        let PacketDecodeToken(header) = token;

        Self {
            header,
            buf,
            pos: 0,
        }
    }

    pub fn header(&self) -> &FixedHeader {
        &self.header
    }

    pub fn remaining_len(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn skip(&mut self, len: usize) -> Result<(), InsufficientLen> {
        let end = self.pos + len;

        if end > self.buf.len() {
            Err(InsufficientLen)
        } else {
            self.pos += len;
            Ok(())
        }
    }

    pub fn take_bytes(&mut self, len: usize) -> Result<&'d [u8], InsufficientLen> {
        let end = self.pos + len;

        if end > self.buf.len() {
            Err(InsufficientLen)
        } else {
            let bytes = &self.buf[self.pos..end];
            self.pos = end;
            Ok(bytes)
        }
    }
}

use core::cmp;
use embedded_io::ErrorType;
use embedded_io_async::{Read, Write};

pub struct BufferedWriter<'b, R> {
    inner: R,
    buffer: &'b mut [u8],
    pos: usize,
}

impl<'b, R> BufferedWriter<'b, R> {
    pub fn new(inner: R, buffer: &'b mut [u8]) -> Self {
        Self {
            inner,
            buffer,
            pos: 0,
        }
    }
}

impl<'b, R: ErrorType> ErrorType for BufferedWriter<'b, R> {
    type Error = R::Error;
}

impl<'b, R: Read + Unpin> Read for BufferedWriter<'b, R> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.inner.read(buf).await
    }
}

impl<'b, R: Write + Unpin> Write for BufferedWriter<'b, R> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let available = self.buffer.len() - self.pos;
        let to_write = cmp::min(available, buf.len());

        if to_write > 0 {
            self.buffer[self.pos..self.pos + to_write].copy_from_slice(&buf[..to_write]);
            self.pos += to_write;
        }

        if self.pos == self.buffer.len() {
            self.inner.write_all(&self.buffer[..self.pos]).await?;
            self.pos = 0;
        }

        Ok(to_write)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        if self.pos > 0 {
            self.inner.write_all(&self.buffer[..self.pos]).await?;
            self.pos = 0;
        }
        self.inner.flush().await
    }
}

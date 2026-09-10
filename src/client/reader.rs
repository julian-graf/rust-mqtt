use core::cmp::min;

use crate::{
    buffer::BufferProvider,
    client::{Client, raw::RawError},
    eio::{ErrorKind, ErrorType, Read},
    fmt::{assert_eq, unreachable},
    io::Transport,
};

pub struct ApplicationMessageReader<
    'a,
    'c,
    'r,
    N: Transport,
    B: BufferProvider<'c>,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
    const MAX_SUBSCRIPTION_IDENTIFIERS: usize,
    const MAX_USER_PROPERTIES: usize,
> {
    client: &'r mut Client<
        'a,
        'c,
        N,
        B,
        SUBSCRIBE_MAXIMUM,
        RECEIVE_MAXIMUM,
        SEND_MAXIMUM,
        MAX_SUBSCRIPTION_IDENTIFIERS,
        MAX_USER_PROPERTIES,
    >,
    remaining_message_len: usize,
}

impl<
    'a,
    'c,
    'r,
    N: Transport,
    B: BufferProvider<'c>,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
    const MAX_SUBSCRIPTION_IDENTIFIERS: usize,
    const MAX_USER_PROPERTIES: usize,
>
    ApplicationMessageReader<
        'a,
        'c,
        'r,
        N,
        B,
        SUBSCRIBE_MAXIMUM,
        RECEIVE_MAXIMUM,
        SEND_MAXIMUM,
        MAX_SUBSCRIPTION_IDENTIFIERS,
        MAX_USER_PROPERTIES,
    >
{
    pub(crate) fn new(
        client: &'r mut Client<
            'a,
            'c,
            N,
            B,
            SUBSCRIBE_MAXIMUM,
            RECEIVE_MAXIMUM,
            SEND_MAXIMUM,
            MAX_SUBSCRIPTION_IDENTIFIERS,
            MAX_USER_PROPERTIES,
        >,
        message_len: usize,
    ) -> Self {
        Self {
            client,
            remaining_message_len: message_len,
        }
    }

    /// Returns the remaining number of bytes in the application message.
    pub fn remaining_len(&self) -> usize {
        self.remaining_message_len
    }
}

impl<
    'a,
    'c,
    'r,
    N: Transport,
    B: BufferProvider<'c>,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
    const MAX_SUBSCRIPTION_IDENTIFIERS: usize,
    const MAX_USER_PROPERTIES: usize,
> Drop
    for ApplicationMessageReader<
        'a,
        'c,
        'r,
        N,
        B,
        SUBSCRIBE_MAXIMUM,
        RECEIVE_MAXIMUM,
        SEND_MAXIMUM,
        MAX_SUBSCRIPTION_IDENTIFIERS,
        MAX_USER_PROPERTIES,
    >
{
    fn drop(&mut self) {
        // If we had an std feature, we could check std::thread::panicking
        // and only cause a panic if we aren't panicking yet. Otherwise this
        // pretty much guarantees an abort.
        // In embedded, aborting on panic is quite common though, so this is
        // acceptable for now.
        assert_eq!(
            self.remaining_message_len, 0,
            "the complete payload of a PUBLISH packet must be read"
        )
    }
}

impl<
    'a,
    'c,
    'r,
    N: Transport,
    B: BufferProvider<'c>,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
    const MAX_SUBSCRIPTION_IDENTIFIERS: usize,
    const MAX_USER_PROPERTIES: usize,
> ErrorType
    for ApplicationMessageReader<
        'a,
        'c,
        'r,
        N,
        B,
        SUBSCRIBE_MAXIMUM,
        RECEIVE_MAXIMUM,
        SEND_MAXIMUM,
        MAX_SUBSCRIPTION_IDENTIFIERS,
        MAX_USER_PROPERTIES,
    >
{
    type Error = ErrorKind;
}
impl<
    'a,
    'c,
    'r,
    N: Transport,
    B: BufferProvider<'c>,
    const SUBSCRIBE_MAXIMUM: usize,
    const RECEIVE_MAXIMUM: usize,
    const SEND_MAXIMUM: usize,
    const MAX_SUBSCRIPTION_IDENTIFIERS: usize,
    const MAX_USER_PROPERTIES: usize,
> Read
    for ApplicationMessageReader<
        'a,
        'c,
        'r,
        N,
        B,
        SUBSCRIBE_MAXIMUM,
        RECEIVE_MAXIMUM,
        SEND_MAXIMUM,
        MAX_SUBSCRIPTION_IDENTIFIERS,
        MAX_USER_PROPERTIES,
    >
{
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let len = min(buf.len(), self.remaining_message_len);
        let buf = &mut buf[..len];

        self.client
            .poll_raw(buf)
            .await
            .map_err(|e| match e {
                RawError::Network(e) => e,
                RawError::Disconnected => ErrorKind::BrokenPipe,
                RawError::Server => unreachable!(),
            })
            .inspect(|i| self.remaining_message_len = self.remaining_message_len.strict_sub(*i))
    }
}

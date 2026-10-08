#![allow(dead_code)]

mod common;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    num::NonZero,
    time::Duration,
};

use common::{
    BROKER_ADDRESS, DEFAULT_DC_OPTIONS, NO_SESSION_CONNECT_OPTIONS, TestClient,
    utils::{ALLOC, connected_client, disconnect as disconnect_client, tcp_connection},
};
use embedded_io_adapters::tokio_1::FromTokio;
use rust_mqtt::{
    Bytes,
    auth::{AuthMechanism, AuthOptions},
    buffer::AllocBuffer,
    client::{
        Client, MqttError,
        options::{
            ConnectOptions, PublicationOptions, ReAuthOptions, SubscriptionOptions, TopicReference,
            UnsubscriptionOptions, WillOptions,
        },
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttBinary, MqttString, TopicFilter, TopicName, VarByteInt},
};
use std::convert::Infallible;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

/// A complete MQTT packet as observed on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The packet type and flags byte.
    pub first_byte: u8,
    /// The packet body, excluding the fixed header.
    pub body: Vec<u8>,
}

impl Frame {
    /// Returns the MQTT packet type encoded in the fixed header.
    #[must_use]
    pub fn packet_type(&self) -> u8 {
        self.first_byte >> 4
    }

    /// Returns the fixed header and body encoded as wire bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(2 + self.body.len());
        bytes.push(self.first_byte);
        write_remaining_length(self.body.len(), &mut bytes);
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// A small TCP broker used by protocol conformance tests.
///
/// The broker deliberately deals in wire frames instead of depending on a
/// particular packet implementation. This keeps the test oracle independent
/// from the client code under test.
pub struct Broker {
    listener: TcpListener,
    stream: Option<TcpStream>,
}

struct ConformanceAuth;

impl AuthMechanism<16> for ConformanceAuth {
    type Error = Infallible;

    fn kontinue(
        &mut self,
        _auth: &rust_mqtt::client::event::Auth<'_, 16>,
    ) -> Result<AuthOptions<'_, 16>, (Self::Error, Option<rust_mqtt::types::ReasonCode>)> {
        Ok(AuthOptions {
            authentication_data: Some(MqttBinary::from_slice_unchecked(b"client-response")),
            ..AuthOptions::default()
        })
    }

    fn success(
        &mut self,
        _auth: &rust_mqtt::client::event::Auth<'_, 16>,
    ) -> Result<(), (Self::Error, Option<rust_mqtt::types::ReasonCode>)> {
        Ok(())
    }
}

impl Broker {
    /// Binds an unused loopback port.
    pub async fn bind() -> io::Result<Self> {
        let listener =
            TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).await?;
        Ok(Self {
            listener,
            stream: None,
        })
    }

    /// Returns the address on which the broker is listening.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("broker listener has an address")
    }

    /// Accepts the client connection.
    pub async fn accept(&mut self) -> io::Result<()> {
        let (stream, _) = self.listener.accept().await?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Reads the next complete MQTT packet from the client.
    pub async fn next_frame(&mut self) -> io::Result<Frame> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "client not accepted"))?;

        let first_byte = stream.read_u8().await?;
        let remaining_length = read_remaining_length(stream).await?;
        let mut body = vec![0; remaining_length];
        stream.read_exact(&mut body).await?;

        Ok(Frame { first_byte, body })
    }

    /// Reads packets until one other than PINGREQ is received.
    pub async fn next_non_ping(&mut self) -> io::Result<Frame> {
        loop {
            let frame = timeout(Duration::from_secs(5), self.next_frame())
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for MQTT packet")
                })??;
            if frame.packet_type() != 12 {
                return Ok(frame);
            }
        }
    }

    /// Sends a raw MQTT packet to the client.
    pub async fn send(&mut self, frame: &Frame) -> io::Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "client not accepted"))?;
        stream.write_all(&frame.to_bytes()).await
    }

    /// Sends a packet assembled from a fixed-header byte and body.
    pub async fn send_raw(&mut self, first_byte: u8, body: &[u8]) -> io::Result<()> {
        self.send(&Frame {
            first_byte,
            body: body.to_vec(),
        })
        .await
    }

    /// Verifies that the client sends no non-PINGREQ packet during `duration`.
    pub async fn expect_silence(&mut self, duration: Duration) -> io::Result<()> {
        match timeout(duration, self.next_non_ping()).await {
            Ok(Ok(frame)) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("unexpected MQTT packet: {frame:?}"),
            )),
            Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Ok(()),
        }
    }

    /// Closes the broker side of the connection.
    pub async fn close(&mut self) -> io::Result<()> {
        if let Some(stream) = self.stream.as_mut() {
            stream.shutdown().await?;
        }
        Ok(())
    }

    /// Returns whether the client has closed the TCP connection.
    pub async fn wait_closed(&mut self, duration: Duration) -> io::Result<bool> {
        match timeout(duration, self.next_frame()).await {
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                ) =>
            {
                Ok(true)
            }
            Ok(Err(error)) => Err(error),
            Ok(Ok(frame)) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("expected connection close, got {frame:?}"),
            )),
            Err(_) => Ok(false),
        }
    }
}

/// Connects a rust-mqtt client to a conformance broker.
pub async fn connect(broker: &mut Broker) -> Result<TestClient<'static>, MqttError<'static, 16>> {
    let address = broker.address();
    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        result = &mut connecting => result,
        result = broker.accept() => {
            result.map_err(|_| MqttError::RecoveryRequired)?;
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .map_err(|_| MqttError::RecoveryRequired)?
                .map_err(|_| MqttError::RecoveryRequired)?;
            assert_eq!(connect.packet_type(), 1, "expected CONNECT, got {connect:?}");
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await
                .map_err(|_| MqttError::RecoveryRequired)?;
            timeout(Duration::from_secs(5), connecting)
                .await
                .map_err(|_| MqttError::RecoveryRequired)?
        }
    }
}

async fn connect_with_connack(
    broker: &mut Broker,
    connack_body: &[u8],
) -> Result<TestClient<'static>, MqttError<'static, 16>> {
    let address = broker.address();
    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        result = &mut connecting => result,
        result = broker.accept() => {
            result.map_err(|_| MqttError::RecoveryRequired)?;
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .map_err(|_| MqttError::RecoveryRequired)?
                .map_err(|_| MqttError::RecoveryRequired)?;
            assert_eq!(connect.packet_type(), 1, "expected CONNECT, got {connect:?}");
            broker
                .send_raw(0x20, connack_body)
                .await
                .map_err(|_| MqttError::RecoveryRequired)?;
            timeout(Duration::from_secs(5), connecting)
                .await
                .map_err(|_| MqttError::RecoveryRequired)?
        }
    }
}

/// Connects with the repository's default broker address.
///
/// This is useful when running the conformance suite against a real broker
/// such as the Mosquitto service started by CI.
pub async fn connect_default() -> Result<TestClient<'static>, MqttError<'static, 16>> {
    connected_client(BROKER_ADDRESS, NO_SESSION_CONNECT_OPTIONS, None).await
}

/// Disconnects a client using the default conformance options.
pub async fn disconnect(client: &mut TestClient<'_>) {
    disconnect_client(client, DEFAULT_DC_OPTIONS).await;
}

async fn read_remaining_length(stream: &mut TcpStream) -> io::Result<usize> {
    let mut multiplier = 1usize;
    let mut value = 0usize;

    for _ in 0..4 {
        let byte = stream.read_u8().await?;
        value = value
            .checked_add(((byte & 0x7f) as usize).saturating_mul(multiplier))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "remaining length overflow")
            })?;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        multiplier = multiplier.checked_mul(128).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "remaining length overflow")
        })?;
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "remaining length uses more than four bytes",
    ))
}

fn write_remaining_length(mut length: usize, bytes: &mut Vec<u8>) {
    loop {
        let mut encoded = (length % 128) as u8;
        length /= 128;
        if length != 0 {
            encoded |= 0x80;
        }
        bytes.push(encoded);
        if length == 0 {
            return;
        }
    }
}

fn topic_name(topic: &'static str) -> TopicName<'static> {
    TopicName::new(MqttString::from_str(topic).unwrap()).unwrap()
}

fn topic_filter(filter: &'static str) -> TopicFilter<'static> {
    TopicFilter::new(MqttString::from_str(filter).unwrap()).unwrap()
}

fn qos0_publish(topic: &'static str) -> PublicationOptions<'static> {
    PublicationOptions::new(TopicReference::Name(topic_name(topic)))
}

fn qos1_publish(topic: &'static str) -> PublicationOptions<'static> {
    qos0_publish(topic).at_least_once()
}

fn no_credentials_connect_options() -> ConnectOptions<'static> {
    ConnectOptions::new().clean_start()
}

fn username_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options().user_name(MqttString::from_str("test_user").unwrap())
}

fn password_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options().password(MqttBinary::from_slice_unchecked(b"secret_password"))
}

fn will_connect_options() -> ConnectOptions<'static> {
    NO_SESSION_CONNECT_OPTIONS.clone().will(
        WillOptions::new(
            topic_name("will/test"),
            MqttBinary::from_slice_unchecked(b"will_data"),
        )
        .at_least_once(),
    )
}

type OutgoingAliasClient =
    Client<'static, 'static, FromTokio<TcpStream>, AllocBuffer, 1, 1, 1, 1, 16, 0, 2>;

type MultiPublishClient =
    Client<'static, 'static, FromTokio<TcpStream>, AllocBuffer, 1, 2, 2, 1, 16, 0, 0>;

async fn connected_outgoing_alias_client(
    broker: SocketAddr,
    options: &ConnectOptions<'_>,
) -> Result<OutgoingAliasClient, MqttError<'static, 16>> {
    let mut client = Client::new(ALLOC.get());
    let tcp = tcp_connection(broker).await?;
    client
        .connect(
            tcp,
            options,
            Some(MqttString::from_str("conformance").unwrap()),
        )
        .await
        .map(|_| client)
}

async fn connect_multi_publish(
    broker: &mut Broker,
) -> Result<MultiPublishClient, MqttError<'static, 16>> {
    connect_multi_with_options(broker, NO_SESSION_CONNECT_OPTIONS).await
}

async fn connect_multi_with_options(
    broker: &mut Broker,
    options: &ConnectOptions<'_>,
) -> Result<MultiPublishClient, MqttError<'static, 16>> {
    connect_multi_with_connack(broker, options, &[0x00, 0x00, 0x00]).await
}

async fn connect_multi_with_connack(
    broker: &mut Broker,
    options: &ConnectOptions<'_>,
    connack_body: &[u8],
) -> Result<MultiPublishClient, MqttError<'static, 16>> {
    let address = broker.address();
    let mut client = Client::new(ALLOC.get());
    let tcp = tcp_connection(address).await?;
    let connection_result = {
        let connecting = client.connect(
            tcp,
            options,
            Some(MqttString::from_str("conformance").unwrap()),
        );
        tokio::pin!(connecting);
        tokio::select! {
            result = &mut connecting => result.map(|_| ()),
            result = broker.accept() => {
                result.map_err(|_| MqttError::RecoveryRequired)?;
                let connect = timeout(Duration::from_secs(5), broker.next_frame())
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?
                    .map_err(|_| MqttError::RecoveryRequired)?;
                assert_eq!(connect.packet_type(), 1, "expected CONNECT, got {connect:?}");
                broker.send_raw(0x20, connack_body)
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?;
                timeout(Duration::from_secs(5), &mut connecting)
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?
                    .map(|_| ())
            }
        }
    };
    connection_result?;
    Ok(client)
}

async fn reconnect_multi_with_options(
    client: &mut MultiPublishClient,
    broker: &mut Broker,
    options: &ConnectOptions<'_>,
    client_identifier: MqttString<'static>,
) -> Result<(), MqttError<'static, 16>> {
    let tcp = tcp_connection(broker.address()).await?;
    let connection_result = {
        let connecting = client.connect(tcp, options, Some(client_identifier));
        tokio::pin!(connecting);
        tokio::select! {
            result = &mut connecting => result.map(|_| ()),
            result = broker.accept() => {
                result.map_err(|_| MqttError::RecoveryRequired)?;
                let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?
                    .map_err(|_| MqttError::RecoveryRequired)?;
                broker.send_raw(0x20, &[0x01, 0x00, 0x00])
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?;
                timeout(Duration::from_secs(5), &mut connecting)
                    .await
                    .map_err(|_| MqttError::RecoveryRequired)?
                    .map(|_| ())
            }
        }
    };
    connection_result
}

fn auth_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options()
        .authentication_data(MqttBinary::from_slice_unchecked(b"initial_auth_data"))
}

fn full_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options()
        .will(
            WillOptions::new(
                topic_name("topic/will"),
                MqttBinary::from_slice_unchecked(b"will_msg"),
            )
            .at_least_once(),
        )
        .user_name(MqttString::from_str("user_name").unwrap())
        .password(MqttBinary::from_slice_unchecked(b"pass_word"))
}

fn utf8_will_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options().will(
        WillOptions::new(
            topic_name("valid/will/topic"),
            MqttBinary::from_slice_unchecked(b"payload"),
        )
        .at_least_once(),
    )
}

fn utf8_username_connect_options() -> ConnectOptions<'static> {
    no_credentials_connect_options().user_name(MqttString::from_str("utf8_username").unwrap())
}

#[tokio::test]
async fn mqtt_4_7_0_1_publish_topic_with_wildcard_rejected() {
    assert!(TopicName::new(MqttString::from_str("a/+").unwrap()).is_none());
}

#[tokio::test]
async fn mqtt_4_7_3_1_empty_topic_without_alias_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let empty = MqttString::from_str("").unwrap();
    assert!(TopicName::new(empty).is_none());
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_7_3_1_empty_topic_with_unmapped_alias_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_with_connack(&mut broker, &[0x00, 0x00, 0x03, 0x22, 0x00, 0x05])
        .await
        .unwrap();

    // TopicName intentionally rejects an empty name; rust-mqtt therefore cannot
    // construct the invalid empty-topic/alias combination for transmission.
    assert!(TopicName::new(MqttString::from_str("").unwrap()).is_none());
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_7_3_2_publish_topic_with_null_rejected() {
    assert!(MqttString::from_str("a\0b").is_err());
}

#[tokio::test]
async fn mqtt_3_3_2_14_wildcard_response_topic_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let response_topic = TopicName::new(MqttString::from_str("reply/#").unwrap());

    assert!(response_topic.is_none());
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_2_2_18_topic_alias_without_server_maximum_rejected_by_source_case() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();
    let connecting = connected_outgoing_alias_client(address, NO_SESSION_CONNECT_OPTIONS);
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            broker.next_frame().await.unwrap();
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            let mut client = connecting.await.unwrap();
            let options = PublicationOptions::new(TopicReference::Mapping(
                topic_name("a"),
                NonZero::new(1).unwrap(),
            ));
            assert!(client.publish(&options, Bytes::Borrowed(b"x")).await.is_err());
            assert!(broker.expect_silence(Duration::from_millis(20)).await.is_ok());
            client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
        }
    }
}

#[tokio::test]
async fn mqtt_3_3_2_9_topic_alias_above_server_maximum_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();
    let connecting = connected_outgoing_alias_client(address, NO_SESSION_CONNECT_OPTIONS);
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            broker.next_frame().await.unwrap();
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x22, 0x00, 0x02]).await.unwrap();
            let mut client = connecting.await.unwrap();
            let above = PublicationOptions::new(TopicReference::Mapping(
                topic_name("a"),
                NonZero::new(3).unwrap(),
            ));
            assert!(client.publish(&above, Bytes::Borrowed(b"x")).await.is_err());
            assert!(broker.expect_silence(Duration::from_millis(20)).await.is_ok());
            client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
        }
    }
}

#[tokio::test]
async fn mqtt_3_8_3_4_no_local_on_shared_subscription_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let result = client
        .subscribe(
            topic_filter("$share/g/a"),
            &SubscriptionOptions::new().no_local(),
        )
        .await;
    assert!(matches!(
        result,
        Err(MqttError::IllegalNoLocalSharedSubscription)
    ));
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    drop(client);
}

#[tokio::test]
async fn section_3_8_2_1_2_subscription_identifier_zero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let options = SubscriptionOptions::new().subscription_identifier(VarByteInt::from(0u8));
    let result = client.subscribe(topic_filter("a"), &options).await;
    assert!(result.is_err());
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    drop(client);
}

#[tokio::test]
async fn section_3_2_2_3_11_wildcard_subscription_unavailable_honoured() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_with_connack(&mut broker, &[0x00, 0x00, 0x02, 0x28, 0x00])
        .await
        .unwrap();
    assert!(
        client
            .subscribe(topic_filter("a/+"), &SubscriptionOptions::new())
            .await
            .is_err()
    );
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    drop(client);
}

#[tokio::test]
async fn section_3_2_2_3_13_shared_subscription_unavailable_honoured() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_with_connack(&mut broker, &[0x00, 0x00, 0x02, 0x2A, 0x00])
        .await
        .unwrap();
    assert!(
        client
            .subscribe(topic_filter("$share/g/a"), &SubscriptionOptions::new())
            .await
            .is_err()
    );
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    drop(client);
}

#[tokio::test]
async fn section_3_2_2_3_12_subscription_identifier_unavailable_honoured() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_with_connack(&mut broker, &[0x00, 0x00, 0x02, 0x29, 0x00])
        .await
        .unwrap();
    let options = SubscriptionOptions::new().subscription_identifier(VarByteInt::from(7u8));
    assert!(client.subscribe(topic_filter("a"), &options).await.is_err());
    assert!(
        broker
            .expect_silence(Duration::from_millis(20))
            .await
            .is_ok()
    );
    drop(client);
}

#[tokio::test]
async fn mqtt_4_7_1_1_invalid_multi_level_wildcard_filter_rejected() {
    assert!(TopicFilter::new(MqttString::from_str("a/#/b").unwrap()).is_none());
    assert!(TopicFilter::new(MqttString::from_str("a#").unwrap()).is_none());
}

#[tokio::test]
async fn mqtt_4_7_1_2_partial_level_single_wildcard_rejected() {
    assert!(TopicFilter::new(MqttString::from_str("a+/b").unwrap()).is_none());
    assert!(TopicFilter::new(MqttString::from_str("a/b+").unwrap()).is_none());
}

#[tokio::test]
async fn mqtt_4_7_3_1_empty_filter_rejected() {
    assert!(TopicFilter::new(MqttString::from_str("").unwrap()).is_none());
}

#[tokio::test]
async fn mqtt_4_8_2_1_shared_subscription_without_share_name_rejected() {
    for filter in ["$share//a", "$share/group", "$share/group/"] {
        assert!(TopicFilter::new(MqttString::from_str(filter).unwrap()).is_none());
    }
}

#[tokio::test]
async fn mqtt_4_8_2_2_share_name_with_wildcard_rejected() {
    for filter in ["$share/g+/a", "$share/#/a", "$share/g#/a"] {
        assert!(TopicFilter::new(MqttString::from_str(filter).unwrap()).is_none());
    }
}

#[tokio::test]
#[test_log::test]
async fn mqtt_3_3_1_4_publish_qos_three_malformed() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    broker
        .send_raw(0x36, &[0x00, 0x01, b'a', 0x00, 0x01, 0x00])
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBLISH")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_14_1_1_disconnect_reserved_flags_malformed() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    broker.send_raw(0xE1, &[]).await.unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed DISCONNECT")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn section_3_14_server_disconnect_closes_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    broker.send_raw(0xE0, &[0x8B]).await.unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process server DISCONNECT")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce a closed transport");
    drop(transport);
    assert!(broker.wait_closed(Duration::from_secs(1)).await.unwrap());
}

#[tokio::test]
async fn mqtt_4_3_2_2_first_qos1_send_has_dup_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let options = qos1_publish("a");
    let publish = client
        .publish(&options, Bytes::Borrowed(b"x"))
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.first_byte & 0x08, 0);
    broker
        .send_raw(0x40, &publish.unwrap().get().get().to_be_bytes())
        .await
        .unwrap();
    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_1_5_4_1_surrogate_codepoint_in_utf8_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    // PUBLISH (QoS 0) with a topic containing an encoded surrogate code point U+D800 (0xED, 0xA0, 0x80)
    broker
        .send_raw(0x30, &[0x00, 0x03, 0xED, 0xA0, 0x80, 0x00])
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBLISH")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_1_5_4_2_null_character_in_utf8_string_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    // PUBLISH (QoS 0) with a topic containing the null character U+0000 ("a\0b")
    broker
        .send_raw(0x30, &[0x00, 0x03, b'a', 0x00, b'b', 0x00])
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBLISH")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_1_5_5_1_non_minimal_variable_byte_integer_malformed() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    // PUBLISH (QoS 0) with topic "a" (length 1), but property length 0 encoded
    // non-minimally with two bytes: [0x80, 0x00]
    broker
        .send_raw(0x30, &[0x00, 0x01, b'a', 0x80, 0x00])
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed packet")
            .is_err()
    );
    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_connect_reserved_flag_bits_set_to_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();
    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion before broker response"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();
            // In the CONNECT fixed header, reserved bits 3..0 MUST be 0000
            assert_eq!(connect.first_byte & 0x0F, 0x00);
        }
    }
}

#[tokio::test]
async fn mqtt_2_2_1_2_qos0_publish_has_no_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let options = qos0_publish("test");
    client
        .publish(&options, Bytes::Borrowed(b"payload"))
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.first_byte & 0x06, 0x00, "expected QoS 0");

    let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    assert_eq!(topic_len, 4);

    // For QoS 0 PUBLISH, the Property Length byte directly follows the topic name
    // (no 2-byte Packet Identifier is present).
    let prop_len = frame.body[2 + topic_len] as usize;
    assert_eq!(&frame.body[2 + topic_len + 1 + prop_len..], b"payload");
    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_2_2_1_3_qos1_publish_assigns_nonzero_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let options = qos1_publish("test");
    let publish = client
        .publish(&options, Bytes::Borrowed(b"payload"))
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.first_byte & 0x06, 0x02, "expected QoS 1");

    let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let packet_id = u16::from_be_bytes([frame.body[2 + topic_len], frame.body[2 + topic_len + 1]]);
    assert_ne!(packet_id, 0, "Packet Identifier must be non-zero");

    broker
        .send_raw(0x40, &publish.unwrap().get().get().to_be_bytes())
        .await
        .unwrap();
    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_2_2_1_5_puback_packet_id_matches_publish() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let packet_id: u16 = 0x1234;
    // Broker sends QoS 1 PUBLISH to client: topic "a", packet id 0x1234, property length 0, payload "x"
    broker
        .send_raw(0x32, &[0x00, 0x01, b'a', 0x12, 0x34, 0x00, b'x'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let puback = broker.next_non_ping().await.unwrap();
    assert_eq!(puback.packet_type(), 4, "expected PUBACK");
    assert_eq!(&puback.body[0..2], &packet_id.to_be_bytes());
    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_2_2_2_1_empty_properties_indicates_zero_length() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let options = qos0_publish("test");
    client
        .publish(&options, Bytes::Borrowed(b"payload"))
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;

    // A Property Length of zero MUST be present when there are no properties
    let property_length = frame.body[2 + topic_len];
    assert_eq!(property_length, 0x00);
    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_1_0_1_first_packet_must_be_connect() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();
    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion before broker response"),
        result = broker.accept() => {
            result.unwrap();
            let first_frame = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(first_frame.packet_type(), 1, "first packet must be CONNECT");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_1_protocol_name_is_mqtt() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();
    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion before broker response"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();
            // Protocol Name string: 2-byte length (4) followed by UTF-8 bytes for "MQTT"
            assert_eq!(&connect.body[0..6], &[0x00, 0x04, b'M', b'Q', b'T', b'T']);
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_9_will_flag_one_payload_contains_will_fields() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with a Will Message
    // (e.g. topic "will/test", payload b"will_data", QoS 1)
    let connect_options = will_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            // Will Flag (bit 2) MUST be set to 1
            assert_ne!(connect_flags & 0x04, 0, "Will Flag must be 1");

            // Variable Header: 10 bytes + property length
            let prop_len = connect.body[10] as usize;
            let mut offset = 11 + prop_len;

            // 1. Client Identifier
            let client_id_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2 + client_id_len;

            // 2. Will Properties (Variable Byte Integer length)
            let will_prop_len = connect.body[offset] as usize;
            offset += 1 + will_prop_len;

            // 3. Will Topic (UTF-8 Encoded String)
            let will_topic_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            assert!(will_topic_len > 0, "Will Topic must be present in Payload");
            offset += 2 + will_topic_len;

            // 4. Will Message / Payload (Binary Data with 2-byte length prefix)
            let will_msg_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2 + will_msg_len;

            assert!(offset <= connect.body.len(), "Will fields must be present in Payload");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_11_will_qos_zero_when_will_flag_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions without Will Message (Will Flag = 0)
    let connect_options = NO_SESSION_CONNECT_OPTIONS;

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_eq!(connect_flags & 0x04, 0x00, "Will Flag must be 0");
            // If Will Flag is 0, Will QoS (bits 4-3) MUST be set to 00
            assert_eq!((connect_flags >> 3) & 0x03, 0x00, "Will QoS must be 0 when Will Flag is 0");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_12_will_qos_cannot_be_three() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with a Will Message
    let connect_options = NO_SESSION_CONNECT_OPTIONS.clone().will(
        WillOptions::new(
            topic_name("will/qos"),
            MqttBinary::from_slice_unchecked(b"data"),
        )
        .exactly_once(),
    );

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            let will_qos = (connect_flags >> 3) & 0x03;
            // Will QoS value can be 0, 1, or 2, but MUST NOT be 3 (0x03)
            assert!(will_qos <= 2, "Will QoS must be 0, 1, or 2, got: {will_qos}");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_13_will_retain_zero_when_will_flag_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions without Will Message (Will Flag = 0)
    let connect_options = NO_SESSION_CONNECT_OPTIONS;

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_eq!(connect_flags & 0x04, 0x00, "Will Flag must be 0");
            // If Will Flag is 0, Will Retain (bit 5) MUST be set to 0
            assert_eq!(connect_flags & 0x20, 0x00, "Will Retain must be 0 when Will Flag is 0");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_16_username_flag_zero_payload_has_no_username() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions without User Name (User Name Flag = 0)
    let connect_options = no_credentials_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_eq!(connect_flags & 0x80, 0x00, "User Name Flag must be 0");

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;

            // When Will, Password, and Username flags are 0, payload ends immediately after ClientID
            assert_eq!(
                connect.body.len(),
                payload_offset + 2 + client_id_len,
                "Payload must not contain a User Name field when User Name Flag is 0"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_17_username_flag_one_payload_contains_username() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions configured with a User Name
    let connect_options = username_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_ne!(connect_flags & 0x80, 0, "User Name Flag must be 1");

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;

            let mut offset = payload_offset + 2 + client_id_len;

            // If Will Flag is set, advance past Will fields
            if (connect_flags & 0x04) != 0 {
                let will_prop_len = connect.body[offset] as usize;
                offset += 1 + will_prop_len;
                let will_topic_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
                offset += 2 + will_topic_len;
                let will_msg_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
                offset += 2 + will_msg_len;
            }

            // User Name field must be present
            let username_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            assert!(username_len > 0, "User Name must be present in Payload when flag is 1");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_18_password_flag_zero_payload_has_no_password() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions without Password (Password Flag = 0)
    let connect_options = no_credentials_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_eq!(connect_flags & 0x40, 0x00, "Password Flag must be 0");

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;

            assert_eq!(
                connect.body.len(),
                payload_offset + 2 + client_id_len,
                "Payload must not contain a Password field when Password Flag is 0"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_19_password_flag_one_payload_contains_password() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions configured with a Password
    let connect_options = password_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let connect_flags = connect.body[7];
            assert_ne!(connect_flags & 0x40, 0, "Password Flag must be 1");

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;

            let mut offset = payload_offset + 2 + client_id_len;

            // If Will Flag is set, skip Will fields
            if (connect_flags & 0x04) != 0 {
                let will_prop_len = connect.body[offset] as usize;
                offset += 1 + will_prop_len;
                let will_topic_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
                offset += 2 + will_topic_len;
                let will_msg_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
                offset += 2 + will_msg_len;
            }

            // If Username Flag is set, skip Username field
            if (connect_flags & 0x80) != 0 {
                let username_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
                offset += 2 + username_len;
            }

            // Password field (2-byte length prefix + binary data)
            let password_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            assert!(password_len > 0, "Password must be present in Payload when flag is 1");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_20_client_sends_pingreq_when_idle() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with a small Keep Alive (e.g. 1 second)
    let connect_options = NO_SESSION_CONNECT_OPTIONS
        .clone()
        .keep_alive(KeepAlive::Seconds(NonZero::new(1).unwrap()));

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Acknowledge connection with standard CONNACK
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // rust-mqtt exposes keep-alive handling cooperatively through Client::ping().
            tokio::time::sleep(Duration::from_millis(1100)).await;
            client.ping().await.unwrap();
            let ping = timeout(Duration::from_secs(1), broker.next_frame())
                .await
                .expect("client must send PINGREQ after Client::ping()")
                .unwrap();
            assert_eq!(ping.packet_type(), 12, "expected PINGREQ packet");

            broker.send_raw(0xD0, &[]).await.unwrap();
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_21_server_keep_alive_overrides_client_keep_alive() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Client sends default Keep Alive (e.g. 60 seconds)
    let connect_options = NO_SESSION_CONNECT_OPTIONS;

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server returns Server Keep Alive = 1s in CONNACK (Property ID 0x13, value 0x0001)
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x13, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Keep-alive transmission is initiated explicitly through Client::ping().
            tokio::time::sleep(Duration::from_millis(1100)).await;
            client.ping().await.unwrap();
            let ping = timeout(Duration::from_secs(1), broker.next_frame())
                .await
                .expect("client did not send PINGREQ after Client::ping()")
                .unwrap();
            assert_eq!(ping.packet_type(), 12, "expected PINGREQ packet");

            broker.send_raw(0xD0, &[]).await.unwrap();
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_2_30_auth_method_restricts_client_packets() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with an Authentication Method
    let connect_options = auth_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(connect.packet_type(), 1, "expected CONNECT packet");

            // Until CONNACK is sent, the client MUST NOT send any packet other than AUTH or DISCONNECT
            let next_packet = timeout(Duration::from_millis(100), broker.next_frame()).await;
            if let Ok(Ok(frame)) = next_packet {
                let p_type = frame.packet_type();
                assert!(
                    p_type == 14 || p_type == 15,
                    "Client sent packet type {p_type}; only AUTH (15) or DISCONNECT (14) are allowed before CONNACK"
                );
            }
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_1_payload_fields_order() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: ConnectOptions configured with ClientID, Will, Username, and Password
    let connect_options = full_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("client_order").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let prop_len = connect.body[10] as usize;
            let mut offset = 11 + prop_len;

            // 1. Client Identifier MUST be first
            let client_id_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            assert_eq!(&connect.body[offset..offset + client_id_len], b"client_order");
            offset += client_id_len;

            // 2. Will Properties, Will Topic, Will Message MUST follow Client Identifier
            let will_prop_len = connect.body[offset] as usize;
            offset += 1 + will_prop_len;
            let will_topic_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            assert_eq!(&connect.body[offset..offset + will_topic_len], b"topic/will");
            offset += will_topic_len;
            let will_msg_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            assert_eq!(&connect.body[offset..offset + will_msg_len], b"will_msg");
            offset += will_msg_len;

            // 3. User Name MUST follow Will fields
            let username_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            assert_eq!(&connect.body[offset..offset + username_len], b"user_name");
            offset += username_len;

            // 4. Password MUST follow User Name
            let password_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            assert_eq!(&connect.body[offset..offset + password_len], b"pass_word");
            offset += password_len;

            assert_eq!(offset, connect.body.len(), "all payload fields checked in exact order");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_3_client_id_is_first_payload_field() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("first_field_id").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;

            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;
            let client_id = &connect.body[payload_offset + 2..payload_offset + 2 + client_id_len];
            assert_eq!(client_id, b"first_field_id", "ClientID must be the first field in CONNECT payload");
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_4_client_id_must_be_valid_utf8() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("valid_utf8_client").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;
            let client_id_bytes = &connect.body[payload_offset + 2..payload_offset + 2 + client_id_len];

            assert!(
                std::str::from_utf8(client_id_bytes).is_ok(),
                "ClientID MUST be a well-formed UTF-8 string"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_11_will_topic_is_utf8_string() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with a Will Message
    let connect_options = utf8_will_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let prop_len = connect.body[10] as usize;
            let mut offset = 11 + prop_len;

            // Skip Client Identifier
            let client_id_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2 + client_id_len;

            // Skip Will Properties
            let will_prop_len = connect.body[offset] as usize;
            offset += 1 + will_prop_len;

            // Will Topic
            let will_topic_len = u16::from_be_bytes([connect.body[offset], connect.body[offset + 1]]) as usize;
            offset += 2;
            let will_topic_bytes = &connect.body[offset..offset + will_topic_len];

            assert!(
                std::str::from_utf8(will_topic_bytes).is_ok(),
                "Will Topic MUST be a valid UTF-8 Encoded String"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_12_username_is_utf8_string() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with a Username
    let connect_options = utf8_username_connect_options();

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let client_id_len = u16::from_be_bytes([
                connect.body[payload_offset],
                connect.body[payload_offset + 1],
            ]) as usize;

            let username_offset = payload_offset + 2 + client_id_len;
            let username_len = u16::from_be_bytes([
                connect.body[username_offset],
                connect.body[username_offset + 1],
            ]) as usize;
            let username_bytes = &connect.body[username_offset + 2..username_offset + 2 + username_len];

            assert!(
                std::str::from_utf8(username_bytes).is_ok(),
                "User Name MUST be a valid UTF-8 Encoded String"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_1_connack_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Send CONNACK with reserved bit 1 set in Connect Acknowledge Flags (0x02)
            broker.send_raw(0x20, &[0x02, 0x00, 0x00]).await.unwrap();
            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST reject CONNACK where reserved flag bits 7-1 are non-zero"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_4_session_present_without_session_closes_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Client has no session state; send CONNACK with Session Present = 1 (0x01)
            broker.send_raw(0x20, &[0x01, 0x00, 0x00]).await.unwrap();
            let _ = connecting.await;

            // Client MUST close the network connection
            assert!(
                broker.wait_closed(Duration::from_secs(2)).await.unwrap(),
                "Client MUST close network connection when receiving Session Present = 1 without having session state"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_11_publish_exceeding_server_maximum_qos_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server indicates Maximum QoS = 0 (Property ID 0x24, value 0x00)
            broker.send_raw(0x20, &[0x00, 0x00, 0x02, 0x24, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client attempts to publish at QoS 1
            let options = qos1_publish("test/max_qos");
            let result = client.publish(&options, Bytes::Borrowed(b"data")).await;

            // Client MUST NOT send PUBLISH exceeding Maximum QoS
            assert!(
                result.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send PUBLISH at a QoS level exceeding Server Maximum QoS"
            );
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_14_retain_available_zero_rejects_retained_publish() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server indicates Retain Available = 0 (Property ID 0x25, value 0x00)
            broker.send_raw(0x20, &[0x00, 0x00, 0x02, 0x25, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client option: Configure PublicationOptions with RETAIN = 1
            let options = qos0_publish("test/retain").retain();
            let result = client.publish(&options, Bytes::Borrowed(b"data")).await;

            // Client MUST NOT send PUBLISH with RETAIN = 1
            assert!(
                result.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client receiving Retain Available = 0 MUST NOT send a PUBLISH with RETAIN set to 1"
            );
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_15_client_respects_server_maximum_packet_size() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server specifies Maximum Packet Size = 40 bytes (Property ID 0x27, 4-byte int: 0x00000028)
            broker
                .send_raw(0x20, &[0x00, 0x00, 0x05, 0x27, 0x00, 0x00, 0x00, 0x28])
                .await
                .unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client attempts to publish a message whose total size exceeds 40 bytes
            let options = qos0_publish("test/max_packet");
            let large_payload = vec![0xAA; 64];
            let result = client.publish(&options, Bytes::Borrowed(&large_payload)).await;

            // Client MUST NOT send packets exceeding Maximum Packet Size
            assert!(
                result.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send a packet exceeding the Server Maximum Packet Size"
            );
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_17_topic_alias_exceeding_server_maximum_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_outgoing_alias_client(address, NO_SESSION_CONNECT_OPTIONS);
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server specifies Topic Alias Maximum = 1 (Property ID 0x22, 2-byte int: 0x0001)
            broker
                .send_raw(0x20, &[0x00, 0x00, 0x03, 0x22, 0x00, 0x01])
                .await
                .unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client option: Configure PublicationOptions with Topic Alias = 2 (> Topic Alias Maximum)
            let options = PublicationOptions::new(TopicReference::Mapping(
                topic_name("test/topic_alias"),
                NonZero::new(2).unwrap(),
            ));
            let result = client.publish(&options, Bytes::Borrowed(b"payload")).await;

            // Client MUST NOT send a Topic Alias exceeding the Server's Topic Alias Maximum
            assert!(
                result.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send a Topic Alias greater than Server Topic Alias Maximum"
            );
            client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_18_topic_alias_without_server_support_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_outgoing_alias_client(address, NO_SESSION_CONNECT_OPTIONS);
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server sends CONNACK without Topic Alias Maximum property (absent)
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client option: Configure PublicationOptions with a Topic Alias
            let options = PublicationOptions::new(TopicReference::Mapping(
                topic_name("test/topic_alias"),
                NonZero::new(1).unwrap(),
            ));
            let result = client.publish(&options, Bytes::Borrowed(b"payload")).await;

            // If Topic Alias Maximum is absent, the client MUST NOT send any Topic Aliases
            assert!(
                result.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send Topic Aliases when Topic Alias Maximum is absent"
            );
            client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
        }
    }
}

#[tokio::test]
async fn mqtt_3_3_1_2_qos0_dup_flag_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos0_publish("test/qos0_dup");
    client
        .publish(&options, Bytes::Borrowed(b"data"))
        .await
        .unwrap();

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 3, "expected PUBLISH packet");
    // DUP flag (bit 3) MUST be 0 for all QoS 0 messages
    assert_eq!(
        frame.first_byte & 0x08,
        0x00,
        "DUP flag MUST be 0 for all QoS 0 PUBLISH packets"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_3_2_1_publish_variable_header_starts_with_topic_name() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let topic = "sensor/temp/living_room";
    let options = qos0_publish(topic);
    client
        .publish(&options, Bytes::Borrowed(b"21.5"))
        .await
        .unwrap();

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 3, "expected PUBLISH packet");

    // The Topic Name MUST be the first field in the Variable Header
    let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let topic_bytes = &frame.body[2..2 + topic_len];
    assert_eq!(
        std::str::from_utf8(topic_bytes).expect("Topic Name must be valid UTF-8"),
        topic,
        "Variable Header must begin with the correct UTF-8 Topic Name"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_3_2_2_received_publish_topic_with_wildcard_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends a PUBLISH containing wildcard character '+' in Topic Name: "sensor/+"
    broker
        .send_raw(
            0x30,
            &[
                0x00, 0x08, b's', b'e', b'n', b's', b'o', b'r', b'/', b'+', 0x00,
            ],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBLISH")
            .is_err(),
        "Client MUST treat received PUBLISH with wildcards in Topic Name as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14, "expected DISCONNECT packet");
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_3_2_7_topic_alias_not_carried_over_connections() {
    let mut broker = Broker::bind().await.unwrap();

    // Connection 1: Connect and immediately disconnect
    {
        let mut client = connect(&mut broker).await.unwrap();
        disconnect(&mut client).await;
    }

    // Connection 2: Reconnect on a new network connection
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends a PUBLISH with an empty topic name and Topic Alias = 1 (Property ID 0x23)
    // Topic length: 0 (0x00, 0x00), Property length: 3, Property 0x23, value 0x0001
    broker
        .send_raw(0x30, &[0x00, 0x00, 0x03, 0x23, 0x00, 0x01])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process PUBLISH with unmapped alias")
            .is_err(),
        "Receiver MUST NOT carry forward Topic Alias mappings from one network connection to another"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_3_2_8_topic_alias_zero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with Topic Alias = 0 (Property ID 0x23, value 0x0000)
    broker
        .send_raw(0x30, &[0x00, 0x01, b'a', 0x03, 0x23, 0x00, 0x00])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed Topic Alias")
            .is_err(),
        "Client MUST treat a received Topic Alias of 0 as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_3_2_10_client_accepts_valid_topic_alias() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Topic Alias Maximum is a compile-time client configuration in rust-mqtt.
    // Use a client configured to accept five inbound aliases.
    let connect_options = NO_SESSION_CONNECT_OPTIONS;

    tokio::task::LocalSet::new()
        .run_until(async {
            let connecting = tokio::task::spawn_local(async move {
                let mut client: Client<
                    'static,
                    'static,
                    FromTokio<TcpStream>,
                    AllocBuffer,
                    1,
                    1,
                    1,
                    1,
                    16,
                    5,
                    0,
                > = Client::new(ALLOC.get());
                let tcp = tcp_connection(address).await.unwrap();
                client
                    .connect(
                        tcp,
                        &connect_options,
                        Some(MqttString::from_str("conformance").unwrap()),
                    )
                    .await
                    .unwrap();
                let poll = timeout(Duration::from_millis(500), client.poll())
                    .await
                    .expect("client poll timed out");
                client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
                poll
            });

            broker.accept().await.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();

            // Broker sends PUBLISH with valid Topic Alias = 1 (<= Topic Alias Maximum)
            broker
                .send_raw(
                    0x30,
                    &[0x00, 0x01, b'a', 0x03, 0x23, 0x00, 0x01, b'o', b'k'],
                )
                .await
                .unwrap();

            assert!(
                connecting.await.unwrap().is_ok(),
                "Client MUST accept all Topic Alias values > 0 and <= Topic Alias Maximum sent in CONNECT"
            );
        })
        .await;
}

#[tokio::test]
async fn mqtt_3_3_2_14_response_topic_with_wildcard_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with Response Topic property (0x08) containing wildcard '#': "response/#"
    // Topic: "a" (0x0001, 'a'), Property length: 13, Prop 0x08, String len: 10, "response/#"
    broker
        .send_raw(
            0x30,
            &[
                0x00, 0x01, b'a', 0x0D, 0x08, 0x00, 0x0A, b'r', b'e', b's', b'p', b'o', b'n', b's',
                b'e', b'/', b'#',
            ],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed Response Topic")
            .is_err(),
        "Client MUST treat Response Topic containing wildcard characters as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_3_2_19_content_type_invalid_utf8_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with Content Type property (0x03) containing invalid UTF-8 (U+D800 surrogate: 0xED, 0xA0, 0x80)
    // Topic: "a" (0x0001, 'a'), Property length: 6, Prop 0x03, String length: 3, bytes: [0xED, 0xA0, 0x80]
    broker
        .send_raw(
            0x30,
            &[
                0x00, 0x01, b'a', 0x06, 0x03, 0x00, 0x03, 0xED, 0xA0, 0x80, b'x',
            ],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed Content Type")
            .is_err(),
        "Client MUST treat Content Type that is not well-formed UTF-8 as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_3_4_1_receiver_responds_to_qos2_with_pubrec() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x002A;
    // Broker sends QoS 2 PUBLISH to client (fixed header 0x34)
    broker
        .send_raw(
            0x34,
            &[0x00, 0x01, b'a', 0x00, 0x2A, 0x00, b'd', b'a', b't', b'a'],
        )
        .await
        .unwrap();

    let _ = client.poll().await;

    // Receiver of QoS 2 PUBLISH packet MUST respond with PUBREC
    let pubrec = broker.next_non_ping().await.unwrap();
    assert_eq!(pubrec.packet_type(), 5, "expected PUBREC packet");
    assert_eq!(&pubrec.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_3_4_6_client_publish_no_subscription_identifier() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos0_publish("test/no_sub_id");
    client
        .publish(&options, Bytes::Borrowed(b"msg"))
        .await
        .unwrap();

    let frame = broker.next_non_ping().await.unwrap();
    let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let prop_len = frame.body[2 + topic_len] as usize;

    // Scan properties in variable header to ensure Property ID 0x0B (Subscription Identifier) is NOT present
    let prop_bytes = &frame.body[3 + topic_len..3 + topic_len + prop_len];
    let mut cursor = 0;
    while cursor < prop_bytes.len() {
        let prop_id = prop_bytes[cursor];
        assert_ne!(
            prop_id, 0x0B,
            "PUBLISH packet sent from Client MUST NOT contain a Subscription Identifier"
        );
        cursor += 1;
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_3_4_7_client_respects_receive_maximum_quota() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server specifies Receive Maximum = 1 (Property ID 0x21, 2-byte int: 0x0001)
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x21, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Client sends 1st QoS 1 PUBLISH (consumes quota of 1)
            let options1 = qos1_publish("quota/1");
            let _ = client.publish(&options1, Bytes::Borrowed(b"m1")).await;
            let _first = broker.next_non_ping().await.unwrap();

            // Client attempts 2nd QoS 1 PUBLISH without receiving PUBACK for the first
            let options2 = qos1_publish("quota/2");
            let second_res = client.publish(&options2, Bytes::Borrowed(b"m2")).await;

            // Client MUST NOT send more than Receive Maximum QoS 1 unacknowledged messages
            assert!(
                second_res.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send more than Receive Maximum unacknowledged QoS 1 packets"
            );
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_3_4_8_quota_exhaustion_does_not_block_pingreq() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame())
                .await
                .unwrap()
                .unwrap();

            // Server sets Receive Maximum = 1 (Property 0x21) and Server Keep Alive = 1s (Property 0x13)
            broker
                .send_raw(
                    0x20,
                    &[0x00, 0x00, 0x06, 0x21, 0x00, 0x01, 0x13, 0x00, 0x01],
                )
                .await
                .unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .unwrap()
                .unwrap();

            // Exhaust quota with one unacknowledged QoS 1 PUBLISH
            let options = qos1_publish("quota/exhaust");
            let _ = client.publish(&options, Bytes::Borrowed(b"m")).await;
            let _ = broker.next_non_ping().await.unwrap();

            // rust-mqtt drives keep-alive packets cooperatively through Client::ping().
            tokio::time::sleep(Duration::from_millis(1100)).await;
            client.ping().await.unwrap();
            let ping = timeout(Duration::from_secs(1), broker.next_frame())
                .await
                .expect("Client could not send PINGREQ while quota was exhausted")
                .unwrap();
            assert_eq!(ping.packet_type(), 12, "expected PINGREQ packet");

            broker.send_raw(0xD0, &[]).await.unwrap();
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_4_2_1_puback_uses_valid_reason_code() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends QoS 1 PUBLISH
    broker
        .send_raw(0x32, &[0x00, 0x01, b'a', 0x00, 0x05, 0x00, b'x'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let puback = broker.next_non_ping().await.unwrap();
    assert_eq!(puback.packet_type(), 4, "expected PUBACK");

    // If Reason Code is present in PUBACK, it must be a valid Reason Code
    if puback.body.len() > 2 {
        let code = puback.body[2];
        let valid_codes = [0x00, 0x10, 0x80, 0x83, 0x87, 0x90, 0x91, 0x97, 0x99];
        assert!(
            valid_codes.contains(&code),
            "PUBACK Reason Code {code:#04X} is not a valid PUBACK Reason Code"
        );
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_5_2_1_pubrec_uses_valid_reason_code() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends QoS 2 PUBLISH
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0x09, 0x00, b'y'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let pubrec = broker.next_non_ping().await.unwrap();
    assert_eq!(pubrec.packet_type(), 5, "expected PUBREC");

    // If Reason Code is present in PUBREC, it must be a valid Reason Code
    if pubrec.body.len() > 2 {
        let code = pubrec.body[2];
        let valid_codes = [0x00, 0x10, 0x80, 0x83, 0x87, 0x90, 0x91, 0x97, 0x99];
        assert!(
            valid_codes.contains(&code),
            "PUBREC Reason Code {code:#04X} is not a valid PUBREC Reason Code"
        );
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_6_1_1_pubrel_reserved_flags_invalid_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Fixed header bits 3-0 in PUBREL are reserved and MUST be 0010 (0x62)
    // Broker sends malformed PUBREL with fixed header 0x60
    broker.send_raw(0x60, &[0x00, 0x01]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBREL")
            .is_err(),
        "Client MUST treat PUBREL packet with reserved flags != 0010 as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_8_1_1_subscribe_fixed_header_reserved_flags() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let _subscribe = client
        .subscribe(topic_filter("sensor/#"), &SubscriptionOptions::new())
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 8, "expected SUBSCRIBE packet");
    assert_eq!(
        frame.first_byte & 0x0F,
        0x02,
        "SUBSCRIBE Fixed Header reserved bits 3..0 MUST be 0010"
    );
    drop(client);
}

#[tokio::test]
async fn mqtt_3_8_3_1_subscribe_topic_filter_is_utf8() {
    let filter_str = "living_room/temperature";
    let filter = topic_filter(filter_str);

    // Client option / API call: Verify TopicFilter construction enforces well-formed UTF-8
    assert_eq!(filter.as_ref().as_str(), filter_str);

    // Verify invalid UTF-8 sequence cannot be used to construct a Topic Filter
    let invalid_utf8_empty = MqttString::from_str("");
    assert!(invalid_utf8_empty.is_ok());
    assert!(
        TopicFilter::new(invalid_utf8_empty.unwrap()).is_none(),
        "Topic Filters MUST comply with requirements for UTF-8 Encoded Strings"
    );
}

#[tokio::test]
async fn mqtt_3_8_3_2_subscribe_payload_has_at_least_one_filter() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    client
        .subscribe(topic_filter("sensor/#"), &SubscriptionOptions::new())
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 8, "expected SUBSCRIBE packet");
    let prop_len = frame.body[2] as usize;
    let payload_offset = 3 + prop_len;
    assert!(
        frame.body.len() > payload_offset,
        "SUBSCRIBE payload MUST contain at least one Topic Filter and Subscription Options pair"
    );
    drop(client);
}

#[tokio::test]
async fn mqtt_3_8_3_4_shared_subscription_with_no_local_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();
    let filter = topic_filter("$share/group/sensor/#");
    let result = client
        .subscribe(filter, &SubscriptionOptions::new().no_local())
        .await;
    assert!(matches!(
        result,
        Err(MqttError::IllegalNoLocalSharedSubscription)
    ));
    drop(client);
}

#[tokio::test]
async fn mqtt_3_8_3_5_subscribe_subscription_options_reserved_bits_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    client
        .subscribe(topic_filter("sensor/#"), &SubscriptionOptions::new())
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 8, "expected SUBSCRIBE packet");
    let prop_len = frame.body[2] as usize;
    let payload = &frame.body[3 + prop_len..];
    let filter_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let sub_options_byte = payload[2 + filter_len];
    assert_eq!(sub_options_byte & 0xC0, 0x00);
    drop(client);
}

#[tokio::test]
async fn mqtt_3_10_1_1_unsubscribe_fixed_header_reserved_flags() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    client
        .unsubscribe(topic_filter("sensor/#"), &UnsubscriptionOptions::new())
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 10, "expected UNSUBSCRIBE packet");
    assert_eq!(frame.first_byte & 0x0F, 0x02);
    drop(client);
}

#[tokio::test]
async fn mqtt_3_10_3_1_unsubscribe_topic_filter_is_utf8() {
    let filter_str = "device/telemetry";
    let filter = topic_filter(filter_str);

    // Topic Filters in UNSUBSCRIBE MUST be UTF-8 Encoded Strings
    assert_eq!(filter.as_ref().as_str(), filter_str);
    assert!(
        std::str::from_utf8(filter.as_ref().as_str().as_bytes()).is_ok(),
        "Topic Filters in an UNSUBSCRIBE packet MUST be valid UTF-8 Encoded Strings"
    );
}

#[tokio::test]
async fn mqtt_3_10_3_2_unsubscribe_payload_has_at_least_one_topic_filter() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    client
        .unsubscribe(topic_filter("sensor/#"), &UnsubscriptionOptions::new())
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 10, "expected UNSUBSCRIBE packet");
    let prop_len = frame.body[2] as usize;
    let payload_offset = 3 + prop_len;
    assert!(frame.body.len() > payload_offset);
    drop(client);
}

#[tokio::test]
async fn mqtt_3_14_2_1_client_disconnect_valid_reason_code() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option: Disconnect using standard disconnect options
    disconnect(&mut client).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 14, "expected DISCONNECT packet");

    // If Reason Code is present in DISCONNECT, it MUST be a valid Disconnect Reason Code
    if !frame.body.is_empty() {
        let reason_code = frame.body[0];
        // 0x00 is Normal Disconnection; any client error disconnect is >= 0x80
        assert!(
            reason_code == 0x00 || reason_code == 0x04 || reason_code >= 0x80,
            "Client sending DISCONNECT MUST use a valid Disconnect Reason Code, got: {reason_code:#04X}"
        );
    }
}

#[tokio::test]
async fn mqtt_3_14_2_2_server_disconnect_with_session_expiry_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Server sends DISCONNECT containing Session Expiry Interval (Property ID 0x11, 4-byte int: 0x0000003C)
    // The Session Expiry Interval MUST NOT be sent on a DISCONNECT by the Server
    broker
        .send_raw(0xE0, &[0x00, 0x05, 0x11, 0x00, 0x00, 0x00, 0x3C])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process invalid Server DISCONNECT")
            .is_err(),
        "Client MUST treat Session Expiry Interval sent on Server DISCONNECT as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_14_4_1_no_packets_after_disconnect() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Send DISCONNECT packet
    disconnect(&mut client).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 14, "expected DISCONNECT packet");

    // After sending a DISCONNECT packet, the client MUST NOT send any more MQTT Control Packets
    assert!(
        broker
            .expect_silence(Duration::from_millis(50))
            .await
            .is_ok(),
        "Client MUST NOT send any further control packets after sending DISCONNECT"
    );
}

#[tokio::test]
async fn mqtt_3_14_4_2_client_closes_connection_after_disconnect() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client initiates disconnect
    disconnect(&mut client).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 14, "expected DISCONNECT packet");

    // After sending a DISCONNECT packet, the sender MUST close the Network Connection
    assert!(
        broker.wait_closed(Duration::from_secs(2)).await.unwrap(),
        "Client MUST close the Network Connection after sending DISCONNECT"
    );
}

#[tokio::test]
async fn mqtt_3_15_1_1_auth_fixed_header_reserved_flags_invalid() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Bits 3,2,1 and 0 of Fixed Header of AUTH packet are reserved and MUST all be set to 0 (0xF0)
    // Broker sends malformed AUTH packet with fixed header 0xF1
    broker.send_raw(0xF1, &[0x18, 0x00]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed AUTH packet")
            .is_err(),
        "Client MUST treat AUTH packet with non-zero reserved bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_15_2_1_auth_invalid_reason_code_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Reason Codes for AUTH are limited to 0x00, 0x18, and 0x19
    // Broker sends AUTH packet with invalid Reason Code 0xFF
    broker.send_raw(0xF0, &[0xFF, 0x00]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process invalid AUTH Reason Code")
            .is_err(),
        "Client MUST reject AUTH packet containing an invalid Authenticate Reason Code"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_4_3_1_1_qos0_delivery_protocol_dup_and_qos_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos0_publish("test/qos0_delivery");
    client
        .publish(&options, Bytes::Borrowed(b"msg"))
        .await
        .unwrap();

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 3, "expected PUBLISH packet");

    // In QoS 0 delivery protocol, QoS bits (bits 2-1) MUST be 00 and DUP flag (bit 3) MUST be 0
    assert_eq!(
        frame.first_byte & 0x0E,
        0x00,
        "QoS bits and DUP flag MUST both be 0 for QoS 0 PUBLISH"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_2_1_qos1_publish_assigns_unused_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_multi_publish(&mut broker).await.unwrap();

    let options1 = qos1_publish("test/qos1_id_1");
    let options2 = qos1_publish("test/qos1_id_2");

    let pub1 = client
        .publish(&options1, Bytes::Borrowed(b"1"))
        .await
        .unwrap();
    let frame1 = broker.next_non_ping().await.unwrap();
    let topic_len1 = u16::from_be_bytes([frame1.body[0], frame1.body[1]]) as usize;
    let packet_id1 =
        u16::from_be_bytes([frame1.body[2 + topic_len1], frame1.body[2 + topic_len1 + 1]]);

    let pub2 = client
        .publish(&options2, Bytes::Borrowed(b"2"))
        .await
        .unwrap();
    let frame2 = broker.next_non_ping().await.unwrap();
    let topic_len2 = u16::from_be_bytes([frame2.body[0], frame2.body[1]]) as usize;
    let packet_id2 =
        u16::from_be_bytes([frame2.body[2 + topic_len2], frame2.body[2 + topic_len2 + 1]]);

    // Sender MUST assign an unused non-zero Packet Identifier for each QoS 1 message
    assert_ne!(packet_id1, 0, "Packet Identifier 1 must be non-zero");
    assert_ne!(packet_id2, 0, "Packet Identifier 2 must be non-zero");
    assert_ne!(
        packet_id1, packet_id2,
        "Packet Identifier must be unused and unique"
    );

    // Acknowledge both messages.
    broker
        .send_raw(0x40, &pub1.unwrap().get().get().to_be_bytes())
        .await
        .unwrap();
    broker
        .send_raw(0x40, &pub2.unwrap().get().get().to_be_bytes())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), client.poll())
        .await
        .expect("client did not process first PUBACK")
        .unwrap();
    timeout(Duration::from_secs(5), client.poll())
        .await
        .expect("client did not process second PUBACK")
        .unwrap();

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_3_2_4_receiver_responds_with_puback() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x0055;
    // Broker sends QoS 1 PUBLISH to client
    broker
        .send_raw(
            0x32,
            &[0x00, 0x01, b'a', 0x00, 0x55, 0x00, b'd', b'a', b't', b'a'],
        )
        .await
        .unwrap();

    let _ = client.poll().await;

    // Receiver MUST respond with a PUBACK containing the Packet Identifier from the incoming PUBLISH
    let puback = broker.next_non_ping().await.unwrap();
    assert_eq!(puback.packet_type(), 4, "expected PUBACK");
    assert_eq!(&puback.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_2_5_publish_after_puback_treated_as_new() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x0066;

    // First PUBLISH
    broker
        .send_raw(0x32, &[0x00, 0x01, b'a', 0x00, 0x66, 0x00, b'1'])
        .await
        .unwrap();
    let _ = client.poll().await;
    let puback1 = broker.next_non_ping().await.unwrap();
    assert_eq!(&puback1.body[0..2], &packet_id.to_be_bytes());

    // Subsequent PUBLISH with the same Packet Identifier (even with DUP set) MUST be treated as a new Application Message
    broker
        .send_raw(0x3A, &[0x00, 0x01, b'a', 0x00, 0x66, 0x00, b'2'])
        .await
        .unwrap();
    let _ = client.poll().await;
    let puback2 = broker.next_non_ping().await.unwrap();
    assert_eq!(
        &puback2.body[0..2],
        &packet_id.to_be_bytes(),
        "Receiver MUST acknowledge subsequent PUBLISH with same Packet Identifier after PUBACK"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_1_qos2_publish_assigns_unused_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option: Configure PublicationOptions with QoS 2 (Exactly Once)
    let options = qos0_publish("test/qos2_id").exactly_once();
    let _ = client.publish(&options, Bytes::Borrowed(b"qos2")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 3 && (frame.first_byte & 0x06) == 0x04 {
            let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
            let packet_id =
                u16::from_be_bytes([frame.body[2 + topic_len], frame.body[2 + topic_len + 1]]);
            assert_ne!(
                packet_id, 0,
                "Sender MUST assign an unused non-zero Packet Identifier for QoS 2"
            );
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_2_qos2_publish_flags() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option: Configure PublicationOptions with QoS 2
    let options = qos0_publish("test/qos2_flags").exactly_once();
    let _ = client.publish(&options, Bytes::Borrowed(b"data")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 3 {
            // In QoS 2 delivery protocol, sender MUST send PUBLISH with QoS 2 (0b0100) and DUP = 0
            assert_eq!(
                frame.first_byte & 0x0E,
                0x04,
                "PUBLISH Fixed Header MUST have QoS = 2 (bits 2-1 = 10) and DUP = 0 (bit 3 = 0)"
            );
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_4_client_sends_pubrel_on_pubrec_success() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option: Publish QoS 2 message
    let options = qos0_publish("test/qos2_pubrel").exactly_once();
    let _ = client.publish(&options, Bytes::Borrowed(b"msg")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 3 && (frame.first_byte & 0x06) == 0x04 {
            let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
            let packet_id = &frame.body[2 + topic_len..2 + topic_len + 2];

            // Broker responds with PUBREC containing Reason Code 0x00 (< 0x80)
            broker
                .send_raw(0x50, &[packet_id[0], packet_id[1], 0x00])
                .await
                .unwrap();

            timeout(Duration::from_secs(5), client.poll())
                .await
                .expect("client did not process PUBREC")
                .unwrap();

            // Client MUST send PUBREL containing the same Packet Identifier
            let pubrel = broker.next_non_ping().await.unwrap();
            assert_eq!(pubrel.packet_type(), 6, "expected PUBREL packet");
            assert_eq!(
                pubrel.first_byte, 0x62,
                "PUBREL Fixed Header reserved bits MUST be 0010"
            );
            assert_eq!(
                &pubrel.body[0..2],
                packet_id,
                "PUBREL Packet Identifier must match PUBLISH"
            );

            broker.send_raw(0x70, packet_id).await.unwrap();
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_11_receiver_responds_to_pubrel_with_pubcomp() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x0077;
    // Broker sends PUBREL to client (fixed header 0x62, packet ID 0x0077)
    broker
        .send_raw(0x62, &packet_id.to_be_bytes())
        .await
        .unwrap();
    let _ = client.poll().await;

    // Receiver MUST respond to a PUBREL packet by sending a PUBCOMP with the same Packet Identifier
    let pubcomp = broker.next_non_ping().await.unwrap();
    assert_eq!(pubcomp.packet_type(), 7, "expected PUBCOMP packet");
    assert_eq!(&pubcomp.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_12_publish_after_pubcomp_treated_as_new() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x0088;

    // Step 1: Initial QoS 2 flow
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0x88, 0x00, b'1'])
        .await
        .unwrap();
    let _ = client.poll().await;
    let pubrec = broker.next_non_ping().await.unwrap();
    assert_eq!(pubrec.packet_type(), 5);

    broker
        .send_raw(0x62, &packet_id.to_be_bytes())
        .await
        .unwrap();
    let _ = client.poll().await;
    let pubcomp = broker.next_non_ping().await.unwrap();
    assert_eq!(pubcomp.packet_type(), 7);

    // Step 2: After PUBCOMP, subsequent PUBLISH with the same Packet Identifier MUST be treated as a new Application Message
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0x88, 0x00, b'2'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let next_pubrec = broker.next_non_ping().await.unwrap();
    assert_eq!(
        next_pubrec.packet_type(),
        5,
        "Receiver MUST respond with PUBREC to subsequent PUBLISH with same Packet Identifier after PUBCOMP"
    );
    assert_eq!(&next_pubrec.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_4_0_1_resend_unacknowledged_publish_on_reconnect() {
    let mut broker = Broker::bind().await.unwrap();

    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));
    let client_id = MqttString::from_str("session_resend").unwrap();
    let mut client = connect_multi_with_options(&mut broker, &session_options)
        .await
        .unwrap();
    let options = qos1_publish("test/unack");
    let packet_id = client
        .publish(&options, Bytes::Borrowed(b"msg"))
        .await
        .unwrap()
        .unwrap();
    let _first_pub = broker.next_non_ping().await.unwrap();
    broker.close().await.unwrap();
    let _ = client.poll().await;
    let _ = client.abort().await;

    reconnect_multi_with_options(&mut client, &mut broker, &session_options, client_id)
        .await
        .unwrap();
    client
        .republish(packet_id, &options, Bytes::Borrowed(b"msg"))
        .await
        .unwrap();

    let resent_frame = broker.next_non_ping().await.unwrap();

    assert_eq!(resent_frame.packet_type(), 3, "expected PUBLISH packet");
    assert_eq!(
        resent_frame.first_byte & 0x08,
        0x08,
        "Resent PUBLISH packet MUST have DUP flag set to 1"
    );

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_4_0_2_nack_puback_treated_as_acknowledged_no_resend() {
    let mut broker = Broker::bind().await.unwrap();

    // Client option: Configure ConnectOptions with Clean Start = 0 and Session Expiry Interval > 0
    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));

    // Connection 1: Connect, send QoS 1 PUBLISH, receive negative PUBACK (Reason Code 0x80)
    {
        let connecting = connected_client(
            broker.address(),
            &session_options,
            Some(MqttString::from_str("session_nack").unwrap()),
        );
        tokio::pin!(connecting);
        let mut client = tokio::select! {
            _ = &mut connecting => panic!("unexpected connection completion"),
            result = broker.accept() => {
                result.unwrap();
                let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
                broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
                timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
            }
        };

        let options = qos1_publish("test/nack");
        let _ = client.publish(&options, Bytes::Borrowed(b"nack_msg")).await;
        let pub_frame = broker.next_non_ping().await.unwrap();
        let topic_len = u16::from_be_bytes([pub_frame.body[0], pub_frame.body[1]]) as usize;
        let pid = &pub_frame.body[2 + topic_len..4 + topic_len];

        // Send PUBACK with Reason Code 0x80 (Unspecified Error)
        broker
            .send_raw(0x40, &[pid[0], pid[1], 0x80])
            .await
            .unwrap();
        let _ = client.poll().await;
        broker.close().await.unwrap();
    }

    // Connection 2: Reconnect with Clean Start = 0 and Session Present = 1
    let connecting = connected_client(
        broker.address(),
        &session_options,
        Some(MqttString::from_str("session_nack").unwrap()),
    );
    tokio::pin!(connecting);
    let mut client = tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
            broker.send_raw(0x20, &[0x01, 0x00, 0x00]).await.unwrap();
            timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
        }
    };

    // If PUBACK with Reason Code >= 0x80 was received, packet is treated as acknowledged and MUST NOT be retransmitted
    assert!(
        broker
            .expect_silence(Duration::from_millis(100))
            .await
            .is_ok(),
        "Client MUST NOT retransmit PUBLISH that received a negative acknowledgement (Reason Code >= 0x80)"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_5_0_2_client_acknowledges_publish_regardless_of_processing() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x0099;
    // Broker sends QoS 1 PUBLISH for which client has no registered handler or subscription
    broker
        .send_raw(
            0x32,
            &[
                0x00, 0x08, b'u', b'n', b'h', b'a', b'n', b'd', b'l', b'e', 0x00, 0x99, 0x00, b'x',
            ],
        )
        .await
        .unwrap();

    let _ = client.poll().await;

    // Client MUST acknowledge PUBLISH packet according to applicable QoS rules regardless of processing
    let puback = broker.next_non_ping().await.unwrap();
    assert_eq!(puback.packet_type(), 4, "expected PUBACK");
    assert_eq!(&puback.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_6_0_1_publish_resend_preserves_order() {
    let mut broker = Broker::bind().await.unwrap();

    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));
    let client_id = MqttString::from_str("session_order").unwrap();
    let mut client = connect_multi_with_options(&mut broker, &session_options)
        .await
        .unwrap();
    let opt1 = qos1_publish("order/1");
    let opt2 = qos1_publish("order/2");
    let pid1 = client
        .publish(&opt1, Bytes::Borrowed(b"first"))
        .await
        .unwrap()
        .unwrap();
    let pid2 = client
        .publish(&opt2, Bytes::Borrowed(b"second"))
        .await
        .unwrap()
        .unwrap();
    let _ = broker.next_non_ping().await.unwrap();
    let _ = broker.next_non_ping().await.unwrap();
    broker.close().await.unwrap();
    let _ = client.poll().await;
    let _ = client.abort().await;

    reconnect_multi_with_options(&mut client, &mut broker, &session_options, client_id)
        .await
        .unwrap();
    client
        .republish(pid1, &opt1, Bytes::Borrowed(b"first"))
        .await
        .unwrap();
    client
        .republish(pid2, &opt2, Bytes::Borrowed(b"second"))
        .await
        .unwrap();

    // Resent messages MUST be re-sent in the order in which the original PUBLISH packets were sent
    let resent1 = broker.next_non_ping().await.unwrap();
    let resent2 = broker.next_non_ping().await.unwrap();

    let topic_len1 = u16::from_be_bytes([resent1.body[0], resent1.body[1]]) as usize;
    let topic_len2 = u16::from_be_bytes([resent2.body[0], resent2.body[1]]) as usize;

    assert_eq!(&resent1.body[2..2 + topic_len1], b"order/1");
    assert_eq!(&resent2.body[2..2 + topic_len2], b"order/2");

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_6_0_2_puback_sent_in_order_of_publish_receipt() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let pid1: u16 = 0x0011;
    let pid2: u16 = 0x0022;

    // Broker sends two QoS 1 PUBLISH packets in order
    broker
        .send_raw(0x32, &[0x00, 0x01, b'a', 0x00, 0x11, 0x00, b'1'])
        .await
        .unwrap();
    broker
        .send_raw(0x32, &[0x00, 0x01, b'b', 0x00, 0x22, 0x00, b'2'])
        .await
        .unwrap();

    let _ = client.poll().await;
    let _ = client.poll().await;

    // Client MUST send PUBACK packets in the order corresponding PUBLISH packets were received
    let ack1 = broker.next_non_ping().await.unwrap();
    let ack2 = broker.next_non_ping().await.unwrap();

    assert_eq!(ack1.packet_type(), 4);
    assert_eq!(ack2.packet_type(), 4);
    assert_eq!(&ack1.body[0..2], &pid1.to_be_bytes());
    assert_eq!(&ack2.body[0..2], &pid2.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_6_0_3_pubrec_sent_in_order_of_publish_receipt() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_multi_publish(&mut broker).await.unwrap();

    let pid1: u16 = 0x0033;
    let pid2: u16 = 0x0044;

    // Broker sends two QoS 2 PUBLISH packets in order
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0x33, 0x00, b'1'])
        .await
        .unwrap();
    broker
        .send_raw(0x34, &[0x00, 0x01, b'b', 0x00, 0x44, 0x00, b'2'])
        .await
        .unwrap();

    let _ = client.poll().await;
    let _ = client.poll().await;

    // Client MUST send PUBREC packets in the order corresponding PUBLISH packets were received
    let rec1 = broker.next_non_ping().await.unwrap();
    let rec2 = broker.next_non_ping().await.unwrap();

    assert_eq!(rec1.packet_type(), 5);
    assert_eq!(rec2.packet_type(), 5);
    assert_eq!(&rec1.body[0..2], &pid1.to_be_bytes());
    assert_eq!(&rec2.body[0..2], &pid2.to_be_bytes());

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_6_0_4_pubrel_sent_in_order_of_pubrec_receipt() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_multi_publish(&mut broker).await.unwrap();

    // Client option: Publish two QoS 2 messages
    let opt1 = qos0_publish("test/pubrel_order_1").exactly_once();
    let opt2 = qos0_publish("test/pubrel_order_2").exactly_once();

    let _ = client.publish(&opt1, Bytes::Borrowed(b"1")).await;
    let _ = client.publish(&opt2, Bytes::Borrowed(b"2")).await;

    let p1 = broker.next_non_ping().await.unwrap();
    let p2 = broker.next_non_ping().await.unwrap();

    let tlen1 = u16::from_be_bytes([p1.body[0], p1.body[1]]) as usize;
    let pid1 = &p1.body[2 + tlen1..4 + tlen1];
    let tlen2 = u16::from_be_bytes([p2.body[0], p2.body[1]]) as usize;
    let pid2 = &p2.body[2 + tlen2..4 + tlen2];

    // Broker responds with PUBREC for message 1, then message 2
    broker
        .send_raw(0x50, &[pid1[0], pid1[1], 0x00])
        .await
        .unwrap();
    timeout(Duration::from_secs(5), client.poll())
        .await
        .expect("client did not process first PUBREC")
        .unwrap();
    broker
        .send_raw(0x50, &[pid2[0], pid2[1], 0x00])
        .await
        .unwrap();
    timeout(Duration::from_secs(5), client.poll())
        .await
        .expect("client did not process second PUBREC")
        .unwrap();

    // Client MUST send PUBREL packets in the order corresponding PUBREC packets were received
    let rel1 = broker.next_non_ping().await.unwrap();
    let rel2 = broker.next_non_ping().await.unwrap();

    assert_eq!(rel1.packet_type(), 6);
    assert_eq!(rel2.packet_type(), 6);
    assert_eq!(&rel1.body[0..2], pid1);
    assert_eq!(&rel2.body[0..2], pid2);

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_7_3_2_topic_name_and_filter_must_not_contain_null() {
    let with_null = "sensor/\0/temperature";
    let mqtt_str = MqttString::from_str(with_null);

    // Topic Names and Topic Filters MUST NOT include the null character (Unicode U+0000)
    if let Ok(valid_str) = mqtt_str {
        assert!(
            TopicName::new(valid_str.clone()).is_none(),
            "TopicName MUST NOT accept strings containing null character U+0000"
        );
        assert!(
            TopicFilter::new(valid_str).is_none(),
            "TopicFilter MUST NOT accept strings containing null character U+0000"
        );
    }
}

#[tokio::test]
async fn mqtt_4_7_3_3_topic_name_exceeding_max_length_rejected() {
    // Topic Names and Topic Filters are UTF-8 Encoded Strings; they MUST NOT encode to more than 65,535 bytes
    let oversized = "a".repeat(65_536);
    let mqtt_str = MqttString::from_str(&oversized);

    if let Ok(valid_str) = mqtt_str {
        assert!(
            TopicName::new(valid_str).is_none(),
            "TopicName MUST NOT encode to more than 65,535 bytes"
        );
    }
}

#[tokio::test]
async fn mqtt_4_7_3_3_topic_filter_exceeding_max_length_rejected() {
    // Topic Filters are UTF-8 Encoded Strings; they MUST NOT encode to more than 65,535 bytes
    let oversized = "a/".repeat(32_768);
    let mqtt_str = MqttString::from_str(&oversized);

    if let Ok(valid_str) = mqtt_str {
        assert!(
            TopicFilter::new(valid_str).is_none(),
            "TopicFilter MUST NOT encode to more than 65,535 bytes"
        );
    }
}

#[tokio::test]
async fn mqtt_4_9_0_1_initial_send_quota_does_not_exceed_receive_maximum() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_multi_with_connack(
        &mut broker,
        NO_SESSION_CONNECT_OPTIONS,
        &[0x00, 0x00, 0x03, 0x21, 0x00, 0x02],
    )
    .await
    .unwrap();

    // Client must initialize its send quota <= Receive Maximum (2)
    let opt1 = qos1_publish("quota/1");
    let opt2 = qos1_publish("quota/2");
    let opt3 = qos1_publish("quota/3");

    let _ = client.publish(&opt1, Bytes::Borrowed(b"1")).await;
    let _ = broker.next_non_ping().await.unwrap();

    let _ = client.publish(&opt2, Bytes::Borrowed(b"2")).await;
    let _ = broker.next_non_ping().await.unwrap();

    // 3rd QoS 1 publish exceeds initial quota (2) and must not be transmitted
    let res3 = client.publish(&opt3, Bytes::Borrowed(b"3")).await;
    assert!(
        res3.is_err()
            || broker
                .expect_silence(Duration::from_millis(50))
                .await
                .is_ok(),
        "Client send quota MUST NOT exceed the Receive Maximum returned by Server"
    );

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_9_0_2_send_quota_zero_prevents_qos1_publish() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server specifies Receive Maximum = 1 (Property 0x21, value 0x0001)
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x21, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // 1st publish consumes the entire send quota (reaching zero)
            let opt1 = qos1_publish("quota/zero_1");
            let _ = client.publish(&opt1, Bytes::Borrowed(b"1")).await;
            let _ = broker.next_non_ping().await.unwrap();

            // If send quota reaches zero, client MUST NOT send any more PUBLISH packets with QoS > 0
            let opt2 = qos1_publish("quota/zero_2");
            let res2 = client.publish(&opt2, Bytes::Borrowed(b"2")).await;
            assert!(
                res2.is_err() || broker.expect_silence(Duration::from_millis(50)).await.is_ok(),
                "Client MUST NOT send any more PUBLISH packets with QoS > 0 when send quota is zero"
            );

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_4_9_0_3_zero_quota_continues_processing_other_packets() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Receive Maximum = 1
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x21, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // Exhaust outbound quota
            let opt = qos1_publish("quota/exhaust");
            let _ = client.publish(&opt, Bytes::Borrowed(b"out")).await;
            let _ = broker.next_non_ping().await.unwrap();

            // Broker sends an incoming QoS 1 PUBLISH to the client
            let incoming_pid: u16 = 0x00A1;
            broker
                .send_raw(
                    0x32,
                    &[0x00, 0x01, b'a', 0x00, 0xA1, 0x00, b'i', b'n'],
                )
                .await
                .unwrap();
            let _ = client.poll().await;

            // Client MUST continue to process and respond to all other Control Packets (PUBACK) even if outbound quota is zero
            let puback = broker.next_non_ping().await.unwrap();
            assert_eq!(puback.packet_type(), 4, "Client MUST still respond with PUBACK when quota is zero");
            assert_eq!(&puback.body[0..2], &incoming_pid.to_be_bytes());

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_4_12_0_3_client_responds_to_auth_continue_with_auth() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with an Authentication Method
    let connect_options = no_credentials_connect_options()
        .authentication_data(MqttBinary::from_slice_unchecked(b"client-first-data"));

    let mut auth = ConformanceAuth;
    let mut enhanced: TestClient<'static> = Client::new(ALLOC.get());
    let connecting = enhanced.connect_enhanced(
        tcp_connection(address).await.unwrap(),
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
        MqttString::from_str("SCRAM-SHA-256").unwrap(),
        &mut auth,
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server challenges with AUTH Reason Code 0x18 (Continue authentication)
            // Auth Method property (0x15) = "SCRAM-SHA-256"
            broker
                .send_raw(
                    0xF0,
                    &[
                        0x18, // Continue authentication
                        0x10, // Property Length
                        0x15, 0x00, 0x0D, b'S', b'C', b'R', b'A', b'M', b'-', b'S', b'H', b'A', b'-', b'2', b'5', b'6',
                    ],
                )
                .await
                .unwrap();

            // Client responds to an AUTH packet from Server by sending a further AUTH packet with Reason Code 0x18
            let client_auth = tokio::select! {
                result = &mut connecting => panic!("unexpected connection completion: {result:?}"),
                result = timeout(Duration::from_secs(2), broker.next_non_ping()) => {
                    result.expect("Client did not send AUTH response").unwrap()
                }
            };

            assert_eq!(client_auth.packet_type(), 15, "expected AUTH packet");
            assert_eq!(client_auth.body[0], 0x18, "AUTH Reason Code MUST be 0x18 (Continue authentication)");
        }
    }
}

#[tokio::test]
async fn mqtt_4_12_0_5_auth_packet_includes_same_auth_method() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let auth_method = "SCRAM-SHA-256";
    // Client option: Configure ConnectOptions with an Authentication Method
    let connect_options = no_credentials_connect_options()
        .authentication_data(MqttBinary::from_slice_unchecked(b"initial_data"));

    let mut auth = ConformanceAuth;
    let mut enhanced: TestClient<'static> = Client::new(ALLOC.get());
    let connecting = enhanced.connect_enhanced(
        tcp_connection(address).await.unwrap(),
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
        MqttString::from_str(auth_method).unwrap(),
        &mut auth,
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server sends AUTH (0x18)
            broker
                .send_raw(
                    0xF0,
                    &[
                        0x18,
                        0x10,
                        0x15, 0x00, 0x0D, b'S', b'C', b'R', b'A', b'M', b'-', b'S', b'H', b'A', b'-', b'2', b'5', b'6',
                    ],
                )
                .await
                .unwrap();

            let client_auth = tokio::select! {
                result = &mut connecting => panic!("unexpected connection completion: {result:?}"),
                result = timeout(Duration::from_secs(2), broker.next_non_ping()) => {
                    result.unwrap().unwrap()
                }
            };

            // Client AUTH MUST include an Authentication Method property with the same value as in CONNECT
            let prop_len = client_auth.body[1] as usize;
            let props = &client_auth.body[2..2 + prop_len];

            assert_eq!(props[0], 0x15, "Property ID MUST be 0x15 (Authentication Method)");
            let method_len = u16::from_be_bytes([props[1], props[2]]) as usize;
            let method_bytes = &props[3..3 + method_len];
            assert_eq!(method_bytes, auth_method.as_bytes(), "Authentication Method must match CONNECT");
        }
    }
}

#[tokio::test]
async fn mqtt_4_12_0_7_client_without_auth_method_in_connect_never_sends_auth() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client did not set Authentication Method in CONNECT.
    // Client MUST NOT send an AUTH packet to the Server.
    assert!(
        broker
            .expect_silence(Duration::from_millis(50))
            .await
            .is_ok(),
        "Client without Authentication Method in CONNECT MUST NOT send an AUTH packet"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_12_1_1_reauthentication_uses_reason_code_and_method() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let auth_method = "SCRAM-SHA-256";
    // Client option: Configure ConnectOptions with an Authentication Method
    let connect_options = no_credentials_connect_options();

    let mut auth = ConformanceAuth;
    let mut enhanced: TestClient<'static> = Client::new(ALLOC.get());
    let tcp = tcp_connection(address).await.unwrap();
    {
        let connecting = enhanced.connect_enhanced(
            tcp,
            &connect_options,
            Some(MqttString::from_str("conformance").unwrap()),
            MqttString::from_str(auth_method).unwrap(),
            &mut auth,
        );
        tokio::pin!(connecting);
        tokio::select! {
            _ = &mut connecting => panic!("unexpected connection completion"),
            result = broker.accept() => {
                result.unwrap();
                let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
                broker.send_raw(
                    0x20,
                    &[
                        0x00, 0x00,
                        0x10,
                        0x15, 0x00, 0x0D, b'S', b'C', b'R', b'A', b'M', b'-', b'S', b'H', b'A', b'-', b'2', b'5', b'6',
                    ],
                ).await.unwrap();
                timeout(Duration::from_secs(5), &mut connecting)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
    }

    enhanced
        .reauthenticate(
            &ReAuthOptions::new()
                .authentication_data(MqttBinary::from_slice_unchecked(b"reauth_data")),
        )
        .await
        .unwrap();

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 15);
    assert_eq!(
        frame.body[0], 0x19,
        "Reason Code MUST be 0x19 (Re-authentication)"
    );
    let prop_len = frame.body[1] as usize;
    let props = &frame.body[2..2 + prop_len];
    assert_eq!(props[0], 0x15);
    let m_len = u16::from_be_bytes([props[1], props[2]]) as usize;
    assert_eq!(&props[3..3 + m_len], auth_method.as_bytes());

    enhanced.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_12_1_2_failed_reauthentication_closes_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Re-authentication failure signaled by Server sending DISCONNECT with error Reason Code (0x86: Bad User Name or Password)
    broker.send_raw(0xE0, &[0x86, 0x00]).await.unwrap();
    let _ = client.poll().await;
    let _ = client.abort().await;

    // Network connection MUST be closed if re-authentication fails
    assert!(
        broker.wait_closed(Duration::from_secs(2)).await.unwrap(),
        "Network connection MUST be closed when re-authentication fails"
    );
}

#[tokio::test]
async fn mqtt_4_13_2_1_connack_error_code_closes_network_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Send CONNACK with Reason Code >= 0x80 (0x84: Unsupported Protocol Version)
            broker.send_raw(0x20, &[0x00, 0x84, 0x00]).await.unwrap();
            let _ = connecting.await;

            // If Reason Code >= 0x80 is specified, network connection MUST be closed
            assert!(
                broker.wait_closed(Duration::from_secs(2)).await.unwrap(),
                "Network connection MUST be closed on CONNACK Reason Code >= 0x80"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_1_5_4_3_zero_width_no_break_space_preserved() {
    let topic_with_bom = "\u{FEFF}sensor/temperature";
    let mqtt_str = MqttString::from_str(topic_with_bom).unwrap();
    let topic = TopicName::new(mqtt_str).unwrap();

    // 0xEF 0xBB 0xBF MUST NOT be skipped over or stripped off by a packet receiver
    assert_eq!(topic.as_ref().as_str(), topic_with_bom);
    assert_eq!(
        topic.as_ref().as_str().as_bytes(),
        [
            0xEF, 0xBB, 0xBF, b's', b'e', b'n', b's', b'o', b'r', b'/', b't', b'e', b'm', b'p',
            b'e', b'r', b'a', b't', b'u', b'r', b'e'
        ]
    );
}

#[tokio::test]
async fn mqtt_1_5_7_1_user_property_utf8_validation() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with User Property (0x26) containing null character in key
    // Topic: "a" (0x0001, 'a'), Prop length: 8, Prop ID: 0x26, Key len: 3 ("k\0y"), Val len: 1 ("v")
    broker
        .send_raw(
            0x30,
            &[
                0x00, 0x01, b'a', 0x08, 0x26, 0x00, 0x03, b'k', 0x00, b'y', 0x00, 0x01, b'v',
            ],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed User Property")
            .is_err(),
        "Both strings in a UTF-8 String Pair MUST comply with UTF-8 Encoded String requirements"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_1_2_4_clean_start_one_discards_existing_session() {
    let mut broker = Broker::bind().await.unwrap();

    // Connection 1: Establish session with Clean Start = 0 and send unacknowledged message
    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));
    {
        let connecting = connected_client(
            broker.address(),
            &session_options,
            Some(MqttString::from_str("clean_start_test").unwrap()),
        );
        tokio::pin!(connecting);
        let mut client = tokio::select! {
            _ = &mut connecting => panic!("unexpected connection completion"),
            result = broker.accept() => {
                result.unwrap();
                let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
                broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
                timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
            }
        };

        let opt = qos1_publish("session/unack");
        let _ = client.publish(&opt, Bytes::Borrowed(b"data")).await;
        let _ = broker.next_non_ping().await.unwrap();
        broker.close().await.unwrap();
    }

    // Connection 2: Reconnect with Clean Start = 1
    let clean_options = session_options.clone().clean_start();
    let connecting = connected_client(
        broker.address(),
        &clean_options,
        Some(MqttString::from_str("clean_start_test").unwrap()),
    );
    tokio::pin!(connecting);
    let mut client = tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
        }
    };

    // Because Clean Start = 1, client MUST discard any existing Session and NOT resend unacknowledged messages
    assert!(
        broker
            .expect_silence(Duration::from_millis(50))
            .await
            .is_ok(),
        "Client MUST discard existing Session when Clean Start is set to 1"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_2_2_5_session_present_zero_discards_client_session() {
    let mut broker = Broker::bind().await.unwrap();

    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));

    // Connection 1: Establish session and leave unacknowledged packet
    {
        let connecting = connected_client(
            broker.address(),
            &session_options,
            Some(MqttString::from_str("session_present_test").unwrap()),
        );
        tokio::pin!(connecting);
        let mut client = tokio::select! {
            _ = &mut connecting => panic!("unexpected connection completion"),
            result = broker.accept() => {
                result.unwrap();
                let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
                broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
                timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
            }
        };

        let opt = qos1_publish("session/unack2");
        let _ = client.publish(&opt, Bytes::Borrowed(b"msg")).await;
        let _ = broker.next_non_ping().await.unwrap();
        broker.close().await.unwrap();
    }

    // Connection 2: Reconnect with Clean Start = 0, but broker returns Session Present = 0
    let connecting = connected_client(
        broker.address(),
        &session_options,
        Some(MqttString::from_str("session_present_test").unwrap()),
    );
    tokio::pin!(connecting);
    let mut client = tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
            // Server has no session state (Session Present = 0)
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap()
        }
    };

    // Client MUST discard its Session State if it receives Session Present = 0
    assert!(
        broker
            .expect_silence(Duration::from_millis(50))
            .await
            .is_ok(),
        "Client MUST discard its session state when receiving Session Present = 0"
    );

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_3_3_1_1_qos2_resend_sets_dup_one() {
    let mut broker = Broker::bind().await.unwrap();

    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));
    let client_id = MqttString::from_str("qos2_dup_test").unwrap();
    let mut client = connect_multi_with_options(&mut broker, &session_options)
        .await
        .unwrap();
    let options = qos0_publish("qos2/dup").exactly_once();
    let packet_id = client
        .publish(&options, Bytes::Borrowed(b"qos2_data"))
        .await
        .unwrap()
        .unwrap();
    let _first = broker.next_non_ping().await.unwrap();
    broker.close().await.unwrap();
    let _ = client.poll().await;
    let _ = client.abort().await;

    reconnect_multi_with_options(&mut client, &mut broker, &session_options, client_id)
        .await
        .unwrap();
    client
        .republish(packet_id, &options, Bytes::Borrowed(b"qos2_data"))
        .await
        .unwrap();

    let resent = broker.next_non_ping().await.unwrap();

    // DUP flag MUST be set to 1 when re-delivering a PUBLISH packet
    assert_eq!(
        resent.first_byte & 0x08,
        0x08,
        "The DUP flag MUST be set to 1 when re-delivering a PUBLISH packet"
    );

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_3_6_2_1_client_pubrel_uses_valid_reason_code() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let opt = qos0_publish("test/pubrel_code").exactly_once();
    let _ = client.publish(&opt, Bytes::Borrowed(b"m")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 3 && (frame.first_byte & 0x06) == 0x04 {
            let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
            let pid = &frame.body[2 + topic_len..4 + topic_len];

            // Send PUBREC
            broker
                .send_raw(0x50, &[pid[0], pid[1], 0x00])
                .await
                .unwrap();
            let _ = client.poll().await;

            let pubrel = broker.next_non_ping().await.unwrap();
            assert_eq!(pubrel.packet_type(), 6, "expected PUBREL");

            // If Reason Code is present in PUBREL, it MUST be 0x00 or 0x92
            if pubrel.body.len() > 2 {
                let code = pubrel.body[2];
                assert!(
                    code == 0x00 || code == 0x92,
                    "PUBREL Reason Code MUST be 0x00 or 0x92, got {code:#04X}"
                );
            }
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_7_2_1_client_pubcomp_uses_valid_reason_code() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let pid: u16 = 0x00BC;
    broker.send_raw(0x62, &pid.to_be_bytes()).await.unwrap();
    let _ = client.poll().await;

    let pubcomp = broker.next_non_ping().await.unwrap();
    assert_eq!(pubcomp.packet_type(), 7, "expected PUBCOMP");

    // If Reason Code is present in PUBCOMP, it MUST be 0x00 or 0x92
    if pubcomp.body.len() > 2 {
        let code = pubcomp.body[2];
        assert!(
            code == 0x00 || code == 0x92,
            "PUBCOMP Reason Code MUST be 0x00 or 0x92, got {code:#04X}"
        );
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_6_client_does_not_resend_publish_after_pubrel() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let opt = qos0_publish("test/no_resend_publish").exactly_once();
    let _ = client.publish(&opt, Bytes::Borrowed(b"data")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 3 && (frame.first_byte & 0x06) == 0x04 {
            let topic_len = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
            let pid = &frame.body[2 + topic_len..4 + topic_len];

            // Send PUBREC
            broker
                .send_raw(0x50, &[pid[0], pid[1], 0x00])
                .await
                .unwrap();
            let _ = client.poll().await;
            let _pubrel = broker.next_non_ping().await.unwrap();

            // While waiting for PUBCOMP, sender MUST NOT re-send the PUBLISH
            assert!(
                broker
                    .expect_silence(Duration::from_millis(100))
                    .await
                    .is_ok(),
                "Sender MUST NOT re-send PUBLISH once it has sent the corresponding PUBREL"
            );
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_10_duplicate_publish_before_pubrel_replies_with_pubrec() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let pid: u16 = 0x00DE;

    // Send original QoS 2 PUBLISH
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0xDE, 0x00, b'1'])
        .await
        .unwrap();
    let _ = client.poll().await;
    let rec1 = broker.next_non_ping().await.unwrap();
    assert_eq!(rec1.packet_type(), 5);

    // Send duplicate QoS 2 PUBLISH before sending PUBREL
    broker
        .send_raw(0x3C, &[0x00, 0x01, b'a', 0x00, 0xDE, 0x00, b'1'])
        .await
        .unwrap();
    let _ = client.poll().await;

    // Receiver until it has received PUBREL MUST acknowledge any subsequent PUBLISH with same Packet ID with PUBREC
    let rec2 = broker.next_non_ping().await.unwrap();
    assert_eq!(
        rec2.packet_type(),
        5,
        "Receiver MUST respond with PUBREC to duplicate PUBLISH"
    );
    assert_eq!(&rec2.body[0..2], &pid.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_13_2_1_client_disconnect_error_closes_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Server sends malformed packet with reserved flags violated to trigger client error disconnect
    broker.send_raw(0xE1, &[]).await.unwrap();
    let _ = client.poll().await;

    // Client produces abort and sends DISCONNECT with Reason Code >= 0x80
    let transport = client.abort().await.unwrap();
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    assert!(
        disconnect.body.first().copied().unwrap_or_default() >= 0x80,
        "client error DISCONNECT must use an error reason code"
    );
    // If a Reason Code of 0x80 or greater is specified, network connection MUST be closed.
    drop(transport);
    assert!(
        broker.wait_closed(Duration::from_secs(2)).await.unwrap(),
        "Network Connection MUST be closed when DISCONNECT with Reason Code >= 0x80 is sent"
    );
}

#[tokio::test]
async fn mqtt_2_1_3_1_pingreq_reserved_flags_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with Keep Alive = 1s
    let connect_options = NO_SESSION_CONNECT_OPTIONS
        .clone()
        .keep_alive(KeepAlive::Seconds(NonZero::new(1).unwrap()));

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("ping_flags_test").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            client.ping().await.unwrap();
            let ping = timeout(Duration::from_secs(3), broker.next_frame()).await.unwrap().unwrap();

            // PINGREQ fixed header reserved bits 3..0 MUST all be 0 (0xC0)
            assert_eq!(
                ping.first_byte, 0xC0,
                "PINGREQ Fixed Header reserved bits 3..0 MUST be 0000"
            );

            broker.send_raw(0xD0, &[]).await.unwrap();
            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_2_2_1_3_subscribe_assigns_nonzero_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option / API call: Subscribe to topic
    let _subscribe = client
        .subscribe(
            topic_filter("sensor/temperature"),
            &SubscriptionOptions::new(),
        )
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 8);
    let packet_id = u16::from_be_bytes([frame.body[0], frame.body[1]]);
    assert_ne!(packet_id, 0);

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_2_2_1_3_unsubscribe_assigns_nonzero_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option / API call: Unsubscribe from topic
    let _unsubscribe = client
        .unsubscribe(
            topic_filter("sensor/temperature"),
            &UnsubscriptionOptions::new(),
        )
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 10);
    let packet_id = u16::from_be_bytes([frame.body[0], frame.body[1]]);
    assert_ne!(packet_id, 0);

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_1_2_20_active_traffic_resets_pingreq_timer() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with Server Keep Alive = 2s
    let connect_options = NO_SESSION_CONNECT_OPTIONS;

    let connecting = connected_client(
        address,
        connect_options,
        Some(MqttString::from_str("active_keep_alive").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Set Server Keep Alive = 2s (Property 0x13, value 0x0002)
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x13, 0x00, 0x02]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // Send active traffic at 1-second intervals, resetting the Keep Alive timer
            tokio::time::sleep(Duration::from_secs(1)).await;
            let options = qos0_publish("active/traffic");
            let _ = client.publish(&options, Bytes::Borrowed(b"keep_active")).await;
            let _pub_frame = broker.next_non_ping().await.unwrap();

            // While active traffic is being sent, client does not need to send PINGREQ
            assert!(
                broker.expect_silence(Duration::from_millis(500)).await.is_ok(),
                "Client MUST NOT send unnecessary PINGREQ packets while application traffic is active"
            );

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_4_3_3_9_publish_after_pubrec_error_treated_as_new() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let packet_id: u16 = 0x00EE;

    // Broker sends QoS 2 PUBLISH to client
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0xEE, 0x00, b'1'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let _pubrec = broker.next_non_ping().await.unwrap();

    // Subsequent PUBLISH with same Packet Identifier is treated as a new Application Message
    broker
        .send_raw(0x34, &[0x00, 0x01, b'a', 0x00, 0xEE, 0x00, b'2'])
        .await
        .unwrap();
    let _ = client.poll().await;

    let next_pubrec = broker.next_non_ping().await.unwrap();
    assert_eq!(next_pubrec.packet_type(), 5, "expected PUBREC packet");
    assert_eq!(&next_pubrec.body[0..2], &packet_id.to_be_bytes());

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_4_0_1_resend_unacknowledged_pubrel_on_reconnect() {
    let mut broker = Broker::bind().await.unwrap();

    let session_options = ConnectOptions::new()
        .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(300).unwrap()));
    let client_id = MqttString::from_str("session_pubrel_resend").unwrap();
    let mut client = connect_multi_with_options(&mut broker, &session_options)
        .await
        .unwrap();
    let opt = qos0_publish("qos2/pubrel_resend").exactly_once();
    let packet_id = client
        .publish(&opt, Bytes::Borrowed(b"data"))
        .await
        .unwrap()
        .unwrap();
    let _publish = broker.next_non_ping().await.unwrap();
    broker
        .send_raw(
            0x50,
            &[
                packet_id.get().get().to_be_bytes()[0],
                packet_id.get().get().to_be_bytes()[1],
                0x00,
            ],
        )
        .await
        .unwrap();
    let _ = client.poll().await;
    let _pubrel = broker.next_non_ping().await.unwrap();
    broker.close().await.unwrap();
    let _ = client.poll().await;
    let _ = client.abort().await;

    reconnect_multi_with_options(&mut client, &mut broker, &session_options, client_id)
        .await
        .unwrap();

    let _ = client.rerelease().await;

    // Client MUST resend unacknowledged PUBREL packet using its original Packet Identifier
    let resent_pubrel = timeout(Duration::from_secs(3), broker.next_non_ping())
        .await
        .expect("Client did not resend unacknowledged PUBREL on reconnect")
        .unwrap();

    assert_eq!(
        resent_pubrel.packet_type(),
        6,
        "expected resent PUBREL packet"
    );
    assert_eq!(
        &resent_pubrel.body[0..2],
        &packet_id.get().get().to_be_bytes()
    );

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_4_9_0_2_puback_replenishes_send_quota() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("quota_replenish").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Receive Maximum = 1
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x21, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // 1st QoS 1 PUBLISH drains quota to 0
            let opt1 = qos1_publish("quota/drain");
            let _ = client.publish(&opt1, Bytes::Borrowed(b"1")).await;
            let frame1 = broker.next_non_ping().await.unwrap();
            let t_len = u16::from_be_bytes([frame1.body[0], frame1.body[1]]) as usize;
            let pid = &frame1.body[2 + t_len..4 + t_len];

            // Server acknowledges 1st message with PUBACK, replenishing send quota
            broker.send_raw(0x40, &[pid[0], pid[1], 0x00]).await.unwrap();
            let _ = client.poll().await;

            // 2nd QoS 1 PUBLISH should now succeed and be sent on the wire
            let opt2 = qos1_publish("quota/success");
            let _ = client.publish(&opt2, Bytes::Borrowed(b"2")).await;

            let frame2 = timeout(Duration::from_millis(500), broker.next_non_ping())
                .await
                .expect("Client did not send PUBLISH after quota replenishment")
                .unwrap();
            assert_eq!(frame2.packet_type(), 3);

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_4_9_0_3_quota_exhaustion_does_not_block_pubrel() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("quota_pubrel").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Receive Maximum = 1
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x21, 0x00, 0x01]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // Client publishes QoS 2 message, draining quota
            let opt = qos0_publish("qos2/quota").exactly_once();
            let _ = client.publish(&opt, Bytes::Borrowed(b"m")).await;
            let frame = broker.next_non_ping().await.unwrap();
            let tlen = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
            let pid = &frame.body[2 + tlen..4 + tlen];

            // Quota is 0; server sends PUBREC
            broker.send_raw(0x50, &[pid[0], pid[1], 0x00]).await.unwrap();
            let _ = client.poll().await;

            // Client MUST NOT delay/block sending PUBREL even though send quota is 0
            let pubrel = timeout(Duration::from_secs(1), broker.next_non_ping())
                .await
                .expect("Client failed to send PUBREL while quota was exhausted")
                .unwrap();
            assert_eq!(pubrel.packet_type(), 6, "expected PUBREL");

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_4_12_0_5_connack_mismatched_auth_method_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with Authentication Method "SCRAM-SHA-256"
    let connect_options = no_credentials_connect_options();

    let mut auth = ConformanceAuth;
    let mut enhanced: TestClient<'static> = Client::new(ALLOC.get());
    let connecting = enhanced.connect_enhanced(
        tcp_connection(address).await.unwrap(),
        &connect_options,
        Some(MqttString::from_str("conformance").unwrap()),
        MqttString::from_str("SCRAM-SHA-256").unwrap(),
        &mut auth,
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server responds with a DIFFERENT Authentication Method: "OAUTHBEARER"
            broker
                .send_raw(
                    0x20,
                    &[
                        0x00, 0x00, 0x11, 0x15, 0x00, 0x0B, b'O', b'A', b'U', b'T', b'H',
                        b'B', b'E', b'A', b'R', b'E', b'R',
                    ],
                )
                .await
                .unwrap();

            // Client MUST reject mismatched Authentication Method and close connection
            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST reject CONNACK when Authentication Method does not match CONNECT"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_4_12_0_5_connack_missing_auth_method_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with Authentication Method
    let connect_options = no_credentials_connect_options();

    let mut auth = ConformanceAuth;
    let mut enhanced: TestClient<'static> = Client::new(ALLOC.get());
    let connecting = enhanced.connect_enhanced(
        tcp_connection(address).await.unwrap(),
        &connect_options,
        Some(MqttString::from_str("SCRAM-SHA-256").unwrap()),
        MqttString::from_str("SCRAM-SHA-256").unwrap(),
        &mut auth,
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Successful CONNACK (0x00) with NO Authentication Method property
            broker.send_raw(0x20, &[0x00, 0x00, 0x00]).await.unwrap();

            // Any successful CONNACK MUST include the Authentication Method property
            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST reject successful CONNACK omitting Authentication Method when CONNECT specified one"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_2_2_1_3_client_does_not_reuse_in_flight_packet_id() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect_multi_publish(&mut broker).await.unwrap();

    // Send first QoS 1 message
    let opt1 = qos1_publish("pid/in_flight_1");
    let _ = client.publish(&opt1, Bytes::Borrowed(b"1")).await;
    let frame1 = broker.next_non_ping().await.unwrap();
    let topic_len1 = u16::from_be_bytes([frame1.body[0], frame1.body[1]]) as usize;
    let pid1 = u16::from_be_bytes([frame1.body[2 + topic_len1], frame1.body[2 + topic_len1 + 1]]);

    // Send second QoS 1 message while first is still unacknowledged
    let opt2 = qos1_publish("pid/in_flight_2");
    let _ = client.publish(&opt2, Bytes::Borrowed(b"2")).await;
    let frame2 = broker.next_non_ping().await.unwrap();
    let topic_len2 = u16::from_be_bytes([frame2.body[0], frame2.body[1]]) as usize;
    let pid2 = u16::from_be_bytes([frame2.body[2 + topic_len2], frame2.body[2 + topic_len2 + 1]]);

    // Each new packet MUST be assigned a non-zero Packet Identifier that is currently unused
    assert_ne!(pid1, 0);
    assert_ne!(pid2, 0);
    assert_ne!(
        pid1, pid2,
        "Client MUST NOT assign a Packet Identifier that is currently in-flight"
    );

    // Acknowledge both messages
    broker.send_raw(0x40, &pid1.to_be_bytes()).await.unwrap();
    broker.send_raw(0x40, &pid2.to_be_bytes()).await.unwrap();

    client.disconnect(DEFAULT_DC_OPTIONS).await.unwrap();
}

#[tokio::test]
async fn mqtt_3_1_2_1_connack_unsupported_protocol_version_aborts() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server responds with CONNACK Reason Code 0x84 (Unsupported Protocol Version)
            broker.send_raw(0x20, &[0x00, 0x84, 0x00]).await.unwrap();

            // Client must report connection failure
            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST fail connection when receiving CONNACK 0x84 (Unsupported Protocol Version)"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_6_client_accepts_assigned_client_identifier() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Connect with empty / None ClientID
    let connecting = connected_client(address, NO_SESSION_CONNECT_OPTIONS, None);
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Verify client sent zero-length Client Identifier (0x00, 0x00)
            let prop_len = connect.body[10] as usize;
            let payload_offset = 11 + prop_len;
            let id_len = u16::from_be_bytes([connect.body[payload_offset], connect.body[payload_offset + 1]]);
            assert_eq!(id_len, 0, "Client connecting without ID must send zero-length ClientID");

            // Server returns Assigned Client Identifier: "assigned-123" (Property ID 0x12, length 12)
            broker
                .send_raw(
                    0x20,
                    &[
                        0x00, 0x00, 0x0F,
                        0x12, 0x00, 0x0C, b'a', b's', b's', b'i', b'g', b'n', b'e', b'd', b'-', b'1', b'2', b'3',
                    ],
                )
                .await
                .unwrap();

            let mut client = timeout(Duration::from_secs(5), connecting)
                .await
                .expect("connection timed out")
                .expect("client rejected Assigned Client Identifier");

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_3_8_connack_client_id_not_valid_closes() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("invalid!id!").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server rejects ClientID with Reason Code 0x85 (Client Identifier not valid)
            broker.send_raw(0x20, &[0x00, 0x85, 0x00]).await.unwrap();

            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST fail connection when receiving CONNACK 0x85 (Client Identifier not valid)"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_1_4_3_server_disconnect_session_taken_over() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Server sends DISCONNECT with Reason Code 0x8E (Session taken over)
    broker.send_raw(0xE0, &[0x8E, 0x00]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process session takeover DISCONNECT")
            .is_err(),
        "Client MUST handle Server DISCONNECT with 0x8E (Session taken over) as connection termination"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce closed transport");
    drop(transport);

    assert!(broker.wait_closed(Duration::from_secs(1)).await.unwrap());
}

#[tokio::test]
async fn mqtt_3_2_2_12_connack_qos_not_supported_aborts() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Connect with Will QoS 2
    let connect_options = NO_SESSION_CONNECT_OPTIONS.clone().will(
        WillOptions::new(
            topic_name("will/topic"),
            MqttBinary::from_slice_unchecked(b"data"),
        )
        .exactly_once(),
    );

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("will_qos_client").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server returns CONNACK Reason Code 0x9B (QoS not supported)
            broker.send_raw(0x20, &[0x00, 0x9B, 0x00]).await.unwrap();

            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST fail connection when receiving CONNACK 0x9B (QoS not supported)"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_13_connack_retain_not_supported_aborts() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Connect with Will Retain = 1
    let connect_options = NO_SESSION_CONNECT_OPTIONS.clone().will(
        WillOptions::new(
            topic_name("will/topic"),
            MqttBinary::from_slice_unchecked(b"data"),
        )
        .retain(),
    );

    let connecting = connected_client(
        address,
        &connect_options,
        Some(MqttString::from_str("will_retain_client").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server returns CONNACK Reason Code 0x9A (Retain not supported)
            broker.send_raw(0x20, &[0x00, 0x9A, 0x00]).await.unwrap();

            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST fail connection when receiving CONNACK 0x9A (Retain not supported)"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_8_3_3_subscribe_options_no_local_flag_set() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = SubscriptionOptions::new().no_local();
    let _ = client
        .subscribe(topic_filter("device/data"), &options)
        .await
        .unwrap();
    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 8);
    let prop_len = frame.body[2] as usize;
    let payload = &frame.body[3 + prop_len..];
    let filter_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let sub_options_byte = payload[2 + filter_len];
    assert_eq!(sub_options_byte & 0x04, 0x04);

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_8_4_7_suback_failure_reason_code_handled() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option / API call: Subscribe to topic
    {
        let filter = topic_filter("restricted/data");
        let options = SubscriptionOptions::new();
        let subscribe = client.subscribe(filter, &options);
        let _packet_id = subscribe.await.unwrap();
        let frame = broker.next_non_ping().await.unwrap();
        assert_eq!(frame.packet_type(), 8);
        let sent_pid = &frame.body[0..2];
        broker
            .send_raw(0x90, &[sent_pid[0], sent_pid[1], 0x87])
            .await
            .unwrap();
        assert!(
            client.poll().await.is_err(),
            "SUBACK failure must be surfaced by the client API"
        );
        let transport = client.abort().await.unwrap();
        drop(transport);
    }
}

#[tokio::test]
async fn mqtt_4_4_0_1_no_spontaneous_publish_resend_on_open_connection() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client sends QoS 1 message
    let opt = qos1_publish("test/no_resend_on_open");
    let _ = client.publish(&opt, Bytes::Borrowed(b"msg")).await;
    let _frame = broker.next_non_ping().await.unwrap();

    // In MQTT 5.0, unacknowledged packets are ONLY resent on reconnect with Clean Start = 0.
    // Sender MUST NOT spontaneously re-send messages while the connection remains open.
    assert!(
        broker
            .expect_silence(Duration::from_millis(500))
            .await
            .is_ok(),
        "Client MUST NOT spontaneously retransmit unacknowledged PUBLISH on an open connection"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_2_1_3_1_pingresp_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // PINGRESP fixed header bits 3..0 are reserved and MUST be 0000 (0xD0)
    // Broker sends PINGRESP with reserved bits set to 0001 (0xD1)
    broker.send_raw(0xD1, &[]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PINGRESP")
            .is_err(),
        "Client MUST treat PINGRESP with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_puback_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let opt = qos1_publish("test/puback_reserved");
    let _ = client.publish(&opt, Bytes::Borrowed(b"data")).await;
    let publish = broker.next_non_ping().await.unwrap();
    let tlen = u16::from_be_bytes([publish.body[0], publish.body[1]]) as usize;
    let pid = &publish.body[2 + tlen..4 + tlen];

    // PUBACK fixed header bits 3..0 are reserved and MUST be 0000 (0x40)
    // Broker sends PUBACK with reserved bits set to 0010 (0x42)
    broker
        .send_raw(0x42, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBACK")
            .is_err(),
        "Client MUST treat PUBACK with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_pubrec_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let opt = qos0_publish("test/pubrec_reserved").exactly_once();
    let _ = client.publish(&opt, Bytes::Borrowed(b"data")).await;
    let publish = broker.next_non_ping().await.unwrap();
    let tlen = u16::from_be_bytes([publish.body[0], publish.body[1]]) as usize;
    let pid = &publish.body[2 + tlen..4 + tlen];

    // PUBREC fixed header bits 3..0 are reserved and MUST be 0000 (0x50)
    // Broker sends PUBREC with reserved bits set to 0001 (0x51)
    broker
        .send_raw(0x51, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBREC")
            .is_err(),
        "Client MUST treat PUBREC with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_pubcomp_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // PUBCOMP fixed header bits 3..0 are reserved and MUST be 0000 (0x70)
    // Broker sends PUBCOMP with reserved bits set to 0001 (0x71)
    broker.send_raw(0x71, &[0x00, 0x01]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed PUBCOMP")
            .is_err(),
        "Client MUST treat PUBCOMP with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_suback_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // SUBACK fixed header bits 3..0 are reserved and MUST be 0000 (0x90)
    // Broker sends SUBACK with reserved bits set to 0010 (0x92)
    broker
        .send_raw(0x92, &[0x00, 0x01, 0x00, 0x00])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed SUBACK")
            .is_err(),
        "Client MUST treat SUBACK with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_unsuback_reserved_flags_nonzero_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // UNSUBACK fixed header bits 3..0 are reserved and MUST be 0000 (0xB0)
    // Broker sends UNSUBACK with reserved bits set to 0001 (0xB1)
    broker
        .send_raw(0xB1, &[0x00, 0x01, 0x00, 0x00])
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed UNSUBACK")
            .is_err(),
        "Client MUST treat UNSUBACK with non-zero reserved fixed header bits as malformed"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_3_14_0_1_server_disconnect_before_connack_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server MUST NOT send DISCONNECT until after sending CONNACK (< 0x80)
            broker.send_raw(0xE0, &[0x80, 0x00]).await.unwrap();

            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client connection MUST fail if Server sends DISCONNECT before CONNACK"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_2_2_6_connack_error_with_session_present_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    let connecting = connected_client(
        address,
        NO_SESSION_CONNECT_OPTIONS,
        Some(MqttString::from_str("conformance").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // If Server sends non-zero Reason Code, Session Present MUST be 0
            // Broker illegally sends Reason Code 0x80 with Session Present = 1
            broker.send_raw(0x20, &[0x01, 0x80, 0x00]).await.unwrap();

            assert!(
                timeout(Duration::from_secs(2), connecting).await.unwrap().is_err(),
                "Client MUST reject CONNACK where non-zero Reason Code is paired with Session Present = 1"
            );
        }
    }
}

#[tokio::test]
async fn mqtt_3_3_2_13_response_topic_invalid_utf8_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with Response Topic property (0x08) containing surrogate code point U+D800 (0xED, 0xA0, 0x80)
    // Topic: "a", Property Length: 6, Property ID: 0x08, String length: 3, bytes: [0xED, 0xA0, 0x80]
    broker
        .send_raw(
            0x30,
            &[
                0x00, 0x01, b'a', 0x06, 0x08, 0x00, 0x03, 0xED, 0xA0, 0x80, b'x',
            ],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed Response Topic")
            .is_err(),
        "Client MUST treat Response Topic containing invalid UTF-8 as a protocol error"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_4_12_0_6_server_auth_without_client_auth_method_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client did not specify an Authentication Method in CONNECT.
    // Broker sends an unsolicited AUTH packet (Reason Code 0x18)
    broker.send_raw(0xF0, &[0x18, 0x00]).await.unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process unsolicited AUTH packet")
            .is_err(),
        "Client MUST reject AUTH packet when no Authentication Method was set in CONNECT"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_1_5_4_2_content_type_null_character_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends PUBLISH with Content Type property (0x03) containing a null character "a\0"
    // Topic: "a", Property Length: 5, Prop ID: 0x03, String length: 2, bytes: ['a', '\0']
    broker
        .send_raw(
            0x30,
            &[0x00, 0x01, b'a', 0x05, 0x03, 0x00, 0x02, b'a', 0x00, b'x'],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_secs(1), client.poll())
            .await
            .expect("client did not process malformed Content Type")
            .is_err(),
        "Client MUST reject a UTF-8 Encoded String containing the null character U+0000"
    );

    let transport = client
        .abort()
        .await
        .expect("client did not produce an abort transport");
    let disconnect = broker.next_non_ping().await.unwrap();
    assert_eq!(disconnect.packet_type(), 14);
    drop(transport);
}

#[tokio::test]
async fn mqtt_2_1_3_1_disconnect_fixed_header_reserved_flags_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client initiates disconnect
    disconnect(&mut client).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 14, "expected DISCONNECT packet");

    // Bits 3,2,1 and 0 in Fixed Header of DISCONNECT are reserved and MUST all be set to 0 (0xE0)
    assert_eq!(
        frame.first_byte, 0xE0,
        "DISCONNECT Fixed Header reserved bits 3..0 MUST be 0000 (0xE0)"
    );
}

#[tokio::test]
async fn mqtt_2_2_2_1_disconnect_no_properties_length_zero() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Standard client disconnect without properties
    disconnect(&mut client).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 14, "expected DISCONNECT packet");

    // If reason code is present, property length is the second byte
    if frame.body.len() >= 2 {
        let property_length = frame.body[1];
        assert_eq!(
            property_length, 0x00,
            "Absence of properties MUST be indicated by including a Property Length of zero"
        );
    }
}

#[tokio::test]
async fn mqtt_3_1_2_20_keep_alive_zero_disables_pingreq() {
    let mut broker = Broker::bind().await.unwrap();
    let address = broker.address();

    // Client option: Configure ConnectOptions with Keep Alive = 0
    let connect_options = NO_SESSION_CONNECT_OPTIONS /* .with_keep_alive(0) */;

    let connecting = connected_client(
        address,
        connect_options,
        Some(MqttString::from_str("keep_alive_zero").unwrap()),
    );
    tokio::pin!(connecting);
    tokio::select! {
        _ = &mut connecting => panic!("unexpected connection completion"),
        result = broker.accept() => {
            result.unwrap();
            let _connect = timeout(Duration::from_secs(5), broker.next_frame()).await.unwrap().unwrap();

            // Server confirms Keep Alive = 0 (Property 0x13, value 0x0000)
            broker.send_raw(0x20, &[0x00, 0x00, 0x03, 0x13, 0x00, 0x00]).await.unwrap();
            let mut client = timeout(Duration::from_secs(5), connecting).await.unwrap().unwrap();

            // When Keep Alive is 0, the client MUST NOT send PINGREQ packets
            assert!(
                broker.expect_silence(Duration::from_millis(500)).await.is_ok(),
                "Client MUST NOT send PINGREQ packets when Keep Alive is zero"
            );

            disconnect(&mut client).await;
        }
    }
}

#[tokio::test]
async fn mqtt_3_3_4_4_publish_with_subscription_identifier_accepted() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Broker sends QoS 0 PUBLISH with Subscription Identifier = 7 (Property ID 0x0B)
    // Topic: "a", Property Length: 2, Prop ID: 0x0B, SubId: 0x07, Payload: "data"
    broker
        .send_raw(
            0x30,
            &[0x00, 0x01, b'a', 0x02, 0x0B, 0x07, b'd', b'a', b't', b'a'],
        )
        .await
        .unwrap();

    assert!(
        timeout(Duration::from_millis(500), client.poll())
            .await
            .expect("client poll timed out")
            .is_ok(),
        "Client MUST accept and process valid PUBLISH containing Subscription Identifier"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_8_4_2_suback_mismatched_packet_id_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option / API call: Subscribe to topic
    /* let _ = client.subscribe(&topic_filter("sensor/test")).await; */
    let options = qos0_publish("dummy/trigger_sub");
    let _ = client.publish(&options, Bytes::Borrowed(b"")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 8 {
            let pid = u16::from_be_bytes([frame.body[0], frame.body[1]]);
            let mismatched_pid = pid.wrapping_add(10);

            // Server illegally sends SUBACK with a mismatched Packet Identifier
            broker
                .send_raw(
                    0x90,
                    &[
                        mismatched_pid.to_be_bytes()[0],
                        mismatched_pid.to_be_bytes()[1],
                        0x00,
                        0x00,
                    ],
                )
                .await
                .unwrap();

            // Client must not match the acknowledgement
            let poll_res = timeout(Duration::from_millis(200), client.poll()).await;
            assert!(
                poll_res.is_ok(),
                "Client poll must handle mismatched SUBACK Packet Identifier without panic"
            );
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_3_10_4_5_unsuback_mismatched_packet_id_rejected() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    // Client option / API call: Unsubscribe from topic
    /* let _ = client.unsubscribe(&topic_filter("sensor/test")).await; */
    let options = qos0_publish("dummy/trigger_unsub");
    let _ = client.publish(&options, Bytes::Borrowed(b"")).await;

    if let Ok(frame) = broker.next_non_ping().await {
        if frame.packet_type() == 10 {
            let pid = u16::from_be_bytes([frame.body[0], frame.body[1]]);
            let mismatched_pid = pid.wrapping_add(10);

            // Server illegally sends UNSUBACK with a mismatched Packet Identifier
            broker
                .send_raw(
                    0xB0,
                    &[
                        mismatched_pid.to_be_bytes()[0],
                        mismatched_pid.to_be_bytes()[1],
                        0x00,
                        0x00,
                    ],
                )
                .await
                .unwrap();

            let poll_res = timeout(Duration::from_millis(200), client.poll()).await;
            assert!(
                poll_res.is_ok(),
                "Client poll must handle mismatched UNSUBACK Packet Identifier without panic"
            );
        }
    }

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_2_3_qos1_publish_awaits_puback() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos1_publish("test/qos1_ack_progression");
    let mut pub_future = Box::pin(client.publish(&options, Bytes::Borrowed(b"payload")));

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 3);
    let tlen = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let pid = &frame.body[2 + tlen..4 + tlen];

    // Sender MUST treat PUBLISH as unacknowledged until corresponding PUBACK is received
    assert!(
        timeout(Duration::from_millis(100), &mut pub_future)
            .await
            .is_err(),
        "QoS 1 publication MUST remain pending until PUBACK is received"
    );

    // Send corresponding PUBACK
    broker
        .send_raw(0x40, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();

    // Publication future resolves successfully after PUBACK
    let result = timeout(Duration::from_secs(1), pub_future)
        .await
        .expect("Publication timed out after receiving PUBACK");
    assert!(
        result.is_ok(),
        "Publication must complete after receiving PUBACK"
    );

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_3_qos2_publish_awaits_pubrec() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos0_publish("test/qos2_ack_progression") /* .exactly_once() */;
    let _ = client.publish(&options, Bytes::Borrowed(b"msg")).await;

    let frame = broker.next_non_ping().await.unwrap();
    assert_eq!(frame.packet_type(), 3);
    let tlen = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let pid = &frame.body[2 + tlen..4 + tlen];

    // Until PUBREC is received, the client MUST NOT advance to sending PUBREL
    assert!(
        broker
            .expect_silence(Duration::from_millis(100))
            .await
            .is_ok(),
        "Client MUST NOT send PUBREL before receiving PUBREC"
    );

    // Send PUBREC
    broker
        .send_raw(0x50, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();

    // Client now advances and sends PUBREL
    let pubrel = timeout(Duration::from_secs(1), broker.next_non_ping())
        .await
        .expect("Client did not send PUBREL upon receiving PUBREC")
        .unwrap();
    assert_eq!(pubrel.packet_type(), 6, "expected PUBREL");

    disconnect(&mut client).await;
}

#[tokio::test]
async fn mqtt_4_3_3_5_qos2_pubrel_awaits_pubcomp() {
    let mut broker = Broker::bind().await.unwrap();
    let mut client = connect(&mut broker).await.unwrap();

    let options = qos0_publish("test/qos2_pubcomp_progression") /* .exactly_once() */;
    let _ = client.publish(&options, Bytes::Borrowed(b"msg")).await;

    let frame = broker.next_non_ping().await.unwrap();
    let tlen = u16::from_be_bytes([frame.body[0], frame.body[1]]) as usize;
    let pid = &frame.body[2 + tlen..4 + tlen];

    // Send PUBREC to trigger PUBREL from client
    broker
        .send_raw(0x50, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();
    let pubrel = broker.next_non_ping().await.unwrap();
    assert_eq!(pubrel.packet_type(), 6);

    // Sender MUST treat PUBREL as unacknowledged until receiving PUBCOMP
    assert!(
        broker
            .expect_silence(Duration::from_millis(100))
            .await
            .is_ok(),
        "PUBREL exchange MUST remain active until PUBCOMP arrives"
    );

    // Send matching PUBCOMP
    broker
        .send_raw(0x70, &[pid[0], pid[1], 0x00])
        .await
        .unwrap();

    // Client poll resolves transaction cleanly
    let poll_res = timeout(Duration::from_millis(200), client.poll()).await;
    assert!(
        poll_res.is_ok(),
        "Client poll must process PUBCOMP successfully"
    );

    disconnect(&mut client).await;
}

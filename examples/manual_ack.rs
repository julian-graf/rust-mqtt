use std::{
    net::{Ipv4Addr, SocketAddr},
    str::from_utf8,
    time::Duration,
};

use embedded_io_adapters::tokio_1::FromTokio;
use log::{error, info};
use rust_mqtt::{
    Bytes,
    buffer::*,
    client::{
        Client,
        event::{Event, Publish, Suback},
        options::{
            ConnectOptions, DisconnectOptions, PublicationOptions, SubscriptionOptions,
            TopicReference,
        },
    },
    types::{MqttBinary, MqttString, TopicFilter, TopicName},
};
use tokio::{net::TcpStream, select, time::sleep};
use tokio_test::assert_ok;

#[tokio::main]
async fn main() {
    env_logger::init();

    #[cfg(feature = "alloc")]
    let mut buffer = AllocBuffer;
    #[cfg(feature = "bump")]
    let mut buffer = [0; 1024];
    #[cfg(feature = "bump")]
    let mut buffer = BumpBuffer::new(&mut buffer);

    let mut client = Client::<'_, _, _, 1, 3, 3, 1, 16>::new(&mut buffer);

    // Acknowledge all packets which have a payload format indicator property and where the payload
    // is valid UTF-8.
    client.manually_ack_on(&|packet| {
        packet.payload_format_indicator.is_some() && from_utf8(packet.message.as_bytes()).is_ok()
    });

    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 1883);
    let connection = TcpStream::connect(addr).await.unwrap();
    let connection = FromTokio::new(connection);

    match client
        .connect(
            connection,
            &ConnectOptions::new()
                .user_name(MqttString::try_from("test").unwrap())
                .password(MqttBinary::try_from("testPass").unwrap())
                .clean_start(),
            None,
        )
        .await
    {
        Ok(c) => info!("Connected to server: {c:?}"),
        Err(e) => {
            error!("Failed to connect to server: {e:?}");
            return;
        }
    }

    let topic_string = MqttString::from_str("rust-mqtt/rocks").unwrap();
    let topic_filter = TopicFilter::new(topic_string.as_borrowed()).unwrap();
    let topic_name = TopicName::new(topic_string.as_borrowed()).unwrap();

    assert_ok!(
        client
            .subscribe(
                topic_filter.as_borrowed(),
                &SubscriptionOptions::new().exactly_once()
            )
            .await
    );

    match assert_ok!(client.poll().await) {
        Event::Suback(Suback {
            packet_identifier: _,
            reason_string: _,
            user_properties: _,
            reason_code,
        }) if reason_code.is_success() => {}
        _ => panic!("subscription failed"),
    }

    let valid_utf8 = Bytes::Borrowed("Hello World!".as_bytes());
    let invalid_utf8 = Bytes::Borrowed(&[0x80]);

    // Has payload format indicator and is valid UTF-8 => should be acknowledged manually
    assert_ok!(
        client
            .publish(
                &PublicationOptions::new(TopicReference::Name(topic_name.as_borrowed()))
                    .payload_format_indicator(true)
                    .exactly_once(),
                valid_utf8.as_borrowed(),
            )
            .await
    );

    // Is valid UTF-8 but misses a payload format indicator => should be acknowledged automatically
    assert_ok!(
        client
            .publish(
                &PublicationOptions::new(TopicReference::Name(topic_name.as_borrowed()))
                    .exactly_once(),
                valid_utf8,
            )
            .await
    );

    // Has a payload format indicator but is invalid UTF-8 => should be acknowledged automatically
    assert_ok!(
        client
            .publish(
                &PublicationOptions::new(TopicReference::Name(topic_name))
                    .payload_format_indicator(false)
                    .exactly_once(),
                invalid_utf8,
            )
            .await
    );

    loop {
        select! {
            () = sleep(Duration::from_secs(5)) => {
                break;
            },
            header = client.poll_header() => {
                let h = assert_ok!(header);
                match assert_ok!(client.poll_body(h).await) {
                    Event::Publish(Publish { manual_ack, topic, payload_format_indicator, message, .. }) => {
                        if manual_ack {
                            let message = assert_ok!(from_utf8(message.as_bytes()));
                            info!("Received publication: manual_ack={manual_ack}, topic={topic:?}, payload_format_indicator={payload_format_indicator:?}, message={message}")

                            // TODO manual ack
                        } else if let Ok(message) = from_utf8(message.as_bytes()) {
                            info!("Received publication: manual_ack={manual_ack}, topic={topic:?}, payload_format_indicator={payload_format_indicator:?}, message={message}")
                        } else {
                            info!("Received publication: manual_ack={manual_ack}, topic={topic:?}, payload_format_indicator={payload_format_indicator:?},, message={:?}", message.as_bytes())
                        }
                    }
                    _ => {},
                }
            }
        };
    }

    client.disconnect(&DisconnectOptions::new()).await.unwrap();
}

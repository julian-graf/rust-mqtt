use std::{
    net::{Ipv4Addr, SocketAddr},
    num::NonZero,
    time::Duration,
};

use embedded_io_adapters::tokio_1::FromTokio;
use log::{error, info};
use rust_mqtt::{
    buffer::*,
    client::{
        Client,
        options::{ConnectOptions, DisconnectOptions, WillOptions},
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttBinary, MqttString, TopicName},
};
use tokio::{io::AsyncWriteExt, net::TcpStream, time::sleep};

#[tokio::main]
async fn main() {
    env_logger::init();

    #[cfg(feature = "alloc")]
    let mut buffer = AllocBuffer;
    #[cfg(feature = "bump")]
    let mut buffer = [0; 1024];
    #[cfg(feature = "bump")]
    let mut buffer = BumpBuffer::new(&mut buffer);

    let mut client = Client::<'_, _, _, 1, 1, 1, 1, 16>::new(&mut buffer);

    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 1883);
    let connection = TcpStream::connect(addr).await.unwrap();
    let connection = FromTokio::new(connection);

    match client
        .connect(
            connection,
            &ConnectOptions::new()
                .clean_start()
                .session_expiry_interval(SessionExpiryInterval::Seconds(NonZero::new(5).unwrap()))
                .keep_alive(KeepAlive::Seconds(NonZero::new(5).unwrap()))
                .user_name(MqttString::try_from("test").unwrap())
                .password(MqttBinary::try_from("testPass").unwrap())
                .will(
                    WillOptions::new(
                        TopicName::new(MqttString::try_from("i/am/dead").unwrap()).unwrap(),
                        MqttBinary::try_from("Have a nice day!").unwrap(),
                    )
                    .exactly_once()
                    .retain()
                    .delay_interval(1)
                    .payload_format_indicator(true)
                    .content_type(MqttString::try_from("txt").unwrap()),
                ),
            Some(MqttString::try_from("rust-mqtt-demo-client").unwrap()),
        )
        .await
    {
        Ok(c) => {
            info!("Connected to server: {c:?}");
            info!("{:?}", client.client_config());
            info!("{:?}", client.server_config());
            info!("{:?}", client.shared_config());
            info!("{:?}", client.session());
        }
        Err(e) => {
            error!("Failed to connect to server: {e:?}");
            return;
        }
    }

    #[cfg(feature = "bump")]
    unsafe {
        client.buffer_mut().reset();
    }

    match client.disconnect(&DisconnectOptions::new()).await {
        Ok(n) => {
            // let mut tcp = n.into_inner();

            // tcp.shutdown().await.unwrap();
            drop(n);
            // info!("Disconnected from server");
            sleep(Duration::from_secs(1)).await;
        }
        Err(e) => {
            error!("Failed to disconnect from server: {e:?}");
            return;
        }
    }
    sleep(Duration::from_secs(5)).await;
}

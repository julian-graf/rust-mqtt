use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::AtomicUsize,
};

use embedded_io_adapters::tokio_1::FromTokio;
use log::info;
use rust_mqtt::{
    buffer::ArcBumpBuffer,
    client::{Client, options::ConnectOptions},
    types::{MqttBinary, MqttString},
};
use tokio::net::TcpStream;

static mut BUFFER_COUNTER: AtomicUsize = AtomicUsize::new(0);
static mut BUFFER: [u8; 1024] = [0; 1024];

#[tokio::main]
async fn main() {
    env_logger::init();

    #[allow(static_mut_refs)]
    let mut buffer = ArcBumpBuffer::new(unsafe { BUFFER.as_mut_slice() }, unsafe {
        &mut BUFFER_COUNTER
    });

    let mut client = Client::<'_, '_, _, _, 1, 1, 1, 1, 16>::new(&mut buffer);

    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 1883);
    let connection = TcpStream::connect(addr).await.unwrap();
    let connection = FromTokio::new(connection);

    match client
        .connect(
            connection,
            &ConnectOptions::new()
                .user_name(MqttString::try_from("test").unwrap())
                .password(MqttBinary::try_from("testPass").unwrap()),
            None,
        )
        .await
    {
        Ok(_) => {
            info!("{:?}", client.client_config());
            info!("{:?}", client.server_config());
            info!("{:?}", client.shared_config());
            info!("{:?}", client.session());
        }
        Err(_) => {
            return;
        }
    }
}

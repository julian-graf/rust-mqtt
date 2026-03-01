use alloc::format;
use embassy_futures::select::{Either3, select3};
use embassy_net::{
    IpAddress, IpEndpoint, Stack,
    dns::{DnsQueryType, DnsSocket, Error as DnsError},
    tcp::{ConnectError, TcpSocket},
};
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    channel::{Receiver, Sender},
};
use embassy_time::Timer;
use rust_mqtt::{
    buffer::AllocBuffer,
    client::{
        Client,
        event::Event,
        options::{ConnectOptions, PublicationOptions, TopicReference},
    },
    config::KeepAlive,
    types::{MqttString, TopicFilter, TopicName},
};

use crate::bufwrite::BufferedWriter;

async fn resolve(stack: Stack<'_>, host: &str) -> Result<IpAddress, DnsError> {
    let dns = DnsSocket::new(stack);

    let query = dns.query(host, DnsQueryType::A).await?;

    query.first().copied().ok_or(DnsError::Failed)
}

async fn tcp<'a>(
    stack: Stack<'a>,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    remote: IpEndpoint,
) -> Result<TcpSocket<'a>, ConnectError> {
    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket.connect(remote).await.map(|_| socket)
}

pub async fn mqtt(
    stack: Stack<'_>,
    rx_buffer: &'static mut [u8],
    tx_buffer: &'static mut [u8],
    write_buffer: &'static mut [u8],
    actor_sender: Sender<'static, NoopRawMutex, f32, 5>,
    sensor_receiver: Receiver<'static, NoopRawMutex, f32, 5>,
) -> ! {
    // let host = "www.meddler.xyz";
    let port = 1883;

    let mut failed = 0;

    loop {
        if failed > 0 {
            Timer::after_secs(1 << failed).await;
        }

        // defmt::info!("[DNS] Resolving {} ...", host);
        // let address = match resolve(stack, host).await {
        //     Ok(addr) => {
        //         defmt::info!("[DNS] Resolved to {}", addr);
        //         addr
        //     }
        //     Err(e) => {
        //         defmt::error!("[DNS] Failed to resolve: {:?}", e);
        //         failed += 1;
        //         continue;
        //     }
        // };

        let broker = IpEndpoint::new(IpAddress::v4(10, 42, 0, 1), port);

        defmt::info!("[TCP] Connecting...");

        let tcp = match tcp(stack, rx_buffer, tx_buffer, broker).await {
            Ok(tcp) => {
                defmt::info!("[TCP] Connected");
                tcp
            }
            Err(e) => {
                defmt::error!("[TCP] Failed to connect: {:?}", e);
                failed += 1;
                continue;
            }
        };
        let transport = BufferedWriter::new(tcp, write_buffer);

        let mut buffer = AllocBuffer;
        let mut client: Client<'_, _, _, 1, 1, 0, 0> = Client::new(&mut buffer);

        let connect_options = ConnectOptions::new();

        defmt::info!("[MQTT] Connecting...");

        match client.connect(transport, &connect_options, None).await {
            Ok(info) => {
                defmt::info!("[MQTT] Connected: {:?}", info);
                defmt::info!("[MQTT] Client: {:?}", client.client_config());
                defmt::info!("[MQTT] Server: {:?}", client.server_config());
                defmt::info!("[MQTT] Shared: {:?}", client.shared_config());
            }
            Err(e) => {
                defmt::error!("[MQTT] Failed to connect: {:?}", e);
                failed += 1;
                continue;
            }
        }
        failed = 0;

        // match client.subscribe(TopicFilter::, options)

        let keep_alive = match client.shared_config().keep_alive {
            KeepAlive::Infinite => 60,
            KeepAlive::Seconds(s) => s.get(),
        };

        loop {
            match select3(
                client.poll_header(),
                sensor_receiver.receive(),
                Timer::after_secs(keep_alive as u64),
            )
            .await
            {
                Either3::First(Ok(header)) => match client.poll_body(header).await {
                    Ok(Event::Publish(p)) => {
                        let topic = p.topic.as_ref().as_str();
                        let mut topic_hierarchy = topic.split('/');
                        let Some("actor") = topic_hierarchy.next() else {
                            continue;
                        };
                        let Some(value) = topic_hierarchy.next() else {
                            continue;
                        };
                        let Ok(actor_value) = value.parse() else {
                            continue;
                        };

                        if topic_hierarchy.next().is_none() {
                            actor_sender.send(actor_value).await;
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.is_recoverable() => {}
                    Err(e) => {
                        client.abort().await;
                        defmt::error!("[MQTT] error occured: {:?}", e);
                        break;
                    }
                },
                Either3::First(Err(e)) => {
                    client.abort().await;
                    defmt::error!("[MQTT] error occured: {:?}", e);
                    break;
                }
                Either3::Second(v) => {
                    let t = format!("sensor/{}", v);
                    let topic = TopicName::new(MqttString::from_str(&t).unwrap()).unwrap();
                    let publish_options = PublicationOptions::new(TopicReference::Name(topic));
                    match client.publish(&publish_options, "".into()).await {
                        Ok(_) => {}
                        Err(e) if e.is_recoverable() => {}
                        Err(e) => {
                            client.abort().await;
                            defmt::error!("[MQTT] error occured: {:?}", e);
                            break;
                        }
                    }
                }
                Either3::Third(_) => match client.ping().await {
                    Ok(_) => {}
                    Err(e) if e.is_recoverable() => {}
                    Err(e) => {
                        client.abort().await;
                        defmt::error!("[MQTT] error occured: {:?}", e);
                        break;
                    }
                },
            };
        }
    }
}

use std::{
    assert_eq,
    net::{Ipv4Addr, SocketAddr},
};

use embedded_io_adapters::tokio_1::FromTokio;
use embedded_io_async::Read;
use log::{debug, error, info};
use rust_mqtt::{
    Bytes,
    buffer::*,
    client::{
        Client,
        event::{Event, PartialPublishEvent, Suback},
        options::{
            ConnectOptions, DisconnectOptions, PublicationOptions, SubscriptionOptions,
            TopicReference,
        },
    },
    header::PacketType,
    types::{MqttBinary, MqttString, TopicFilter, TopicName},
};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    env_logger::init();

    #[cfg(feature = "alloc")]
    let mut buffer = AllocBuffer;
    #[cfg(feature = "bump")]
    let mut buffer = [0; 1024];
    #[cfg(feature = "bump")]
    let mut buffer = BumpBuffer::new(&mut buffer);

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
            error!("Failed to connect to server");
            return;
        }
    }

    #[cfg(feature = "bump")]
    unsafe {
        client.buffer_mut().reset();
    }

    let topic_string = MqttString::<&[u8]>::from_str("rust-mqtt/is/great").unwrap();
    let topic_filter = TopicFilter::new(topic_string.as_borrowed()).unwrap();
    let topic_name = TopicName::new(topic_string.as_borrowed()).unwrap();

    match client
        .subscribe(topic_filter.as_borrowed(), &SubscriptionOptions::new())
        .await
    {
        Ok(_) => info!("Sent Subscribe"),
        Err(_) => {
            error!("Failed to subscribe");
            return;
        }
    }

    match client.poll().await {
        Ok(Event::Suback(Suback {
            packet_identifier: _,
            reason_string: _,
            user_properties: _,
            reason_code,
        })) => {
            info!("Subscribed with reason code {reason_code:?}");
        }
        Ok(_) => {
            error!("Expected Suback but received event");
            return;
        }
        Err(_) => {
            error!("Failed to receive Suback");
            return;
        }
    }

    let pub_options = PublicationOptions::new(TopicReference::Name(topic_name.as_borrowed()));

    // Just a little bit of Lorem Ipsum
    let lorem_ipsum = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Praesent eget tempus sapien, porttitor condimentum lacus. Proin eget ipsum a mi facilisis pharetra non sed risus. Vestibulum dui odio, euismod et condimentum id, ullamcorper vel est. Sed facilisis tellus vel dolor mattis, in sodales nibh iaculis. Donec ultricies pulvinar tincidunt. Nullam suscipit et purus dignissim consectetur. Interdum et malesuada fames ac ante ipsum primis in faucibus. Ut scelerisque eros nec quam aliquet, sed consequat ipsum lobortis. Phasellus aliquet lacinia libero sed porta. Sed non ultrices nibh. Nulla condimentum quis nisi feugiat auctor. Maecenas et diam rhoncus, aliquet leo ac, mattis diam. Interdum et malesuada fames ac ante ipsum primis in faucibus. Fusce lacus ante, accumsan a lobortis et, sodales eget augue. Aliquam quam nibh, scelerisque eu est nec, mollis dapibus lorem. Maecenas mollis hendrerit massa vel laoreet.

    Cras vel feugiat tellus. Sed suscipit lorem ante, molestie facilisis lectus mollis ut. Fusce sodales tempus odio in tempus. Vestibulum id hendrerit nunc. Vestibulum consectetur vitae mi id eleifend. Maecenas quam felis, eleifend et pharetra vitae, lobortis non velit. Curabitur varius nibh dui, nec condimentum lectus interdum vitae. Maecenas pretium, arcu vel tempus suscipit, diam eros vulputate ante, nec vestibulum nisi turpis a neque. Suspendisse lobortis mi eu tellus gravida scelerisque. Aliquam laoreet quam non velit rhoncus imperdiet. Cras tempus pretium sagittis. Duis viverra pretium odio, non venenatis nulla sodales eget.

    Sed vitae ante auctor, viverra ligula id, semper felis. Sed dapibus lectus sed risus venenatis, id elementum neque vestibulum. Suspendisse potenti. Quisque sed dignissim orci. Pellentesque nec ex et velit auctor auctor ac in libero. Donec eu tortor eget magna finibus sagittis vitae sed dui. In bibendum urna ut leo blandit accumsan. Aenean risus dolor, lacinia ut orci at, eleifend vehicula ipsum. Curabitur aliquet ultrices urna, quis tincidunt orci porttitor vel. Nullam vulputate gravida rhoncus. Donec accumsan sed tellus ut consectetur. Mauris diam dolor, gravida sed ante sed, ultricies rhoncus nunc. Nulla felis augue, finibus et nisl nec, condimentum laoreet magna.

    Integer justo lacus, laoreet quis ornare tempor, porta at sem. Sed nec ligula rhoncus, placerat magna vel, tempor eros. Integer interdum nunc sit amet metus pharetra auctor. Maecenas vel ex ex. Duis porta molestie nisl, varius porta mauris cursus non. Phasellus mattis nibh ipsum, eleifend venenatis ligula vulputate ac. Sed commodo sagittis tellus, vitae mollis sapien accumsan nec. Donec lacus erat, hendrerit eget rhoncus vel, posuere ac nulla. Sed placerat volutpat arcu, elementum sagittis ligula vulputate eget. Cras aliquam lectus tellus, vitae tempus nunc imperdiet non. Maecenas imperdiet metus lectus, eu ornare eros congue sed. Morbi a ex in arcu venenatis tincidunt. Nullam consectetur ultrices nibh, a commodo nisl aliquam in. Nulla lacus massa, ultrices ut elementum ut, elementum eu metus. Interdum et malesuada fames ac ante ipsum primis in faucibus. Mauris vitae maximus lacus, sit amet molestie quam.

    Nulla eu ante diam. Praesent bibendum leo in auctor pellentesque. Aenean quis consequat augue, id lacinia ligula. Etiam aliquet a enim nec elementum. Nullam neque est, porttitor et volutpat a, laoreet vitae felis. Aliquam erat volutpat. Morbi non convallis tellus. Pellentesque purus turpis, venenatis id risus ut, rhoncus ornare lacus. Nullam suscipit neque sed sollicitudin scelerisque. Sed pellentesque orci ut rhoncus auctor. Donec luctus luctus purus et finibus. In feugiat pellentesque elit eu elementum. Nullam nec maximus nisl. Fusce non mattis eros. Orci varius natoque penatibus et magnis dis parturient montes, nascetur ridiculus mus. Praesent tincidunt eros a justo sagittis semper.

    Nullam ultrices condimentum varius. Nulla facilisi. Sed blandit dui ipsum, in euismod arcu accumsan vitae. Vestibulum in hendrerit lectus, vel molestie orci. Duis tempus ac mi ac malesuada. Quisque et ligula mi. Nulla purus enim, luctus a ligula non, sagittis mattis urna. Morbi pretium cursus ante, ultricies sollicitudin turpis suscipit et. Nulla iaculis blandit augue a efficitur. Ut tincidunt blandit ipsum, sit amet lobortis ante semper in. Donec egestas maximus vulputate.

    Class aptent taciti sociosqu ad litora torquent per conubia nostra, per inceptos himenaeos. Pellentesque justo tortor, ornare eu ultrices ut, varius eu ex. Vestibulum imperdiet condimentum tortor, vel auctor augue cursus id. Sed volutpat magna at velit lacinia convallis. Cras et sem quam. Fusce commodo odio finibus odio dignissim laoreet. Curabitur cursus varius purus ut lobortis. Nunc tempus dignissim velit non sollicitudin. Vivamus nec lectus et libero faucibus commodo. Ut placerat tortor nibh, et pulvinar lorem ultrices et. Cras mollis faucibus nunc sit amet cursus.

    Praesent lobortis dignissim velit, et maximus risus pulvinar nec. Ut nulla justo, fermentum vehicula sem id, euismod eleifend sapien. Maecenas non purus quam. Donec non porttitor ipsum, in ullamcorper turpis. Donec bibendum turpis at pellentesque condimentum. Curabitur sit amet orci diam. Quisque sagittis ex quis diam placerat pharetra.

    Aenean pellentesque nisi ex, vitae finibus urna rutrum efficitur. Mauris bibendum rhoncus pulvinar. Vestibulum nec congue ex, tincidunt maximus velit. Maecenas bibendum risus a arcu varius, eu vulputate purus pulvinar. Nulla magna nibh, malesuada vitae libero id, maximus pretium ipsum. Integer suscipit faucibus lacus a faucibus. Phasellus et sagittis nunc, quis faucibus tortor. Sed convallis urna nec arcu commodo cursus. Pellentesque habitant morbi tristique senectus et netus et malesuada fames ac turpis egestas. Nulla eu est finibus, sodales ex id, ultricies tellus. Pellentesque luctus elit magna, a vehicula ligula interdum sed. Nullam lacinia eu justo et ultrices. Quisque commodo nunc id orci facilisis viverra. Nullam blandit, tortor sit amet pulvinar faucibus, leo turpis ornare ante, ut cursus lectus risus quis lacus.

    Etiam pellentesque dignissim urna at vestibulum. Nulla ullamcorper feugiat dui sit amet fringilla. Phasellus feugiat purus eget diam commodo vestibulum. Curabitur commodo tortor ut quam rutrum ullamcorper. In hac habitasse platea dictumst. Ut nunc dui, ornare id congue eget, faucibus vitae felis. Suspendisse in vestibulum lectus. Nullam congue a nisl eget facilisis. Nullam fermentum ligula sit amet ex tristique, a commodo mi euismod. Mauris commodo felis sit amet tortor iaculis aliquam. Quisque iaculis lacus sed mollis venenatis. Sed eget leo vel dui aliquam vehicula sed commodo enim. Ut maximus mauris eu commodo iaculis.

    Pellentesque felis tortor, hendrerit id consequat quis, consequat eu diam. Interdum et malesuada fames ac ante ipsum primis in faucibus. Nullam vitae dictum arcu. Suspendisse potenti. Curabitur hendrerit pharetra quam a sodales. Nam molestie non elit id tempor. Nunc sed tincidunt augue. Aenean tristique neque commodo purus suscipit mattis. Suspendisse tristique lectus ut ex tempor consectetur.

    Morbi placerat dui vitae lacus venenatis rutrum. Fusce auctor scelerisque laoreet. Nulla facilisi. Sed laoreet risus vitae finibus volutpat. Nullam blandit tempor enim vel sagittis. Donec vehicula metus in ante congue, et varius lacus porttitor. Nullam nec sapien suscipit, tempus ipsum nec, elementum nibh. Fusce at elit venenatis, gravida augue eu, volutpat purus. Vestibulum purus lacus, gravida et ultrices at, pellentesque vel augue. Ut neque nisi, varius eget hendrerit a, elementum vel quam. Maecenas lacinia ex ac tellus feugiat vulputate.

    Nulla eget sapien sed nisi mattis finibus ac a lacus. Donec egestas, risus at imperdiet consectetur, orci ipsum hendrerit tortor, id consequat justo nisl non dui. Integer augue diam, tristique at nulla eu, facilisis finibus enim. Donec nisi mauris, ultricies et neque vitae, suscipit molestie justo. Ut vitae nibh posuere, eleifend dolor quis, auctor arcu. Ut porttitor, lacus sed consequat fermentum, magna lectus sagittis nunc, in euismod augue orci quis elit. Praesent non scelerisque metus. Phasellus vel justo a neque molestie vehicula.

    Lorem ipsum dolor sit amet, consectetur adipiscing elit. Donec vitae porttitor turpis. Donec enim eros, convallis id iaculis in, semper a mi. Praesent fermentum ultricies dui. Vestibulum hendrerit libero nulla, nec condimentum ipsum auctor sit amet. Quisque dapibus sem interdum ultricies posuere. Quisque et dignissim felis.

    Nulla et sapien nec risus elementum efficitur. In fermentum augue eget leo interdum varius. Nulla facilisi. Suspendisse et sagittis lectus, a semper ante. Nullam sollicitudin enim orci, vel suscipit ex iaculis iaculis. Donec eu ornare sapien. Vestibulum porttitor pretium eleifend. Proin eget turpis ut velit sagittis tincidunt. Morbi luctus lorem ut mi accumsan vestibulum. Vestibulum eu laoreet nulla, at dapibus elit. Suspendisse et justo augue. Suspendisse potenti. Duis facilisis dolor sed lacus lacinia fringilla. Mauris consequat sapien nec libero vehicula molestie. Aenean scelerisque euismod arcu quis placerat. Proin non pellentesque nunc, at dignissim orci.

    In hac habitasse platea dictumst. Aliquam vulputate, lectus sed consequat elementum, lorem orci consequat ex, sed sollicitudin magna nunc ut libero. Cras quis massa dignissim, tincidunt mauris pharetra, tempor mi. Duis in laoreet leo, pharetra commodo eros. Quisque egestas metus ut justo lobortis commodo. Pellentesque habitant morbi tristique senectus et netus et malesuada fames ac turpis egestas. Morbi eu sodales nibh, id aliquet dui. Suspendisse scelerisque imperdiet tempus. Sed porttitor odio suscipit arcu luctus tristique. Pellentesque consequat, purus in sodales efficitur, dui tortor aliquet nisl, non lobortis ante nunc finibus mauris. Nulla auctor pulvinar nibh, et hendrerit ex commodo nec. Praesent vel mauris id libero congue sodales a vitae metus.

    In iaculis massa eget nisi vestibulum rutrum. Phasellus interdum sapien efficitur, auctor nisi nec, finibus ligula. Vivamus at mauris maximus, mollis justo nec, fermentum quam. Sed dapibus pharetra tincidunt. In hac habitasse platea dictumst. Sed vulputate dolor at massa eleifend varius. Mauris ex orci, varius eu dui at, ultrices imperdiet felis. Maecenas scelerisque maximus feugiat. Nam vestibulum rutrum varius. Vestibulum tortor risus, mattis in nisl a, efficitur dignissim nisl. Duis odio nisl, imperdiet vitae quam quis, congue egestas magna. Morbi efficitur mauris et condimentum aliquet.

    In ac turpis maximus, congue risus eu, semper dolor. Cras vel tristique lorem. Ut eu erat lobortis, cursus nulla et, elementum purus. Donec bibendum dapibus mauris, a ultrices ipsum consectetur at. Maecenas sodales erat sit amet felis efficitur, non sollicitudin nulla interdum. Suspendisse porta sagittis ultricies. In vitae nisl vel libero venenatis vulputate sed ac metus. Mauris nec elementum tortor. Donec mattis nisi mollis nunc feugiat, ac blandit felis blandit. Aenean et laoreet nulla. Sed magna neque, fringilla non neque et, varius gravida neque. Curabitur tincidunt maximus nisl, et placerat felis convallis in. Ut non iaculis leo. In ultrices lacus non sapien tincidunt, ac finibus nunc facilisis. Nullam vitae tempus quam, a eleifend velit. Interdum et malesuada fames ac ante ipsum primis in faucibus.

    Vivamus eu magna tempus, mollis magna nec, egestas nisi. Vivamus eget sem nec augue tincidunt venenatis in porttitor risus. Sed lobortis sed ex sed tincidunt. Donec in odio tortor. Pellentesque orci nunc, luctus vel leo mattis, semper interdum odio. Ut eu leo non metus tincidunt vehicula. Maecenas et odio urna. Fusce posuere tellus diam, vitae malesuada neque consequat sit amet. Integer condimentum malesuada magna vitae mattis.

    Ut lobortis tellus sit amet cursus tristique. Duis at elit varius, euismod mauris in, ultrices massa. Ut efficitur maximus arcu, ut maximus risus porttitor sit amet. Fusce egestas varius ullamcorper. Fusce vel ullamcorper massa. Proin malesuada nisl ligula, ac vestibulum metus euismod in. Morbi ullamcorper ante ullamcorper elit aliquam, vel convallis nibh tincidunt. Sed eleifend lectus ut leo pulvinar laoreet.";

    match client
        .publish(&pub_options, Bytes::from(lorem_ipsum.as_bytes()))
        .await
    {
        Ok(_) => {
            info!("Published message");
        }
        Err(_) => {
            error!("Failed to send Publish");
            return;
        }
    };

    let header = loop {
        match client.poll_header().await {
            Ok(header) if header.packet_type().is_ok_and(|h| h == PacketType::Publish) => {
                break header;
            }
            Ok(e) => info!("Received unexpected Event {e:?}"),
            Err(_) => {
                error!("Failed to poll");
                return;
            }
        }
    };

    let (e, mut reader) = match client.poll_publish_payload(header).await {
        Ok(e) => e,
        Err(_) => {
            error!("Failed to poll");
            return;
        }
    };

    let _publish = match e {
        PartialPublishEvent::Publish(p) => p,
        PartialPublishEvent::Duplicate(p) => p,
    };

    // As a "test", we check that we read as many bytes as the reader
    // claims at the start.
    let mut remaining = reader.remaining_len();

    let mut buffer = [0; 256];

    while reader.remaining_len() > 0 {
        debug!("reading... {} vs {}", remaining, reader.remaining_len());
        match reader.read(&mut buffer).await {
            Ok(i) => remaining = remaining.strict_sub(i),
            Err(e) => {
                error!("Failed to poll: {e:?}");
                return;
            }
        }
    }

    assert_eq!(remaining, 0);

    drop(reader);

    match client.disconnect(&DisconnectOptions::new()).await {
        Ok(_n) => {
            // For a correct TCP disconnection, one should make sure the underlying TCP socket
            // sends a FIN segment. However I could not get the tokio::TcpStream to behave that
            // way, so we just do nothing here. It's fine for MQTT operability realistically,
            // but for clean usage, the TCP should be closed properly.

            info!("Disconnected from server")
        }
        Err(_) => {
            error!("Failed to disconnect from server");
        }
    }
}

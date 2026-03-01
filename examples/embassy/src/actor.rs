use defmt::info;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Receiver};

#[embassy_executor::task]
pub async fn run_actor(input: Receiver<'static, NoopRawMutex, f32, 5>) {
    loop {
        let value = input.receive().await;

        info!("[ACTOR] received value {}", value);
    }
}

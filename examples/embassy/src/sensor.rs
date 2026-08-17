use defmt::info;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Sender};
use embassy_time::Timer;
use esp_hal::rng::Rng;

#[embassy_executor::task]
pub async fn run_sensor(output: Sender<'static, NoopRawMutex, f32, 5>, rng: Rng) {
    loop {
        let delay = rng.random().clamp(0, 4000);
        info!("[SENSOR] sampling in {} ms", delay);
        Timer::after_millis(delay as u64).await;


        let data = f32::from_bits(rng.random());
        info!("[SENSOR] sampled");
        output.send(data).await;
    }
}

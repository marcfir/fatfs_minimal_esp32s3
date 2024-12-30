#![no_std]
#![no_main]

use block_device_adapters::BufStream;
use block_device_adapters::BufStreamError;
use embassy_executor::Spawner;
use embedded_fatfs::FsOptions;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_io_async::{Read, Seek, Write};
use esp_backtrace as _;
use esp_hal::dma::DmaRxBuf;
use esp_hal::dma::DmaTxBuf;
use esp_hal::dma_buffers;
use esp_hal::gpio::Level;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Config;
use esp_hal::spi::master::SpiDmaBus;
use esp_hal::timer::timg::TimerGroup;

use esp_hal::{
    dma::Dma,
    dma::DmaPriority,
    prelude::*,
    spi::{master::Spi, SpiMode},
};
use sdspi::SdSpi;

#[main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    log::info!("Hello world!");
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    // let io = Io::new(peripherals.IO_MUX);
    let sclk = peripherals.GPIO12;
    let miso = peripherals.GPIO11;
    let mosi = peripherals.GPIO13;
    let mut cs = Output::new(peripherals.GPIO10, Level::High);

    // let sclk = peripherals.GPIO12;
    // let miso = peripherals.GPIO11;
    // let mosi = peripherals.GPIO13;
    // let cs = peripherals.GPIO10;

    let dma = Dma::new(peripherals.DMA);
    let dma_channel = dma.channel0;

    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(32000);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();

    let spi = Spi::new_with_config(
        peripherals.SPI2,
        Config {
            frequency: 400.kHz(),
            mode: SpiMode::Mode0,
            ..Config::default()
        },
    )
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_miso(miso)
    // .with_cs(cs)
    .with_dma(dma_channel.configure(false, DmaPriority::Priority0))
    .into_async();

    let mut spi = SpiDmaBus::new(spi, dma_rx_buf, dma_tx_buf);
    loop {
        match sdspi::sd_init(&mut spi, &mut cs).await {
            Ok(_) => break,
            Err(e) => {
                log::warn!("Sd init error: {:?}", e);
                embassy_time::Timer::after_millis(10).await;
            }
        }
    }

    let spid = ExclusiveDevice::new(spi, cs, embassy_time::Delay).unwrap();
    let mut sd = SdSpi::<_, _, aligned::A1>::new(spid, embassy_time::Delay);

    loop {
        // Initialize the card
        if (sd.init().await).is_ok() {
            // Increase the speed up to the SD max of
            sd.spi()
                .bus_mut()
                .apply_config(&Config {
                    frequency: 25u32.MHz(),
                    mode: SpiMode::Mode0,
                    ..Config::default()
                })
                .unwrap();
            log::info!("Initialization complete!");

            break;
        }
        log::info!("Failed to init card, retrying...");
        embassy_time::Delay.delay_ns(5000u32).await;
    }

    let inner = BufStream::<_, 512>::new(sd);

    async {
        let fs = embedded_fatfs::FileSystem::new(inner, FsOptions::new()).await?;
        {
            let mut f = fs.root_dir().create_file("test.log").await?;
            let hello = b"Hello world!";
            log::info!("Writing to file...");
            f.write_all(hello).await?;
            f.flush().await?;

            let mut buf = [0u8; 12];
            f.rewind().await?;
            f.read_exact(&mut buf[..]).await?;
            log::info!(
                "Read from file: {}",
                core::str::from_utf8(&buf[..]).unwrap()
            );
        }
        fs.unmount().await?;

        Ok::<(), embedded_fatfs::Error<BufStreamError<sdspi::Error>>>(())
    }
    .await
    .expect("Filesystem tests failed!");

    loop {
        embassy_time::Timer::after_millis(500).await;
    }
}

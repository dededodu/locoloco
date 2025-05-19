// USE MODIFIED mfrc522 crate
//
// // - Set the reload value to determine the timeout
// //   for a 5ms timeout, we need a value of 200 = 0xC8
// self.write(Register::TReloadRegHigh, 0x00)?;
// self.write(Register::TReloadRegLow, 0xC8)?;

#![no_std]
#![no_main]

use core::cell::RefCell;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::spi::{self, Spi};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{Delay, Instant, Timer};
use embedded_hal_bus::spi::RefCellDevice;
use mfrc522::comm::blocking::spi::SpiInterface;
use mfrc522::{Mfrc522, RxGain, Uid};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let driver = Driver::new(p.USB, Irqs);
    spawner.spawn(logger_task(driver)).unwrap();

    let inner = p.SPI0;
    let clk = p.PIN_2;
    let mosi = p.PIN_3;
    let miso = p.PIN_4;
    let cs = Output::new(p.PIN_1, Level::High);

    let mut config = spi::Config::default();
    config.frequency = 2_000_000;
    let spi = Spi::new_blocking(inner, clk, mosi, miso, config);

    let spi_rc = RefCell::new(spi);
    let spi_dev = RefCellDevice::new(&spi_rc, cs, Delay);
    let spi_if = SpiInterface::new(spi_dev);
    let mut rfid_reader = Mfrc522::new(spi_if)
        .init()
        .expect("could not create MFRC522");
    rfid_reader.set_antenna_gain(RxGain::DB48).unwrap();

    let mut counter = 0;
    while counter < 10 {
        counter += 1;
        log::info!("Tick {}", counter);
        Timer::after_secs(1).await;
    }

    loop {
        let start = Instant::now();
        log::info!("WUPA waiting...");
        if let Ok(atqa) = rfid_reader.wupa() {
            log::info!("WUPA command took {} ms", start.elapsed().as_millis());
            let start = Instant::now();
            match rfid_reader.select(&atqa) {
                Ok(Uid::Single(ref inner)) => {
                    log::info!(
                        "Card UID {:?}, Type {:?}",
                        inner.as_bytes(),
                        inner.get_type()
                    );
                }
                Ok(Uid::Double(ref inner)) => {
                    log::info!("Card double UID {:?}", inner.as_bytes());
                }
                Ok(_) => log::info!("Got other UID size"),
                Err(e) => {
                    log::error!("Error getting card UID: {:?}", e);
                }
            }
            log::info!("SELECT command took {} ms", start.elapsed().as_millis());
        } else {
            log::info!("WUPA command took {} ms", start.elapsed().as_millis());
        }
        log::info!("sleep 50ms");
        Timer::after_millis(50).await;
    }
}

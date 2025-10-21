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
    let cs0 = Output::new(p.PIN_0, Level::High);
    let cs1 = Output::new(p.PIN_1, Level::High);

    let mut config = spi::Config::default();
    config.frequency = 2_000_000;
    let spi = Spi::new_blocking(inner, clk, mosi, miso, config);

    let spi_rc = RefCell::new(spi);
    let spi_dev0 = RefCellDevice::new(&spi_rc, cs0, Delay);
    let spi_dev1 = RefCellDevice::new(&spi_rc, cs1, Delay);
    let spi_if0 = SpiInterface::new(spi_dev0);
    let spi_if1 = SpiInterface::new(spi_dev1);
    let mut rfid_reader0 = Mfrc522::new(spi_if0)
        .init()
        .expect("could not create MFRC522");
    let mut rfid_reader1 = Mfrc522::new(spi_if1)
        .init()
        .expect("could not create MFRC522");
    rfid_reader0.set_receive_timeout(1).unwrap();
    rfid_reader0.set_antenna_gain(RxGain::DB48).unwrap();
    rfid_reader1.set_receive_timeout(1).unwrap();
    rfid_reader1.set_antenna_gain(RxGain::DB48).unwrap();

    let mut counter = 0;
    while counter < 5 {
        counter += 1;
        log::info!("Tick {}", counter);
        Timer::after_secs(1).await;
    }

    loop {
        let start = Instant::now();
        log::info!("[reader0] WUPA waiting...");
        if let Ok(atqa) = rfid_reader0.wupa() {
            log::info!(
                "[reader0] WUPA command took {} ms",
                start.elapsed().as_millis()
            );
            let start = Instant::now();
            match rfid_reader0.select(&atqa) {
                Ok(Uid::Single(ref inner)) => {
                    log::info!(
                        "[reader0] Card UID {:?}, Type {:?}",
                        inner.as_bytes(),
                        inner.get_type()
                    );
                }
                Ok(Uid::Double(ref inner)) => {
                    log::info!("[reader0] Card double UID {:?}", inner.as_bytes());
                }
                Ok(_) => log::info!("[reader0] Got other UID size"),
                Err(e) => {
                    log::error!("[reader0] Error getting card UID: {:?}", e);
                }
            }
            log::info!(
                "[reader0] SELECT command took {} ms",
                start.elapsed().as_millis()
            );
        } else {
            log::info!(
                "[reader0] WUPA command took {} ms",
                start.elapsed().as_millis()
            );
        }
        let _ = rfid_reader0.hlta();

        let start = Instant::now();
        log::info!("[reader1] WUPA waiting...");
        if let Ok(atqa) = rfid_reader1.wupa() {
            log::info!(
                "[reader1] WUPA command took {} ms",
                start.elapsed().as_millis()
            );
            let start = Instant::now();
            match rfid_reader1.select(&atqa) {
                Ok(Uid::Single(ref inner)) => {
                    log::info!(
                        "[reader1] Card UID {:?}, Type {:?}",
                        inner.as_bytes(),
                        inner.get_type()
                    );
                }
                Ok(Uid::Double(ref inner)) => {
                    log::info!("[reader1] Card double UID {:?}", inner.as_bytes());
                }
                Ok(_) => log::info!("[reader1] Got other UID size"),
                Err(e) => {
                    log::error!("[reader1] Error getting card UID: {:?}", e);
                }
            }
            log::info!(
                "[reader1] SELECT command took {} ms",
                start.elapsed().as_millis()
            );
        } else {
            log::info!(
                "[reader1] WUPA command took {} ms",
                start.elapsed().as_millis()
            );
        }
        let _ = rfid_reader1.hlta();

        log::info!("sleep 10ms");
        Timer::after_millis(10).await;
    }
}

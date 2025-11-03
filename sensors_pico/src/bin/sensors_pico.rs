#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use core::cell::RefCell;
use core::num::TryFromIntError;

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::encode_into_slice;
use bincode::error::EncodeError;
use common_pico::{
    HEADER_SIZE, REQUEST_MAX_SIZE, SERVER_IP_ADDRESS, SERVER_TCP_PORT_SENSORS, WIFI_NETWORK,
    WIFI_PASSWORD,
};
use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, IpEndpoint, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::peripherals::{SPI0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::RefCellDevice;
use embedded_io_async::Write as _;
use loco_protocol::{
    BACKEND_PROTOCOL_MAGIC_NUMBER, Header, LocoId, Operation, SensorId, SensorStatus,
    SensorsStatusArray,
};
use mfrc522::comm::blocking::spi::{DummyDelay, SpiInterface};
use mfrc522::{Initialized, Mfrc522, RxGain, Uid};
use rand::RngCore;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

struct RfidReader<'a> {
    mfrc522: Mfrc522<
        SpiInterface<
            RefCellDevice<'a, Spi<'static, SPI0, Blocking>, Output<'static>, Delay>,
            DummyDelay,
        >,
        Initialized,
    >,
    sensor_id: SensorId,
    sensor_data_idx: usize,
}

struct SensorData {
    loco_id: LocoId,
    sensor_id: SensorId,
}

type SensorsData = [Option<SensorData>; 2];
static SENSORS_DATA: Mutex<CriticalSectionRawMutex, RefCell<SensorsData>> =
    Mutex::new(RefCell::new([None, None]));

#[embassy_executor::task]
async fn logger_task(driver: UsbDriver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn tag_reader_task(
    spi: Spi<'static, SPI0, Blocking>,
    cs1: Output<'static>,
    cs2: Output<'static>,
) {
    let spi_rc = RefCell::new(spi);
    let mut readers = [
        RfidReader {
            mfrc522: Mfrc522::new(SpiInterface::new(RefCellDevice::new(&spi_rc, cs1, Delay)))
                .init()
                .expect("could not create reader 1"),
            sensor_id: SensorId::RfidReader1,
            sensor_data_idx: 0,
        },
        RfidReader {
            mfrc522: Mfrc522::new(SpiInterface::new(RefCellDevice::new(&spi_rc, cs2, Delay)))
                .init()
                .expect("could not create reader 2"),
            sensor_id: SensorId::RfidReader2,
            sensor_data_idx: 1,
        },
    ];

    for reader in readers.iter_mut() {
        reader.mfrc522.set_receive_timeout(1).unwrap();
        reader.mfrc522.set_antenna_gain(RxGain::DB48).unwrap();
    }

    loop {
        for reader in readers.iter_mut() {
            if let Ok(atqa) = reader.mfrc522.wupa() {
                match reader.mfrc522.select(&atqa) {
                    Ok(Uid::Single(ref uid)) => match LocoId::try_from(uid.as_bytes()) {
                        Ok(loco_id) => {
                            log::debug!("[{}] Detected {}", reader.sensor_id, loco_id);
                            SENSORS_DATA.lock(|d| {
                                d.borrow_mut()[reader.sensor_data_idx] = Some(SensorData {
                                    loco_id,
                                    sensor_id: reader.sensor_id,
                                })
                            });
                        }
                        Err(e) => log::error!("[{}] Invalid UID: {:?}", reader.sensor_id, e),
                    },
                    Ok(_) => log::debug!("[{}] Got other UID size", reader.sensor_id),
                    Err(e) => {
                        log::debug!("[{}] Error getting card UID: {:?}", reader.sensor_id, e);
                    }
                }
                let _ = reader.mfrc522.hlta();
            }
        }

        Timer::after_millis(1).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let usb_driver = UsbDriver::new(p.USB, Irqs);
    unwrap!(spawner.spawn(logger_task(usb_driver)));
    let mut rng = RoscRng;

    let mut counter = 0;
    while counter < 10 {
        counter += 1;
        log::debug!("Tick {}", counter);
        Timer::after_secs(1).await;
    }
    log::info!("Hello SensorsPico!");

    unwrap!(spawner.spawn(tag_reader_task(
        Spi::new_blocking(p.SPI0, p.PIN_2, p.PIN_3, p.PIN_4, spi::Config::default()),
        Output::new(p.PIN_0, Level::High),
        Output::new(p.PIN_1, Level::High),
    )));

    let fw = include_bytes!("../../../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(cyw43_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());

    // Generate random seed
    let seed = rng.next_u64();

    // Init network stack
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    unwrap!(spawner.spawn(net_task(runner)));

    loop {
        match control
            .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(err) => {
                log::error!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP
    log::info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_secs(1).await;
    }
    log::info!("DHCP is now up!");

    // And now we can use it!

    let sensors = Sensors::new();

    // Spawn a dedicated task that periodically read from all RFID readers

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);

        control.gpio_set(0, false).await;

        let remote_endpoint = IpEndpoint {
            addr: SERVER_IP_ADDRESS,
            port: SERVER_TCP_PORT_SENSORS,
        };
        log::info!("Connecting to {:?}...", remote_endpoint);
        if let Err(e) = socket.connect(remote_endpoint).await {
            log::warn!("connection error: {:?}", e);
            Timer::after_secs(1).await;
            continue;
        }
        log::info!("Connected to {:?}", socket.remote_endpoint());

        control.gpio_set(0, true).await;

        // Periodically check sensors status and send updated status to
        // loco_controller
        if let Err(e) = sensors.handle_sensors_updates(&mut socket).await {
            log::error!("{:?}", e);
            continue;
        }
    }
}

#[derive(Debug)]
pub enum Error {
    EncodeIntoSlice(EncodeError),
    InvalidEncodedHeaderSize(usize),
    PayloadSizeTooLarge(TryFromIntError),
    TcpWrite(embassy_net::tcp::Error),
}

type Result<T> = core::result::Result<T, Error>;

struct Sensors {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
}

impl Sensors {
    pub fn new() -> Self {
        log::debug!("Sensors::new()");

        Sensors {
            bincode_cfg: bincode::config::legacy(),
        }
    }

    fn build_sensors_status_payload(&self, payload: &mut [u8]) -> Result<Option<u8>> {
        log::debug!("Sensors::build_sensors_status_payload()");

        let mut payload_offset: usize = size_of::<SensorsStatusArray>();
        let mut updated_sensors: u8 = 0;
        SENSORS_DATA.lock(|d| {
            let mut sensors_data = d.borrow_mut();
            for sensor_data in sensors_data.iter_mut() {
                if let Some(d) = sensor_data.take() {
                    log::info!("{} detected by reader {}", d.loco_id, d.sensor_id);
                    payload_offset += encode_into_slice(
                        SensorStatus {
                            sensor_id: d.sensor_id.into(),
                            loco_id: d.loco_id.into(),
                        },
                        &mut payload[payload_offset..],
                        self.bincode_cfg,
                    )
                    .unwrap();
                    updated_sensors += 1;
                }
            }
        });

        if updated_sensors == 0 {
            return Ok(None);
        }

        encode_into_slice(
            SensorsStatusArray {
                len: updated_sensors,
            },
            &mut payload[0..],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        Ok(Some(
            u8::try_from(payload_offset).map_err(Error::PayloadSizeTooLarge)?,
        ))
    }

    async fn send_sensors_status_op(
        &self,
        socket: &mut TcpSocket<'_>,
        message: &mut [u8],
        payload_len: u8,
    ) -> Result<()> {
        log::debug!("Sensors::send_sensors_status_op()");

        let header_len = encode_into_slice(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::SensorsStatus.into(),
                payload_len,
            },
            &mut message[..HEADER_SIZE],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        if header_len != HEADER_SIZE {
            return Err(Error::InvalidEncodedHeaderSize(header_len));
        }

        socket
            .write_all(&message[..header_len + usize::from(payload_len)])
            .await
            .map_err(Error::TcpWrite)?;

        Ok(())
    }

    pub async fn handle_sensors_updates(&self, socket: &mut TcpSocket<'_>) -> Result<()> {
        log::debug!("Sensors::handle_sensors_updates()");

        loop {
            let mut message = [0u8; REQUEST_MAX_SIZE];
            // Check sensors which need to be updated and return payload
            if let Some(payload_len) =
                self.build_sensors_status_payload(&mut message[HEADER_SIZE..])?
            {
                // Send update to the loco_controller server
                self.send_sensors_status_op(socket, &mut message, payload_len)
                    .await?;
            }

            Timer::after_millis(100).await;
        }
    }
}

#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use core::num::TryFromIntError;

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::encode_into_slice;
use bincode::error::EncodeError;
use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, IpAddress, IpEndpoint, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::Timer;
use embedded_io_async::Write as _;
use loco_protocol::{Header, Operation, SensorsStatusArray};
use rand::RngCore;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

const WIFI_NETWORK: &str = "loco-controller";
const WIFI_PASSWORD: &str = "locoloco";
const SERVER_IP_ADDRESS: IpAddress = IpAddress::v4(10, 42, 0, 1);
const SERVER_TCP_PORT: u16 = 8005;

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
            port: SERVER_TCP_PORT,
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

const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;
const PAYLOAD_MAX_SIZE: usize = 256;
const HEADER_SIZE: usize = 0x3;
const REQUEST_MAX_SIZE: usize = HEADER_SIZE + PAYLOAD_MAX_SIZE;

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

    fn build_sensors_status_payload(&self, payload: &mut [u8]) -> Result<u8> {
        // TODO: Go over every sensor and build the payload accordingly
        let payload_len =
            encode_into_slice(SensorsStatusArray { len: 0 }, payload, self.bincode_cfg)
                .map_err(Error::EncodeIntoSlice)?;

        Ok(u8::try_from(payload_len).map_err(Error::PayloadSizeTooLarge)?)
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
            let payload_len = self.build_sensors_status_payload(&mut message[HEADER_SIZE..])?;

            // Send update to the loco_controller server
            self.send_sensors_status_op(socket, &mut message, payload_len)
                .await?;
        }
    }
}

#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::decode_from_slice;
use bincode::error::DecodeError;
use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, IpAddress, IpEndpoint, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::Timer;
use embedded_io_async::{Read, ReadExactError};
use loco_protocol::{
    ActuatorId, ActuatorType, DriveActuatorPayload, Error as LocoProtocolError, Header, Operation,
    SwitchRailsState,
};
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
const SERVER_TCP_PORT: u16 = 8006;

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
    log::info!("Hello ActuatorsPico!");

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

    let mut actuators = Actuators::new([
        SwitchRails {
            gpio: Output::new(p.PIN_1, Level::Low),
            id: ActuatorId::SwitchRails1,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_2, Level::Low),
            id: ActuatorId::SwitchRails2,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_3, Level::Low),
            id: ActuatorId::SwitchRails3,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_4, Level::Low),
            id: ActuatorId::SwitchRails4,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_5, Level::Low),
            id: ActuatorId::SwitchRails5,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_6, Level::Low),
            id: ActuatorId::SwitchRails6,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_7, Level::Low),
            id: ActuatorId::SwitchRails7,
        },
        SwitchRails {
            gpio: Output::new(p.PIN_8, Level::Low),
            id: ActuatorId::SwitchRails8,
        },
    ]);

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

        // Handle incoming messages from the server
        if let Err(e) = actuators.handle_messages(&mut socket).await {
            log::error!("{:?}", e);
            continue;
        }
    }
}

#[derive(Debug)]
pub enum Error {
    ConvertLocoProtocolType(LocoProtocolError),
    DecodeFromSlice(DecodeError),
    InvalidBackendProtocolMagicNumber(u8),
    TcpRead(ReadExactError<embassy_net::tcp::Error>),
    UnsupportedOperation(Operation),
}

type Result<T> = core::result::Result<T, Error>;

const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;
const PAYLOAD_MAX_SIZE: usize = 256;
const HEADER_SIZE: usize = 0x3;

struct SwitchRails {
    gpio: Output<'static>,
    id: ActuatorId,
}

impl SwitchRails {
    fn switch(&mut self, state: SwitchRailsState) -> Result<()> {
        log::debug!("SwitchRails::switch()");
        let level = match state {
            SwitchRailsState::Direct => Level::Low,
            SwitchRailsState::Diverted => Level::High,
        };
        log::info!(
            "SwitchRails::switch(): Setting {} to {} ({:?})",
            self.id,
            state,
            level
        );
        self.gpio.set_level(level);
        Ok(())
    }
}

struct Actuators {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    switch_rails: [SwitchRails; 8],
}

impl Actuators {
    pub fn new(switch_rails: [SwitchRails; 8]) -> Self {
        log::debug!("Actuators::new()");

        Actuators {
            bincode_cfg: bincode::config::legacy(),
            switch_rails,
        }
    }

    fn update_switch_rails(&mut self, id: ActuatorId, state: SwitchRailsState) -> Result<()> {
        log::debug!("Actuators::update_actuator()");
        for switch_rail in self.switch_rails.iter_mut() {
            if switch_rail.id == id {
                switch_rail.switch(state)?;
                break;
            }
        }

        Ok(())
    }

    fn handle_op_drive_actuator(&mut self, payload: &[u8]) -> Result<()> {
        log::debug!("Actuators::handle_op_drive_actuator()");

        let (drive_actuator_payload, _): (DriveActuatorPayload, usize) =
            decode_from_slice(payload, self.bincode_cfg).map_err(Error::DecodeFromSlice)?;
        let actuator_id: ActuatorId = drive_actuator_payload
            .actuator_id
            .try_into()
            .map_err(Error::ConvertLocoProtocolType)?;
        let actuator_type: ActuatorType = drive_actuator_payload
            .actuator_type
            .try_into()
            .map_err(Error::ConvertLocoProtocolType)?;

        match actuator_type {
            ActuatorType::SwitchRails => {
                let state: SwitchRailsState = drive_actuator_payload
                    .actuator_state
                    .try_into()
                    .map_err(Error::ConvertLocoProtocolType)?;
                self.update_switch_rails(actuator_id, state)?;
            }
        }

        Ok(())
    }

    pub async fn handle_messages(&mut self, socket: &mut TcpSocket<'_>) -> Result<()> {
        log::debug!("Actuators::handle_messages()");
        loop {
            log::info!("Actuators::handle_messages(): Waiting for incoming bytes...");

            let mut hdr = [0; HEADER_SIZE];
            socket.read_exact(&mut hdr).await.map_err(Error::TcpRead)?;

            let (header, _): (Header, usize) =
                decode_from_slice(&hdr, self.bincode_cfg).map_err(Error::DecodeFromSlice)?;

            if header.magic != BACKEND_PROTOCOL_MAGIC_NUMBER {
                return Err(Error::InvalidBackendProtocolMagicNumber(header.magic));
            }

            let op =
                Operation::try_from(header.operation).map_err(Error::ConvertLocoProtocolType)?;
            log::info!("Actuators::handle_messages(): Operation {:?}", op);

            let mut payload_buf = [0u8; PAYLOAD_MAX_SIZE];
            let payload = &mut payload_buf[..header.payload_len as usize];
            if !payload.is_empty() {
                socket.read_exact(payload).await.map_err(Error::TcpRead)?;
            }

            match op {
                Operation::DriveActuator => self.handle_op_drive_actuator(payload)?,
                Operation::Connect
                | Operation::SensorsStatus
                | Operation::ControlLoco
                | Operation::LocoStatus => {
                    return Err(Error::UnsupportedOperation(op));
                }
            }

            log::info!("Actuators::handle_messages(): Operation {:?} completed", op);
        }
    }
}

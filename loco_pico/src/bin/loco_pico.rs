#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::error::{DecodeError, EncodeError};
use bincode::{decode_from_slice, encode_into_slice};
use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, IpAddress, IpEndpoint, StackResources};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_0, PIO0, PWM_SLICE0};
use embassy_rp::peripherals::{PIN_3, PWM_SLICE1, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pwm::{Config as PwmConfig, Pwm, PwmError, SetDutyCycle};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_rp::{Peri, bind_interrupts};
use embassy_time::Timer;
use embedded_io_async::{Read, ReadExactError, Write as _};
use loco_protocol::{
    ConnectPayload, ControlLocoPayload, Direction, Error as LocoProtocolError, Header,
    LocoStatusResponse, Operation, Speed,
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
const SERVER_TCP_PORT: u16 = 8004;

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
    log::info!("Hello LocoPico!");

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

    let pwm_ctrl = PwmController::new(p.PWM_SLICE0, p.PIN_0, p.PWM_SLICE1, p.PIN_3).unwrap();

    let mut loco = Loco::new(pwm_ctrl);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        // Reset the loco to a well known state
        if let Err(e) = loco.reset() {
            log::error!("{:?}", e);
            continue;
        }

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

        // Send CONNECT operation
        if let Err(e) = loco.send_connect_op(&mut socket).await {
            log::error!("{:?}", e);
            continue;
        }

        // Handle incoming messages from the server
        if let Err(e) = loco.handle_messages(&mut socket).await {
            log::error!("{:?}", e);
            continue;
        }
    }
}

#[derive(Debug)]
pub enum Error {
    ConvertLocoProtocolType(LocoProtocolError),
    DecodeFromSlice(DecodeError),
    EncodeIntoSlice(EncodeError),
    InvalidBackendProtocolMagicNumber(u8),
    InvalidEncodedHeaderSize(usize),
    ReadEof,
    ReadLessThanExpected,
    SetPwmDutyCycle(PwmError),
    TcpRead(ReadExactError<embassy_net::tcp::Error>),
    TcpWrite(embassy_net::tcp::Error),
    UnknownDirection(u8),
    UnknownOperation(u8),
    UnknownSpeed(u8),
    UnsupportedOperation(Operation),
}

type Result<T> = core::result::Result<T, Error>;

const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;
const PAYLOAD_MAX_SIZE: usize = 256;
const HEADER_SIZE: usize = 0x3;
const REQUEST_MAX_SIZE: usize = HEADER_SIZE + PAYLOAD_MAX_SIZE;
const RESPONSE_MAX_SIZE: usize = 1024;
const LOCO_ID: u8 = 0x1;

struct Loco<'a> {
    direction: Direction,
    speed: Speed,
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    response: [u8; RESPONSE_MAX_SIZE],
    pwm_ctrl: PwmController<'a>,
}

impl<'a> Loco<'a> {
    pub fn new(pwm_ctrl: PwmController<'a>) -> Self {
        log::debug!("Loco::new()");

        Loco {
            direction: Direction::default(),
            speed: Speed::default(),
            bincode_cfg: bincode::config::legacy(),
            response: [0u8; RESPONSE_MAX_SIZE],
            pwm_ctrl,
        }
    }

    fn handle_op_control_loco(&mut self, payload: &[u8]) -> Result<Option<usize>> {
        log::debug!("Loco::handle_op_control_loco()");

        let (ctrl_loco_payload, _): (ControlLocoPayload, usize) =
            decode_from_slice(payload, self.bincode_cfg).map_err(Error::DecodeFromSlice)?;
        self.direction = ctrl_loco_payload
            .direction
            .try_into()
            .map_err(Error::ConvertLocoProtocolType)?;
        self.speed = ctrl_loco_payload
            .speed
            .try_into()
            .map_err(Error::ConvertLocoProtocolType)?;

        self.pwm_ctrl.control_loco(self.direction, self.speed)?;

        log::debug!(
            "Loco::handle_op_control_loco(): Direction {:?}, Speed {:?}",
            self.direction,
            self.speed
        );

        Ok(None)
    }

    fn handle_op_loco_status(&mut self, _payload: &[u8]) -> Result<Option<usize>> {
        log::debug!("Loco::handle_op_loco_status()");

        let loco_st_resp = LocoStatusResponse {
            direction: self.direction.into(),
            speed: self.speed.into(),
        };

        log::debug!("Loco::handle_op_loco_status(): Sending {:?}", loco_st_resp);

        let resp_len = encode_into_slice(loco_st_resp, &mut self.response, self.bincode_cfg)
            .map_err(Error::EncodeIntoSlice)?;

        Ok(Some(resp_len))
    }

    pub async fn send_connect_op(&self, socket: &mut TcpSocket<'_>) -> Result<()> {
        log::debug!("Loco::send_connect_op()");

        let mut message = [0u8; REQUEST_MAX_SIZE];
        let payload_len = encode_into_slice(
            ConnectPayload { loco_id: LOCO_ID },
            &mut message[HEADER_SIZE..],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        let header_len = encode_into_slice(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::Connect.into(),
                payload_len: payload_len as u8,
            },
            &mut message[..HEADER_SIZE],
            self.bincode_cfg,
        )
        .map_err(Error::EncodeIntoSlice)?;

        if header_len != HEADER_SIZE {
            return Err(Error::InvalidEncodedHeaderSize(header_len));
        }

        socket
            .write_all(&message[..header_len + payload_len])
            .await
            .map_err(Error::TcpWrite)?;

        Ok(())
    }

    pub async fn handle_messages(&mut self, socket: &mut TcpSocket<'_>) -> Result<()> {
        loop {
            log::info!("Loco::handle_messages(): Waiting for incoming bytes...");

            let mut hdr = [0; HEADER_SIZE];
            socket.read_exact(&mut hdr).await.map_err(Error::TcpRead)?;

            let (header, _): (Header, usize) =
                decode_from_slice(&hdr, self.bincode_cfg).map_err(Error::DecodeFromSlice)?;

            if header.magic != BACKEND_PROTOCOL_MAGIC_NUMBER {
                return Err(Error::InvalidBackendProtocolMagicNumber(header.magic));
            }

            let op =
                Operation::try_from(header.operation).map_err(Error::ConvertLocoProtocolType)?;
            log::info!("Loco::handle_messages(): Operation {:?}", op);

            let mut payload_buf = [0u8; PAYLOAD_MAX_SIZE];
            let payload = &mut payload_buf[..header.payload_len as usize];
            if !payload.is_empty() {
                socket.read_exact(payload).await.map_err(Error::TcpRead)?;
            }

            let send_response = match op {
                Operation::Connect => return Err(Error::UnsupportedOperation(op)),
                Operation::ControlLoco => self.handle_op_control_loco(payload)?,
                Operation::LocoStatus => self.handle_op_loco_status(payload)?,
            };

            if let Some(resp_len) = send_response {
                log::debug!("Loco::handle_messages(): Sending response");
                socket
                    .write_all(&self.response[..resp_len])
                    .await
                    .map_err(Error::TcpWrite)?;
            }

            log::info!("Loco::handle_messages(): Operation {:?} completed", op);
        }
    }

    pub fn reset(&mut self) -> Result<()> {
        self.direction = Direction::default();
        self.speed = Speed::default();

        self.pwm_ctrl.control_loco(self.direction, self.speed)
    }
}

struct PwmController<'a> {
    pwm_forward: Pwm<'a>,
    pwm_backward: Pwm<'a>,
}

impl PwmController<'_> {
    pub fn new(
        slice0: Peri<'static, PWM_SLICE0>,
        pin0: Peri<'static, PIN_0>,
        slice1: Peri<'static, PWM_SLICE1>,
        pin3: Peri<'static, PIN_3>,
    ) -> Result<Self> {
        // If we aim for a specific frequency, here is how we can calculate the top value.
        // The top value sets the period of the PWM cycle, so a counter goes from 0 to top and then wraps around to 0.
        // Every such wraparound is one PWM cycle. So here is how we get 25KHz:
        let desired_freq_hz = 25_000;
        let clock_freq_hz = embassy_rp::clocks::clk_sys_freq();
        let divider = 16u8;
        let period = (clock_freq_hz / (desired_freq_hz * divider as u32)) as u16 - 1;

        let mut cfg = PwmConfig::default();
        cfg.top = period;
        cfg.divider = divider.into();

        let mut pwm_forward = Pwm::new_output_a(slice0, pin0, cfg.clone());
        let mut pwm_backward = Pwm::new_output_b(slice1, pin3, cfg);
        pwm_forward
            .set_duty_cycle_fully_off()
            .map_err(Error::SetPwmDutyCycle)?;
        pwm_backward
            .set_duty_cycle_fully_off()
            .map_err(Error::SetPwmDutyCycle)?;

        Ok(PwmController {
            pwm_forward,
            pwm_backward,
        })
    }

    fn control_loco(&mut self, direction: Direction, speed: Speed) -> Result<()> {
        let (pwm_set, pwm_clear) = match direction {
            Direction::Forward => (&mut self.pwm_forward, &mut self.pwm_backward),
            Direction::Backward => (&mut self.pwm_backward, &mut self.pwm_forward),
        };

        let duty_cycle = match speed {
            Speed::Stop => 0,
            Speed::Slow => 25,
            Speed::Normal => 75,
            Speed::Fast => 100,
        };

        pwm_clear
            .set_duty_cycle_fully_off()
            .map_err(Error::SetPwmDutyCycle)?;
        pwm_set
            .set_duty_cycle_percent(duty_cycle)
            .map_err(Error::SetPwmDutyCycle)?;

        Ok(())
    }
}

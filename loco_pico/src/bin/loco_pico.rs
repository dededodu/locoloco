#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::error::{DecodeError, EncodeError};
use bincode::{decode_from_slice, encode_into_slice};
use common_pico::{
    HEADER_SIZE, PAYLOAD_MAX_SIZE, REQUEST_MAX_SIZE, RESPONSE_MAX_SIZE, SERVER_IP_ADDRESS,
    SERVER_TCP_PORT_LOCOS, connect_loco_controller, initialize_logger, initialize_program,
    initialize_wifi,
};
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_rp::Peri;
use embassy_rp::peripherals::{PIN_0, PWM_SLICE0};
use embassy_rp::peripherals::{PIN_3, PWM_SLICE1};
use embassy_rp::pwm::{Config as PwmConfig, Pwm, PwmError, SetDutyCycle};
use embassy_time::Timer;
use embedded_io_async::{Read, ReadExactError, Write as _};
use loco_protocol::{
    BACKEND_PROTOCOL_MAGIC_NUMBER, ConnectPayload, ControlLocoPayload, Direction,
    Error as LocoProtocolError, Header, LocoStatusResponse, Operation, Speed,
};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    initialize_logger(&spawner, p.USB);
    initialize_program("LocoPico").await;
    let (mut control, stack) = initialize_wifi(
        &spawner, p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    )
    .await;

    let pwm_ctrl = PwmController::new(p.PWM_SLICE0, p.PIN_0, p.PWM_SLICE1, p.PIN_3).unwrap();

    let mut loco = Loco::new(pwm_ctrl);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    control.gpio_set(0, false).await;

    loop {
        // Reset the loco to a well known state
        if let Err(e) = loco.reset() {
            log::error!("{:?}", e);
            continue;
        }

        let mut socket = match connect_loco_controller(
            stack,
            &mut rx_buffer,
            &mut tx_buffer,
            SERVER_IP_ADDRESS,
            SERVER_TCP_PORT_LOCOS,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!("connection error: {:?}", e);
                Timer::after_secs(1).await;
                continue;
            }
        };

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

        control.gpio_set(0, false).await;
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
                Operation::ControlLoco => self.handle_op_control_loco(payload)?,
                Operation::LocoStatus => self.handle_op_loco_status(payload)?,
                Operation::Connect | Operation::SensorsStatus | Operation::DriveActuator => {
                    return Err(Error::UnsupportedOperation(op));
                }
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

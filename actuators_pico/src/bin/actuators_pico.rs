#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::decode_from_slice;
use bincode::error::DecodeError;
use common_pico::{
    HEADER_SIZE, PAYLOAD_MAX_SIZE, SERVER_IP_ADDRESS, SERVER_TCP_PORT_ACTUATORS,
    connect_loco_controller, initialize_logger, initialize_program, initialize_wifi,
};
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_rp::gpio::{Level, Output};
use embassy_time::Timer;
use embedded_io_async::{Read, ReadExactError};
use loco_protocol::{
    ActuatorId, ActuatorType, BACKEND_PROTOCOL_MAGIC_NUMBER, DriveActuatorPayload,
    Error as LocoProtocolError, Header, Operation, SwitchRailsState,
};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    initialize_logger(&spawner, p.USB);
    initialize_program("ActuatorsPico").await;
    let (mut control, stack) = initialize_wifi(
        &spawner, p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    )
    .await;

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

    control.gpio_set(0, false).await;

    loop {
        let mut socket = match connect_loco_controller(
            stack,
            &mut rx_buffer,
            &mut tx_buffer,
            SERVER_IP_ADDRESS,
            SERVER_TCP_PORT_ACTUATORS,
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

        // Handle incoming messages from the server
        if let Err(e) = actuators.handle_messages(&mut socket).await {
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
    InvalidBackendProtocolMagicNumber(u8),
    TcpRead(ReadExactError<embassy_net::tcp::Error>),
    UnsupportedOperation(Operation),
}

type Result<T> = core::result::Result<T, Error>;

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

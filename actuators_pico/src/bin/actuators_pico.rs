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
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_rp::gpio::{Level, Output};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::Timer;
use embedded_io_async::{Read, ReadExactError};
use loco_protocol::{
    ActuatorId, ActuatorType, BACKEND_PROTOCOL_MAGIC_NUMBER, DriveActuatorPayload,
    Error as LocoProtocolError, Header, Operation, SwitchRailsState,
};
use {defmt_rtt as _, panic_probe as _};

/**
 * List of channels for dedicated communication between the main thread and
 * every switch_rail_controller tasks.
 */
static CHANNEL1: Channel<ThreadModeRawMutex, SwitchRailsState, 1> = Channel::new();
static CHANNEL2: Channel<ThreadModeRawMutex, SwitchRailsState, 1> = Channel::new();
static CHANNEL3: Channel<ThreadModeRawMutex, SwitchRailsState, 1> = Channel::new();
static CHANNEL4: Channel<ThreadModeRawMutex, SwitchRailsState, 1> = Channel::new();

#[embassy_executor::task(pool_size = 4)]
async fn switch_rail_controller(
    actuator_id: ActuatorId,
    channel: Receiver<'static, ThreadModeRawMutex, SwitchRailsState, 1>,
    mut gpio_direct: Output<'static>,
    mut gpio_diverted: Output<'static>,
) {
    log::debug!("switch_rails_controller(): actuator {}", actuator_id);

    loop {
        let switch_state = channel.receive().await;
        let (gpio_set, gpio_clear) = match switch_state {
            SwitchRailsState::Direct => (&mut gpio_direct, &mut gpio_diverted),
            SwitchRailsState::Diverted => (&mut gpio_diverted, &mut gpio_direct),
        };

        log::debug!(
            "switch_rails_controller(): driving {} to {}",
            actuator_id,
            switch_state
        );

        gpio_clear.set_level(Level::Low);
        gpio_set.set_level(Level::High);
        Timer::after_millis(500).await;
        gpio_set.set_level(Level::Low);
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    initialize_logger(&spawner, p.USB);
    initialize_program("ActuatorsPico").await;
    let (mut control, stack) = initialize_wifi(
        &spawner, p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    )
    .await;

    // Spawning one dedicated task per switch rails
    spawner.spawn(unwrap!(switch_rail_controller(
        ActuatorId::SwitchRails1,
        CHANNEL1.receiver(),
        Output::new(p.PIN_2, Level::Low),
        Output::new(p.PIN_3, Level::Low)
    )));
    spawner.spawn(unwrap!(switch_rail_controller(
        ActuatorId::SwitchRails2,
        CHANNEL2.receiver(),
        Output::new(p.PIN_4, Level::Low),
        Output::new(p.PIN_5, Level::Low)
    )));
    spawner.spawn(unwrap!(switch_rail_controller(
        ActuatorId::SwitchRails3,
        CHANNEL3.receiver(),
        Output::new(p.PIN_6, Level::Low),
        Output::new(p.PIN_7, Level::Low)
    )));
    spawner.spawn(unwrap!(switch_rail_controller(
        ActuatorId::SwitchRails4,
        CHANNEL4.receiver(),
        Output::new(p.PIN_8, Level::Low),
        Output::new(p.PIN_9, Level::Low)
    )));

    let mut actuators = Actuators::new([
        SwitchRails {
            id: ActuatorId::SwitchRails1,
            channel: CHANNEL1.sender(),
        },
        SwitchRails {
            id: ActuatorId::SwitchRails2,
            channel: CHANNEL2.sender(),
        },
        SwitchRails {
            id: ActuatorId::SwitchRails3,
            channel: CHANNEL3.sender(),
        },
        SwitchRails {
            id: ActuatorId::SwitchRails4,
            channel: CHANNEL4.sender(),
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
    id: ActuatorId,
    channel: Sender<'static, ThreadModeRawMutex, SwitchRailsState, 1>,
}

impl SwitchRails {
    async fn switch(&mut self, state: SwitchRailsState) -> Result<()> {
        log::debug!("SwitchRails::switch()");
        self.channel.send(state).await;
        log::info!("SwitchRails::switch(): Setting {} to {}", self.id, state,);
        Ok(())
    }
}

struct Actuators {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    switch_rails: [SwitchRails; 4],
}

impl Actuators {
    pub fn new(switch_rails: [SwitchRails; 4]) -> Self {
        log::debug!("Actuators::new()");

        Actuators {
            bincode_cfg: bincode::config::legacy(),
            switch_rails,
        }
    }

    async fn update_switch_rails(&mut self, id: ActuatorId, state: SwitchRailsState) -> Result<()> {
        log::debug!("Actuators::update_actuator()");
        for switch_rail in self.switch_rails.iter_mut() {
            if switch_rail.id == id {
                switch_rail.switch(state).await?;
                break;
            }
        }

        Ok(())
    }

    async fn handle_op_drive_actuator(&mut self, payload: &[u8]) -> Result<()> {
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
                self.update_switch_rails(actuator_id, state).await?;
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
                Operation::DriveActuator => self.handle_op_drive_actuator(payload).await?,
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

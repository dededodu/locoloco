use std::{
    collections::HashMap,
    io::{self, Write},
    net::TcpStream,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use bincode::{
    config::{Configuration, Fixint, LittleEndian, NoLimit},
    decode_from_std_read, encode_to_vec,
    error::{DecodeError, EncodeError},
};
use loco_protocol::{
    ActuatorId, ActuatorType, BACKEND_PROTOCOL_MAGIC_NUMBER, ConnectPayload, ControlLocoPayload,
    Direction, DriveActuatorPayload, Error as LocoProtocolError, Header, LocoId,
    LocoStatusResponse, Operation, SensorId, SensorStatus, SensorsStatusArray, Speed,
};
use log::debug;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rail_network::{CheckpointId, TrackId};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Actuators not connected")]
    ActuatorsNotConnected,
    #[error("Error converting into expected type")]
    ConvertLocoProtocolType(LocoProtocolError),
    #[error("Error decoding from TCP stream: {0}")]
    DecodeFromStream(#[source] DecodeError),
    #[error("Error encoding to vec: {0}")]
    EncodeToVec(#[source] EncodeError),
    #[error("Invalid backend protocol magic number {0}")]
    InvalidBackendProtocolMagicNumber(u8),
    #[error("Loco {0} not connected")]
    LocoNotConnected(LocoId),
    #[error("Unsupported operation {0}")]
    UnsupportedOperation(Operation),
    #[error("Error writing to TCP stream {0}")]
    WriteTcpStream(#[source] io::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum OracleMode {
    Off,
    Auto,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LocoIntent {
    Drive(Direction, TrackId),
    Stop(Direction, CheckpointId),
}

#[derive(Serialize, Deserialize)]
pub struct LocoStatus {
    direction: Direction,
    speed: Speed,
    location: Option<SensorId>,
    intent: Option<LocoIntent>,
}

impl LocoStatus {
    pub fn speed(&self) -> Speed {
        self.speed
    }

    pub fn location(&self) -> Option<SensorId> {
        self.location
    }

    pub fn intent(&self) -> Option<LocoIntent> {
        self.intent
    }
}

#[derive(Default)]
struct LocoInfo {
    stream: Option<TcpStream>,
    location: Option<SensorId>,
    intent: Option<LocoIntent>,
}

#[derive(Default)]
struct ActuatorInfo {
    stream: Option<TcpStream>,
}

pub struct Backend {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    loco_info: HashMap<LocoId, Mutex<LocoInfo>>,
    actuator_info: Mutex<ActuatorInfo>,
    oracle_enabled: AtomicBool,
}

impl Backend {
    pub fn new() -> Self {
        debug!("Backend::new()");

        let bincode_cfg = bincode::config::legacy();
        let loco_info = HashMap::from([
            (LocoId::Loco1, Mutex::new(LocoInfo::default())),
            (LocoId::Loco2, Mutex::new(LocoInfo::default())),
        ]);
        let actuator_info = Mutex::new(ActuatorInfo::default());
        let oracle_enabled = AtomicBool::new(false);

        Backend {
            bincode_cfg,
            loco_info,
            actuator_info,
            oracle_enabled,
        }
    }

    pub fn loco_ids(&self) -> Vec<LocoId> {
        self.loco_info.iter().map(|(id, _)| *id).collect()
    }

    fn loco_info(&self, loco_id: &LocoId) -> &Mutex<LocoInfo> {
        // Safe to unwrap since loco_info has been filled with every LocoId
        self.loco_info.get(loco_id).unwrap()
    }

    fn retrieve_header_op(&self, stream: &mut TcpStream) -> Result<Operation> {
        debug!("Backend::retrieve_header_op()");

        // Retrieve header
        let header: Header =
            decode_from_std_read(stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?;

        debug!("Backend::retrieve_header_op(): {:?}", header);

        if header.magic != BACKEND_PROTOCOL_MAGIC_NUMBER {
            return Err(Error::InvalidBackendProtocolMagicNumber(header.magic));
        }

        let op = Operation::try_from(header.operation).map_err(Error::ConvertLocoProtocolType)?;
        debug!("Backend::retrieve_header_op(): Operation {:?}", op);

        Ok(op)
    }

    fn handle_op_connect(&self, mut stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_op_connect()");

        // Retrieve payload
        let payload: ConnectPayload =
            decode_from_std_read(&mut stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?;
        let loco_id = LocoId::try_from(payload.loco_id).map_err(Error::ConvertLocoProtocolType)?;
        debug!("Backend::handle_op_connect(): LocoId {:?}", loco_id);

        self.loco_info(&loco_id).lock().unwrap().stream = Some(stream);

        Ok(())
    }

    pub fn handle_loco_connection(&self, mut stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_connection()");

        let op = self.retrieve_header_op(&mut stream)?;

        match op {
            Operation::Connect => self.handle_op_connect(stream)?,
            Operation::ControlLoco
            | Operation::LocoStatus
            | Operation::SensorsStatus
            | Operation::DriveActuator => {
                return Err(Error::UnsupportedOperation(op));
            }
        }

        Ok(())
    }

    pub fn control_loco(&self, loco_id: LocoId, direction: Direction, speed: Speed) -> Result<()> {
        debug!(
            "Backend::control_loco(): loco_id {:?}, direction {:?}, speed {:?}",
            loco_id, direction, speed
        );

        let mut payload = encode_to_vec(
            ControlLocoPayload {
                direction: direction.into(),
                speed: speed.into(),
            },
            self.bincode_cfg,
        )
        .map_err(Error::EncodeToVec)?;

        let mut message = encode_to_vec(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::ControlLoco.into(),
                payload_len: payload.len() as u8,
            },
            self.bincode_cfg,
        )
        .map_err(Error::EncodeToVec)?;

        message.append(&mut payload);

        self.loco_info(&loco_id)
            .lock()
            .unwrap()
            .stream
            .as_mut()
            .ok_or(Error::LocoNotConnected(loco_id))?
            .write_all(message.as_slice())
            .map_err(Error::WriteTcpStream)?;

        Ok(())
    }

    pub fn loco_status(&self, loco_id: LocoId) -> Result<LocoStatus> {
        debug!("Backend::loco_status(): loco_id {:?}", loco_id);

        let message = encode_to_vec(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::LocoStatus.into(),
                payload_len: 0,
            },
            self.bincode_cfg,
        )
        .map_err(Error::EncodeToVec)?;

        let status = {
            let mut loco_info = self.loco_info(&loco_id).lock().unwrap();

            let stream = loco_info
                .stream
                .as_mut()
                .ok_or(Error::LocoNotConnected(loco_id))?;

            stream
                .write_all(message.as_slice())
                .map_err(Error::WriteTcpStream)?;

            let resp: LocoStatusResponse =
                decode_from_std_read(stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?;

            LocoStatus {
                direction: Direction::try_from(resp.direction)
                    .map_err(Error::ConvertLocoProtocolType)?,
                speed: Speed::try_from(resp.speed).map_err(Error::ConvertLocoProtocolType)?,
                location: loco_info.location,
                intent: loco_info.intent,
            }
        };

        Ok(status)
    }

    pub fn drive_actuator(
        &self,
        actuator_id: ActuatorId,
        actuator_type: ActuatorType,
        actuator_state: u8,
    ) -> Result<()> {
        debug!(
            "Backend::drive_switch_rails(): actuator_id {:?}, actuator_type {:?}, state {}",
            actuator_id, actuator_type, actuator_state
        );

        let mut payload = encode_to_vec(
            DriveActuatorPayload {
                actuator_id: actuator_id.into(),
                actuator_type: actuator_type.into(),
                actuator_state,
            },
            self.bincode_cfg,
        )
        .map_err(Error::EncodeToVec)?;

        let mut message = encode_to_vec(
            Header {
                magic: BACKEND_PROTOCOL_MAGIC_NUMBER,
                operation: Operation::DriveActuator.into(),
                payload_len: payload.len() as u8,
            },
            self.bincode_cfg,
        )
        .map_err(Error::EncodeToVec)?;

        message.append(&mut payload);

        self.actuator_info
            .lock()
            .unwrap()
            .stream
            .as_mut()
            .ok_or(Error::ActuatorsNotConnected)?
            .write_all(message.as_slice())
            .map_err(Error::WriteTcpStream)?;

        Ok(())
    }

    pub fn set_oracle_mode(&self, mode: OracleMode) {
        let enable = match mode {
            OracleMode::Off => false,
            OracleMode::Auto => true,
        };
        self.oracle_enabled.store(enable, Ordering::Release);
    }

    pub fn oracle_enabled(&self) -> bool {
        self.oracle_enabled.load(Ordering::Acquire)
    }

    pub fn set_loco_intent(&self, loco_id: LocoId, intent: LocoIntent) {
        self.loco_info(&loco_id)
            .lock()
            .unwrap()
            .intent
            .replace(intent);
    }

    fn handle_op_sensors_status(&self, stream: &mut TcpStream) -> Result<()> {
        debug!("Backend::handle_op_sensors_status()");

        // Retrieve number of sensors being updated
        let sensors_status_array: SensorsStatusArray =
            decode_from_std_read(stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?;

        for _ in 0..sensors_status_array.len {
            let sensor_status: SensorStatus =
                decode_from_std_read(stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?;
            let loco_id =
                LocoId::try_from(sensor_status.loco_id).map_err(Error::ConvertLocoProtocolType)?;
            let sensor_id = SensorId::try_from(sensor_status.sensor_id)
                .map_err(Error::ConvertLocoProtocolType)?;
            debug!(
                "Backend::handle_op_sensors_status(): {} detected at {}",
                loco_id, sensor_id
            );
            self.loco_info(&loco_id).lock().unwrap().location = Some(sensor_id);
        }

        debug!(
            "Backend::handle_op_sensors_status(): {} sensors updated",
            sensors_status_array.len
        );

        Ok(())
    }

    pub fn serve_sensors(&self, mut stream: TcpStream) -> Result<()> {
        debug!("Backend::serve_sensors()");

        loop {
            let op = self.retrieve_header_op(&mut stream)?;

            match op {
                Operation::SensorsStatus => self.handle_op_sensors_status(&mut stream)?,
                Operation::Connect
                | Operation::ControlLoco
                | Operation::LocoStatus
                | Operation::DriveActuator => {
                    return Err(Error::UnsupportedOperation(op));
                }
            }
        }
    }

    pub fn handle_actuators_connection(&self, stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_actuators_connection()");

        self.actuator_info.lock().unwrap().stream = Some(stream);

        Ok(())
    }
}

#![no_std]

use core::fmt;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Error {
    UidTooLong,
    UnknownActuatorId(u8),
    UnknownActuatorType(u8),
    UnknownDirection(u8),
    UnknownLocoId(u8),
    UnknownOperation(u8),
    UnknownSensorId(u8),
    UnknownSpeed(u8),
    UnknownSwitchRailsState(u8),
    UnknownUid,
    UnsupportedOperation(Operation),
}

pub type Result<T> = core::result::Result<T, Error>;

pub const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum LocoId {
    Loco1,
    Loco2,
}

impl TryFrom<u8> for LocoId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => LocoId::Loco1,
            2 => LocoId::Loco2,
            _ => return Err(Error::UnknownLocoId(value)),
        })
    }
}

impl TryFrom<&[u8]> for LocoId {
    type Error = Error;

    fn try_from(uid: &[u8]) -> Result<Self> {
        if uid.len() != 4 {
            return Err(Error::UidTooLong);
        }
        Ok(match uid {
            [0xe3, 0xa6, 0xaf, 0x05] => LocoId::Loco1,
            [0xf1, 0x65, 0xb2, 0x01] => LocoId::Loco2,
            _ => return Err(Error::UnknownUid),
        })
    }
}

impl From<LocoId> for u8 {
    fn from(item: LocoId) -> Self {
        match item {
            LocoId::Loco1 => 1,
            LocoId::Loco2 => 2,
        }
    }
}

impl fmt::Display for LocoId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            LocoId::Loco1 => "Loco1",
            LocoId::Loco2 => "Loco2",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SensorId {
    RfidReader1,
    RfidReader2,
    RfidReader3,
    RfidReader4,
    RfidReader5,
    RfidReader6,
    RfidReader7,
    RfidReader8,
}

impl TryFrom<u8> for SensorId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => SensorId::RfidReader1,
            2 => SensorId::RfidReader2,
            3 => SensorId::RfidReader3,
            4 => SensorId::RfidReader4,
            5 => SensorId::RfidReader5,
            6 => SensorId::RfidReader6,
            7 => SensorId::RfidReader7,
            8 => SensorId::RfidReader8,
            _ => return Err(Error::UnknownSensorId(value)),
        })
    }
}

impl From<SensorId> for u8 {
    fn from(item: SensorId) -> Self {
        match item {
            SensorId::RfidReader1 => 1,
            SensorId::RfidReader2 => 2,
            SensorId::RfidReader3 => 3,
            SensorId::RfidReader4 => 4,
            SensorId::RfidReader5 => 5,
            SensorId::RfidReader6 => 6,
            SensorId::RfidReader7 => 7,
            SensorId::RfidReader8 => 8,
        }
    }
}

impl fmt::Display for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            SensorId::RfidReader1 => "Checkpoint1",
            SensorId::RfidReader2 => "Checkpoint2",
            SensorId::RfidReader3 => "Checkpoint3",
            SensorId::RfidReader4 => "Checkpoint4",
            SensorId::RfidReader5 => "Checkpoint5",
            SensorId::RfidReader6 => "Checkpoint6",
            SensorId::RfidReader7 => "Checkpoint7",
            SensorId::RfidReader8 => "Checkpoint8",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ActuatorId {
    SwitchRails1,
    SwitchRails2,
    SwitchRails3,
    SwitchRails4,
    SwitchRails5,
    SwitchRails6,
    SwitchRails7,
    SwitchRails8,
}

impl TryFrom<u8> for ActuatorId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => ActuatorId::SwitchRails1,
            2 => ActuatorId::SwitchRails2,
            3 => ActuatorId::SwitchRails3,
            4 => ActuatorId::SwitchRails4,
            5 => ActuatorId::SwitchRails5,
            6 => ActuatorId::SwitchRails6,
            7 => ActuatorId::SwitchRails7,
            8 => ActuatorId::SwitchRails8,
            _ => return Err(Error::UnknownActuatorId(value)),
        })
    }
}

impl From<ActuatorId> for u8 {
    fn from(item: ActuatorId) -> Self {
        match item {
            ActuatorId::SwitchRails1 => 1,
            ActuatorId::SwitchRails2 => 2,
            ActuatorId::SwitchRails3 => 3,
            ActuatorId::SwitchRails4 => 4,
            ActuatorId::SwitchRails5 => 5,
            ActuatorId::SwitchRails6 => 6,
            ActuatorId::SwitchRails7 => 7,
            ActuatorId::SwitchRails8 => 8,
        }
    }
}

impl fmt::Display for ActuatorId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            ActuatorId::SwitchRails1 => "SwitchRails1",
            ActuatorId::SwitchRails2 => "SwitchRails2",
            ActuatorId::SwitchRails3 => "SwitchRails3",
            ActuatorId::SwitchRails4 => "SwitchRails4",
            ActuatorId::SwitchRails5 => "SwitchRails5",
            ActuatorId::SwitchRails6 => "SwitchRails6",
            ActuatorId::SwitchRails7 => "SwitchRails7",
            ActuatorId::SwitchRails8 => "SwitchRails8",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ActuatorType {
    #[default]
    SwitchRails,
}

impl TryFrom<u8> for ActuatorType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => ActuatorType::SwitchRails,
            _ => return Err(Error::UnknownActuatorType(value)),
        })
    }
}

impl From<ActuatorType> for u8 {
    fn from(item: ActuatorType) -> Self {
        match item {
            ActuatorType::SwitchRails => 1,
        }
    }
}

impl fmt::Display for ActuatorType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            ActuatorType::SwitchRails => "SwitchRails",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum SwitchRailsState {
    #[default]
    Direct,
    Diverted,
}

impl TryFrom<u8> for SwitchRailsState {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => SwitchRailsState::Direct,
            2 => SwitchRailsState::Diverted,
            _ => return Err(Error::UnknownSwitchRailsState(value)),
        })
    }
}

impl From<SwitchRailsState> for u8 {
    fn from(item: SwitchRailsState) -> Self {
        match item {
            SwitchRailsState::Direct => 1,
            SwitchRailsState::Diverted => 2,
        }
    }
}

impl fmt::Display for SwitchRailsState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            SwitchRailsState::Direct => "Direct",
            SwitchRailsState::Diverted => "Diverted",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Forward,
    Backward,
}

impl TryFrom<u8> for Direction {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => Direction::Forward,
            2 => Direction::Backward,
            _ => return Err(Error::UnknownDirection(value)),
        })
    }
}

impl From<Direction> for u8 {
    fn from(item: Direction) -> Self {
        match item {
            Direction::Forward => 1,
            Direction::Backward => 2,
        }
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    #[default]
    Stop,
    Slow,
    Normal,
    Fast,
    PwmDutyCycle(u8),
}

const SPEED_PWM_RANGE: u8 = 100;
const SPEED_PWM_IDX_L: u8 = 100;
const SPEED_PWM_IDX_H: u8 = SPEED_PWM_IDX_L + SPEED_PWM_RANGE;

impl TryFrom<u8> for Speed {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Speed::Stop,
            1 => Speed::Slow,
            2 => Speed::Normal,
            3 => Speed::Fast,
            SPEED_PWM_IDX_L..SPEED_PWM_IDX_H => Speed::PwmDutyCycle(value - SPEED_PWM_IDX_L),
            _ => return Err(Error::UnknownSpeed(value)),
        })
    }
}

impl From<Speed> for u8 {
    fn from(item: Speed) -> Self {
        match item {
            Speed::Stop => 0,
            Speed::Slow => 1,
            Speed::Normal => 2,
            Speed::Fast => 3,
            Speed::PwmDutyCycle(mut duty_percent) => {
                if duty_percent > SPEED_PWM_RANGE {
                    duty_percent = SPEED_PWM_RANGE;
                }
                duty_percent + SPEED_PWM_IDX_L
            }
        }
    }
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub enum Operation {
    Connect,
    ControlLoco,
    LocoStatus,
    SensorsStatus,
    DriveActuator,
}

impl TryFrom<u8> for Operation {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => Operation::Connect,
            2 => Operation::ControlLoco,
            3 => Operation::LocoStatus,
            4 => Operation::SensorsStatus,
            5 => Operation::DriveActuator,
            _ => return Err(Error::UnknownOperation(value)),
        })
    }
}

impl From<Operation> for u8 {
    fn from(item: Operation) -> Self {
        match item {
            Operation::Connect => 1,
            Operation::ControlLoco => 2,
            Operation::LocoStatus => 3,
            Operation::SensorsStatus => 4,
            Operation::DriveActuator => 5,
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op = match *self {
            Operation::Connect => "Connect",
            Operation::ControlLoco => "ControlLoco",
            Operation::LocoStatus => "LocoStatus",
            Operation::SensorsStatus => "SensorsStatus",
            Operation::DriveActuator => "DriveActuator",
        };
        write!(f, "{}", op)
    }
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct ConnectPayload {
    pub loco_id: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct ControlLocoPayload {
    pub direction: u8,
    pub speed: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct SensorsStatusArray {
    pub len: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct SensorStatus {
    pub sensor_id: u8,
    pub loco_id: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct LocoStatusResponse {
    pub direction: u8,
    pub speed: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct DriveActuatorPayload {
    pub actuator_id: u8,
    pub actuator_type: u8,
    pub actuator_state: u8,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub struct Header {
    pub magic: u8,
    pub operation: u8,
    pub payload_len: u8,
}

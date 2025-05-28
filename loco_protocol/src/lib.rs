#![no_std]

use core::fmt;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Error {
    UnknownDirection(u8),
    UnknownLocoId(u8),
    UnknownOperation(u8),
    UnknownSensorId(u8),
    UnknownSpeed(u8),
    UnsupportedOperation(Operation),
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, Hash, PartialEq)]
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
}

impl TryFrom<u8> for SensorId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => SensorId::RfidReader1,
            2 => SensorId::RfidReader2,
            3 => SensorId::RfidReader3,
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
        }
    }
}

impl fmt::Display for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let id = match *self {
            SensorId::RfidReader1 => "Checkpoint1",
            SensorId::RfidReader2 => "Checkpoint2",
            SensorId::RfidReader3 => "Checkpoint3",
        };
        write!(f, "{}", id)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default)]
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

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    #[default]
    Stop,
    Slow,
    Normal,
    Fast,
}

impl TryFrom<u8> for Speed {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Speed::Stop,
            1 => Speed::Slow,
            2 => Speed::Normal,
            3 => Speed::Fast,
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
        }
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub struct ControlLoco {
    pub loco_id: LocoId,
    pub direction: Direction,
    pub speed: Speed,
}

#[derive(Encode, Decode, Copy, Clone, Debug)]
pub enum Operation {
    Connect,
    ControlLoco,
    LocoStatus,
    SensorsStatus,
}

impl TryFrom<u8> for Operation {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            1 => Operation::Connect,
            2 => Operation::ControlLoco,
            3 => Operation::LocoStatus,
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
pub struct Header {
    pub magic: u8,
    pub operation: u8,
    pub payload_len: u8,
}

#[derive(Serialize, Deserialize)]
pub struct LocoStatus {
    pub direction: Direction,
    pub speed: Speed,
}

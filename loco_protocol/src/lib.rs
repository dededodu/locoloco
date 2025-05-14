#![no_std]

use core::fmt;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Error {
    UnknownDirection(u8),
    UnknownLocoId(u8),
    UnknownOperation(u8),
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

impl Into<u8> for LocoId {
    fn into(self) -> u8 {
        match self {
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

impl Into<u8> for Direction {
    fn into(self) -> u8 {
        match self {
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

impl Into<u8> for Speed {
    fn into(self) -> u8 {
        match self {
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

impl Into<u8> for Operation {
    fn into(self) -> u8 {
        match self {
            Operation::Connect => 1,
            Operation::ControlLoco => 2,
            Operation::LocoStatus => 3,
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op = match *self {
            Operation::Connect => "Connect",
            Operation::ControlLoco => "ControlLoco",
            Operation::LocoStatus => "LocoStatus",
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

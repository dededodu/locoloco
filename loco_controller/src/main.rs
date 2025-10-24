use actix_web::{
    App, HttpResponse, HttpServer, Responder, body::BoxBody, get, http::StatusCode, post, web,
};
use bincode::{
    config::{Configuration, Fixint, LittleEndian, NoLimit},
    decode_from_std_read, encode_to_vec,
    error::{DecodeError, EncodeError},
};
use clap::Parser;
use loco_protocol::{
    ActuatorId, ActuatorType, ConnectPayload, ControlLoco, ControlLocoPayload, Direction,
    DriveActuatorPayload, DriveSwitchRails, Error as LocoProtocolError, Header, LocoId,
    LocoStatusResponse, Operation, OracleMode, SensorId, SensorStatus, SensorsStatusArray, Speed,
    SwitchRailsState,
};
use log::{debug, error};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, sleep},
    time::Duration,
};
use thiserror::Error;

const BACKEND_PROTOCOL_MAGIC_NUMBER: u8 = 0xab;

#[derive(Debug, Error)]
enum Error {
    #[error("Actuators not connected")]
    ActuatorsNotConnected,
    #[error("Error binding listener {0}")]
    BindListener(#[source] io::Error),
    #[error("Error converting into expected type")]
    ConvertLocoProtocolType(LocoProtocolError),
    #[error("Error converting Checkpoints into SegmentId")]
    ConvertCheckpointsIntoSegmentId,
    #[error("Error decoding from TCP stream: {0}")]
    DecodeFromStream(#[source] DecodeError),
    #[error("Error encoding to vec: {0}")]
    EncodeToVec(#[source] EncodeError),
    #[error("Error running HTTP server {0}")]
    HttpServer(#[source] io::Error),
    #[error("Invalid backend protocol magic number {0}")]
    InvalidBackendProtocolMagicNumber(u8),
    #[error("Loco {0} not connected")]
    LocoNotConnected(LocoId),
    #[error("Could not find the next checkpoint")]
    NextCheckpointNotFound,
    #[error("Unsupported operation {0}")]
    UnsupportedOperation(Operation),
    #[error("Error writing to TCP stream {0}")]
    WriteTcpStream(#[source] io::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LocoIntent {
    Drive(Direction, TrackId),
    Stop(Direction, CheckpointId),
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub struct LocoIntentJson {
    pub loco_id: LocoId,
    pub loco_intent: LocoIntent,
}

#[derive(Serialize, Deserialize)]
pub struct LocoStatus {
    pub direction: Direction,
    pub speed: Speed,
    pub location: Option<SensorId>,
    pub intent: Option<LocoIntent>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SegmentPriority {
    Priority0,
    Priority1,
    Priority2,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TrackId {
    Track1,
    Station1,
    Station2,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointId {
    Checkpoint1,
    Checkpoint2,
    Checkpoint3,
    Checkpoint4,
    Checkpoint5,
    Checkpoint6,
    Station1,
    Station2,
}

impl Into<SensorId> for CheckpointId {
    fn into(self) -> SensorId {
        match self {
            CheckpointId::Checkpoint1 => SensorId::RfidReader1,
            CheckpointId::Checkpoint2 => SensorId::RfidReader2,
            CheckpointId::Checkpoint3 => SensorId::RfidReader3,
            CheckpointId::Checkpoint4 => SensorId::RfidReader4,
            CheckpointId::Checkpoint5 => SensorId::RfidReader5,
            CheckpointId::Checkpoint6 => SensorId::RfidReader6,
            CheckpointId::Station1 => SensorId::RfidReader7,
            CheckpointId::Station2 => SensorId::RfidReader8,
        }
    }
}

impl Into<CheckpointId> for SensorId {
    fn into(self) -> CheckpointId {
        match self {
            SensorId::RfidReader1 => CheckpointId::Checkpoint1,
            SensorId::RfidReader2 => CheckpointId::Checkpoint2,
            SensorId::RfidReader3 => CheckpointId::Checkpoint3,
            SensorId::RfidReader4 => CheckpointId::Checkpoint4,
            SensorId::RfidReader5 => CheckpointId::Checkpoint5,
            SensorId::RfidReader6 => CheckpointId::Checkpoint6,
            SensorId::RfidReader7 => CheckpointId::Station1,
            SensorId::RfidReader8 => CheckpointId::Station2,
        }
    }
}

struct Checkpoint {
    checkpoint_ids: BTreeMap<Direction, Vec<CheckpointId>>,
    track_id: TrackId,
    priority: SegmentPriority,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SegmentId {
    Segment1,
    Segment2,
    Segment3,
    Segment4,
    Segment5,
    Segment6,
    Segment7,
    Segment8,
    Segment9,
    Segment10,
}

impl TryInto<SegmentId> for (CheckpointId, CheckpointId) {
    type Error = Error;

    fn try_into(self) -> Result<SegmentId> {
        Ok(match self {
            (CheckpointId::Checkpoint1, CheckpointId::Checkpoint2) => SegmentId::Segment1,
            (CheckpointId::Checkpoint2, CheckpointId::Checkpoint1) => SegmentId::Segment1,
            (CheckpointId::Checkpoint2, CheckpointId::Checkpoint3) => SegmentId::Segment2,
            (CheckpointId::Checkpoint3, CheckpointId::Checkpoint2) => SegmentId::Segment2,
            (CheckpointId::Checkpoint3, CheckpointId::Checkpoint4) => SegmentId::Segment3,
            (CheckpointId::Checkpoint4, CheckpointId::Checkpoint3) => SegmentId::Segment3,
            (CheckpointId::Checkpoint4, CheckpointId::Checkpoint5) => SegmentId::Segment4,
            (CheckpointId::Checkpoint5, CheckpointId::Checkpoint4) => SegmentId::Segment4,
            (CheckpointId::Checkpoint5, CheckpointId::Checkpoint6) => SegmentId::Segment5,
            (CheckpointId::Checkpoint6, CheckpointId::Checkpoint5) => SegmentId::Segment5,
            (CheckpointId::Checkpoint6, CheckpointId::Checkpoint1) => SegmentId::Segment6,
            (CheckpointId::Checkpoint1, CheckpointId::Checkpoint6) => SegmentId::Segment6,
            (CheckpointId::Checkpoint6, CheckpointId::Station1) => SegmentId::Segment7,
            (CheckpointId::Station1, CheckpointId::Checkpoint6) => SegmentId::Segment7,
            (CheckpointId::Station1, CheckpointId::Checkpoint2) => SegmentId::Segment8,
            (CheckpointId::Checkpoint2, CheckpointId::Station1) => SegmentId::Segment8,
            (CheckpointId::Checkpoint3, CheckpointId::Station2) => SegmentId::Segment9,
            (CheckpointId::Station2, CheckpointId::Checkpoint3) => SegmentId::Segment9,
            (CheckpointId::Checkpoint5, CheckpointId::Station2) => SegmentId::Segment10,
            (CheckpointId::Station2, CheckpointId::Checkpoint5) => SegmentId::Segment10,
            _ => return Err(Error::ConvertCheckpointsIntoSegmentId),
        })
    }
}

#[derive(Copy, Clone, Debug)]
struct SwitchRails {
    actuator_id: ActuatorId,
    state: SwitchRailsState,
}

#[derive(Clone, Debug)]
struct Segment {
    priority: SegmentPriority,
    switch_rails: Vec<SwitchRails>,
    conflicts: Vec<SegmentId>,
}

#[derive(Clone, Debug)]
struct ActiveSegment {
    id: Option<SegmentId>,
    segment: Option<Segment>,
    direction: Direction,
    loco_id: LocoId,
}

struct ActiveLoco {
    id: LocoId,
    speed: Speed,
    location: Option<CheckpointId>,
    intent: Option<LocoIntent>,
}

pub struct RailNetwork {
    checkpoints: BTreeMap<CheckpointId, Checkpoint>,
    segments: BTreeMap<SegmentId, Segment>,
    longest_path: usize,
}

impl RailNetwork {
    fn new() -> Self {
        RailNetwork {
            checkpoints: BTreeMap::from([
                (
                    CheckpointId::Checkpoint1,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint2])),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint6])),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Checkpoint2,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint3])),
                            (
                                Direction::Backward,
                                Vec::from([CheckpointId::Checkpoint1, CheckpointId::Station1]),
                            ),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Checkpoint3,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (
                                Direction::Forward,
                                Vec::from([CheckpointId::Checkpoint4, CheckpointId::Station2]),
                            ),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint2])),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Checkpoint4,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint5])),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint3])),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Checkpoint5,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint6])),
                            (
                                Direction::Backward,
                                Vec::from([CheckpointId::Checkpoint4, CheckpointId::Station2]),
                            ),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Checkpoint6,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (
                                Direction::Forward,
                                Vec::from([CheckpointId::Checkpoint1, CheckpointId::Station1]),
                            ),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint5])),
                        ]),
                        track_id: TrackId::Track1,
                        priority: SegmentPriority::Priority0,
                    },
                ),
                (
                    CheckpointId::Station1,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint2])),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint6])),
                        ]),
                        track_id: TrackId::Station1,
                        priority: SegmentPriority::Priority1,
                    },
                ),
                (
                    CheckpointId::Station2,
                    Checkpoint {
                        checkpoint_ids: BTreeMap::from([
                            (Direction::Forward, Vec::from([CheckpointId::Checkpoint5])),
                            (Direction::Backward, Vec::from([CheckpointId::Checkpoint3])),
                        ]),
                        track_id: TrackId::Station2,
                        priority: SegmentPriority::Priority1,
                    },
                ),
            ]),
            segments: BTreeMap::from([
                (
                    SegmentId::Segment1,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails2,
                            state: SwitchRailsState::Direct,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment8]),
                    },
                ),
                (
                    SegmentId::Segment2,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::new(),
                        conflicts: Vec::new(),
                    },
                ),
                (
                    SegmentId::Segment3,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails3,
                            state: SwitchRailsState::Direct,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment9]),
                    },
                ),
                (
                    SegmentId::Segment4,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails4,
                            state: SwitchRailsState::Direct,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment10]),
                    },
                ),
                (
                    SegmentId::Segment5,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::new(),
                        conflicts: Vec::new(),
                    },
                ),
                (
                    SegmentId::Segment6,
                    Segment {
                        priority: SegmentPriority::Priority0,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails1,
                            state: SwitchRailsState::Direct,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment7]),
                    },
                ),
                (
                    SegmentId::Segment7,
                    Segment {
                        priority: SegmentPriority::Priority1,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails1,
                            state: SwitchRailsState::Diverted,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment6]),
                    },
                ),
                (
                    SegmentId::Segment8,
                    Segment {
                        priority: SegmentPriority::Priority1,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails2,
                            state: SwitchRailsState::Diverted,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment1]),
                    },
                ),
                (
                    SegmentId::Segment9,
                    Segment {
                        priority: SegmentPriority::Priority1,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails3,
                            state: SwitchRailsState::Diverted,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment3]),
                    },
                ),
                (
                    SegmentId::Segment10,
                    Segment {
                        priority: SegmentPriority::Priority1,
                        switch_rails: Vec::from([SwitchRails {
                            actuator_id: ActuatorId::SwitchRails4,
                            state: SwitchRailsState::Diverted,
                        }]),
                        conflicts: Vec::from([SegmentId::Segment4]),
                    },
                ),
            ]),
            longest_path: 6,
        }
    }

    fn next_checkpoint_id_for_track_id_target(
        &self,
        iteration: usize,
        cp_id: CheckpointId,
        direction: Direction,
        target_track_id: TrackId,
    ) -> Option<CheckpointId> {
        let mut next_cp_ids = self
            .checkpoints
            .get(&cp_id)
            .unwrap()
            .checkpoint_ids
            .get(&direction)
            .unwrap()
            .clone();

        for next_cp_id in next_cp_ids.iter() {
            let next_cp = self.checkpoints.get(next_cp_id).unwrap();
            if next_cp.track_id == target_track_id {
                return Some(*next_cp_id);
            }
        }

        // Order the checkpoints by priority
        next_cp_ids.sort_by_key(|cid| self.checkpoints.get(cid).unwrap().priority);

        for next_cp_id in next_cp_ids.iter() {
            if iteration >= self.longest_path {
                return None;
            }

            if self
                .next_checkpoint_id_for_track_id_target(
                    iteration + 1,
                    *next_cp_id,
                    direction,
                    target_track_id,
                )
                .is_some()
            {
                return Some(*next_cp_id);
            }
        }

        return None;
    }

    fn next_checkpoint_id_for_checkpoint_id_target(
        &self,
        iteration: usize,
        cp_id: CheckpointId,
        direction: Direction,
        target_cp_id: CheckpointId,
    ) -> Option<CheckpointId> {
        let mut next_cp_ids = self
            .checkpoints
            .get(&cp_id)
            .unwrap()
            .checkpoint_ids
            .get(&direction)
            .unwrap()
            .clone();

        for next_cp_id in next_cp_ids.iter() {
            if *next_cp_id == target_cp_id {
                return Some(*next_cp_id);
            }
        }

        // Order the checkpoints by priority
        next_cp_ids.sort_by_key(|cid| self.checkpoints.get(cid).unwrap().priority);

        for next_cp_id in next_cp_ids.iter() {
            if iteration >= self.longest_path {
                return None;
            }

            if self
                .next_checkpoint_id_for_checkpoint_id_target(
                    iteration + 1,
                    *next_cp_id,
                    direction,
                    target_cp_id,
                )
                .is_some()
            {
                return Some(*next_cp_id);
            }
        }

        return None;
    }
}

struct Oracle {
    backend: Arc<Backend>,
    rail_network: RailNetwork,
    last_segment_id: BTreeMap<LocoId, SegmentId>,
}

impl Oracle {
    fn new(backend: Arc<Backend>) -> Self {
        debug!("Oracle::new()");
        Oracle {
            backend,
            rail_network: RailNetwork::new(),
            last_segment_id: BTreeMap::new(),
        }
    }

    fn active_locos(&self) -> Result<Vec<ActiveLoco>> {
        let mut active_locos = Vec::new();
        for loco_id in self.backend.loco_info.keys() {
            match self.backend.loco_status(*loco_id) {
                Ok(status) => {
                    active_locos.push(ActiveLoco {
                        id: *loco_id,
                        speed: status.speed,
                        location: status.location.map(|l| l.into()),
                        intent: status.intent,
                    });
                }
                Err(Error::LocoNotConnected(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(active_locos)
    }

    fn determine_active_segments(&self) -> Result<Vec<ActiveSegment>> {
        let mut active_segments: Vec<ActiveSegment> = Vec::new();
        let mut busy_checkpoint_ids: Vec<CheckpointId> = Vec::new();
        let active_locos = self.active_locos()?;

        // For every loco:
        //  - Check if loco is stopped to identify a busy checkpoint
        for active_loco in active_locos.iter() {
            if let Some(location) = active_loco.location {
                if active_loco.speed == Speed::Stop {
                    busy_checkpoint_ids.push(location);
                }
            }
        }

        // For every loco:
        //  - Identify where it's located on the network
        //  - Compute the next checkpoint based on the intent
        //  - Find out active segment based on current and next checkpoints
        for active_loco in active_locos.iter() {
            // If a loco has no known location or no known intent, there's
            // nothing that can be done to drive it.
            if active_loco.location.is_none() || active_loco.intent.is_none() {
                continue;
            }

            let checkpoint_id = active_loco.location.unwrap();
            let intent = active_loco.intent.unwrap();

            let (next_checkpoint_id, direction) = match intent {
                LocoIntent::Drive(direction, target_track_id) => (
                    self.rail_network
                        .next_checkpoint_id_for_track_id_target(
                            0,
                            checkpoint_id,
                            direction,
                            target_track_id,
                        )
                        .ok_or(Error::NextCheckpointNotFound)?,
                    direction,
                ),
                LocoIntent::Stop(direction, target_checkpoint_id) => {
                    if target_checkpoint_id == checkpoint_id {
                        active_segments.push(ActiveSegment {
                            id: None,
                            segment: None,
                            direction,
                            loco_id: active_loco.id,
                        });
                        continue;
                    }

                    (
                        self.rail_network
                            .next_checkpoint_id_for_checkpoint_id_target(
                                0,
                                checkpoint_id,
                                direction,
                                target_checkpoint_id,
                            )
                            .ok_or(Error::NextCheckpointNotFound)?,
                        direction,
                    )
                }
            };

            if busy_checkpoint_ids.contains(&next_checkpoint_id) {
                active_segments.push(ActiveSegment {
                    id: None,
                    segment: None,
                    direction,
                    loco_id: active_loco.id,
                });
                continue;
            }

            let active_segment_id: SegmentId = (checkpoint_id, next_checkpoint_id).try_into()?;
            active_segments.push(ActiveSegment {
                id: Some(active_segment_id),
                segment: Some(
                    self.rail_network
                        .segments
                        .get(&active_segment_id)
                        .unwrap()
                        .clone(),
                ),
                direction,
                loco_id: active_loco.id,
            });
        }

        Ok(active_segments)
    }

    fn sort_active_segments(&self, active_segments: Vec<ActiveSegment>) -> Vec<ActiveSegment> {
        // First, let's re-order so that two identical active segments are
        // correctly ordered with the first one being the first loco on the
        // network. We use the information about the last segment being
        // identical or not to determine which loco is ahead.
        // Given the whole logic, we don't expect more than 2 locos to be
        // sharing the same active segments.
        let mut sorted_active_segments: Vec<ActiveSegment> = Vec::new();
        for segment in active_segments.iter() {
            if let Some(sid) = segment.id {
                if let Some(last_segment_id) = self.last_segment_id.get(&segment.loco_id) {
                    let mut insertion_idx = None;
                    for (i, sorted_segment) in sorted_active_segments.iter().enumerate() {
                        if let Some(sorted_sid) = sorted_segment.id {
                            if (sid == sorted_sid) && (sid == *last_segment_id) {
                                insertion_idx = Some(i);
                                break;
                            }
                        }
                    }
                    if let Some(i) = insertion_idx {
                        sorted_active_segments.insert(i, segment.clone());
                        continue;
                    }
                }
            }
            sorted_active_segments.push(segment.clone());
        }

        // Let's now make sure that we sort the active segments based on every
        // segment's priority. It's important to note that two elements with
        // the same priority won't get re-ordered. This is mandatory to ensure
        // the previous ordering won't get broken given two identical segments
        // will always have the same priority.
        sorted_active_segments.sort_by_key(|s| {
            if let Some(segment) = s.segment.as_ref() {
                segment.priority
            } else {
                SegmentPriority::Priority2
            }
        });

        sorted_active_segments
    }

    fn determine_controls(
        &mut self,
        active_segments: Vec<ActiveSegment>,
    ) -> (
        Vec<(ActuatorId, ActuatorType, u8)>,
        Vec<(LocoId, Direction, Speed)>,
    ) {
        let mut actuator_controls: Vec<(ActuatorId, ActuatorType, u8)> = Vec::new();
        let mut loco_controls: Vec<(LocoId, Direction, Speed)> = Vec::new();
        let mut busy_segment_ids: Vec<SegmentId> = Vec::new();

        // For every active segment:
        //  - Find out if the segment conflicts with an already busy segment
        //  - Determine if some actuator control needs to be applied
        //  - Determine the control that should be applied for the loco
        for active_segment in active_segments.iter() {
            let loco_id = active_segment.loco_id;
            let direction = active_segment.direction;

            if let (Some(segment_id), Some(segment)) =
                (active_segment.id, active_segment.segment.as_ref())
            {
                if !busy_segment_ids.contains(&segment_id) {
                    let mut conflict_found = false;
                    for conflict_segment_id in segment.conflicts.iter() {
                        if busy_segment_ids.contains(conflict_segment_id) {
                            conflict_found = true;
                            break;
                        }
                    }

                    if !conflict_found {
                        for switch_rails in segment.switch_rails.iter() {
                            actuator_controls.push((
                                switch_rails.actuator_id,
                                ActuatorType::SwitchRails,
                                switch_rails.state.into(),
                            ));
                        }

                        loco_controls.push((loco_id, direction, Speed::Normal));
                        busy_segment_ids.push(segment_id);
                        self.last_segment_id.insert(loco_id, segment_id);
                        continue;
                    }
                }
            }

            loco_controls.push((loco_id, direction, Speed::Stop));
        }

        (actuator_controls, loco_controls)
    }

    fn process(&mut self) -> Result<()> {
        if !self.backend.oracle_enabled() {
            return Ok(());
        }

        // Get the active segments
        let active_segments = self.determine_active_segments()?;
        // Sort the segments by order of loco on the same segment, and by overall priority
        let sorted_active_segments = self.sort_active_segments(active_segments);
        let (actuator_controls, loco_controls) = self.determine_controls(sorted_active_segments);

        // Apply controls for actuators
        for (actuator_id, actuator_type, actuator_state) in actuator_controls {
            self.backend
                .drive_actuator(actuator_id, actuator_type, actuator_state)?;
        }

        // Apply controls for locos
        for (loco_id, direction, speed) in loco_controls {
            self.backend.control_loco(loco_id, direction, speed)?;
        }

        Ok(())
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

struct Backend {
    bincode_cfg: Configuration<LittleEndian, Fixint, NoLimit>,
    loco_info: HashMap<LocoId, Mutex<LocoInfo>>,
    actuator_info: Mutex<ActuatorInfo>,
    oracle_enabled: AtomicBool,
}

impl Backend {
    fn new() -> Self {
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

        self.loco_info.get(&loco_id).unwrap().lock().unwrap().stream = Some(stream);

        Ok(())
    }

    fn handle_loco_connection(&self, mut stream: TcpStream) -> Result<()> {
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

    fn control_loco(&self, loco_id: LocoId, direction: Direction, speed: Speed) -> Result<()> {
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

        self.loco_info
            .get(&loco_id)
            .unwrap()
            .lock()
            .unwrap()
            .stream
            .as_mut()
            .ok_or(Error::LocoNotConnected(loco_id))?
            .write_all(message.as_slice())
            .map_err(Error::WriteTcpStream)?;

        Ok(())
    }

    fn loco_status(&self, loco_id: LocoId) -> Result<LocoStatus> {
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

        let resp: LocoStatusResponse = {
            let mut loco_info = self.loco_info.get(&loco_id).unwrap().lock().unwrap();

            let stream = loco_info
                .stream
                .as_mut()
                .ok_or(Error::LocoNotConnected(loco_id))?;

            stream
                .write_all(message.as_slice())
                .map_err(Error::WriteTcpStream)?;

            decode_from_std_read(stream, self.bincode_cfg).map_err(Error::DecodeFromStream)?
        };

        let status = LocoStatus {
            direction: Direction::try_from(resp.direction)
                .map_err(Error::ConvertLocoProtocolType)?,
            speed: Speed::try_from(resp.speed).map_err(Error::ConvertLocoProtocolType)?,
            location: self
                .loco_info
                .get(&loco_id)
                .unwrap()
                .lock()
                .unwrap()
                .location,
            intent: self.loco_info.get(&loco_id).unwrap().lock().unwrap().intent,
        };

        Ok(status)
    }

    fn drive_actuator(
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

    fn set_oracle_mode(&self, mode: OracleMode) {
        let enable = match mode {
            OracleMode::Off => false,
            OracleMode::Auto => true,
        };
        self.oracle_enabled.store(enable, Ordering::Release);
    }

    fn oracle_enabled(&self) -> bool {
        self.oracle_enabled.load(Ordering::Acquire)
    }

    fn set_loco_intent(&self, loco_id: LocoId, intent: LocoIntent) {
        self.loco_info
            .get(&loco_id)
            .unwrap()
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
            self.loco_info
                .get(&loco_id)
                .unwrap()
                .lock()
                .unwrap()
                .location = Some(sensor_id);
        }

        debug!(
            "Backend::handle_op_sensors_status(): {} sensors updated",
            sensors_status_array.len
        );

        Ok(())
    }

    fn serve_sensors(&self, mut stream: TcpStream) -> Result<()> {
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

    fn handle_actuators_connection(&self, stream: TcpStream) -> Result<()> {
        debug!("Backend::handle_actuators_connection()");

        self.actuator_info.lock().unwrap().stream = Some(stream);

        Ok(())
    }
}

#[get("/")]
async fn index(_data: web::Data<Arc<Backend>>) -> impl Responder {
    HttpResponse::Ok().body("Loco controller running!")
}

#[get("/loco_status/{loco_id}")]
async fn loco_status(path: web::Path<LocoId>, data: web::Data<Arc<Backend>>) -> impl Responder {
    let loco_id = path.into_inner();

    match data.loco_status(loco_id) {
        Ok(status) => HttpResponse::Ok().json(status),
        Err(e) => {
            error!("{}", e);
            HttpResponse::with_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                BoxBody::new(format!("{}", e)),
            )
        }
    }
}

#[post("/control_loco")]
async fn control_loco(
    form: web::Json<ControlLoco>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    if data.oracle_enabled() {
        let e = "Oracle is running, can't manually control the loco";
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    if let Err(e) = data.control_loco(form.loco_id, form.direction, form.speed) {
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    HttpResponse::Ok().body(format!(
        "Move {:?} loco {:?} at {:?} speed",
        form.direction, form.loco_id, form.speed
    ))
}

#[post("/loco_intent")]
async fn loco_intent(
    form: web::Json<LocoIntentJson>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    data.set_loco_intent(form.loco_id, form.loco_intent);
    HttpResponse::Ok().body(format!(
        "Setting loco intent {:?} for {:?}",
        form.loco_intent, form.loco_id
    ))
}

#[post("/drive_switch_rails")]
async fn drive_switch_rails(
    form: web::Json<DriveSwitchRails>,
    data: web::Data<Arc<Backend>>,
) -> impl Responder {
    if data.oracle_enabled() {
        let e = "Oracle is running, can't manually drive switch rails";
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    if let Err(e) = data.drive_actuator(
        form.actuator_id,
        ActuatorType::SwitchRails,
        form.state.into(),
    ) {
        error!("{}", e);
        return HttpResponse::with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            BoxBody::new(format!("{}", e)),
        );
    }

    HttpResponse::Ok().body(format!("Drive {:?} to {:?}", form.actuator_id, form.state))
}

#[post("/oracle_mode")]
async fn oracle_mode(form: web::Json<OracleMode>, data: web::Data<Arc<Backend>>) -> impl Responder {
    data.set_oracle_mode(form.0);
    HttpResponse::Ok().body(format!("Setting Oracle to mode {:?}", form.0))
}

#[actix_web::main]
async fn http_main(port: u16, backend: Arc<Backend>) -> std::io::Result<()> {
    debug!("http_main(): Waiting for incoming connection...");
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(backend.clone()))
            .service(index)
            .service(loco_status)
            .service(control_loco)
            .service(loco_intent)
            .service(drive_switch_rails)
            .service(oracle_mode)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

fn backend_locos(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_locos(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_locos(): Connected");
        if let Err(e) = backend.handle_loco_connection(stream) {
            error!("{}", e);
        }
    }
}

fn backend_sensors(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_sensors(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_sensors(): Connected");
        if let Err(e) = backend.serve_sensors(stream) {
            error!("{}", e);
        }
    }
}

fn backend_actuators(port: u16, backend: Arc<Backend>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(Error::BindListener)?;

    loop {
        debug!("backend_actuators(): Waiting for incoming connection...");
        let (stream, _) = listener.accept().map_err(Error::BindListener)?;
        debug!("backend_actuators(): Connected");
        if let Err(e) = backend.handle_actuators_connection(stream) {
            error!("{}", e);
        }
    }
}

fn backend_oracle(backend: Arc<Backend>) -> Result<()> {
    debug!("backend_oracle()");
    let mut oracle = Oracle::new(backend);
    loop {
        if let Err(e) = oracle.process() {
            error!("{}", e);
        }
        sleep(Duration::from_millis(10));
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 8080)]
    http_port: u16,
    #[arg(long, default_value_t = 8004)]
    backend_locos_port: u16,
    #[arg(long, default_value_t = 8005)]
    backend_sensors_port: u16,
    #[arg(long, default_value_t = 8006)]
    backend_actuators_port: u16,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();

    // Initialize backend
    let backend = Arc::new(Backend::new());
    let shared_backend_locos = backend.clone();
    let shared_backend_sensors = backend.clone();
    let shared_backend_actuators = backend.clone();
    let shared_backend_oracle = backend.clone();

    // Start backend server, waiting for incoming connections from locos
    thread::spawn(move || backend_locos(args.backend_locos_port, shared_backend_locos));

    // Start backend server, waiting for updates on locos' positions
    thread::spawn(move || backend_sensors(args.backend_sensors_port, shared_backend_sensors));

    // Start backend server, waiting for incoming connection from actuators
    thread::spawn(move || backend_actuators(args.backend_actuators_port, shared_backend_actuators));

    // Start railway network automation process
    thread::spawn(move || backend_oracle(shared_backend_oracle));

    http_main(args.http_port, backend).map_err(Error::HttpServer)?;

    Ok(())
}

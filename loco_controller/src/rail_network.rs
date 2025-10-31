use std::collections::BTreeMap;

use loco_protocol::{ActuatorId, Direction, SensorId, SwitchRailsState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Error converting Checkpoints into SegmentId")]
    ConvertCheckpointsIntoSegmentId,
}

type Result<T> = std::result::Result<T, Error>;

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

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SegmentId {
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
pub struct SwitchRails {
    actuator_id: ActuatorId,
    state: SwitchRailsState,
}

impl SwitchRails {
    pub fn actuator_id(&self) -> ActuatorId {
        self.actuator_id
    }

    pub fn state(&self) -> SwitchRailsState {
        self.state
    }
}

#[derive(Clone, Debug)]
pub struct Segment {
    priority: SegmentPriority,
    switch_rails: Vec<SwitchRails>,
    conflicts: Vec<SegmentId>,
}

impl Segment {
    pub fn priority(&self) -> SegmentPriority {
        self.priority
    }

    pub fn switch_rails(&self) -> &[SwitchRails] {
        self.switch_rails.as_slice()
    }

    pub fn conflicts(&self) -> &[SegmentId] {
        self.conflicts.as_slice()
    }
}

struct Checkpoint {
    checkpoint_ids: BTreeMap<Direction, Vec<CheckpointId>>,
    track_id: TrackId,
    priority: SegmentPriority,
}

impl Checkpoint {
    fn checkpoint_ids(&self, direction: &Direction) -> &Vec<CheckpointId> {
        // Safe to unwrap since checkpoint_ids has been filled with every Direction
        self.checkpoint_ids.get(direction).unwrap()
    }
}

pub struct RailNetwork {
    checkpoints: BTreeMap<CheckpointId, Checkpoint>,
    segments: BTreeMap<SegmentId, Segment>,
    longest_path: usize,
}

impl RailNetwork {
    pub fn new() -> Self {
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

    pub fn segment(&self, segment_id: &SegmentId) -> &Segment {
        // Safe to unwrap since segments has been filled with every SegmentId
        self.segments.get(segment_id).unwrap()
    }

    fn checkpoint(&self, checkpoint_id: &CheckpointId) -> &Checkpoint {
        // Safe to unwrap since checkpoints has been filled with every CheckpointId
        self.checkpoints.get(checkpoint_id).unwrap()
    }

    pub fn next_checkpoint_id_for_track_id_target(
        &self,
        iteration: usize,
        cp_id: CheckpointId,
        direction: Direction,
        target_track_id: TrackId,
    ) -> Option<CheckpointId> {
        let mut next_cp_ids = self.checkpoint(&cp_id).checkpoint_ids(&direction).clone();

        for next_cp_id in next_cp_ids.iter() {
            if self.checkpoint(next_cp_id).track_id == target_track_id {
                return Some(*next_cp_id);
            }
        }

        // Order the checkpoints by priority
        next_cp_ids.sort_by_key(|cid| self.checkpoint(cid).priority);

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

    pub fn next_checkpoint_id_for_checkpoint_id_target(
        &self,
        iteration: usize,
        cp_id: CheckpointId,
        direction: Direction,
        target_cp_id: CheckpointId,
    ) -> Option<CheckpointId> {
        let mut next_cp_ids = self.checkpoint(&cp_id).checkpoint_ids(&direction).clone();

        for next_cp_id in next_cp_ids.iter() {
            if *next_cp_id == target_cp_id {
                return Some(*next_cp_id);
            }
        }

        // Order the checkpoints by priority
        next_cp_ids.sort_by_key(|cid| self.checkpoint(cid).priority);

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

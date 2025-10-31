use std::{collections::BTreeMap, sync::Arc};

use loco_protocol::{ActuatorId, ActuatorType, Direction, LocoId, Speed};
use log::debug;
use thiserror::Error;

use crate::{
    backend::{Backend, Error as BackendError, LocoIntent},
    rail_network::{
        CheckpointId, Error as RailNetworkError, RailNetwork, Segment, SegmentId, SegmentPriority,
    },
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Error controlling loco: {0}")]
    ControlLoco(#[source] BackendError),
    #[error("Error converting Checkpoints into SegmentId")]
    ConvertCheckpointsIntoSegmentId(RailNetworkError),
    #[error("Error driving actuator: {0}")]
    DriveActuator(#[source] BackendError),
    #[error("Error getting loco status: {0}")]
    LocoStatus(#[source] BackendError),
    #[error("Could not find the next checkpoint")]
    NextCheckpointNotFound,
}

type Result<T> = std::result::Result<T, Error>;

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

pub struct Oracle {
    backend: Arc<Backend>,
    rail_network: RailNetwork,
    last_segment_id: BTreeMap<LocoId, SegmentId>,
}

impl Oracle {
    pub fn new(backend: Arc<Backend>) -> Self {
        debug!("Oracle::new()");
        Oracle {
            backend,
            rail_network: RailNetwork::new(),
            last_segment_id: BTreeMap::new(),
        }
    }

    fn active_locos(&self) -> Result<Vec<ActiveLoco>> {
        let mut active_locos = Vec::new();
        for loco_id in self.backend.loco_ids() {
            match self.backend.loco_status(loco_id) {
                Ok(status) => {
                    active_locos.push(ActiveLoco {
                        id: loco_id,
                        speed: status.speed(),
                        location: status.location().map(|l| l.into()),
                        intent: status.intent(),
                    });
                }
                Err(BackendError::LocoNotConnected(_)) => continue,
                Err(e) => return Err(Error::LocoStatus(e)),
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

            // Safe to unwrap location and intent since they have been checked
            // above for not being None.
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

            let active_segment_id: SegmentId = (checkpoint_id, next_checkpoint_id)
                .try_into()
                .map_err(Error::ConvertCheckpointsIntoSegmentId)?;
            active_segments.push(ActiveSegment {
                id: Some(active_segment_id),
                segment: Some(self.rail_network.segment(&active_segment_id).clone()),
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
                segment.priority()
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
                    for conflict_segment_id in segment.conflicts().iter() {
                        if busy_segment_ids.contains(conflict_segment_id) {
                            conflict_found = true;
                            break;
                        }
                    }

                    if !conflict_found {
                        for switch_rails in segment.switch_rails().iter() {
                            actuator_controls.push((
                                switch_rails.actuator_id(),
                                ActuatorType::SwitchRails,
                                switch_rails.state().into(),
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

    pub fn process(&mut self) -> Result<()> {
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
                .drive_actuator(actuator_id, actuator_type, actuator_state)
                .map_err(Error::DriveActuator)?;
        }

        // Apply controls for locos
        for (loco_id, direction, speed) in loco_controls {
            self.backend
                .control_loco(loco_id, direction, speed)
                .map_err(Error::ControlLoco)?;
        }

        Ok(())
    }
}

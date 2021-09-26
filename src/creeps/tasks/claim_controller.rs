use std::fmt;

use log::warn;
use screeps::{HasPosition, Position, RoomName, RoomObjectProperties, SharedCreepProperties, StructureController, game::rooms};

use crate::{
    creeps::races::{OokRace, RepresentsCreep},
    state::BWState,
};
use anyhow::{Result, anyhow, bail};

use super::{OokTaskRunnable, OokTaskRunnableResult};


#[derive(Clone, Debug)]
pub enum ControllerPosToClaim {
    InRoom {
        room_name: RoomName,
    },
    KnowingPos {
        pos: Position,
    },
}

#[derive(Clone)]
pub enum Step {
    Move {
        pos: ControllerPosToClaim,
    },
    Claim {
        controller: StructureController,
    },
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Move { pos } => {
                f.debug_struct("Step::Move")
                    .field("pos", pos)
                    .finish()
            },
            Step::Claim { controller } => {
                f.debug_struct("Step::Claim")
                    .field("controller in room", &controller.room().map(|r| r.name()))
                    .finish()
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    step: Step,
}

impl Task {
    pub fn new(state: &BWState, citizen: &OokRace, target_controller_at: RoomName) -> Result<Self> {
        let room = rooms::get(target_controller_at);
        let controller_pos = if let Some(Some(controller)) = room.map(|r| r.controller()) {
            ControllerPosToClaim::KnowingPos{ pos: controller.pos() }
        } else {
            ControllerPosToClaim::InRoom{ room_name: target_controller_at }
        };
        let mut task = Task {
            step: Step::Move {
                pos: controller_pos
            },
        };
        task.precheck(state, citizen)?;
        Ok(task)
    }

    fn precheck(&mut self, _state: &BWState, citizen: &OokRace) -> Result<()> {
        let creep = citizen.creep()?;
        match &mut self.step {
            Step::Move { pos } => {
                match pos {
                    ControllerPosToClaim::InRoom { room_name } => {
                        let room = creep.room().ok_or(anyhow!("Could not get room from creep"))?;
                        if room.name() == *room_name {
                            if let Some(controller) = room.controller() {
                                *pos = ControllerPosToClaim::KnowingPos {
                                    pos: controller.pos(),
                                };
                            } else {
                                bail!("Room {} has no controller, cannot claim!", room_name);
                            }
                        }
                    },
                    ControllerPosToClaim::KnowingPos { pos } => {
                        let room = creep.room().ok_or(anyhow!("Could not get room from creep"))?;
                        if room.name() == pos.room_name() && pos.is_near_to(&creep.pos()) {
                            if let Some(controller) = room.controller() {
                                self.step = Step::Claim {
                                    controller,
                                };
                            } else {
                                bail!("Room {} has no controller, cannot claim!", room.name());
                            }
                        }
                    },
                }
            }
            Step::Claim { controller } => {
                if !controller.pos().in_range_to(&creep.pos(), 3) {
                    self.step = Step::Move {
                        pos: ControllerPosToClaim::KnowingPos {
                            pos: controller.pos(),
                        }
                    };
                }
            }
        };
        Ok(())
    }
}

impl OokTaskRunnable for Task {
    fn run(&mut self, state: &mut BWState, race: &OokRace) -> Result<OokTaskRunnableResult> {
        self.precheck(state, &race)?;
        let creep = race.creep()?;
        match &self.step {
            Step::Move { pos } => {
                match pos {
                    ControllerPosToClaim::InRoom { room_name } => {
                        creep.move_to(&Position::new(25, 25, *room_name));
                    },
                    ControllerPosToClaim::KnowingPos { pos } => {
                        creep.move_to(pos);
                    },
                };
            },
            Step::Claim { controller } => {
                let return_code = creep.claim_controller(&controller);
                match return_code {
                    screeps::ReturnCode::Ok => {},
                    _ => {
                        warn!("Could not claim controller, return code {:?}", return_code);
                    },
                }
            },
        }
        Ok(OokTaskRunnableResult::Continue)
    }
}

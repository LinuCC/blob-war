use std::{collections::HashMap, fmt};

use log::warn;
use screeps::{
    game::rooms, HasPosition, HasStore, Position, ResourceType, RoomObjectProperties,
    SharedCreepProperties,
};

use crate::{
    creeps::{
        generic_creep_fetch_from_provider_prio,
        races::{generic_calc_energy_resource_provider, OokRace, RepresentsCreep},
    },
    rooms::{
        resource_provider::{ResourceData, ResourceProvider, TakeResourceResult},
        room_state::{RoomState, SetupBaseStateVisibility},
    },
    state::BWState,
};
use anyhow::{anyhow, Context, Result};

use super::{
    CalcResourceProviderResult, FetchesFromResourceProvider, OokTaskRunnable, OokTaskRunnableResult,
};

#[derive(Clone)]
pub enum Step {
    GetEnergy {
        target: ResourceProvider,
        controller_pos: Position,
    },
    Upgrade {
        controller_pos: Position,
    },
    WaitForResource {
        controller_pos: Position,
    },
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::GetEnergy {
                target,
                controller_pos,
            } => f
                .debug_struct("Step::GetEnergy")
                .field("target", target)
                .field("controller_pos", &controller_pos)
                .finish(),
            Step::Upgrade { controller_pos } => f
                .debug_struct("Step::Upgrade")
                .field("controllers", &controller_pos)
                .finish(),
            Step::WaitForResource { controller_pos } => f
                .debug_struct("Step::WaitForResource")
                .field("controller_pos", &controller_pos)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    step: Step,
}

impl Task {
    pub fn new(target_controller_at: Position, state: &BWState, race: &OokRace) -> Result<Self> {
        let mut task = Task {
            step: Step::WaitForResource {
                controller_pos: target_controller_at,
            },
        };
        task.precheck(state, race)?;
        Ok(task)
    }

    fn precheck(&mut self, state: &BWState, race: &OokRace) -> Result<()> {
        let creep = race.creep()?;
        match &self.step {
            Step::GetEnergy { controller_pos, .. } => {
                if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
                    creep.say("⏫", false);
                    self.step = Step::Upgrade {
                        controller_pos: *controller_pos,
                    };
                }
            }
            Step::Upgrade { controller_pos } => {
                if creep.store_used_capacity(Some(ResourceType::Energy)) == 0 {
                    let calc_result = self
                        .calc_resource_provider(&state.room_states, race)
                        .map_err(|err| {
                            anyhow!("UpgradeController precheck calc_resource_provider, {}", err)
                        })?;
                    match calc_result {
                        Some(calc_result) => {
                            creep.say("📦", false);
                            self.step = Step::GetEnergy {
                                controller_pos: *controller_pos,
                                target: calc_result.resource_provider,
                            };
                        }
                        None => {
                            self.step = Step::WaitForResource {
                                controller_pos: *controller_pos,
                            };
                        }
                    }
                }
            }
            Step::WaitForResource { controller_pos } => {
                let calc_result = self
                    .calc_resource_provider(&state.room_states, race)
                    .map_err(|err| {
                        anyhow!("UpgradeController precheck calc_resource_provider, {}", err)
                    })?;
                match calc_result {
                    Some(calc_result) => {
                        creep.say("📦", false);
                        self.step = Step::GetEnergy {
                            controller_pos: *controller_pos,
                            target: calc_result.resource_provider,
                        };
                    }
                    None => {}
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
            Step::GetEnergy {
                target,
                controller_pos,
            } => {
                let target_pos = target.pos()?;
                if creep.pos().is_near_to(&target_pos) {
                    match target.creep_get_resource(
                        &creep,
                        ResourceType::Energy,
                        creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                    )? {
                        TakeResourceResult::Withdraw { .. } => {
                            creep.say("⏫", false);
                            self.step = Step::Upgrade {
                                controller_pos: controller_pos.to_owned(),
                            };
                        }
                        TakeResourceResult::Harvest { .. } => {
                            // Continue harvest until we are full
                        }
                        TakeResourceResult::Pickup { .. } => {
                            creep.say("⏫", false);
                            self.step = Step::Upgrade {
                                controller_pos: controller_pos.to_owned(),
                            };
                        }
                    }
                } else {
                    creep.move_to(&target_pos);
                }
            }
            Step::Upgrade { controller_pos } => {
                if creep.pos().in_range_to(controller_pos, 3) {
                    let room = rooms::get(controller_pos.room_name())
                        .ok_or_else(|| anyhow!("uc: room not found"))?;
                    let controller = room
                        .controller()
                        .ok_or_else(|| anyhow!("uc: controller not found"))?;
                    creep.upgrade_controller(&controller);
                } else {
                    creep.move_to(controller_pos);
                }
            }
            Step::WaitForResource { .. } => {
                creep.say("⏱ ", false);
            }
        }
        // TODO
        Ok(OokTaskRunnableResult::Continue)
    }
}

impl<'a> FetchesFromResourceProvider<'a> for Task {
    fn calc_resource_provider(
        &self,
        rooms_state: &'a HashMap<screeps::RoomName, RoomState>,
        race: &'a OokRace,
    ) -> Result<Option<CalcResourceProviderResult>> {
        let creep = race.creep()?;
        let room = creep.room().ok_or(anyhow!("room of creep not found"))?;
        let room_state = rooms_state
            .get(&room.name())
            .ok_or_else(|| anyhow!("Room not found"))?;
        let amount = creep.store_free_capacity(Some(ResourceType::Energy));
        match room_state {
            RoomState::Base(room_state) => generic_calc_energy_resource_provider(
                &room_state.resource_providers,
                &creep,
                &room,
                amount as u32,
            ),
            RoomState::SetupBase(room_state) => {
                if let SetupBaseStateVisibility::Visible {
                    ref resource_providers,
                    ..
                } = room_state.state
                {
                    generic_calc_energy_resource_provider(
                        &resource_providers,
                        &creep,
                        &room,
                        amount as u32,
                    )
                } else {
                    Ok(None)
                }
            }
        }
    }
}

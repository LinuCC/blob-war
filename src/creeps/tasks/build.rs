use std::{collections::HashMap, fmt, mem};

use log::warn;
use screeps::{
    game::{get_object_typed, rooms},
    ConstructionSite, HasId, HasPosition, HasStore, ResourceType, RoomObjectProperties,
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
    state::{build_target::BuildTarget, BWState},
};
use anyhow::{anyhow, Context, Result};

use super::{
    CalcResourceProviderResult, FetchesFromResourceProvider, OokTaskRunnable, OokTaskRunnableResult,
};

#[derive(Clone)]
pub enum Step {
    GetEnergy {
        target: ResourceProvider,
        build_target: BuildTarget,
        took_energy_for: u8,
    },
    Build {
        build_target: BuildTarget,
    },
    WaitForResource {
        build_target: BuildTarget,
    },
}

impl<'a> Step {
    fn build_target(&'a self) -> &'a BuildTarget {
        match self {
            Step::GetEnergy { build_target, .. } => build_target,
            Step::Build { build_target } => build_target,
            Step::WaitForResource { build_target } => build_target,
        }
    }
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::GetEnergy {
                target,
                build_target,
                took_energy_for,
            } => f
                .debug_struct("Step::GetEnergy")
                .field("target", target)
                .field("build_target", &build_target)
                .field("took_energy_for", &took_energy_for)
                .finish(),
            Step::Build { build_target } => f
                .debug_struct("Step::Build")
                .field("build_target", &build_target)
                .finish(),
            Step::WaitForResource { build_target } => f
                .debug_struct("Step::WaitForResource")
                .field("build_target", &build_target)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    step: Step,
}

impl Task {
    pub fn new(target: ConstructionSite, state: &BWState, race: &OokRace) -> Result<Self> {
        let mut task = Task {
            step: Step::WaitForResource {
                build_target: target.into(),
            },
        };
        task.precheck(state, race)?;
        Ok(task)
    }

    fn precheck(&mut self, state: &BWState, race: &OokRace) -> Result<()> {
        let creep = race.creep()?;
        match &self.step {
            Step::GetEnergy { build_target, .. } => {
                if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
                    creep.say("🏗", false);
                    self.step = Step::Build {
                        build_target: build_target.to_owned(),
                    };
                }
            }
            Step::Build { build_target } => {
                if creep.store_used_capacity(Some(ResourceType::Energy)) == 0 {
                    let calc_result = self
                        .calc_resource_provider(&state.room_states, race)
                        /*.context("Build precheck calc_resource_provider")*/?;
                    match calc_result {
                        Some(calc_result) => {
                            creep.say("📦", false);
                            self.step = Step::GetEnergy {
                                build_target: build_target.to_owned(),
                                target: calc_result.resource_provider,
                                took_energy_for: 0,
                            };
                        }
                        None => {
                            self.step = Step::WaitForResource {
                                build_target: build_target.to_owned(),
                            };
                        }
                    }
                }
            }
            Step::WaitForResource { build_target } => {
                let calc_result = self
                    .calc_resource_provider(&state.room_states, race)
                    /*.context("Build precheck calc_resource_provider")*/?;
                match calc_result {
                    Some(calc_result) => {
                        creep.say("📦", false);
                        self.step = Step::GetEnergy {
                            build_target: build_target.to_owned(),
                            target: calc_result.resource_provider,
                            took_energy_for: 0,
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
        Ok(match &mut self.step {
            Step::GetEnergy {
                target,
                build_target,
                took_energy_for,
            } => {
                let target_pos = target.pos()?;
                if creep.pos().is_near_to(&target_pos) {
                    *took_energy_for += 1;
                    match target.creep_get_resource(
                        &creep,
                        ResourceType::Energy,
                        creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                    )? {
                        TakeResourceResult::Withdraw { .. } => {
                            creep.say("⏫", false);
                            self.step = Step::Build {
                                build_target: build_target.to_owned(),
                            };
                        }
                        TakeResourceResult::Harvest { return_code, .. } => {
                            // Continue harvest until we are full
                            match return_code {
                                screeps::ReturnCode::Ok => {}
                                screeps::ReturnCode::NotEnough => {
                                    creep.say("⏫", false);
                                    self.step = Step::Build {
                                        build_target: build_target.to_owned(),
                                    };
                                }
                                _ => {
                                    warn!("Harvest unknown result_code {:?}", return_code);
                                }
                            }
                        }
                        TakeResourceResult::Pickup { .. } => {
                            creep.say("⏫", false);
                            self.step = Step::Build {
                                build_target: build_target.to_owned(),
                            };
                        }
                    }
                } else {
                    creep.move_to(&target_pos);
                }
                OokTaskRunnableResult::Continue
            }
            Step::Build { build_target } => {
                if let Some(construction_site) = get_object_typed(build_target.id)? {
                    if creep.pos().in_range_to(&build_target.pos, 3) {
                        creep.build(&construction_site);
                    } else {
                        creep.move_to(&construction_site);
                    }
                    OokTaskRunnableResult::Continue
                } else {
                    let creep_room = creep
                        .room()
                        .ok_or_else(|| anyhow!("Room not found for creep {:?}", creep.id()))?;
                    if creep_room.name() != build_target.pos.room_name() {
                        // We just don't see the room
                        creep.move_to(&build_target.pos);
                        OokTaskRunnableResult::Continue
                    } else {
                        // Construction site gone
                        OokTaskRunnableResult::CancelAndDoAnother
                    }
                }
            }
            Step::WaitForResource { .. } => {
                creep.say("⏱ ", false);
                OokTaskRunnableResult::Continue
            }
        })
    }
}

impl<'a> FetchesFromResourceProvider<'a> for Task {
    fn calc_resource_provider(
        &self,
        rooms_state: &'a HashMap<screeps::RoomName, RoomState>,
        race: &'a OokRace,
    ) -> Result<Option<CalcResourceProviderResult>> {
        // Mining in the same room as the build target should make sense most of the time
        let target_room_name = self.step.build_target().pos.room_name();
        let creep = race.creep()?;
        let room = rooms::get(target_room_name)
            .ok_or_else(|| anyhow!("Room not found {}", target_room_name))?;
        let room_state = rooms_state
            .get(&target_room_name)
            .ok_or_else(|| anyhow!("Room state not found"))?;
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
                        resource_providers,
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

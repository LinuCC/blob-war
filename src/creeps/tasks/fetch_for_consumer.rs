use std::{cmp, collections::HashMap, fmt};

use log::{info, warn};
use screeps::{
    game::{get_object_typed, rooms},
    look, HasPosition, HasStore, Position, ResourceType, RoomObjectProperties,
    SharedCreepProperties, Structure,
};

use crate::{creeps::{CreepRunnerDeliverTarget, CreepRunnerFetchTarget, CreepRunnerState, generic_creep_fetch_from_provider_prio, races::{generic_calc_energy_resource_provider, OokRace, RepresentsCreep}}, rooms::{
        resource_provider::{ResourceData, ResourceProvider, TakeResourceResult},
        room_state::{RoomState, SetupBaseStateVisibility},
    }, state::BWState, utils::AnyhowOptionExt};
use anyhow::{anyhow, Context, Result};

use super::{
    CalcResourceProviderResult, FetchesFromResourceProvider, OokTaskRunnable, OokTaskRunnableResult,
};

#[derive(Clone, Debug)]
pub struct DeliverResourceTargets {
    targets: Vec<()>,
}

#[derive(Clone)]
pub enum Step {
    GetResource {
        source_target: ResourceProvider,
        deliver_target: (),
    },
    DeliverResource(DeliverResourceTargets),
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::GetResource {
                source_target,
                deliver_target,
            } => f
                .debug_struct("Step::GetResource")
                .field("source_target", &source_target)
                .field("deliver_target", &deliver_target)
                .finish(),
            Step::DeliverResource(targets) => f
                .debug_struct("Step::DeliverResource")
                .field("0", targets)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    // step: Step,
    // requested_amount: u32,
    pub state: CreepRunnerState,
}

impl Task {
    /// TODO Rewrite as RunnerState is legacy stuff
    pub fn new(runner_state: CreepRunnerState, state: &BWState, race: &OokRace) -> Result<Self> {
        let mut task = Task {
            state: runner_state,
        };
        task.precheck(state, race)?;
        Ok(task)
    }

    fn precheck(&mut self, state: &BWState, race: &OokRace) -> Result<bool> {
        let creep = race.creep()?;
        let state = &self.state;
        match state {
            CreepRunnerState::Fetching { to, .. } => {
                if creep.store_free_capacity(Some(ResourceType::Energy)) == 0
                    || creep.store_used_capacity(Some(ResourceType::Energy)) >= to.requested()
                {
                    self.state = CreepRunnerState::Delivering {
                        to: to.clone(),
                        provided: 0,
                    };
                }
            }
            CreepRunnerState::Delivering { to, provided } => {
                if creep.store_used_capacity(Some(ResourceType::Energy)) == 0
                    || *provided >= to.requested()
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

impl OokTaskRunnable for Task {
    fn run(&mut self, state: &mut BWState, race: &OokRace) -> Result<OokTaskRunnableResult> {
        if !self.precheck(state, &race)? {
            return Ok(OokTaskRunnableResult::Finish);
        }
        let creep = race.creep()?;
        let room = creep
            .room()
            .ok_or_else(|| anyhow!("task fetchfc Room not found"))?;
        let state = &mut self.state;

        match state {
            CreepRunnerState::Fetching { from, .. } => {
                if creep.pos().is_near_to(&from.pos()) {
                    match from {
                        CreepRunnerFetchTarget::PermanentFarmerContainer { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("fetchfc farmer container not found"))?;
                            let amount = cmp::min(
                                // to.requested(),
                                creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                                obj.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep.withdraw_amount(&obj, ResourceType::Energy, amount);
                            Ok(OokTaskRunnableResult::Continue)
                        }
                        CreepRunnerFetchTarget::Ruin { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("fetchfc ruin not found"))?;
                            let amount = cmp::min(
                                creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                                // HACK stupid if I fill one extension requesting 50 energy
                                // cmp::min(
                                //     to.requested(),
                                obj.store_used_capacity(Some(ResourceType::Energy)),
                                // ),
                            );
                            creep.withdraw_amount(&obj, ResourceType::Energy, amount);
                            Ok(OokTaskRunnableResult::Continue)
                        }
                        CreepRunnerFetchTarget::DroppedSource { id, pos, .. } => {
                            let obj = get_object_typed(*id)?;
                            let farmer_container = room.look_for_at(look::STRUCTURES, pos);

                            if let Some(obj) = obj {
                                if obj.amount() < 200 && farmer_container.len() > 0 {
                                    creep.pickup(&obj);
                                    // HACK Remove me breaks taking energy
                                    // We might not have picked up enough, and there might be a
                                    // container from a farmer underneath with more
                                    if let Some(Structure::Container(container)) =
                                        farmer_container.first()
                                    {
                                        let container_amount = cmp::min(
                                            // to.requested(),
                                            // HACK Based on the run, it should take all or ony
                                            // some energy
                                            creep.store_free_capacity(Some(ResourceType::Energy)),
                                            container
                                                .store_used_capacity(Some(ResourceType::Energy))
                                                as i32,
                                        ) - obj.amount() as i32;
                                        info!(
                                            "Grabbing from Container: {} // Amount: {}",
                                            farmer_container.len(),
                                            container_amount
                                        );
                                        if container_amount > 0 {
                                            creep.withdraw_amount(
                                                container,
                                                ResourceType::Energy,
                                                container_amount as u32,
                                            );
                                        }
                                    }
                                    Ok(OokTaskRunnableResult::Continue)
                                } else {
                                    // NOTE Can't control how much I pick up with `pickup` ಠ_ಠ
                                    creep.pickup(&obj);
                                    Ok(OokTaskRunnableResult::Continue)
                                }
                            } else {
                                if farmer_container.len() > 0 {
                                    // HACK
                                    // If no dropped source is there, perhaps the container
                                    // still has resource
                                    if let Some(Structure::Container(container)) =
                                        farmer_container.first()
                                    {
                                        let amount = cmp::min(
                                            // to.requested(),
                                            // HACK Based on the run, it should take all or ony
                                            // some energy
                                            creep.store_free_capacity(Some(ResourceType::Energy))
                                                as u32,
                                            container
                                                .store_used_capacity(Some(ResourceType::Energy)),
                                        );
                                        match creep.withdraw_amount(
                                            container,
                                            ResourceType::Energy,
                                            amount,
                                        ) {
                                            screeps::ReturnCode::Ok => {
                                                Ok(OokTaskRunnableResult::Continue)
                                            }
                                            // Is a hack anyway
                                            _ => Ok(OokTaskRunnableResult::Continue),
                                        }
                                    } else {
                                        Ok(OokTaskRunnableResult::Continue)
                                    }
                                } else {
                                    Ok(OokTaskRunnableResult::CancelAndDoAnother)
                                }
                            }
                        }
                        CreepRunnerFetchTarget::Terminal { id, .. } => {
                            let obj = get_object_typed(*id)?.anyhow("Terminal not found")?;
                            let amount = cmp::min(
                                // to.requested(),
                                creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                                obj.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .withdraw_amount(&obj, ResourceType::Energy, amount);
                            Ok(OokTaskRunnableResult::Continue)
                        }
                    }
                } else {
                    creep.move_to(&from.pos());
                    Ok(OokTaskRunnableResult::Continue)
                }
            }
            CreepRunnerState::Delivering { to, provided } => {
                if creep.pos().is_near_to(&to.pos()) {
                    match to {
                        CreepRunnerDeliverTarget::Tower { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("failed getting deliver target"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                        CreepRunnerDeliverTarget::Extension { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("failed getting deliver target"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                        CreepRunnerDeliverTarget::Spawn { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("failed getting deliver target"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                        CreepRunnerDeliverTarget::PermanentUpgraderContainer { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("failed getting deliver target"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                        CreepRunnerDeliverTarget::TempStorage { id, .. } => {
                            let obj = get_object_typed(*id)?
                                .ok_or_else(|| anyhow!("failed getting deliver target"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                        CreepRunnerDeliverTarget::TradeTransactionFee { id, .. } => {
                            let obj = get_object_typed(*id)?.ok_or_else(|| anyhow!("failed getting trade transaciton fee"))?;
                            let amount = cmp::min(
                                to.requested(),
                                creep.store_used_capacity(Some(ResourceType::Energy)),
                            );
                            creep
                                .transfer_amount(&obj, ResourceType::Energy, amount);
                            *provided += amount;
                            Ok(OokTaskRunnableResult::Finish)
                        }
                    }
                } else {
                    creep.move_to(&to.pos());
                    Ok(OokTaskRunnableResult::Continue)
                }
            }
        }
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

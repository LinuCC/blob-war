use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

use log::{info, warn};
use screeps::{
    find,
    game::{get_object_typed, rooms},
    memory::MemoryReference,
    Creep, HasId, HasPosition, HasStore, ObjectId, Position, ResourceType, Room, RoomName,
    RoomObjectProperties, SharedCreepProperties, SpawnOptions,
};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_JOB, MEM_POST, MEM_RACE_KIND, MEM_REQUEST_ID, MEM_ROOM_BASE},
    creeps::{
        get_prio_deliver_target, get_prio_fetch_target,
        jobs::{self, OokCreepJob, StorableJob},
        races::OokRace,
        tasks::{self, OokCreepTask, OokTaskRunnable},
        utils::create_creep_name,
        CalcSpawnBodyResult, CreepRunnerState, Spawnable, TrySpawnOptions, TrySpawnResult,
        TrySpawnResultData,
    },
    rooms::room_state::{
        base::{BaseData, BaseState},
        RoomState,
    },
    state::{BWState, UniqId},
};

use super::{
    DoJobResult, DynamicTasked, Memorizing, OokRaceBodyComposition, OokRaceKind, RepresentsCreep,
    RoomBound,
};
use anyhow::{anyhow, bail, Context, Result};

const COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 1,
    carry: 2,
    work: 0,
    attack: 0,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 0,
};

#[derive(Debug, Clone)]
struct OokCreepCarrierMemory {
    race_kind: OokRaceKind,
    job: OokCreepJob,
    post_ident: String,
    base_room: RoomName,
    request_id: Option<UniqId>,
}

impl OokCreepCarrierMemory {
    fn new(
        job: OokCreepJob,
        post_ident: String,
        base_room: RoomName,
        request_id: Option<UniqId>,
    ) -> Self {
        Self {
            race_kind: OokRaceKind::Carrier,
            job,
            post_ident,
            base_room,
            request_id,
        }
    }
}

impl From<OokCreepCarrierMemory> for MemoryReference {
    fn from(mem: OokCreepCarrierMemory) -> Self {
        let memory = MemoryReference::new();
        memory.set(MEM_RACE_KIND, mem.race_kind as i32);
        memory.set(
            MEM_JOB,
            mem.job
                .to_js_serialize()
                .iter()
                .map(|(i, v)| (i.clone(), &**v))
                .collect::<HashMap<String, &dyn JsSerialize>>(),
        );
        memory.set(MEM_POST, mem.post_ident);
        memory.set(MEM_ROOM_BASE, mem.base_room.to_string());
        if let Some(id) = mem.request_id {
            memory.set(MEM_REQUEST_ID, id.to_string());
        }
        memory
    }
}

#[derive(Debug, Clone)]
pub struct OokCreepCarrier {
    pub creep_id: ObjectId<Creep>,
    pub job: OokCreepJob,

    pub task: Option<OokCreepTask>,
    // cached_creep: (u64, Creep),
}

impl OokCreepCarrier {
    #[deprecated]
    fn new_run(&mut self, room: &Room) -> Result<(), Box<dyn std::error::Error>> {
        let deliver_target = get_prio_deliver_target(&room, &self.creep()?)?;
        info!("del target {:?} in {}", deliver_target, room.name());
        if let Some(deliver_target) = deliver_target {
            if deliver_target.requested()
                <= self
                    .creep()?
                    .store_used_capacity(Some(ResourceType::Energy))
            {
                self.task = Some(OokCreepTask::FetchForConsumer(
                    tasks::fetch_for_consumer::Task {
                        state: CreepRunnerState::Delivering {
                            to: deliver_target,
                            provided: 0,
                        },
                    },
                ));
            } else {
                let fetch_target =
                    get_prio_fetch_target(&room, &deliver_target, &self.creep()?.pos())?;
                if let Some(fetch_target) = fetch_target {
                    self.task = Some(OokCreepTask::FetchForConsumer(
                        tasks::fetch_for_consumer::Task {
                            state: CreepRunnerState::Fetching {
                                from: fetch_target,
                                to: deliver_target,
                            },
                        },
                    ));
                } else {
                    info!(
                        "Delivery requested, but no provider in room {}",
                        room.name()
                    );
                }
            }
            Ok(())
        } else {
            self.creep()?.say("...", false);
            Ok(())
        }
    }

    fn assign_task_for_room_logistics(&mut self, state: &mut BWState) -> Result<()> {
        let room = rooms::get(self.job.target_room())
            .ok_or_else(|| anyhow!("carrier None job RoomLogistics room not found"))?;
        let room_state = state.room_states.get(&room.name());
        match room_state {
            Some(RoomState::Base(base_state)) => {
                info!("da length {}", base_state.get_open_suppliers_reach_points(state)?.len());
                if base_state.get_open_suppliers_reach_points(state)?.len() > 0 {
                    self.task =
                        Some(OokCreepTask::SpawnSuppliesRun(tasks::spawn_supplies_run::Task::new(
                            room.name(),
                            state,
                            &OokRace::Carrier(self.clone()),
                        )?));
                } else {
                    self.new_run(&room)
                        .map_err(|err| anyhow!("new_ron fauled: {}", err))?;
                }
            }
            Some(RoomState::SetupBase(_)) => {
                self.new_run(&room)
                    .map_err(|err| anyhow!("new_ron fauled: {}", err))?;
            }
            None => {
                bail!("RoomState not found for {}", room.name());
            }
        }
        Ok(())
    }
}

impl RepresentsCreep for OokCreepCarrier {
    fn creep(&self) -> Result<Creep> {
        get_object_typed(self.creep_id)
            .context("Carrier creep")?
            .ok_or(anyhow!("Carrier creep not found {}", self.creep_id))
    }
}

impl TryFrom<&screeps::Creep> for OokCreepCarrier {
    type Error = anyhow::Error;

    fn try_from(creep: &Creep) -> Result<Self, Self::Error> {
        let memory = creep.memory();
        let job_dict = memory
            .dict(MEM_JOB)
            .context("loading mem job")?
            .ok_or(anyhow!("mem job missing"))?;
        let carrier_memory = OokCreepCarrierMemory {
            race_kind: memory
                .i32(MEM_RACE_KIND)
                .context("loading mem race_kind")?
                .ok_or(anyhow!("mem race_kind missing"))?
                .try_into()?,
            job: OokCreepJob::from_js_serialize(&job_dict)
                .context("loading mem job data")?
                .ok_or(anyhow!("mem job data"))?
                .try_into()?,
            post_ident: memory
                .string(MEM_POST)
                .context("loading mem post")?
                .ok_or(anyhow!("mem post missing"))?,
            base_room: RoomName::new(
                &memory
                    .string(MEM_ROOM_BASE)
                    .context("loading mem room_base")?
                    .ok_or(anyhow!("mem room_base missing"))?,
            )
            .context("loading mem room_base")?,
            request_id: memory
                .string(MEM_REQUEST_ID)
                .context("loading mem request_id")?
                .map(|s| UniqId::from(s)),
        };

        Ok(Self {
            creep_id: creep.id(),
            job: carrier_memory.job,
            task: None,
        })
    }
}

impl Memorizing<OokCreepCarrierMemory> for OokCreepCarrier {
    // fn get_memory(&self) -> Result<OokCreepCarrierMemory> {
    //     let creep = get_object_typed(self.creep_id)?
    //         .ok_or(anyhow!("Memo: Creep {} not found", self.creep_id))?;
    //     let memory = creep.memory();
    //     Ok(OokCreepCarrierMemory {
    //         race_kind: memory
    //             .i32(MEM_RACE_KIND)
    //             .context("loading mem race_kind")?
    //             .ok_or(anyhow!("mem race_kind missing"))?
    //             .try_into()?,
    //         task_kind: memory
    //             .i32(MEM_TASK_KIND)
    //             .context("loading mem task_kind")?
    //             .ok_or(anyhow!("mem task_kind missing"))?
    //             .try_into()?,
    //     })
    // }
    fn creep_mem_race_ident() -> OokRaceKind {
        OokRaceKind::Carrier
    }

    fn set_memory(&self, mem: OokCreepCarrierMemory) -> Result<()> {
        let creep = get_object_typed(self.creep_id)?
            .ok_or(anyhow!("Memo: Creep {} not found", self.creep_id))?;
        let memory = creep.memory();
        memory.set(MEM_RACE_KIND, mem.race_kind as i32);
        memory.set(
            MEM_JOB,
            mem.job
                .to_js_serialize()
                .iter()
                .map(|(i, v)| (i.clone(), &**v))
                .collect::<HashMap<String, &dyn JsSerialize>>(),
        );
        memory.set(MEM_POST, mem.post_ident);
        memory.set(MEM_ROOM_BASE, mem.base_room.to_string());
        if let Some(id) = mem.request_id {
            memory.set(MEM_REQUEST_ID, id.to_string());
        } else {
            memory.del(MEM_REQUEST_ID);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TrySpawnCarrierOptions {
    pub base_room: RoomName,
    pub post_ident: String,
}

impl Spawnable<TrySpawnCarrierOptions> for OokCreepCarrier {
    fn try_spawn(
        opts: &TrySpawnOptions,
        race_opts: &TrySpawnCarrierOptions,
    ) -> Result<TrySpawnResult> {
        let avail_energy = opts.spawn_room.energy_available();
        let calc_result = if let Some(preset_parts) = &opts.preset_parts {
            CalcSpawnBodyResult {
                amount: preset_parts.iter().fold(0, |acc, &p| acc + p.cost()),
                body: preset_parts.to_owned(),
            }
        } else {
            Self::calc_spawn_body(opts, race_opts)?
        };
        if calc_result.amount <= avail_energy {
            let spawn_id = opts
                .available_spawns
                .first()
                .ok_or(anyhow!("try_spawn called without available_spawns"))?;
            let spawn = get_object_typed(*spawn_id)
                .context("try_spawn")?
                .ok_or(anyhow!("Could not find spawn {} for try_spawn", spawn_id))?;
            let new_memory = OokCreepCarrierMemory::new(
                opts.assumed_job.to_owned(),
                race_opts.post_ident.to_owned(),
                opts.spawn_room.name(),
                opts.request_id.to_owned(),
            );
            let creep_name = create_creep_name(&opts.race);
            let return_code = spawn.spawn_creep_with_options(
                &calc_result.body,
                &creep_name,
                &SpawnOptions::default().memory(Some(new_memory.into())),
            );
            Ok(TrySpawnResult::Spawned(TrySpawnResultData {
                return_code,
                used_energy_amount: calc_result.amount,
                used_spawn: spawn.id(),
                creep_name,
            }))
        } else {
            if opts.force_spawn {
                let calc_result = Self::calc_spawn_body(
                    &TrySpawnOptions {
                        target_energy_usage: avail_energy,
                        ..opts.to_owned()
                    },
                    race_opts,
                )
                .context("force spawn calc_spawn_body")?;
                let spawn_id = opts
                    .available_spawns
                    .first()
                    .ok_or(anyhow!("try_spawn called without available_spawns"))?;
                let spawn = get_object_typed(*spawn_id)
                    .context("try_spawn")?
                    .ok_or(anyhow!("Could not find spawn {} for try_spawn", spawn_id))?;
                let new_memory = OokCreepCarrierMemory::new(
                    opts.assumed_job.to_owned(),
                    race_opts.post_ident.to_owned(),
                    opts.spawn_room.name(),
                    opts.request_id.to_owned(),
                );
                let creep_name = create_creep_name(&opts.race);
                let return_code = spawn.spawn_creep_with_options(
                    &calc_result.body,
                    &create_creep_name(&opts.race),
                    &SpawnOptions::default().memory(Some(new_memory.into())),
                );
                Ok(TrySpawnResult::ForceSpawned(TrySpawnResultData {
                    return_code,
                    used_energy_amount: calc_result.amount,
                    used_spawn: spawn.id(),
                    creep_name,
                }))
            } else {
                Ok(TrySpawnResult::Skipped)
            }
        }
    }

    fn calc_spawn_body(
        opts: &crate::creeps::TrySpawnOptions,
        race_opts: &TrySpawnCarrierOptions,
    ) -> anyhow::Result<CalcSpawnBodyResult> {
        if let Some((body, amount)) = COMPOSITION.parts_for_x_energy(opts.target_energy_usage) {
            Ok(CalcSpawnBodyResult { amount, body })
        } else {
            bail!(
                "Could not calc_spawn_body for {:?} // {:?}",
                opts,
                race_opts
            );
        }
    }
}

impl DynamicTasked for OokCreepCarrier {
    fn task(&self) -> Option<&OokCreepTask> {
        self.task.as_ref()
    }

    fn job(&self) -> OokCreepJob {
        self.job.to_owned()
    }

    fn do_job(&mut self, state: &mut BWState) -> Result<DoJobResult> {
        let cloned_self = self.clone();
        match &mut self.task {
            Some(task) => {
                let run_result = match task {
                    OokCreepTask::MaintainResource => bail!("carrier task not handled"),
                    OokCreepTask::MaintainStructures => bail!("carrier task not handled"),
                    OokCreepTask::ClaimController(_) => bail!("carrier task not handled"),
                    OokCreepTask::UpgradeController(_) => bail!("carrier task not handled"),
                    OokCreepTask::FarmSource(_) => bail!("carrier task not handled"),
                    OokCreepTask::Build(_) => bail!("carrier task not handled"),
                    OokCreepTask::FetchForConsumer(task) => {
                        task.run(state, &OokRace::Carrier(cloned_self))?
                    }
                    OokCreepTask::SpawnSuppliesRun(task) => {
                        task.run(state, &OokRace::Carrier(cloned_self))?
                    }
                };
                match run_result {
                    tasks::OokTaskRunnableResult::Continue => {}
                    tasks::OokTaskRunnableResult::Finish => {
                        self.task = None;
                    }
                    tasks::OokTaskRunnableResult::CancelAndDoAnother => {
                        info!("Cancelling and doing another task");
                        self.task = None;
                        return self.do_job(state);
                    }
                }
            }
            None => match &self.job {
                OokCreepJob::RoomLogistics { .. } => {
                    self.assign_task_for_room_logistics(state);
                }
                job => {
                    if let Ok(creep) = self.creep() {
                        creep.say("wut job??", false);
                    }
                    bail!("OokCreepCarrier::do_task unknown job {:?}", job);
                }
            },
        }
        Ok(DoJobResult::None)
    }
}

impl RoomBound<String> for OokCreepCarrier {
    fn room_name_of_base(&self) -> Result<RoomName> {
        Ok(self
            .creep()?
            .memory()
            .string(MEM_ROOM_BASE)?
            .ok_or_else(|| anyhow!("OokRaceKind: Unknown post ident"))
            .map(|str| RoomName::new(&str))??)
    }

    fn post_ident(&self) -> Result<String> {
        self.creep()?
            .memory()
            .string(MEM_POST)?
            .ok_or_else(|| anyhow!("OokRaceKind: Unknown post ident"))
    }
}

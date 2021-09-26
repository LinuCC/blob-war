use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

use screeps::{
    game::get_object_typed, memory::MemoryReference, Creep, HasId, ObjectId, SharedCreepProperties,
    SpawnOptions,
};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_JOB, MEM_POST, MEM_RACE_KIND, MEM_REQUEST_ID},
    creeps::{
        jobs::{OokCreepJob, StorableJob},
        races::OokRace,
        tasks::{self, OokCreepTask, OokTaskRunnable},
        utils::create_creep_name,
        CalcSpawnBodyResult, Spawnable, TrySpawnOptions, TrySpawnResult, TrySpawnResultData,
    },
    state::{BWState, UniqId},
};

use super::{
    DoJobResult, DynamicTasked, Memorizing, OokRaceBodyComposition, OokRaceKind, RepresentsCreep,
};

use anyhow::{anyhow, bail, Context, Result};

const COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 1,
    carry: 0,
    work: 0,
    attack: 0,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 1,
};

#[derive(Debug, Clone)]
struct OokCreepClaimerMemory {
    race_kind: OokRaceKind,
    job: OokCreepJob,
    post_ident: String,
    request_id: Option<UniqId>,
}

impl OokCreepClaimerMemory {
    fn new(job: OokCreepJob, post_ident: String, request_id: Option<UniqId>) -> Self {
        Self {
            race_kind: OokRaceKind::Claimer,
            job,
            post_ident,
            request_id,
        }
    }
}

impl From<OokCreepClaimerMemory> for MemoryReference {
    fn from(mem: OokCreepClaimerMemory) -> Self {
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
        if let Some(id) = mem.request_id {
            memory.set(MEM_REQUEST_ID, id.to_string());
        }
        memory
    }
}

#[derive(Debug, Clone)]
pub struct OokCreepClaimer {
    pub creep_id: ObjectId<Creep>,
    job: OokCreepJob,
    task: Option<OokCreepTask>,
    // cached_creep: (u64, Creep),
}

impl OokCreepClaimer {
    /// HACK Currently claimer is spawned based on room settings, I should dynamically
    ///   get the best room to spawn in
    pub fn post_ident(&self) -> Result<String> {
        self.creep()?
            .memory()
            .string(MEM_POST)?
            .ok_or_else(|| anyhow!("OokRaceKind: Unknown post ident"))
    }
}

impl RepresentsCreep for OokCreepClaimer {
    fn creep(&self) -> Result<Creep> {
        get_object_typed(self.creep_id)
            .context("Claimer creep")?
            .ok_or(anyhow!("Claimer creep not found {}", self.creep_id))
    }
}

impl TryFrom<&screeps::Creep> for OokCreepClaimer {
    type Error = anyhow::Error;

    fn try_from(creep: &Creep) -> Result<Self, Self::Error> {
        let memory = creep.memory();
        let job_dict = memory
            .dict(MEM_JOB)
            .context("loading mem job")?
            .ok_or(anyhow!("mem job missing"))?;
        let worker_memory = OokCreepClaimerMemory {
            race_kind: memory
                .i32(MEM_RACE_KIND)
                .context("loading mem race_kind")?
                .ok_or(anyhow!("mem race_kind missing"))?
                .try_into()?,
            job: OokCreepJob::from_js_serialize(&job_dict)
                .context("loading mem job data")?
                .ok_or(anyhow!("mem task_kind job data"))?
                .try_into()?,
            post_ident: memory
                .string(MEM_POST)
                .context("loading mem post")?
                .ok_or(anyhow!("mem post missing"))?,
            request_id: memory
                .string(MEM_REQUEST_ID)
                .context("loading mem request_id")?
                .map(|s| UniqId::from(s)),
        };

        Ok(Self {
            creep_id: creep.id(),
            job: worker_memory.job,
            task: None,
        })
    }
}

impl Memorizing<OokCreepClaimerMemory> for OokCreepClaimer {
    fn creep_mem_race_ident() -> OokRaceKind {
        OokRaceKind::Claimer
    }

    fn set_memory(&self, mem: OokCreepClaimerMemory) -> Result<()> {
        let creep = get_object_typed(self.creep_id)?
            .ok_or(anyhow!("Memo: Creep {} not found", self.creep_id))?;
        let memory = creep.memory();
        memory.set(MEM_RACE_KIND, mem.race_kind as i32);
        let val = mem.job.to_js_serialize();
        let val = val
            .iter()
            .map(|(i, v)| (i.clone(), &**v))
            .collect::<HashMap<String, &dyn JsSerialize>>();
        memory.set(MEM_JOB, val);
        memory.set(MEM_POST, mem.post_ident);
        if let Some(id) = mem.request_id {
            memory.set(MEM_REQUEST_ID, id.to_string());
        } else {
            memory.del(MEM_REQUEST_ID);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TrySpawnClaimerOptions {
    pub post_ident: String,
}

#[derive(Debug, Clone)]
pub struct TrySpawnClaimerResult {}

impl Spawnable<TrySpawnClaimerOptions> for OokCreepClaimer {
    fn try_spawn(
        opts: &TrySpawnOptions,
        race_opts: &TrySpawnClaimerOptions,
    ) -> Result<TrySpawnResult> {
        let avail_energy = opts.spawn_room.energy_available();
        let calc_result = Self::calc_spawn_body(opts, race_opts)?;
        if calc_result.amount <= avail_energy {
            let spawn_id = opts
                .available_spawns
                .first()
                .ok_or(anyhow!("try_spawn called without available_spawns"))?;
            let spawn = get_object_typed(*spawn_id)
                .context("try_spawn")?
                .ok_or(anyhow!("Could not find spawn {} for try_spawn", spawn_id))?;
            let new_memory = OokCreepClaimerMemory::new(
                opts.assumed_job.to_owned(),
                race_opts.post_ident.to_owned(),
                opts.request_id.to_owned(),
            );

            let creep_name = create_creep_name(&opts.race);
            let return_code = spawn.spawn_creep_with_options(
                &calc_result.body,
                &create_creep_name(&opts.race),
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
                let new_memory = OokCreepClaimerMemory::new(
                    opts.assumed_job.to_owned(),
                    race_opts.post_ident.to_owned(),
                    opts.request_id.to_owned(),
                );
                let creep_name = create_creep_name(&opts.race);
                let return_code = spawn.spawn_creep_with_options(
                    &calc_result.body,
                    &creep_name,
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
        _race_opts: &TrySpawnClaimerOptions,
    ) -> anyhow::Result<CalcSpawnBodyResult> {
        let unit_cost = COMPOSITION.single_parts_unit_cost();
        let spawn_unit_count = 1; // HACK why whould I want multiple claims?
                                  // let spawn_unit_count =
                                  //     (opts.target_energy_usage as f32 / unit_cost as f32).floor() as usize;
        Ok(CalcSpawnBodyResult {
            amount: spawn_unit_count as u32 * unit_cost,
            body: COMPOSITION.parts_for_x_units(spawn_unit_count as u32),
        })
    }
}

impl DynamicTasked for OokCreepClaimer {
    fn task(&self) -> Option<&OokCreepTask> {
        self.task.as_ref()
    }

    fn job(&self) -> OokCreepJob {
        self.job.to_owned()
    }

    fn do_job(&mut self, state: &mut BWState) -> Result<DoJobResult> {
        let cloned_self = self.clone();
        match &mut self.task {
            Some(OokCreepTask::ClaimController(task)) => {
                task.run(state, &OokRace::Claimer(cloned_self))?;
            }
            Some(_) => bail!("Unhandled task"),
            None => match &self.job {
                OokCreepJob::ClaimRoom { target_room } => {
                    // NOTE Do I want to use DoJobResult::RequestingNewTask or create task
                    // directly in here?
                    let task = tasks::claim_controller::Task::new(
                        &state,
                        &OokRace::Claimer(cloned_self),
                        *target_room,
                    )?;
                    self.task = Some(OokCreepTask::ClaimController(task));
                }
                job => {
                    if let Ok(creep) = self.creep() {
                        creep.say("wut job??", false);
                    }
                    bail!("OokCreepClaimer::do_task unknown job {:?}", job);
                }
            },
        }
        Ok(DoJobResult::None)
    }
}

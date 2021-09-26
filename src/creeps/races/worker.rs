use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

use log::{info, warn};
use screeps::{
    find,
    game::{get_object_typed, rooms},
    memory::MemoryReference,
    Creep, HasId, HasPosition, ObjectId, Position, RoomName, RoomObjectProperties,
    SharedCreepProperties, SpawnOptions,
};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_JOB, MEM_POST, MEM_RACE_KIND, MEM_REQUEST_ID, MEM_ROOM_BASE},
    creeps::{
        jobs::{self, OokCreepJob, StorableJob},
        races::OokRace,
        tasks::{self, OokCreepTask, OokTaskRunnable},
        utils::create_creep_name,
        CalcSpawnBodyResult, Spawnable, TrySpawnOptions, TrySpawnResult, TrySpawnResultData,
    },
    state::{BWState, UniqId},
};

use super::{
    DoJobResult, DynamicTasked, Memorizing, OokRaceBodyComposition, OokRaceKind, RepresentsCreep,
    RoomBound,
};
use anyhow::{anyhow, bail, Context, Result};

const FALLBACK_COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 2,
    carry: 2,
    work: 1,
    attack: 0,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 0,
};

// Especially helpful for upgraders
const SECOND_CL_COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 3,
    carry: 2,
    work: 3,
    attack: 0,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 0,
};

const LARGE_COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 1,
    carry: 1,
    work: 1,
    attack: 0,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 0,
};

#[derive(Debug, Clone)]
struct OokCreepWorkerMemory {
    race_kind: OokRaceKind,
    job: OokCreepJob,
    post_ident: String,
    base_room: RoomName,
    request_id: Option<UniqId>,
}

impl OokCreepWorkerMemory {
    fn new(
        job: OokCreepJob,
        post_ident: String,
        base_room: RoomName,
        request_id: Option<UniqId>,
    ) -> Self {
        Self {
            race_kind: OokRaceKind::Worker,
            job,
            post_ident,
            base_room,
            request_id,
        }
    }
}

impl From<OokCreepWorkerMemory> for MemoryReference {
    fn from(mem: OokCreepWorkerMemory) -> Self {
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
pub struct OokCreepWorker {
    pub creep_id: ObjectId<Creep>,
    pub job: OokCreepJob,

    task: Option<OokCreepTask>,
    // cached_creep: (u64, Creep),
}

impl RepresentsCreep for OokCreepWorker {
    fn creep(&self) -> Result<Creep> {
        get_object_typed(self.creep_id)
            .context("Worker creep")?
            .ok_or(anyhow!("Worker creep not found {}", self.creep_id))
    }
}

impl TryFrom<&screeps::Creep> for OokCreepWorker {
    type Error = anyhow::Error;

    fn try_from(creep: &Creep) -> Result<Self, Self::Error> {
        let memory = creep.memory();
        let job_dict = memory
            .dict(MEM_JOB)
            .context("loading mem job")?
            .ok_or(anyhow!("mem job missing"))?;
        let worker_memory = OokCreepWorkerMemory {
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
            job: worker_memory.job,
            task: None,
        })
    }
}

impl Memorizing<OokCreepWorkerMemory> for OokCreepWorker {
    // fn get_memory(&self) -> Result<OokCreepWorkerMemory> {
    //     let creep = get_object_typed(self.creep_id)?
    //         .ok_or(anyhow!("Memo: Creep {} not found", self.creep_id))?;
    //     let memory = creep.memory();
    //     Ok(OokCreepWorkerMemory {
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
        OokRaceKind::Worker
    }

    fn set_memory(&self, mem: OokCreepWorkerMemory) -> Result<()> {
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
pub struct TrySpawnWorkerOptions {
    pub base_room: RoomName,
    pub post_ident: String,
}

#[derive(Debug, Clone)]
pub struct TrySpawnWorkerResult {}

impl Spawnable<TrySpawnWorkerOptions> for OokCreepWorker {
    fn try_spawn(
        opts: &TrySpawnOptions,
        race_opts: &TrySpawnWorkerOptions,
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
            let new_memory = OokCreepWorkerMemory::new(
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
                let new_memory = OokCreepWorkerMemory::new(
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
        race_opts: &TrySpawnWorkerOptions,
    ) -> anyhow::Result<CalcSpawnBodyResult> {
        if opts.target_energy_usage <= 300 {
            Ok(CalcSpawnBodyResult {
                amount: FALLBACK_COMPOSITION.single_parts_unit_cost(),
                body: FALLBACK_COMPOSITION.parts_for_x_units(1),
            })
        } else if opts.target_energy_usage <= 550 {
            Ok(CalcSpawnBodyResult {
                amount: SECOND_CL_COMPOSITION.single_parts_unit_cost(),
                body: SECOND_CL_COMPOSITION.parts_for_x_units(1),
            })
        } else {
            if let Some((body, amount)) =
                LARGE_COMPOSITION.parts_for_x_energy(opts.target_energy_usage)
            {
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
}

impl DynamicTasked for OokCreepWorker {
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
                    OokCreepTask::UpgradeController(task) => {
                        task.run(state, &OokRace::Worker(cloned_self))?
                    }
                    OokCreepTask::Build(task) => {
                        task.run(state, &OokRace::Worker(cloned_self))?
                    }
                    OokCreepTask::FarmSource(task) => {
                        task.run(state, &OokRace::Worker(cloned_self))?
                    }
                    OokCreepTask::MaintainResource => bail!("worker task not handled"),
                    OokCreepTask::MaintainStructures => bail!("worker task not handled"),
                    OokCreepTask::ClaimController(_) => bail!("worker task not handled"),
                    OokCreepTask::FetchForConsumer(_) => bail!("worker task not handled"),
                    OokCreepTask::SpawnSuppliesRun(_) => bail!("worker task not handled"),
                };
                match run_result {
                    tasks::OokTaskRunnableResult::Continue => {},
                    tasks::OokTaskRunnableResult::Finish => {self.task = None;},
                    tasks::OokTaskRunnableResult::CancelAndDoAnother => {
                        info!("Cancelling and doing another task");
                        self.task = None;
                        return self.do_job(state);
                    },
                }
            }
            None => match &self.job {
                OokCreepJob::UpgradeController { target_room } => {
                    // TODO Should also work for rooms that are not directly visible
                    let pos = rooms::get(*target_room)
                        .ok_or_else(|| anyhow!("Room not found {:?}", target_room))?
                        .controller()
                        .ok_or_else(|| anyhow!("Room controller missing"))?
                        .pos();
                    let task = tasks::upgrade_controller::Task::new(
                        pos,
                        &state,
                        &OokRace::Worker(cloned_self),
                    )?;
                    self.task = Some(OokCreepTask::UpgradeController(task));
                }
                OokCreepJob::BootstrapRoom { target_room } => {
                    // TODO Should also work for rooms that are not directly visible
                    let pos = rooms::get(*target_room)
                        .map(|r| {
                            r.controller().map(|c| c.pos()).unwrap_or(Position::new(
                                25,
                                25,
                                *target_room,
                            ))
                        })
                        .unwrap_or(Position::new(25, 25, *target_room));
                    // HACK BIG FIN HACK LOL
                    if self.creep()?.room().ok_or(anyhow!("Wer room wut"))?.name() != *target_room {
                        self.creep()?.move_to(&pos);
                    } else {
                        self.creep()?.move_to(&pos); // HACK
                        if let Some(construction_site) = self
                            .creep()?
                            .room()
                            .ok_or(anyhow!("Room wut wut"))?
                            .find(find::CONSTRUCTION_SITES)
                            .first()
                        {
                            let task = tasks::build::Task::new(
                                construction_site.to_owned(),
                                &state,
                                &OokRace::Worker(cloned_self),
                            )?;
                            info!("cons site: {:?}", construction_site.id());
                            self.task = Some(OokCreepTask::Build(task));
                        } else {
                            let task = tasks::upgrade_controller::Task::new(
                                pos,
                                &state,
                                &OokRace::Worker(cloned_self),
                            )?;
                            self.task = Some(OokCreepTask::UpgradeController(task));
                        }
                    }
                }
                OokCreepJob::FarmSource(jobs::FarmSource {
                    target_room,
                    target_source,
                }) => {
                    let creep = self.creep()?;
                    if let Some(target_source) = get_object_typed(*target_source)? {
                        if creep.pos().room_name() == target_source.pos().room_name() {
                            self.task = Some(OokCreepTask::FarmSource(tasks::farm::Task::new(
                                &target_source,
                                &state,
                                &OokRace::Worker(cloned_self),
                            )?));
                        }
                    } else {
                        // We don't see the room, move to it
                        if rooms::get(*target_room).is_some() {
                            // Indicates that target_source is not in target_room, which is stupid
                            // Or that target_source id is wrong
                            warn!(
                                "FarmSource: target_source {} not found, but target_room {} ?!",
                                target_source, target_room
                            );
                        }
                        let pos = Position::new(25, 25, *target_room);
                        creep.move_to(&pos);
                    }
                }
                job => {
                    if let Ok(creep) = self.creep() {
                        creep.say("wut job??", false);
                    }
                    bail!("OokCreepWorker::do_task unknown job {:?}", job);
                }
            },
        }
        Ok(DoJobResult::None)
    }
}

impl RoomBound<String> for OokCreepWorker {
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

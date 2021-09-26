use std::convert::{TryFrom, TryInto};

use screeps::{Color, Creep, HasId, ObjectId, RoomName, SharedCreepProperties, SpawnOptions, game::get_object_typed, memory::MemoryReference};

use crate::{constants::{MEM_FLAG_PRIMARY_COLOR, MEM_FLAG_SECONDARY_COLOR, MEM_POST, MEM_RACE_KIND, MEM_TARGET_ROOM, MEM_TASK_KIND}, creeps::{CalcSpawnBodyResult, Spawnable, TrySpawnOptions, TrySpawnResult, TrySpawnResultData, races::OokRace, tasks::{self, OokCreepTask, OokCreepTaskKind, OokTaskRunnable}, utils::create_creep_name}, state::BWContext};

use super::{DoTaskResult, DynamicTasked, Memorizing, OokRaceBodyComposition, OokRaceKind, RepresentsCreep};

use anyhow::{Context, Result, anyhow, bail};

const COMPOSITION: OokRaceBodyComposition = OokRaceBodyComposition {
    mov: 4,
    carry: 0,
    work: 0,
    attack: 20,
    ranged_attack: 0,
    heal: 0,
    tough: 0,
    claim: 0,
};


#[derive(Debug, Clone)]
struct OokCreepDefenderMemory {
    race_kind: OokRaceKind,
    task_kind: OokCreepTaskKind,
    target_room: RoomName,
    flag_primary: Color,
    flag_secondary: Color,
}

impl OokCreepDefenderMemory {
    fn new(task_kind: OokCreepTaskKind, target_room: RoomName, flag_primary: Color, flag_secondary: Color) -> Self {
        Self {
            race_kind: OokRaceKind::CloseCombatDefender,
            task_kind,
            target_room,
            flag_primary,
            flag_secondary,
        }
    }
}

impl From<OokCreepDefenderMemory> for MemoryReference {
    fn from(mem: OokCreepDefenderMemory) -> Self {
        let memory = MemoryReference::new();
        memory.set(MEM_RACE_KIND, mem.race_kind as i32);
        memory.set(MEM_TASK_KIND, mem.task_kind as i32);
        memory.set(MEM_FLAG_PRIMARY_COLOR, mem.flag_primary as u8);
        memory.set(MEM_FLAG_SECONDARY_COLOR, mem.flag_secondary as u8);
        memory.set(MEM_TARGET_ROOM, mem.target_room.to_string());

        memory
    }
}

#[derive(Debug, Clone)]
pub struct OokCreepDefender {
    pub creep_id: ObjectId<Creep>,

    task_kind: OokCreepTaskKind,
    task: Option<OokCreepTask>,
    target_room: RoomName,
    // cached_creep: (u64, Creep),
}

impl OokCreepDefender {

    /// HACK Currently defender is spawned based on room settings, I should dynamically
    ///   get the best room to spawn in
    pub fn post_ident(&self) -> Result<String> {
        self.creep()?
            .memory()
            .string(MEM_POST)?
            .ok_or_else(|| anyhow!("OokRaceKind: Unknown post ident"))
    }
}

impl RepresentsCreep for OokCreepDefender {
    fn creep(&self) -> Result<Creep> {
        get_object_typed(self.creep_id)
            .context("Defender creep")?
            .ok_or(anyhow!("Defender creep not found {}", self.creep_id))
    }
}

impl TryFrom<&screeps::Creep> for OokCreepDefender {
    type Error = anyhow::Error;

    fn try_from(creep: &Creep) -> Result<Self, Self::Error> {
        let memory = creep.memory();
        let worker_memory = OokCreepDefenderMemory {
            race_kind: memory
                .i32(MEM_RACE_KIND)
                .context("loading mem race_kind")?
                .ok_or(anyhow!("mem race_kind missing"))?
                .try_into()?,
            task_kind: memory
                .i32(MEM_TASK_KIND)
                .context("loading mem task_kind")?
                .ok_or(anyhow!("mem task_kind missing"))?
                .try_into()?,
            target_room: RoomName::new(&memory
                .string(MEM_TARGET_ROOM)
                .context("loading mem target_room")?
                .ok_or(anyhow!("mem target_room missing"))?,
            )
            .context("loading mem room_base")?,
            flag_primary: FromPrimitive::from_i32(memory
                .i32(MEM_FLAG_PRIMARY_COLOR)
                .context("loading mem primary color")?
                .ok_or(anyhow!("mem primary color missing"))?)
                .ok_or(anyhow!("mem primary color not a color"))?,

            flag_secondary: memory
                .i32(MEM_FLAG_SECONDARY_COLOR)
                .context("loading mem secondary color")?
                .ok_or(anyhow!("mem secondary color missing"))?
                .try_into()?,
        };

        Ok(Self {
            creep_id: creep.id(),
            task_kind: worker_memory.task_kind,
            target_room: worker_memory.target_room,
            task: None,
        })
    }
}

impl Memorizing<OokCreepDefenderMemory> for OokCreepDefender {
    fn creep_mem_race_ident() -> OokRaceKind {
        OokRaceKind::Defender
    }

    fn set_memory(&self, mem: OokCreepDefenderMemory) -> Result<()> {
        let creep = get_object_typed(self.creep_id)?
            .ok_or(anyhow!("Memo: Creep {} not found", self.creep_id))?;
        let memory = creep.memory();
        memory.set(MEM_RACE_KIND, mem.race_kind as i32);
        memory.set(MEM_TASK_KIND, mem.task_kind as i32);
        memory.set(MEM_POST, mem.post_ident);
        memory.set(MEM_FLAG_PRIMARY_COLOR, mem.flag_primary);
        memory.set(MEM_FLAG_SECONDARY_COLOR, mem.flag_secondary);
        memory.set(MEM_TARGET_ROOM, mem.target_room.to_string());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TrySpawnDefenderOptions {
    pub target_room: RoomName,
    pub post_ident: String,
}

#[derive(Debug, Clone)]
pub struct TrySpawnDefenderResult {
    
}

impl Spawnable<TrySpawnDefenderOptions> for OokCreepDefender {
    fn try_spawn(opts: &TrySpawnOptions, race_opts: &TrySpawnDefenderOptions) -> Result<TrySpawnResult> {
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
            let new_memory = OokCreepDefenderMemory::new(
                opts.assumed_task.to_owned(),
                race_opts.target_room,
                race_opts.post_ident.to_owned(),
            );
            let return_code = spawn.spawn_creep_with_options(
                &calc_result.body,
                &create_creep_name(&opts.race),
                &SpawnOptions::default().memory(Some(new_memory.into())),
            );
            Ok(TrySpawnResult::Spawned(TrySpawnResultData {
                return_code,
                used_energy_amount: calc_result.amount,
                used_spawn: spawn.id(),
            }))
        } else {
            if opts.force_spawn {
                let calc_result = Self::calc_spawn_body(
                    &TrySpawnOptions {
                        target_energy_usage: avail_energy,
                        ..opts.to_owned()
                    },
                    race_opts
                )
                .context("force spawn calc_spawn_body")?;
                let spawn_id = opts
                    .available_spawns
                    .first()
                    .ok_or(anyhow!("try_spawn called without available_spawns"))?;
                let spawn = get_object_typed(*spawn_id)
                    .context("try_spawn")?
                    .ok_or(anyhow!("Could not find spawn {} for try_spawn", spawn_id))?;
                let new_memory = OokCreepDefenderMemory::new(
                    opts.assumed_task.to_owned(),
                    race_opts.target_room,
                    race_opts.post_ident.to_owned(),
                );
                let return_code = spawn.spawn_creep_with_options(
                    &calc_result.body,
                    &create_creep_name(&opts.race),
                    &SpawnOptions::default().memory(Some(new_memory.into())),
                );
                Ok(TrySpawnResult::ForceSpawned(TrySpawnResultData {
                    return_code,
                    used_energy_amount: calc_result.amount,
                    used_spawn: spawn.id(),
                }))
            } else {
                Ok(TrySpawnResult::Skipped)
            }
        }
    }

    fn calc_spawn_body(
        opts: &crate::creeps::TrySpawnOptions,
        _race_opts: &TrySpawnDefenderOptions,
    ) -> anyhow::Result<CalcSpawnBodyResult> {
        let unit_cost = COMPOSITION.single_parts_unit_cost();
        let spawn_unit_count =
            (opts.target_energy_usage as f32 / unit_cost as f32).floor() as usize;
        Ok(CalcSpawnBodyResult {
            amount: spawn_unit_count as u32 * unit_cost,
            body: COMPOSITION.parts_for_x_units(spawn_unit_count as u32),
        })
    }
}

impl DynamicTasked for OokCreepDefender {
    fn task_kind(&self) -> OokCreepTaskKind {
        self.task_kind.clone()
    }

    fn task(&self) -> Option<&OokCreepTask> {
        self.task.as_ref()
    }

    fn do_task(&mut self, _opts: super::DoTaskOptions) -> Result<DoTaskResult> {
        let context = BWContext::get();
        let state = context.state()?;
        let cloned_self = self.clone();
        match &mut self.task {
            Some(OokCreepTask::DefendController(task)) => {
                task.run( state, &OokRace::Defender(cloned_self))?;
            },
            Some(_) => {
                bail!("Unhandled task")
            },
            None => match self.task_kind {
                OokCreepTaskKind::DefendRoom => {
                    // NOTE Do I want to use DoTaskResult::RequestingNewTask or create task
                    // directly in here?
                    let task = tasks::claim_controller::Task::new(
                        &state,
                        &OokRace::Defender(cloned_self),
                        self.target_room,
                    )?;
                    self.task = Some(OokCreepTask::DefendController(task));
                }
                _ => {
                    bail!("OokCreepDefender::do_task unknown task_kind");
                }
            },
        }
        Ok(DoTaskResult::None)
    }
}


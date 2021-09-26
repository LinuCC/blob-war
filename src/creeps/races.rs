use anyhow::{anyhow, bail, Context, Result};
use core::fmt;
use log::{info, warn};
use std::{cmp, collections::HashMap, convert::TryFrom, iter};

use screeps::{Creep, HasId, HasPosition, MAX_CREEP_SIZE, ObjectId, ResourceType, Room, RoomName, RoomObjectProperties, SharedCreepProperties, creep};

use crate::{constants::MEM_RACE_KIND, rooms::resource_provider::{ResourceData, ResourceProvider}, state::BWState};

use self::{carrier::OokCreepCarrier, claimer::OokCreepClaimer, worker::OokCreepWorker};

use super::{generic_creep_fetch_from_provider_prio, jobs::OokCreepJob, tasks::{CalcResourceProviderResult, OokCreepTask}, utils::SpawnableTimer};

pub mod claimer;
pub mod worker;
pub mod carrier;
// pub mod close_combat_defender;

#[derive(thiserror::Error, Debug)]
pub enum RacesError {
    #[error("missing mem race_kind")]
    MemRaceKindMissing,
}

pub trait RepresentsCreep {
    fn creep(&self) -> Result<Creep>;
}

pub trait DynamicTasked {
    fn task(&self) -> Option<&OokCreepTask>;
    fn job(&self) -> OokCreepJob;
    fn do_job(&mut self, state: &mut BWState) -> Result<DoJobResult>;
}

pub trait Memorizing<T> {
    // NOTE Maybe `TryFrom` is enough?
    // fn get_memory(&self) -> Result<T>;
    fn creep_mem_race_ident() -> OokRaceKind;
    fn set_memory(&self, mem: T) -> Result<()>;
}

pub trait RoomBound<T>
where
    T: fmt::Debug + stdweb::JsSerialize,
{
    fn room_name_of_base(&self) -> Result<RoomName>;

    /// The identifier of the post the citizen is at
    fn post_ident(&self) -> Result<T>;
}

#[derive(Debug, Clone)]
pub struct CalcPartsOptions {
    limit_work_parts: Option<u8>,
}

impl CalcPartsOptions {
    // pub fn limit_work_parts(&mut self, max_work_parts: u8) -> Self {
    //     self.limit_work_parts = Some(max_work_parts);
    //     self
    // }
}

impl Default for CalcPartsOptions {
    fn default() -> Self {
        Self {
            limit_work_parts: None,
        }
    }
}


#[derive(Debug, Clone)]
pub struct OokRaceBodyComposition {
    pub mov: u32,
    pub carry: u32,
    pub work: u32,
    pub attack: u32,
    pub ranged_attack: u32,
    pub heal: u32,
    pub tough: u32,
    pub claim: u32,
}

impl OokRaceBodyComposition {
    pub fn single_parts_unit_count(&self) -> u32 {
        self.mov
            + self.carry
            + self.work
            + self.attack
            + self.ranged_attack
            + self.heal
            + self.tough
            + self.claim
    }

    pub fn single_parts_unit_cost(&self) -> u32 {
        (self.mov * creep::Part::Move.cost())
            + (self.carry * creep::Part::Carry.cost())
            + (self.work * creep::Part::Work.cost())
            + (self.attack * creep::Part::Attack.cost())
            + (self.ranged_attack * creep::Part::RangedAttack.cost())
            + (self.heal * creep::Part::Heal.cost())
            + (self.tough * creep::Part::Tough.cost())
            + (self.claim * creep::Part::Claim.cost())
    }

    pub fn parts_for_x_units(&self, unit_count: u32) -> Vec<creep::Part> {
        iter::repeat(creep::Part::Move)
            .take((self.mov * unit_count) as usize)
            .chain(iter::repeat(creep::Part::Carry).take((self.carry * unit_count) as usize))
            .chain(iter::repeat(creep::Part::Work).take((self.work * unit_count) as usize))
            .chain(iter::repeat(creep::Part::Attack).take((self.attack * unit_count) as usize))
            .chain(
                iter::repeat(creep::Part::RangedAttack)
                    .take((self.ranged_attack * unit_count) as usize),
            )
            .chain(iter::repeat(creep::Part::Heal).take((self.heal * unit_count) as usize))
            .chain(iter::repeat(creep::Part::Tough).take((self.tough * unit_count) as usize))
            .chain(iter::repeat(creep::Part::Claim).take((self.claim * unit_count) as usize))
            .collect()
    }

    pub fn parts_for_x_energy(&self, target_energy: u32, ) -> Option<(Vec<creep::Part>, u32)> {
        let unit_cost = self.single_parts_unit_cost();
        let max_unit_count = (MAX_CREEP_SIZE as f32 / self.single_parts_unit_count() as f32).floor() as usize;
        let target_unit_count = (target_energy as f32 / unit_cost as f32).floor() as usize;
        let spawn_unit_count = cmp::min(target_unit_count, max_unit_count);
        if spawn_unit_count > 0 {
            Some((
                self.parts_for_x_units(spawn_unit_count as u32),
                spawn_unit_count as u32 * unit_cost,
            ))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub enum OokRace {
    Worker(OokCreepWorker),
    Claimer(OokCreepClaimer),
    // StaticWorker(OokCreepWorker),
    Carrier(OokCreepCarrier),
    // Claimer(OokCreepClaimer),
    // Attacker(OokCreepAttacker),
    // CloseCombatDefender(OokCreepDefender),
}

impl TryFrom<&Creep> for OokRace {
    type Error = anyhow::Error;

    fn try_from(creep: &Creep) -> Result<Self> {
        match creep.memory().i32(MEM_RACE_KIND)? {
            Some(kind) if kind == OokRaceKind::Worker as i32 => Ok(OokRace::Worker(
                OokCreepWorker::try_from(creep).context("OokCreepWorker: try_from creep")?,
            )),
            Some(kind) if kind == OokRaceKind::Claimer as i32 => {
                Ok(OokRace::Claimer(OokCreepClaimer::try_from(creep)?))
            }
            Some(kind) if kind == OokRaceKind::Carrier as i32 => {
                Ok(OokRace::Carrier(OokCreepCarrier::try_from(creep)?))
            }
            Some(val) => Err(anyhow!("OokRace: Unknown race mem {}", val)),
            None => Err(RacesError::MemRaceKindMissing.into()),
        }
    }
}

impl RepresentsCreep for OokRace {
    fn creep(&self) -> Result<Creep> {
        match self {
            OokRace::Worker(worker) => worker.creep(),
            OokRace::Claimer(claimer) => claimer.creep(),
            OokRace::Carrier(carrier) => carrier.creep(),
        }
    }
}

impl SpawnableTimer for OokRace {
    fn get_spawn_time(&self) -> usize {
        if let Ok(creep) = self.creep() {
            creep.body().get_spawn_time()
        } else {
            warn!("get_spawn_time: creep not found");
            0
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OokRaceKind {
    Worker = 0,
    // Probably should remove him, as the Jobs are more specific
    StaticWorker = 1,
    Carrier = 2,
    Attacker = 3,
    CloseCombatDefender = 4,
    Claimer = 5,
}

impl TryFrom<i32> for OokRaceKind {
    type Error = anyhow::Error;

    fn try_from(val: i32) -> Result<Self, Self::Error> {
        Ok(match val {
            0 => Self::Worker,
            1 => Self::StaticWorker,
            2 => Self::Carrier,
            3 => Self::Attacker,
            4 => Self::CloseCombatDefender,
            5 => Self::Claimer,
            _ => Err(anyhow!("Unknown OokRaceKind {}", val))?,
        })
    }
}

impl fmt::Display for OokRaceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OokRaceKind::Worker => f.write_str("worker"),
            OokRaceKind::StaticWorker => f.write_str("static-worker"),
            OokRaceKind::Carrier => f.write_str("carrier"),
            OokRaceKind::Attacker => f.write_str("attacker"),
            OokRaceKind::CloseCombatDefender => f.write_str("defender"),
            OokRaceKind::Claimer => f.write_str("claimer"),
        }
    }
}

pub enum DoJobResult {
    None,
}

pub fn get_all_citizens_from_creeps(
    creeps: Vec<Creep>,
    cached_citizens: &HashMap<ObjectId<screeps::Creep>, OokRace>,
) -> Result<HashMap<ObjectId<Creep>, OokRace>> {
    let mut citizens = HashMap::new();
    let mut unknown_creeps = 0;
    for creep in creeps {
        if let Some(cached_citizen) = cached_citizens.get(&creep.id()) {
            citizens.insert(creep.id(), cached_citizen.to_owned());
        } else if let Some(room) = creep.room() {
            match OokRace::try_from(&creep) {
                Ok(c) => {
                    citizens.insert(creep.id(), c);
                }
                Err(err) => match err.downcast_ref::<RacesError>() {
                    Some(RacesError::MemRaceKindMissing) => {
                        unknown_creeps += 1;
                    }
                    None => warn!("unhandled OokRace try_from error {}", err),
                },
            }
        } else {
            bail!("creep has no room");
        }
    }
    // info!("{:?}", citizens);
    // if unknown_creeps > 0 {
    //     info!("{} creeps have no race", unknown_creeps);
    // }
    Ok(citizens)
}

pub fn generic_calc_energy_resource_provider(
    resource_providers: &HashMap<String, ResourceProvider>,
    creep: &Creep,
    room: &Room,
    amount: u32,
) -> Result<Option<CalcResourceProviderResult>> {
    let working_providers: Vec<&ResourceProvider> = resource_providers
        .iter()
        .filter_map(|(_id, p)| match p.creep_can_use(&creep) {
            Ok(true) => Some(p),
            Ok(false) => None,
            Err(err) => {
                warn!("Could not check for `creep_can_use`: {}", err);
                None
            }
        })
        .collect();
    let prioed = generic_creep_fetch_from_provider_prio(&room, creep.pos(), working_providers)?;
    match prioed {
        Some(prov) => Ok(Some(CalcResourceProviderResult {
            resource_provider: prov.to_owned(),
            resource_type: ResourceType::Energy,
            amount: amount as u32,
        })),
        None => Ok(None),
    }
}

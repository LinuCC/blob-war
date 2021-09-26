pub mod upgrade_controller;
pub mod claim_controller;
pub mod build;
pub mod farm;
pub mod fetch_for_consumer;
pub mod spawn_supplies_run;

use std::{collections::HashMap, convert::TryFrom};

use screeps::{ResourceType, RoomName};
use serde::{Serialize, Deserialize};
use anyhow::{anyhow, Result};

use crate::{rooms::{resource_provider::ResourceProvider, room_state::RoomState}, state::BWState};

use super::races::OokRace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OokCreepTaskKind {
    UpgradeController = 0,
    MaintainResource = 1,
    FarmSource = 2,
    MaintainStructures = 3,
    ClaimRoom = 4,
    DefendRoom = 5,
    BootstrapRoom = 6,
    Build = 7,
    FetchForConsumer = 8,
}

impl TryFrom<i32> for OokCreepTaskKind {
    type Error = anyhow::Error;

    fn try_from( val: i32 ) -> Result< Self, Self::Error > {
        Ok(match val {
            0 => Self::UpgradeController,
            1 => Self::MaintainResource,
            2 => Self::FarmSource,
            3 => Self::MaintainStructures,
            4 => Self::ClaimRoom,
            5 => Self::DefendRoom,
            6 => Self::BootstrapRoom,
            7 => Self::Build,
            8 => Self::FetchForConsumer,
            _ => Err(anyhow!("Unknown OokCreepTaskKind {}", val))?
        })
    }
}

#[derive(Debug, Clone)]
pub enum OokCreepTask {
    UpgradeController(upgrade_controller::Task),
    MaintainResource,
    FarmSource(farm::Task),
    MaintainStructures,
    ClaimController(claim_controller::Task),
    Build(build::Task),
    // WithdrawFromProvider(build::Task),
    /// Something requests a resource which needs to be carried over
    FetchForConsumer(fetch_for_consumer::Task),
    SpawnSuppliesRun(spawn_supplies_run::Task),
    // DefendRoom(defend_room::Task),
    // BootstrapRoom(bootstrap_room::Task),
}

pub enum OokTaskRunnableResult {
    Continue,
    /// Task is done in this tick, get task for next tick
    Finish,
    /// Task is done but creep did not do anything yet, get another task
    CancelAndDoAnother,
}

pub trait OokTaskRunnable {
    fn run(&mut self, state: &mut BWState, race: &OokRace) -> Result<OokTaskRunnableResult>;
}

pub trait FetchesResource {
    fn calc_next_fetch<'a>(
        &mut self,
        rooms_state: &'a HashMap<RoomName, RoomState>,
    ) -> Result<Option<(&'a ResourceProvider, ResourceType, u32)>>;
    // fn select_target_provider(states: RoomState) -> Result<(ResourceProvider, ResourceType, u32), Box<dyn Error>>;
}

pub struct CalcResourceProviderResult {
    pub resource_provider: ResourceProvider,
    pub resource_type: ResourceType,
    pub amount: u32,
}

 pub trait FetchesFromResourceProvider<'a> {
    fn calc_resource_provider(
        &self,
        rooms_state: &'a HashMap<RoomName, RoomState>,
        race: &'a OokRace,
    ) -> Result<Option<CalcResourceProviderResult>>;
 }

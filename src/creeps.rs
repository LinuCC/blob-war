use core::fmt;
use std::{cmp::{self, Reverse}, collections::HashMap, convert::TryFrom, error::Error};

use log::{debug, info, warn};
use screeps::{Attackable, ConstructionSite, FindOptions, HasId, HasPosition, HasStore, MoveToOptions, ObjectId, Part, Path, Position, RawObjectId, Resource, ResourceType, ReturnCode, Room, RoomName, RoomObjectProperties, Ruin, SharedCreepProperties, Source, Structure, StructureContainer, StructureExtension, StructureSpawn, StructureStorage, StructureTerminal, StructureTower, creep, find, game::get_object_typed, look, memory::MemoryReference};

use crate::{constants::{CREEP_ID_BITCH, CREEP_ID_BUILDER, CREEP_ID_FARMER, CREEP_ID_RUNNER, CREEP_ID_UNKNOWN, MEM_ASSIGNED_SOURCE, MEM_FARM_POSITION_X, MEM_FARM_POSITION_Y, MEM_HARVESTING, MEM_KIND, MEM_POST, MEM_RESOURCE_PROVIDER_ID, TERMINAL_TRADE_BUFFER}, rooms::{FarmPosition, MyRoom, PathOptionUnwrapper, RoomMaintenance, resource_provider::{ResourceData, ResourceProvider, RoomObjectData, TakeResourceResult}, room_ext::RoomExt, room_state::{RoomState, SetupBaseStateVisibility}}, state::{BWContext, UniqId}, utils::HexStr};

use self::{jobs::OokCreepJob, races::{OokRace, OokRaceKind}};

use anyhow::anyhow;

pub mod harvesting;
pub mod races;
pub mod tasks;
pub mod utils;
pub mod jobs;

#[derive(thiserror::Error, Debug)]
pub enum CreepError {
    #[error("Could not convert creep")]
    CreepNotConvertible(),
    #[error("[Creep] Could not find object {0}")]
    ObjectNotFound(String),
    #[error("Could not find room for creep")]
    RoomNotFound(),
    #[error("Could not find source {0}")]
    SourceNotFound(String),
    #[error("Creep {0} has no post")]
    MissingPost(String),
    #[error("Creep {0} has no assigned_source")]
    MissingAssignedSource(String),
    #[error("Creep {0} has no correct farm position")]
    MissingFarmPosition(String),
    #[error("Trying to repair target {0}, but its not attackable")]
    RepairNotAttackable(String),
    #[error("Trying to load resource provider id failed")]
    ResourceProviderIdNotStored,
}

#[derive(Clone, Debug )]
pub enum CreepKind {
    Bitch(CreepBitch),
    Builder(CreepBuilder),
    Farmer(CreepFarmer),
    Runner(CreepRunner),
    Unknown(CreepUnknown),
}

trait HandlesResource {
    fn calc_next_fetch<'a>(
        &mut self,
        rooms_state: &'a HashMap<RoomName, RoomState>,
    ) -> Result<Option<(&'a ResourceProvider, ResourceType, u32)>, Box<dyn Error>>;
    // fn select_target_provider(states: RoomState) -> Result<(ResourceProvider, ResourceType, u32), Box<dyn Error>>;
}

impl CreepKind {
    // fn ident(&self) -> &str {
    //     use CreepKind::*;
    //     match self {
    //         Bitch(_) => CREEP_ID_BITCH,
    //         Builder(_) => CREEP_ID_BUILDER,
    //         Farmer(_) => CREEP_ID_FARMER,
    //         Runner(_) => CREEP_ID_RUNNER,
    //         Unknown(_) => CREEP_ID_UNKNOWN,
    //     }
    // }
    //
    // pub fn get_creep(&self) -> &screeps::Creep {
    //     use CreepKind::*;
    //     match self {
    //         Bitch(data) => &data.creep,
    //         Builder(data) => &data.creep,
    //         Farmer(data) => &data.creep,
    //         Runner(data) => &data.creep,
    //         Unknown(data) => &data.creep,
    //     }
    // }

    pub fn set_creep(&mut self, creep: screeps::Creep) {
        use CreepKind::*;
        match self {
            Bitch(data) => data.creep = creep,
            Builder(data) => data.creep = creep,
            Farmer(data) => data.creep = creep,
            Runner(data) => data.creep = creep,
            Unknown(data) => data.creep = creep,
        };
    }
}

impl TryFrom<screeps::objects::Creep> for CreepKind {
    type Error = Box<dyn std::error::Error>;

    fn try_from(creep: screeps::objects::Creep) -> Result<Self, Self::Error> {
        let mem = creep.memory();
        if let Some(kind_str) = mem.string(MEM_KIND)? {
            let my_room = MyRoom::by_room_name(
                creep
                    .room()
                    .ok_or(Box::new(CreepError::RoomNotFound()))?
                    .name(),
            )
            .ok_or(Box::new(CreepError::RoomNotFound()))?;
            Ok(match kind_str.as_str() {
                k if k == CREEP_ID_BITCH => CreepKind::Bitch(CreepBitch {
                    my_room,
                    id: creep.id(),
                    post: mem
                        .string(MEM_POST)?
                        .ok_or(Box::new(CreepError::MissingPost(format!("{}", creep.id()))))?,
                    creep,
                }),
                k if k == CREEP_ID_BUILDER => CreepKind::Builder(CreepBuilder {
                    my_room,
                    id: creep.id(),
                    post: mem
                        .string(MEM_POST)?
                        .ok_or(Box::new(CreepError::MissingPost(format!("{}", creep.id()))))?,
                    creep,
                    harvesting: mem.bool(MEM_HARVESTING),
                    target: None,
                }),
                k if k == CREEP_ID_FARMER => {
                    let assigned_source = ObjectId::from(RawObjectId::from_hex_string(
                        &mem.string(MEM_ASSIGNED_SOURCE)?.ok_or(Box::new(
                            CreepError::MissingAssignedSource(format!("{}", creep.id())),
                        ))?,
                    )?);
                    let room = creep.room().ok_or(Box::new(CreepError::RoomNotFound()))?;
                    CreepKind::Farmer(CreepFarmer {
                        my_room,
                        id: creep.id(),
                        post: mem
                            .string(MEM_POST)?
                            .ok_or(Box::new(CreepError::MissingPost(format!("{}", creep.id()))))?,
                        creep: creep.clone(),
                        assigned_source,
                        farm_position: FarmPosition::from_basic(
                            mem.i32(MEM_FARM_POSITION_X)?.ok_or(Box::new(
                                CreepError::MissingFarmPosition(format!("{}", creep.id())),
                            ))? as u32,
                            mem.i32(MEM_FARM_POSITION_Y)?.ok_or(Box::new(
                                CreepError::MissingFarmPosition(format!("{}", creep.id())),
                            ))? as u32,
                            assigned_source,
                            room,
                        ),
                    })
                }
                k if k == CREEP_ID_RUNNER => {
                    CreepKind::Runner(CreepRunner {
                        my_room,
                        id: creep.id(),
                        post: mem
                            .string(MEM_POST)?
                            .ok_or(Box::new(CreepError::MissingPost(format!("{}", creep.id()))))?,
                        creep: creep.clone(),
                        state: None,
                    })
                }
                k if k == CREEP_ID_UNKNOWN => CreepKind::Unknown(CreepUnknown {
                    creep: creep.clone(),
                }),
                _ => CreepKind::Unknown(CreepUnknown {
                    creep: creep.clone(),
                }),
            })
        } else {
            Err(Box::new(CreepError::CreepNotConvertible()))
        }
    }
}

pub trait Creep {}

#[derive(Clone)]
pub struct CreepBitch {
    pub id: ObjectId<screeps::objects::Creep>,
    /// Identifier for the creep behaviour to link it to the RoomSettings
    pub post: String,
    pub my_room: MyRoom,
    creep: screeps::Creep,
}

impl fmt::Debug for CreepBitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreepBitch")
            .field("id", &self.id)
            .field("post", &self.post)
            .field("my_room", &self.my_room)
            .finish()
    }
}

impl CreepBitch {
    // pub fn memory_for_spawn(post: String) -> MemoryReference {
    //     let memory = MemoryReference::new();
    //     memory.set(MEM_POST, post.clone());
    //     memory.set(MEM_KIND, CREEP_ID_BITCH);
    //     memory.set(MEM_HARVESTING, false);
    //     memory
    // }
    //
    // pub fn name_prefix() -> String {
    //     CREEP_ID_BITCH.into()
    // }
    //
    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        if self.creep.memory().bool(MEM_HARVESTING) {
            if self.creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
                self.creep.memory().set(MEM_HARVESTING, false);
                self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
            }
        } else {
            self.creep.say("ᕕ( ᐛ )ᕗ", true);
            if self.creep.store_used_capacity(None) == 0 {
                let context = BWContext::get();
                let state = context.state()?;
                if let Some(fetch_target) = self.calc_next_fetch(&state.room_states)? {
                    self.creep.memory().set(MEM_HARVESTING, true);
                    self.creep
                        .memory()
                        .set(MEM_RESOURCE_PROVIDER_ID, fetch_target.0.ident());
                }
            }
        }

        if self.creep.memory().bool(MEM_HARVESTING) {
            let context = BWContext::get();
            let state = context.state()?;
            let resource_provider_id = self
                .creep
                .memory()
                .string(MEM_RESOURCE_PROVIDER_ID)?
                .ok_or(Box::new(CreepError::ResourceProviderIdNotStored))?;
            let resource_provider = state
                .room_states
                .get(&self.my_room.room()?.name())
                .map(|room_state| room_state.resource_provider(&resource_provider_id));
            if let Some(Some(resource_provider)) = resource_provider {
                if self.creep.pos().is_near_to(&resource_provider.pos()?) {
                    let res = resource_provider.creep_get_resource(
                        &self.creep,
                        ResourceType::Energy,
                        self.creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                    );
                    match res {
                        Ok(TakeResourceResult::Withdraw {
                            tried_amount: 0, ..
                        }) => {
                            info!("Got 0 amount while withdrawing, resetting...");
                            self.creep.memory().set(MEM_HARVESTING, false);
                            self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
                        }
                        Ok(TakeResourceResult::Withdraw {
                            return_code: ReturnCode::NotEnough,
                            ..
                        }) => {
                            info!("Return code NotEnough while Withdrawing, resetting...");
                            self.creep.memory().set(MEM_HARVESTING, false);
                            self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
                        }
                        Ok(TakeResourceResult::Withdraw {
                            return_code: ReturnCode::Ok,
                            ..
                        }) => {}
                        Ok(TakeResourceResult::Pickup {
                            return_code: ReturnCode::Ok,
                        }) => {}
                        Ok(TakeResourceResult::Harvest {
                            return_code: ReturnCode::Ok,
                        }) => {}
                        Ok(res) => {
                            warn!("Unhandled TakeResoult {:?}", res);
                        }
                        Err(err) => {
                            warn!(
                                "Error getting resource: {}. Resetting resource_provider",
                                err
                            );
                            self.creep.memory().set(MEM_HARVESTING, false);
                            self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
                        }
                    };
                } else {
                    self.creep.move_to_with_options(&resource_provider.pos()?, MoveToOptions::new().ignore_creeps(true));
                }
            } else {
                warn!("Room provider missing, resetting Bitch {}", self.creep.id());
                self.creep.memory().set(MEM_HARVESTING, false);
                self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
            }
        } else {
            if let Some(c) = self
                .creep
                .room()
                .expect("room is not visible to you")
                .controller()
            {
                let r = self.creep.upgrade_controller(&c);
                if r == ReturnCode::NotInRange {
                    self.creep.move_to(&c);
                } else if r != ReturnCode::Ok {
                    warn!("couldn't upgrade: {:?}", r);
                }
            } else {
                warn!("creep room has no controller!");
            }
        }
        Ok(())
    }
}

impl HandlesResource for CreepBitch {
    fn calc_next_fetch<'a>(
        &mut self,
        rooms_state: &'a HashMap<RoomName, RoomState>,
    ) -> Result<Option<(&'a ResourceProvider, ResourceType, u32)>, Box<dyn Error>> {
        let room = self.my_room.room()?;
        let room_state = rooms_state
            .get(&room.name())
            .ok_or_else(|| Box::new(CreepError::RoomNotFound()))?;
        let amount = self.creep.store_free_capacity(Some(ResourceType::Energy));
        match room_state {
            RoomState::Base(room_state) => {
                let working_providers: Vec<&ResourceProvider> = room_state
                    .resource_providers
                    .iter()
                    .filter_map(|(_id, p)| match p.creep_can_use(&self.creep) {
                        Ok(true) => Some(p),
                        Ok(false) => None,
                        Err(err) => {
                            warn!("Could not check for `creep_can_use`: {}", err);
                            None
                        }
                    })
                    .collect();
                let prioed = generic_creep_fetch_from_provider_prio(
                    &room,
                    self.creep.pos(),
                    working_providers,
                )?;
                match prioed {
                    Some(prov) => Ok(Some((prov, ResourceType::Energy, amount as u32))),
                    None => Ok(None),
                }
            }
            RoomState::SetupBase(room_state) => {
                if let SetupBaseStateVisibility::Visible{ref resource_providers, ..} = room_state.state {
                    let working_providers: Vec<&ResourceProvider> = resource_providers
                        .iter()
                        .filter_map(|(_id, p)| match p.creep_can_use(&self.creep) {
                            Ok(true) => Some(p),
                            Ok(false) => None,
                            Err(err) => {
                                warn!("Could not check for `creep_can_use`: {}", err);
                                None
                            }
                        })
                        .collect();
                    let prioed = generic_creep_fetch_from_provider_prio(
                        &room,
                        self.creep.pos(),
                        working_providers,
                    )?;
                    match prioed {
                        Some(prov) => Ok(Some((prov, ResourceType::Energy, amount as u32))),
                        None => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            },
        }
    }
}

fn generic_creep_fetch_from_provider_prio<'a>(
    room: &Room,
    creep_pos: Position,
    working_providers: Vec<&'a ResourceProvider>,
) -> anyhow::Result<Option<&'a ResourceProvider>> {
    let mut sorted = working_providers.clone();
    sorted.sort_by_cached_key(|a| {
        Reverse(generic_working_providers_points(room, a, &creep_pos)
            .unwrap_or(Some(-10000))
            .unwrap_or(-10000))
    });
    // sorted.sort_by(|a, b| {
    //     let a_p = generic_working_providers_points(room, a, &creep_pos)
    //         .unwrap_or(Some(-10000))
    //         .unwrap_or(-10000);
    //     let b_p = generic_working_providers_points(room, b, &creep_pos)
    //         .unwrap_or(Some(-10000))
    //         .unwrap_or(-10000);
    //     a_p.cmp(&b_p).reverse()
    // });
    Ok(sorted.first().map(|s| *s))
}

// TODO needs to know the resource type!
fn generic_working_providers_points(
    room: &Room,
    prov: &ResourceProvider,
    for_pos: &Position,
) -> Result<Option<i32>, Box<dyn Error>> {
    let mut points: i32 = 0;
    match prov {
        ResourceProvider::EnergyFarm { resource_farm_data } => {
            points += 100;
            // if let Some(source) = get_object_typed(resource_farm_data.obj_id)? {
            //     points += (source.energy() as f32 / 1000.).ceil() as i32;
            // }
            let path = resource_farm_data
                .pos()?
                .find_path_to(for_pos, FindOptions::default());
            let vec_path = match path {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            };
            points -= vec_path.len() as i32;
        }
        ResourceProvider::SourceDump { room_object_data } => {
            points += 200;
            // TODO Doesnt check which type of resoure yet
            let resource_amount = match room_object_data {
                RoomObjectData::StorageStructure { obj_id } => {
                    let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                        Box::new(CreepError::ObjectNotFound(format!("{}", *obj_id)))
                    })?;
                    obj.as_has_store()
                        .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                        .unwrap_or(0)
                }
                RoomObjectData::Litter { obj_id } => {
                    let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                        Box::new(CreepError::ObjectNotFound(format!("{}", *obj_id)))
                    })?;
                    obj.amount()
                }
            };
            // Poor man's curve
            if resource_amount == 0 {
                points = 0;
            } else if resource_amount < 100 {
                points -= resource_amount as i32;
            } else if resource_amount < 500 {
                points -= (resource_amount as f32 / 5.).round() as i32;
            } else {
                points += (resource_amount as f32 / 100.).round() as i32;
            }
            let path = room_object_data
                .pos()?
                .find_path_to(for_pos, FindOptions::default());
            let vec_path = match path {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            };
            points -= vec_path.len() as i32 * 3;
        }
        ResourceProvider::BufferControllerUpgrade { room_object_data } => {
            points += 200;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?.ok_or_else(|| {
                Box::new(CreepError::ObjectNotFound(format!(
                    "{}",
                    room_object_data.obj_id
                )))
            })?;
            let resource_amount = obj
                .as_has_store()
                .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                .unwrap_or(0);
            // Poor man's curve
            if resource_amount == 0 {
                points = 0;
            } else if resource_amount < 100 {
                points -= resource_amount as i32;
            } else if resource_amount < 500 {
                points -= 50 - (resource_amount as f32 / 5.).round() as i32;
            } else {
                points += (resource_amount as f32 / 100.).round() as i32;
            }
            let path = room_object_data
                .pos()?
                .find_path_to(for_pos, FindOptions::default());
            let vec_path = match path {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            };
            points -= vec_path.len() as i32 * 3;
        }
        ResourceProvider::LongTermStorage { room_object_data } => {
            points += 200;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?.ok_or_else(|| {
                Box::new(CreepError::ObjectNotFound(format!(
                    "{}",
                    room_object_data.obj_id
                )))
            })?;
            let resource_amount = obj
                .as_has_store()
                .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                .unwrap_or(0);
            if resource_amount < 20000 {
                // Ensure minimum of energy
                points = 1;
            }
            let path = room_object_data
                .pos()?
                .find_path_to(for_pos, FindOptions::default());
            let vec_path = match path {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            };
            points -= vec_path.len() as i32 * 3;
        }
        ResourceProvider::TerminalOverflow { room_object_data } => {
            points += 150;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?.ok_or_else(|| {
                Box::new(CreepError::ObjectNotFound(format!(
                    "{}",
                    room_object_data.obj_id
                )))
            })?;
            let resource_amount = obj
                .as_has_store()
                .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                .unwrap_or(0);
            let overflow_resource_amount = resource_amount as i32 - TERMINAL_TRADE_BUFFER as i32;
            if overflow_resource_amount < 0 {
                // Ensure minimum of energy
                points = -100;
            } else if overflow_resource_amount > 1000 {
                points += cmp::max((overflow_resource_amount as f32 / 10000.).round() as i32, 5);
            }
            let path = room_object_data
                .pos()?
                .find_path_to(for_pos, FindOptions::default());
            let vec_path = match path {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            };
            points -= vec_path.len() as i32 * 3;
        }
        _ => return Ok(None),
    };
    return Ok(Some(points));
}

#[derive(Clone)]
pub struct CreepBuilder {
    pub id: ObjectId<screeps::objects::Creep>,
    /// Identifier for the creep behaviour to link it to the RoomSettings
    pub post: String,
    pub my_room: MyRoom,
    pub harvesting: bool,
    // pub settings_ref:
    creep: screeps::Creep,
    target: Option<CreepBuilderTarget>,
}

impl fmt::Debug for CreepBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreepBuilder")
            .field("id", &self.id)
            .field("post", &self.post)
            .field("my_room", &self.my_room)
            .field("my_room", &self.my_room)
            .field("target", &self.target)
            .finish()
    }
}

impl HandlesResource for CreepBuilder {
    fn calc_next_fetch<'a>(
        &mut self,
        rooms_state: &'a HashMap<RoomName, RoomState>,
    ) -> Result<Option<(&'a ResourceProvider, ResourceType, u32)>, Box<dyn Error>> {
        let room = self.my_room.room()?;
        let room_state = rooms_state
            .get(&room.name())
            .ok_or_else(|| Box::new(CreepError::RoomNotFound()))?;
        let amount = self.creep.store_free_capacity(Some(ResourceType::Energy));
        match room_state {
            RoomState::Base(room_state) => {
                let working_providers: Vec<&ResourceProvider> = room_state
                    .resource_providers
                    .iter()
                    .filter_map(|(_id, p)| match p.creep_can_use(&self.creep) {
                        Ok(true) => Some(p),
                        Ok(false) => None,
                        Err(err) => {
                            warn!("Could not check for `creep_can_use`: {}", err);
                            None
                        }
                    })
                    .collect();
                let prioed = generic_creep_fetch_from_provider_prio(
                    &room,
                    self.creep.pos(),
                    working_providers,
                )?;
                match prioed {
                    Some(prov) => Ok(Some((prov, ResourceType::Energy, amount as u32))),
                    None => Ok(None),
                }
            }
            RoomState::SetupBase(room_state) => {
                if let SetupBaseStateVisibility::Visible{ref resource_providers, ..} = room_state.state {
                    let working_providers: Vec<&ResourceProvider> = resource_providers
                        .iter()
                        .filter_map(|(_id, p)| match p.creep_can_use(&self.creep) {
                            Ok(true) => Some(p),
                            Ok(false) => None,
                            Err(err) => {
                                warn!("Could not check for `creep_can_use`: {}", err);
                                None
                            }
                        })
                        .collect();
                    let prioed = generic_creep_fetch_from_provider_prio(
                        &room,
                        self.creep.pos(),
                        working_providers,
                    )?;
                    match prioed {
                        Some(prov) => Ok(Some((prov, ResourceType::Energy, amount as u32))),
                        None => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            },
        }
    }
}

#[derive(Clone, Debug)]
enum CreepBuilderTarget {
    Build(ObjectId<ConstructionSite>),
    Repair(ObjectId<Structure>),
}

impl CreepBuilder {
    pub fn memory_for_spawn(post: String) -> MemoryReference {
        let memory = MemoryReference::new();
        memory.set(MEM_POST, post.clone());
        memory.set(MEM_KIND, CREEP_ID_BUILDER);
        memory.set(MEM_HARVESTING, false);
        memory
    }

    pub fn name_prefix() -> String {
        CREEP_ID_BUILDER.into()
    }

    fn set_getting_resource(&mut self, resource_provider: Option<&ResourceProvider>) {
        self.creep
            .memory()
            .set(MEM_HARVESTING, resource_provider.is_some());
        self.harvesting = resource_provider.is_some();
        if let Some(resource_provider) = resource_provider {
            self.creep
                .memory()
                .set(MEM_RESOURCE_PROVIDER_ID, resource_provider.ident());
        } else {
            self.creep.memory().del(MEM_RESOURCE_PROVIDER_ID);
        }
    }

    fn set_target(&mut self, target: Option<CreepBuilderTarget>) {
        // TODO Serialization of RawObjectId
        // self.creep.memory().set(MEM_BUILD_TARGET, harvesting);
        self.target = target;
    }

    pub fn harvest_check(&mut self) -> Result<(), Box<dyn Error>> {
        if self.harvesting {
            if self.creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
                self.set_getting_resource(None);
            }
        } else {
            self.creep.say("ᕕ( ᐛ )ᕗ", true);
            if self.creep.store_used_capacity(None) == 0 {
                let context = BWContext::get();
                let state = context.state()?;
                if let Some(fetch_target) = self.calc_next_fetch(&state.room_states)? {
                    self.set_getting_resource(Some(fetch_target.0));
                }
            }
        }
        Ok(())
    }

    pub fn harvest(&mut self) -> Result<(), Box<dyn Error>> {
        let context = BWContext::get();
        let state = context.state()?;
        let resource_provider_id = self.creep.memory().string(MEM_RESOURCE_PROVIDER_ID)?;
        let resource_provider_id = match resource_provider_id {
            Some(id) => id,
            None => {
                warn!(
                    "Room provider not found for id, resetting Builder {}",
                    self.creep.id()
                );
                self.set_getting_resource(None);
                return Err(Box::new(CreepError::ResourceProviderIdNotStored));
            }
        };
        let resource_provider = state
            .room_states
            .get(&self.my_room.room()?.name())
            .map(|room_state| room_state.resource_provider(&resource_provider_id));
        if let Some(Some(resource_provider)) = resource_provider {
            if self.creep.pos().is_near_to(&resource_provider.pos()?) {
                let res = resource_provider.creep_get_resource(
                    &self.creep,
                    ResourceType::Energy,
                    self.creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                );
                match res {
                    Ok(TakeResourceResult::Withdraw {
                        tried_amount: 0, ..
                    }) => {
                        info!("Got 0 amount while withdrawing, resetting...");
                        self.set_getting_resource(None);
                    }
                    Ok(TakeResourceResult::Withdraw {
                        return_code: ReturnCode::NotEnough,
                        ..
                    }) => {
                        info!("Return code NotEnough while Withdrawing, resetting...");
                        self.set_getting_resource(None);
                    }
                    Ok(TakeResourceResult::Withdraw {
                        return_code: ReturnCode::Ok,
                        ..
                    }) => {}
                    Ok(TakeResourceResult::Pickup {
                        return_code: ReturnCode::Ok,
                    }) => {}
                    Ok(TakeResourceResult::Harvest {
                        return_code: ReturnCode::Ok,
                    }) => {}
                    Ok(res) => {
                        warn!("Unhandled TakeResoult {:?}", res);
                    }
                    Err(err) => {
                        warn!(
                            "Error getting resource: {}. Resetting resource_provider",
                            err
                        );
                        self.set_getting_resource(None);
                    }
                };
            } else {
                self.creep.move_to(&resource_provider.pos()?);
            }
        } else {
            warn!(
                "Room provider missing, resetting Builder {}",
                self.creep.id()
            );
            self.set_getting_resource(None);
        }
        Ok(())
    }

    pub fn build(&mut self) -> Result<(), Box<dyn Error>> {
        let room = &self
            .creep
            .room()
            .ok_or(Box::new(CreepError::RoomNotFound()))?;

        // Precursory checks
        match &self.target {
            Some(_target) => {}
            None => {
                // Get new target
                let context = BWContext::get();
                let state = context.state()?;
                let room_settings = state
                    .room_settings
                    .get(&self.my_room)
                    .ok_or(Box::new(CreepError::RoomNotFound()))?;

                match (
                    get_prio_repair_target(room)?,
                    room_settings.maintenance.priority_item()?,
                ) {
                    // TODO Use `RoomMaintenance also for repairs
                    (Some(RepairTarget::Important { target }), _) => {
                        self.set_target(Some(CreepBuilderTarget::Repair(target.id().into())));
                    }
                    (Some(RepairTarget::Arbeitsbeschaffung { .. }), Some(item)) => {
                        match item {
                            RoomMaintenance::NewBuild { object_id } => {
                                self.set_target(Some(CreepBuilderTarget::Build(
                                    object_id.to_owned(),
                                )));
                            }
                            RoomMaintenance::Repair { object_id } => {
                                // TODO Better way of getting an ObjectId<Structure> from the
                                //   `RoomMaintenance` object
                                let structure =
                                    get_object_typed::<Structure>(object_id.to_owned().into());
                                if let Ok(Some(structure)) = structure {
                                    self.set_target(Some(CreepBuilderTarget::Repair(
                                        structure.id(),
                                    )));
                                } else {
                                    warn!("Unknown repair `object_id` {:?}", object_id);
                                }
                            }
                        }
                    }
                    (Some(RepairTarget::Arbeitsbeschaffung { target }), None) => {
                        self.set_target(Some(CreepBuilderTarget::Repair(target.id().into())));
                    }
                    (None, Some(item)) => {
                        match item {
                            RoomMaintenance::NewBuild { object_id } => {
                                self.set_target(Some(CreepBuilderTarget::Build(
                                    object_id.to_owned(),
                                )));
                            }
                            RoomMaintenance::Repair { object_id } => {
                                // TODO Better way of getting an ObjectId<Structure> from the
                                //   `RoomMaintenance` object
                                let structure =
                                    get_object_typed::<Structure>(object_id.to_owned().into());
                                if let Ok(Some(structure)) = structure {
                                    self.set_target(Some(CreepBuilderTarget::Repair(
                                        structure.id(),
                                    )));
                                } else {
                                    warn!("Unknown repair `object_id` {:?}", object_id);
                                }
                            }
                        }
                    }
                    _ => {}
                };
            }
        }

        // Move & build/repair
        if let Some(target) = &self.target {
            match target {
                CreepBuilderTarget::Build(build_target) => {
                    let object = get_object_typed(build_target.to_owned().into());
                    match object {
                        Ok(Some(target)) => {
                            if self.creep.pos().is_near_to(&target) {
                                let r = self.creep.build(&target);

                                if r != ReturnCode::Ok {
                                    warn!("couldn't build: {:?}", r);
                                    self.set_target(None);
                                }
                            } else {
                                self.creep.move_to(&target);
                            }
                        }
                        Ok(None) => {
                            warn!("Build target missing");
                            self.set_target(None);
                        }
                        Err(_) => {
                            // Object not with the expected type
                            warn!("Build target unexpected type");
                            self.set_target(None);
                        }
                    }
                }
                CreepBuilderTarget::Repair(repair_target) => {
                    let object = get_object_typed(repair_target.to_owned().into())?;
                    match object {
                        Some(target) => {
                            if let Some(attackable_target) = target.as_attackable() {
                                if self.creep.pos().in_range_to(&target, 3) {
                                    let r = self.creep.repair(&target);

                                    if r != ReturnCode::Ok {
                                        warn!("couldn't repair: {:?}", r);
                                        self.set_target(None);
                                    }
                                } else {
                                    self.creep.move_to(&target);
                                }
                                if attackable_target.hits() == attackable_target.hits_max() {
                                    self.set_target(None);
                                }
                            } else {
                                Err(CreepError::RepairNotAttackable(format!("{}", target.id())))?;
                            }
                        }
                        None => {
                            warn!("Repair target missing");
                            self.set_target(None);
                        }
                    };
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum RepairTarget {
    Arbeitsbeschaffung { target: Structure },
    Important { target: Structure },
}

const HIGHER_NUM: f64 = 1_000_000_000_000.;
// const HIGHER_NUM: f32 = 10.;

pub fn get_prio_repair_target(room: &Room) -> Result<Option<RepairTarget>, Box<dyn Error>> {
    let mut repairable_structures: Vec<Structure> = room
        .find(find::STRUCTURES)
        .into_iter()
        .filter(|struc| match struc {
            Structure::Road(road) => road.hits() < (road.hits_max() as f32 * 0.5).round() as u32,
            Structure::Container(container) => {
                container.hits() < (container.hits_max() as f32 * 0.7).round() as u32
            }
            Structure::Wall(_) => true,
            Structure::Rampart(_) => true,
            _ => false,
        })
        .collect();
    repairable_structures.sort_by_cached_key(|a| {
        -get_structure_prio_val(a)
    });
    Ok(repairable_structures.first().map(|s| {
        if get_structure_prio_val(s) < HIGHER_NUM as i64 + 10 {
            RepairTarget::Arbeitsbeschaffung { target: s.clone() }
        } else {
            RepairTarget::Important { target: s.clone() }
        }
    }))
}

const TARGET_WALLING: f64 = 10_000_000.;

fn get_structure_prio_val(structure: &Structure) -> i64 {
    match structure {
        Structure::Road(road) => (HIGHER_NUM
            + ((1. - road.hits() as f64 / road.hits_max() as f64) * 100.))
            .round() as i64,
        Structure::Container(container) => (HIGHER_NUM
            + ((1. - container.hits() as f64 / container.hits_max() as f64) * 100.))
            .round() as i64,
        Structure::Rampart(rampart) => (HIGHER_NUM
            + ((1. - rampart.hits() as f64 / TARGET_WALLING as f64) * 1.01))
            .round() as i64,
        Structure::Wall(wall) => {
            (HIGHER_NUM * ((1. - wall.hits() as f64 / TARGET_WALLING) * 1.)).round() as i64
        }
        _ => -1,
    }
}

#[derive(Clone)]
pub struct CreepFarmer {
    pub id: ObjectId<screeps::objects::Creep>,
    /// Identifier for the creep behaviour to link it to the RoomSettings
    pub post: String,
    pub my_room: MyRoom,
    creep: screeps::Creep,
    assigned_source: ObjectId<Source>,
    farm_position: FarmPosition,
}

impl fmt::Debug for CreepFarmer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreepFarmer")
            .field("id", &self.id)
            .field("post", &self.post)
            .field("my_room", &self.my_room)
            .field("assigned_source", &self.assigned_source)
            .field("farm_position", &self.farm_position)
            .finish()
    }
}

impl CreepFarmer {
    pub fn memory_for_spawn(post: String, farm_position: &FarmPosition) -> MemoryReference {
        let memory = MemoryReference::new();
        memory.set(MEM_POST, post.clone());
        memory.set(MEM_KIND, CREEP_ID_FARMER);
        memory.set(MEM_FARM_POSITION_X, farm_position.position().x());
        memory.set(MEM_FARM_POSITION_Y, farm_position.position().y());
        memory.set(
            MEM_ASSIGNED_SOURCE,
            RawObjectId::from(farm_position.for_source()).to_hex_string(),
        );
        memory
    }

    pub fn name_prefix() -> String {
        CREEP_ID_FARMER.into()
    }
    //
    // fn set_assigned_source(&mut self, assigned_source: ObjectId<Source>) {
    //     // TODO Serialization of RawObjectId
    //     // self.creep.memory().set(MEM_assigned_source, harvesting);
    //     self.assigned_source = assigned_source;
    // }

    pub fn harvest(&mut self) -> Result<(), Box<dyn Error>> {
        let source = get_object_typed(self.assigned_source)?.ok_or_else(|| {
            Box::new(CreepError::SourceNotFound(format!(
                "{}",
                self.assigned_source
            )))
        })?;
        let target_pos = self.farm_position.position();
        if self.creep.pos() == target_pos {
            let r = self.creep.harvest(&source);
            if r != ReturnCode::Ok {
                warn!("couldn't harvest: {:?}", r);
            }
        } else {
            self.creep.move_to(&target_pos);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct CreepRunner {
    pub id: ObjectId<screeps::objects::Creep>,
    /// Identifier for the creep behaviour to link it to the RoomSettings
    pub post: String,
    pub my_room: MyRoom,
    pub state: Option<CreepRunnerState>,
    creep: screeps::Creep,
}

impl fmt::Debug for CreepRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreepRunner")
            .field("id", &self.id)
            .field("post", &self.post)
            .field("my_room", &self.my_room)
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum CreepRunnerState {
    Fetching {
        from: CreepRunnerFetchTarget,
        to: CreepRunnerDeliverTarget,
    },
    Delivering {
        to: CreepRunnerDeliverTarget,
        provided: u32,
    },
}

impl CreepRunner {
    pub fn memory_for_spawn(post: String) -> MemoryReference {
        let memory = MemoryReference::new();
        memory.set(MEM_POST, post.clone());
        memory.set(MEM_KIND, CREEP_ID_RUNNER);
        memory
    }

    pub fn name_prefix() -> String {
        CREEP_ID_RUNNER.into()
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let room = self.my_room.room()?;
        if let Some(state) = &self.state {
            match state {
                CreepRunnerState::Fetching { to, .. } => {
                    if self.creep.store_free_capacity(Some(ResourceType::Energy)) == 0
                        || self.creep.store_used_capacity(Some(ResourceType::Energy))
                            >= to.requested()
                    {
                        warn!("to deliver");
                        self.state = Some(CreepRunnerState::Delivering {
                            to: to.clone(),
                            provided: 0,
                        });
                    }
                }
                CreepRunnerState::Delivering { to, provided } => {
                    if self.creep.store_used_capacity(Some(ResourceType::Energy)) == 0
                        || *provided >= to.requested()
                    {
                        warn!("deliver to new");
                        self.new_run()?;
                    }
                }
            }
        } else {
            warn!("no state to new");
            self.new_run()?;
        }

        if let Some(state) = &mut self.state {
            match state {
                CreepRunnerState::Fetching { from, .. } => {
                    if self.creep.pos().is_near_to(&from.pos()) {
                        match from {
                            CreepRunnerFetchTarget::PermanentFarmerContainer { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    // to.requested(),
                                    self.creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                                    obj.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .withdraw_amount(&obj, ResourceType::Energy, amount);
                            }
                            CreepRunnerFetchTarget::Ruin { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    self.creep.store_free_capacity(Some(ResourceType::Energy))
                                        as u32,
                                    // HACK stupid if I fill one extension requesting 50 energy
                                    // cmp::min(
                                    //     to.requested(),
                                        obj.store_used_capacity(Some(ResourceType::Energy)),
                                    // ),
                                );
                                self.creep
                                    .withdraw_amount(&obj, ResourceType::Energy, amount);
                            }
                            CreepRunnerFetchTarget::DroppedSource { id, pos, .. } => {
                                let obj = get_object_typed(*id)?;
                                let farmer_container =
                                    room.look_for_at(look::STRUCTURES, pos);

                                if let Some(obj) = obj {
                                    info!("We have object, also container?");
                                    if obj.amount() < 200 && farmer_container.len() > 0 {
                                        self.creep.pickup(&obj);
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
                                                self.creep
                                                    .store_free_capacity(Some(ResourceType::Energy)),
                                                container
                                                    .store_used_capacity(Some(ResourceType::Energy)) as i32,
                                            ) - obj.amount() as i32;
                                            info!("Grabbing from Container: {} // Amount: {}", farmer_container.len(), container_amount);
                                            if container_amount > 0 {
                                                self.creep.withdraw_amount(
                                                    container,
                                                    ResourceType::Energy,
                                                    container_amount as u32,
                                                );
                                            }
                                        }
                                    } else {
                                        // NOTE Can't control how much I pick up with `pickup` ಠ_ಠ
                                        self.creep.pickup(&obj);
                                    }
                                } else {
                                    if farmer_container.len() > 0 {
                                        warn!("Dropped source not found, using container");
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
                                                self.creep
                                                    .store_free_capacity(Some(ResourceType::Energy)) as u32,
                                                container
                                                    .store_used_capacity(Some(ResourceType::Energy)),
                                            );
                                            self.creep.withdraw_amount(
                                                container,
                                                ResourceType::Energy,
                                                amount,
                                            );
                                        }
                                    } else {
                                        warn!("Dropped source not found, resetting Runner");
                                        self.new_run()?;
                                    }
                                }
                            }
                            CreepRunnerFetchTarget::Terminal { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    // to.requested(),
                                    self.creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                                    obj.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .withdraw_amount(&obj, ResourceType::Energy, amount);
                            }
                        }
                        // FIXME Hack
                        self.new_run()?;
                    } else {
                        self.creep.move_to(&from.pos());
                    }
                }
                CreepRunnerState::Delivering { to, provided } => {
                    if self.creep.pos().is_near_to(&to.pos()) {
                        match to {
                            CreepRunnerDeliverTarget::Tower { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                            CreepRunnerDeliverTarget::Extension { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                            CreepRunnerDeliverTarget::Spawn { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                            CreepRunnerDeliverTarget::PermanentUpgraderContainer { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                            CreepRunnerDeliverTarget::TempStorage { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("{}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                            CreepRunnerDeliverTarget::TradeTransactionFee { id, .. } => {
                                let obj = get_object_typed(*id)?.ok_or(Box::new(
                                    CreepError::ObjectNotFound(format!("terminal TradeTransactionFee {}", id)),
                                ))?;
                                let amount = cmp::min(
                                    to.requested(),
                                    self.creep.store_used_capacity(Some(ResourceType::Energy)),
                                );
                                self.creep
                                    .transfer_amount(&obj, ResourceType::Energy, amount);
                                *provided += amount;
                            }
                        }
                    } else {
                        self.creep.move_to(&to.pos());
                    }
                }
            }
        }
        Ok(())
    }

    pub fn new_run(&mut self) -> Result<(), Box<dyn Error>> {
        let room = self.my_room.room()?;
        let deliver_target = get_prio_deliver_target(&room, &self.creep)?;
        info!("del target {:?} in {}", deliver_target, room.name());
        if let Some(deliver_target) = deliver_target {
            if deliver_target.requested()
                <= self.creep.store_used_capacity(Some(ResourceType::Energy))
            {
                self.state = Some(CreepRunnerState::Delivering {
                    to: deliver_target,
                    provided: 0,
                });
            } else {
                let fetch_target = get_prio_fetch_target(&room, &deliver_target, &self.creep.pos())?;
                if let Some(fetch_target) = fetch_target {
                    self.state = Some(CreepRunnerState::Fetching {
                        from: fetch_target,
                        to: deliver_target,
                    });
                } else {
                    info!(
                        "Delivery requested, but no provider in room {}",
                        room.name()
                    );
                }
            }
            Ok(())
        } else {
            debug!("Nothing to do in room {}", room.name());
            Ok(())
        }
    }
}

/// Searches for something that provides the resources for the delivery_target
fn get_prio_fetch_target(
    room: &Room,
    _delivery_target: &CreepRunnerDeliverTarget,
    creep_pos: &Position,
) -> Result<Option<CreepRunnerFetchTarget>, Box<dyn Error>> {
    let controller = room.controller().ok_or(anyhow!("Controller not found"))?; 
    let mut containers: Vec<StructureContainer> = room
        .find(find::STRUCTURES)
        .into_iter()
        .filter_map(|s| match s {
            Structure::Container(container) => Some(container),
            _ => None,
        })
        .collect();
    // TODO Dummy implementation
    containers.sort_by_cached_key(|container| {
        let path_len = container.pos().find_path_to(creep_pos, FindOptions::default()).vectorized().unwrap_or(vec![]).len() as i32;
        -(container.store_used_capacity(Some(ResourceType::Energy)) as i32
            - path_len * 100)
    });
    let viable_containers: Vec<CreepRunnerFetchTarget> = containers
        .into_iter()
        // HACK controller check will be done differently
        .filter(|c| c.store_used_capacity(Some(ResourceType::Energy)) > 100 && !c.pos().in_range_to(&controller, 3))
        .map(|c| CreepRunnerFetchTarget::PermanentFarmerContainer {
            id: c.id(),
            pos: c.pos(),
            provides: c.store_used_capacity(Some(ResourceType::Energy)),
        })
        .collect();
    let mut dropped_resources: Vec<screeps::Resource> = room
        .find(find::DROPPED_RESOURCES)
        .into_iter()
        .filter(|res| res.resource_type() == ResourceType::Energy)
        .collect();
    dropped_resources.sort_by(|res_a, res_b| res_a.amount().cmp(&res_b.amount()).reverse());

    let viable_dropped_sources: Vec<CreepRunnerFetchTarget> = dropped_resources
        .into_iter()
        .map(|res| CreepRunnerFetchTarget::DroppedSource {
            id: res.id(),
            pos: res.pos(),
            provides: res.amount(),
        })
        .collect();

    let viable_ruins: Vec<CreepRunnerFetchTarget> = room
        .find(find::RUINS)
        .into_iter()
        .filter_map(|r| {
            let energy = r.store_used_capacity(Some(ResourceType::Energy));
            if energy == 0 {
                return None;
            }
            Some(CreepRunnerFetchTarget::Ruin {
                id: r.id(),
                pos: r.pos(),
                provides: energy,
            })
        })
        .collect();
    let terminal: Vec<CreepRunnerFetchTarget> = room
        .find(find::STRUCTURES)
        .into_iter()
        .filter_map(|s| match s {
            Structure::Terminal(terminal) => {
                if terminal.store_used_capacity(Some(ResourceType::Energy)) > TERMINAL_TRADE_BUFFER {
                    Some(CreepRunnerFetchTarget::Terminal {
                        id: terminal.id(),
                        pos: terminal.pos(),
                        provides: terminal.store_free_capacity(Some(ResourceType::Energy))
                            as u32 - TERMINAL_TRADE_BUFFER,
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    if viable_ruins.len() > 0 {
        Ok(viable_ruins.first().and_then(|c| Some(c.clone())))
    } else if viable_dropped_sources.len() > 0 {
        Ok(viable_dropped_sources.first().and_then(|c| Some(c.clone())))
    } else if viable_containers.len() > 0 {
        Ok(viable_containers.first().and_then(|c| Some(c.clone())))
    } else {
        Ok(terminal.first().and_then(|c| Some(c.clone())))
    }
}

fn get_prio_deliver_target(
    room: &Room,
    creep: &screeps::Creep,
) -> Result<Option<CreepRunnerDeliverTarget>, Box<dyn Error>> {
    // TODO Dummy implementation
    let structures = room.find(find::STRUCTURES);
    let mut extensions: Vec<&StructureExtension> = structures
        .iter()
        .filter_map(|s| match s {
            Structure::Extension(ext) => {
                if ext.store_free_capacity(Some(ResourceType::Energy)) > 0 {
                    Some(ext)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    extensions.sort_by_cached_key(|ext| {
        // let a_cap = ext
        //     .store_free_capacity(Some(ResourceType::Energy));
        // let b_cap = ext_b
        //     .store_free_capacity(Some(ResourceType::Energy));
        let range = ext.pos().find_path_to(creep, FindOptions::default());
        range.vectorized().unwrap().len() as i32
    });
    let viable_extensions: Vec<CreepRunnerDeliverTarget> = extensions
        .into_iter()
        .map(|ext| CreepRunnerDeliverTarget::Extension {
            id: ext.id(),
            pos: ext.pos(),
            requested: ext.store_free_capacity(Some(ResourceType::Energy)) as u32,
        })
        .collect();
    let mut spawns: Vec<&StructureSpawn> = structures
        .iter()
        .filter_map(|s| match s {
            Structure::Spawn(spawn) => {
                if spawn.store_free_capacity(Some(ResourceType::Energy)) > 0 {
                    Some(spawn)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    spawns.sort_by(|spawn_a, spawn_b| {
        spawn_a
            .store_free_capacity(Some(ResourceType::Energy))
            .cmp(&spawn_b.store_free_capacity(Some(ResourceType::Energy)))
            .reverse()
    });
    let viable_spawns: Vec<CreepRunnerDeliverTarget> = spawns
        .into_iter()
        .map(|spawn| CreepRunnerDeliverTarget::Spawn {
            id: spawn.id(),
            pos: spawn.pos(),
            requested: spawn.store_free_capacity(Some(ResourceType::Energy)) as u32,
        })
        .collect();

    let mut towers: Vec<&StructureTower> = structures
        .iter()
        .filter_map(|s| match s {
            Structure::Tower(tower) => {
                if tower.store_free_capacity(Some(ResourceType::Energy)) > 0 {
                    Some(tower)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    towers.sort_by(|tower_a, tower_b| {
        tower_a
            .store_free_capacity(Some(ResourceType::Energy))
            .cmp(&tower_b.store_free_capacity(Some(ResourceType::Energy)))
            .reverse()
    });
    let viable_towers: Vec<CreepRunnerDeliverTarget> = towers
        .into_iter()
        .map(|tower| CreepRunnerDeliverTarget::Tower {
            id: tower.id(),
            pos: tower.pos(),
            requested: tower.store_free_capacity(Some(ResourceType::Energy)) as u32,
        })
        .collect();
    let viable_containers = if let Some(controller) = room.controller() {
        let structures = room.look_for_around(look::STRUCTURES, controller.pos(), 3)?;
        structures
            .iter()
            .filter_map(|s| match s {
                Structure::Container(container) => {
                    if container.store_free_capacity(Some(ResourceType::Energy)) > 50 {
                        Some(CreepRunnerDeliverTarget::PermanentUpgraderContainer {
                            id: container.id(),
                            pos: container.pos(),
                            requested: container.store_free_capacity(Some(ResourceType::Energy))
                                as u32,
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect()
    } else {
        vec![]
    };
    let storage: Vec<CreepRunnerDeliverTarget> = structures
        .iter()
        .filter_map(|s| match s {
            Structure::Storage(storage) => {
                if storage.store_free_capacity(Some(ResourceType::Energy)) > 0 {
                    Some(CreepRunnerDeliverTarget::TempStorage {
                        id: storage.id(),
                        pos: storage.pos(),
                        requested: storage.store_free_capacity(Some(ResourceType::Energy))
                            as u32,
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    let terminal: Vec<CreepRunnerDeliverTarget> = structures
        .iter()
        .filter_map(|s| match s {
            Structure::Terminal(terminal) => {
                if terminal.store_used_capacity(Some(ResourceType::Energy)) < TERMINAL_TRADE_BUFFER {
                    Some(CreepRunnerDeliverTarget::TradeTransactionFee {
                        id: terminal.id(),
                        pos: terminal.pos(),
                        requested: terminal.store_free_capacity(Some(ResourceType::Energy))
                            as u32,
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    if viable_extensions.len() > 0 {
        Ok(viable_extensions.first().and_then(|c| Some(c.clone())))
    } else if viable_spawns.len() > 0 {
        Ok(viable_spawns.first().and_then(|c| Some(c.clone())))
    } else if viable_towers.len() > 0 {
        Ok(viable_towers.first().and_then(|c| Some(c.clone())))
    } else if viable_containers.len() > 0 {
        Ok(viable_containers.first().and_then(|c| Some(c.clone())))
    } else if terminal.len() > 0 {
        Ok(terminal.first().and_then(|c| Some(c.clone())))
    } else{
        Ok(storage.first().and_then(|c| Some(c.clone())))
    }
}

#[derive(Clone, Debug)]
pub enum CreepRunnerFetchTarget {
    PermanentFarmerContainer {
        id: ObjectId<StructureContainer>,
        pos: Position,
        provides: u32,
    },
    Ruin {
        id: ObjectId<Ruin>,
        pos: Position,
        provides: u32,
    },
    DroppedSource {
        id: ObjectId<Resource>,
        pos: Position,
        provides: u32,
    },
    Terminal {
        id: ObjectId<StructureTerminal>,
        pos: Position,
        provides: u32,
    },
}

impl CreepRunnerFetchTarget {
    fn pos(&self) -> Position {
        use CreepRunnerFetchTarget::*;
        match self {
            PermanentFarmerContainer { pos, .. } => *pos,
            Ruin { pos, .. } => *pos,
            DroppedSource { pos, .. } => *pos,
            Terminal { pos, .. } => *pos,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CreepRunnerDeliverTarget {
    Extension {
        id: ObjectId<StructureExtension>,
        pos: Position,
        requested: u32,
    },
    Tower {
        id: ObjectId<StructureTower>,
        pos: Position,
        requested: u32,
    },
    Spawn {
        id: ObjectId<StructureSpawn>,
        pos: Position,
        requested: u32,
    },
    PermanentUpgraderContainer {
        id: ObjectId<StructureContainer>,
        pos: Position,
        requested: u32,
    },
    TempStorage {
        id: ObjectId<StructureStorage>,
        pos: Position,
        requested: u32,
    },
    TradeTransactionFee {
        id: ObjectId<StructureTerminal>,
        pos: Position,
        requested: u32,
    },
    // TODO might make sense to differentiate the two, e.g. backup Storage
    //   should always be there in times of needs, TempStorage just for if
    //   nothing else accepts energy.
    // BackupStorage {
    //     id: ObjectId<StructureContainer>,
    //     pos: Position,
    //     requested: u32,
    // },
}

impl CreepRunnerDeliverTarget {
    pub fn pos(&self) -> Position {
        use CreepRunnerDeliverTarget::*;
        match self {
            Extension { pos, .. } => *pos,
            Tower { pos, .. } => *pos,
            Spawn { pos, .. } => *pos,
            PermanentUpgraderContainer { pos, .. } => *pos,
            TempStorage { pos, .. } => *pos,
            TradeTransactionFee { pos, .. } => *pos,
        }
    }

    fn requested(&self) -> u32 {
        use CreepRunnerDeliverTarget::*;
        match self {
            Extension { requested, .. } => *requested,
            Tower { requested, .. } => *requested,
            Spawn { requested, .. } => *requested,
            PermanentUpgraderContainer { requested, .. } => *requested,
            TempStorage { requested, .. } => *requested,
            TradeTransactionFee { requested, .. } => *requested,
        }
    }
}

#[derive(Clone)]
pub struct CreepUnknown {
    creep: screeps::Creep,
}

impl fmt::Debug for CreepUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreepUnknown").finish()
    }
}

#[derive(Clone)]
pub struct TrySpawnOptions<'a> {
    pub assumed_job: OokCreepJob,
    pub available_spawns: Vec<ObjectId<StructureSpawn>>,
    /// We really need the creep, allow to go way below `target_energy_usage`
    pub force_spawn: bool,
    pub race: OokRaceKind,
    pub spawn_room: &'a Room,
    pub target_energy_usage: u32,
    pub request_id: Option<UniqId>,
    pub preset_parts: Option<Vec<Part>>,
}

impl<'a> fmt::Debug for TrySpawnOptions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrySpawnOptions")
            .field("assumed_job", &self.assumed_job)
            .field("available_spawns", &self.available_spawns)
            .field("force_spawn", &self.force_spawn)
            .field("race", &self.race)
            .field("spawn_room", &self.spawn_room.name())
            .field("target_energy_usage", &self.target_energy_usage)
            .field("request_id", &self.request_id)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum TrySpawnResult {
    Spawned(TrySpawnResultData),
    ForceSpawned(TrySpawnResultData),
    Skipped,
}

#[derive(Debug, Clone)]
pub struct TrySpawnResultData {
    pub return_code: ReturnCode,
    pub used_energy_amount: u32,
    pub used_spawn: ObjectId<StructureSpawn>,
    pub creep_name: String,
}

#[derive(Debug, Clone)]
pub struct CalcSpawnBodyResult {
    pub amount: u32,
    pub body: Vec<creep::Part>,
}

pub trait Spawnable<O: fmt::Debug + Clone> {
    fn try_spawn(opts: &TrySpawnOptions, race_opts: &O) -> anyhow::Result<TrySpawnResult>;
    fn calc_spawn_body(opts: &TrySpawnOptions, race_opts: &O) -> anyhow::Result<CalcSpawnBodyResult>;
}

#[derive(Debug, Clone)]
struct MoveMatrix {
    road: u32,
    land: u32,
    swamp: u32,
}

#[derive(Debug, Clone)]
pub enum OokPresentCreep {
    Spawning(OokRace),
    Alive(OokRace),
    Dead(),
}

impl OokPresentCreep {
    pub fn alive(&self) -> Option<&OokRace> {
        use OokPresentCreep::*;
        match self {
            Alive(creep) => Some(creep),
            _ => None,
        }
    }
}

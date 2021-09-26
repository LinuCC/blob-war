use std::{cmp, error::Error};

use log::warn;
use screeps::{
    find, game::get_object_typed, look, HasId, HasPosition, HasStore, ObjectId, Position,
    RawObjectId, ResourceType, ReturnCode, Room, SharedCreepProperties, StructureProperties,
};

use super::room_ext::RoomExt;

#[derive(thiserror::Error, Debug)]
pub enum ResourceProviderError {
    #[error("[ResourceProviderError] Object not found {0}")]
    ObjectNotFound(String),
    #[error("[ResourceProviderError] Object has no store {0}")]
    ObjectNoStore(String),
    #[error("[ResourceProviderError] Object is not withdrawable {0}")]
    ObjectNoWithdrawable(String),
    #[error("[ResourceProviderError] ResourceType {0} requested, but was {1} for {2}")]
    ResourceTypeMismatch(screeps::ResourceType, screeps::ResourceType, String),
}

trait StructureWithStore: HasPosition + HasId + HasStore {}

pub trait ResourceData {
    fn pos(&self) -> anyhow::Result<Position>;
    fn provides(&self, resource_type: &screeps::ResourceType) -> Result<u32, Box<dyn Error>>;
    /// Checks if the creep can use this resource
    fn creep_can_use(&self, creep: &screeps::Creep) -> Result<bool, Box<dyn Error>>;
    fn creep_get_resource(
        &self,
        creep: &screeps::Creep,
        resource_type: ResourceType,
        ideal_amount: u32,
    ) -> anyhow::Result<TakeResourceResult>;
    // TODO
    // fn reserve(&mut self, type: ResourceType, amount: u32) -> Result<bool, Box<dyn Error>>;
}

#[derive(Clone, Debug)]
pub enum TakeResourceResult {
    Withdraw {
        return_code: ReturnCode,
        tried_amount: u32,
    },
    Harvest {
        return_code: ReturnCode,
    },
    Pickup {
        return_code: ReturnCode,
    },
}

// `T` is used for any structure with a store. If its Litter, its not used (Cuz `Resource` has no
// store, the fuck)
#[derive(Clone, Debug)]
pub enum ResourceProvider {
    /// The source itself, put some work in you creep! As long as there is no farmer there
    EnergyFarm {
        resource_farm_data: ResourceFarmData,
    },
    /// Mined source gets put here
    SourceDump { room_object_data: RoomObjectData },
    /// Container to upgrade the controller
    BufferControllerUpgrade { room_object_data: StructureData },
    /// Store or stuff like that
    LongTermStorage { room_object_data: StructureData },
    /// Overflow in Terminal
    TerminalOverflow { room_object_data: StructureData },
    /// Some source somewhere
    Unknown { room_object_data: RoomObjectData },
}

impl ResourceProvider {
    pub fn ident(&self) -> String {
        use ResourceProvider::*;
        let obj_id: RawObjectId = match self {
            EnergyFarm { resource_farm_data } => resource_farm_data.obj_id.into(),
            SourceDump { room_object_data } => room_object_data.obj_id(),
            BufferControllerUpgrade { room_object_data } => room_object_data.obj_id.into(),
            LongTermStorage { room_object_data } => room_object_data.obj_id.into(),
            TerminalOverflow { room_object_data } => room_object_data.obj_id.into(),
            Unknown { room_object_data } => room_object_data.obj_id(),
        };
        format!("{}", obj_id)
    }
}

impl ResourceData for ResourceProvider {
    fn pos(&self) -> anyhow::Result<Position> {
        use ResourceProvider::*;
        match self {
            EnergyFarm { resource_farm_data } => resource_farm_data.pos(),
            SourceDump { room_object_data } => room_object_data.pos(),
            BufferControllerUpgrade { room_object_data } => room_object_data.pos(),
            LongTermStorage { room_object_data } => room_object_data.pos(),
            TerminalOverflow { room_object_data } => room_object_data.pos(),
            Unknown { room_object_data } => room_object_data.pos(),
        }
    }

    fn provides(&self, resource_type: &screeps::ResourceType) -> Result<u32, Box<dyn Error>> {
        use ResourceProvider::*;
        match self {
            EnergyFarm { resource_farm_data } => resource_farm_data.provides(resource_type),
            SourceDump { room_object_data } => room_object_data.provides(resource_type),
            BufferControllerUpgrade { room_object_data } => {
                room_object_data.provides(resource_type)
            }
            LongTermStorage { room_object_data } => room_object_data.provides(resource_type),
            TerminalOverflow { room_object_data } => room_object_data.provides(resource_type),
            Unknown { room_object_data } => room_object_data.provides(resource_type),
        }
    }

    fn creep_can_use(&self, creep: &screeps::Creep) -> Result<bool, Box<dyn Error>> {
        use ResourceProvider::*;
        match self {
            EnergyFarm { resource_farm_data } => resource_farm_data.creep_can_use(creep),
            SourceDump { room_object_data } => room_object_data.creep_can_use(creep),
            BufferControllerUpgrade { room_object_data } => room_object_data.creep_can_use(creep),
            LongTermStorage { room_object_data } => room_object_data.creep_can_use(creep),
            TerminalOverflow { room_object_data } => room_object_data.creep_can_use(creep),
            Unknown { room_object_data } => room_object_data.creep_can_use(creep),
        }
    }

    fn creep_get_resource(
        &self,
        creep: &screeps::Creep,
        resource_type: ResourceType,
        ideal_amount: u32,
    ) -> anyhow::Result<TakeResourceResult> {
        use ResourceProvider::*;
        match self {
            EnergyFarm { resource_farm_data } => {
                resource_farm_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
            SourceDump { room_object_data } => {
                room_object_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
            BufferControllerUpgrade { room_object_data } => {
                room_object_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
            LongTermStorage { room_object_data } => {
                room_object_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
            TerminalOverflow { room_object_data } => {
                room_object_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
            Unknown { room_object_data } => {
                room_object_data.creep_get_resource(creep, resource_type, ideal_amount)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceFarmData {
    pub obj_id: ObjectId<screeps::Source>,
}

impl ResourceData for ResourceFarmData {
    fn pos(&self) -> anyhow::Result<Position> {
        let obj = get_object_typed(self.obj_id)?.ok_or_else(|| {
            Box::new(ResourceProviderError::ObjectNotFound(format!(
                "pos: {}",
                self.obj_id
            )))
        })?;
        Ok(obj.pos())
    }

    fn provides(&self, resource_type: &screeps::ResourceType) -> Result<u32, Box<dyn Error>> {
        if *resource_type != ResourceType::Energy {
            return Err(Box::new(ResourceProviderError::ResourceTypeMismatch(
                ResourceType::Energy,
                *resource_type,
                format!("{}", self.obj_id),
            )));
        }
        let obj = get_object_typed(self.obj_id)?.ok_or_else(|| {
            Box::new(ResourceProviderError::ObjectNotFound(format!(
                "prov: {}",
                self.obj_id
            )))
        })?;
        Ok(obj.energy())
    }

    fn creep_can_use(&self, creep: &screeps::Creep) -> Result<bool, Box<dyn Error>> {
        Ok(creep.get_active_bodyparts(screeps::Part::Work) > 0)
    }

    fn creep_get_resource(
        &self,
        creep: &screeps::Creep,
        resource_type: ResourceType,
        ideal_amount: u32,
    ) -> anyhow::Result<TakeResourceResult> {
        let obj = get_object_typed(self.obj_id)?;
        if let Some(obj) = obj {
            if resource_type == ResourceType::Energy {
                Ok(TakeResourceResult::Harvest {
                    return_code: creep.harvest(&obj),
                })
            } else {
                Err(anyhow::Error::from(
                    ResourceProviderError::ResourceTypeMismatch(
                        ResourceType::Energy,
                        resource_type,
                        format!("{}", self.obj_id),
                    ),
                ))
            }
        } else {
            Err(anyhow::Error::from(ResourceProviderError::ObjectNotFound(
                format!("get_res: {}", self.obj_id),
            )))
        }
    }
}

#[derive(Clone, Debug)]
pub struct StructureData {
    pub obj_id: ObjectId<screeps::Structure>,
}

impl ResourceData for StructureData {
    fn pos(&self) -> anyhow::Result<Position> {
        let obj = get_object_typed(self.obj_id)?.ok_or_else(|| {
            Box::new(ResourceProviderError::ObjectNotFound(format!(
                "pos2: {}",
                self.obj_id
            )))
        })?;
        Ok(obj.pos())
    }

    fn provides(&self, resource_type: &screeps::ResourceType) -> Result<u32, Box<dyn Error>> {
        let obj = get_object_typed(self.obj_id)?.ok_or_else(|| {
            Box::new(ResourceProviderError::ObjectNotFound(format!(
                "prov2: {}",
                self.obj_id
            )))
        })?;
        let obj_with_store = obj
            .as_has_store()
            .ok_or_else(|| ResourceProviderError::ObjectNoStore(format!("{}", self.obj_id)))?;
        Ok(obj_with_store.store_used_capacity(Some(*resource_type)))
    }

    fn creep_can_use(&self, creep: &screeps::Creep) -> Result<bool, Box<dyn Error>> {
        Ok(creep.get_active_bodyparts(screeps::Part::Carry) > 0)
    }

    fn creep_get_resource(
        &self,
        creep: &screeps::Creep,
        resource_type: ResourceType,
        ideal_amount: u32,
    ) -> anyhow::Result<TakeResourceResult> {
        let obj = get_object_typed(self.obj_id)?.ok_or_else(|| {
            Box::new(ResourceProviderError::ObjectNotFound(format!(
                "gets-r2: {}",
                self.obj_id
            )))
        })?;
        let store_obj = obj
            .as_has_store()
            .ok_or_else(|| ResourceProviderError::ObjectNoStore(format!("{}", self.obj_id)))?;
        let withdraw_obj = obj.as_withdrawable().ok_or_else(|| {
            ResourceProviderError::ObjectNoWithdrawable(format!("{}", self.obj_id))
        })?;
        let amount = cmp::min(
            store_obj.store_used_capacity(Some(resource_type)),
            cmp::min(
                ideal_amount,
                creep.store_free_capacity(Some(resource_type)) as u32,
            ),
        );

        Ok(TakeResourceResult::Withdraw {
            return_code: creep.withdraw_amount(withdraw_obj, resource_type, amount),
            tried_amount: amount,
        })
    }
}

#[derive(Clone, Debug)]
pub enum RoomObjectData {
    StorageStructure {
        obj_id: ObjectId<screeps::Structure>,
    },
    Litter {
        obj_id: ObjectId<screeps::Resource>,
    },
    // Later on you might want to add variants for Ruin and Tombstone
}

impl RoomObjectData {
    fn obj_id(&self) -> RawObjectId {
        use RoomObjectData::*;
        match self {
            StorageStructure { obj_id } => (*obj_id).into(),
            Litter { obj_id } => (*obj_id).into(),
        }
    }
}

impl ResourceData for RoomObjectData {
    fn pos(&self) -> anyhow::Result<Position> {
        use RoomObjectData::*;
        match self {
            StorageStructure { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "pos3: {}",
                        *obj_id
                    )))
                })?;
                Ok(obj.pos())
            }
            Litter { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "pos31 {}",
                        *obj_id
                    )))
                })?;
                Ok(obj.pos())
            }
        }
    }

    fn provides(&self, resource_type: &screeps::ResourceType) -> Result<u32, Box<dyn Error>> {
        use RoomObjectData::*;
        match self {
            StorageStructure { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "prov3: {}",
                        *obj_id
                    )))
                })?;
                let obj_with_store = obj
                    .as_has_store()
                    .ok_or_else(|| ResourceProviderError::ObjectNoStore(format!("{}", obj_id)))?;
                Ok(obj_with_store.store_used_capacity(Some(*resource_type)))
            }
            Litter { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "prov31: {}",
                        *obj_id
                    )))
                })?;
                if obj.resource_type() != *resource_type {
                    Err(ResourceProviderError::ResourceTypeMismatch(
                        *resource_type,
                        obj.resource_type(),
                        format!("{}", obj_id),
                    ))?
                }
                Ok(obj.amount())
            }
        }
    }

    fn creep_can_use(&self, creep: &screeps::Creep) -> Result<bool, Box<dyn Error>> {
        Ok(creep.get_active_bodyparts(screeps::Part::Carry) > 0)
    }

    fn creep_get_resource(
        &self,
        creep: &screeps::Creep,
        resource_type: ResourceType,
        ideal_amount: u32,
    ) -> anyhow::Result<TakeResourceResult> {
        use RoomObjectData::*;
        match self {
            StorageStructure { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "get_r4: {}",
                        obj_id
                    )))
                })?;
                let store_obj = obj.as_has_store().ok_or_else(|| {
                    ResourceProviderError::ObjectNoStore(format!("get_r41: {}", obj_id))
                })?;
                let withdraw_obj = obj.as_withdrawable().ok_or_else(|| {
                    ResourceProviderError::ObjectNoWithdrawable(format!("getr42{}", obj_id))
                })?;
                let amount = cmp::min(
                    store_obj.store_used_capacity(Some(resource_type)),
                    cmp::min(
                        ideal_amount,
                        creep.store_free_capacity(Some(resource_type)) as u32,
                    ),
                );

                Ok(TakeResourceResult::Withdraw {
                    return_code: creep.withdraw_amount(withdraw_obj, resource_type, amount),
                    tried_amount: amount,
                })
            }
            Litter { obj_id } => {
                let obj = get_object_typed(*obj_id)?.ok_or_else(|| {
                    Box::new(ResourceProviderError::ObjectNotFound(format!(
                        "getr45{}",
                        obj_id
                    )))
                })?;
                if obj.resource_type() != resource_type {
                    Err(Box::new(ResourceProviderError::ResourceTypeMismatch(
                        resource_type,
                        obj.resource_type(),
                        format!("{}", obj.id()),
                    )))?;
                }
                Ok(TakeResourceResult::Pickup {
                    return_code: creep.pickup(&obj),
                })
            }
        }
    }
}

pub fn calc_resource_providers(room: &Room) -> anyhow::Result<Vec<ResourceProvider>> {
    let structures: Vec<screeps::Structure> = room.find(find::STRUCTURES);

    // let containers: Vec<&screeps::StructureContainer> = structures
    //     .iter()
    //     .filter_map(|s| match s {
    //         screeps::Structure::Container(container) => Some(container),
    //         _ => None,
    //     })
    //     .collect();
    // let containers: Vec<ResourceProvider> = containers
    //     .iter()
    //     .filter_map(|c| {
    //         calc_container(&room, c).unwrap_or_else(|err| {
    //             warn!("failed calcing container: {}", err);
    //             None
    //         })
    //     })
    //     .collect();

    let structure_providers = structures.into_iter().filter_map(|s| match s {
        screeps::Structure::Container(container) => calc_container(&room, container)
            .unwrap_or_else(|err| {
                warn!("failed calcing container: {}", err);
                None
            }),
        screeps::Structure::Storage(storage) => calc_storage(storage).unwrap_or_else(|err| {
            warn!("failed calcing storage: {}", err);
            None
        }),
        screeps::Structure::Terminal(terminal) => calc_terminal(terminal).unwrap_or_else(|err| {
            warn!("failed calcing terminal: {}", err);
            None
        }),
        _ => None,
    });

    let litters: Vec<screeps::Resource> = room.find(find::DROPPED_RESOURCES);
    let litters: Vec<ResourceProvider> = litters
        .iter()
        .filter_map(|l| {
            calc_litter(&room, l).unwrap_or_else(|err| {
                warn!("failed calcing litter: {}", err);
                None
            })
        })
        .collect();

    let sources: Vec<ResourceProvider> = room
        .find(find::SOURCES)
        .iter()
        .filter_map(|s| {
            Some(ResourceProvider::EnergyFarm {
                resource_farm_data: ResourceFarmData { obj_id: s.id() },
            })
        })
        .collect();

    let mut providers = vec![];
    providers.extend(structure_providers);
    providers.extend(litters);
    providers.extend(sources);

    Ok(providers)
}

fn calc_container(
    room: &Room,
    container: screeps::StructureContainer,
) -> Result<Option<ResourceProvider>, Box<dyn Error>> {
    let container_pos = container.pos();
    let sources = room.look_for_around(look::SOURCES, container_pos, 1)?;
    if sources.len() > 0 {
        return Ok(Some(ResourceProvider::SourceDump {
            room_object_data: RoomObjectData::StorageStructure {
                obj_id: container.as_structure().id(),
            },
        }));
    }
    if let Some(controller) = room.controller() {
        if container_pos.in_range_to(&controller, 3) {
            return Ok(Some(ResourceProvider::BufferControllerUpgrade {
                room_object_data: StructureData {
                    obj_id: container.as_structure().id(),
                },
            }));
        }
    }

    // TODO Unknown container
    Ok(None)
}

fn calc_storage(
    storage: screeps::StructureStorage,
) -> Result<Option<ResourceProvider>, Box<dyn Error>> {
    Ok(Some(ResourceProvider::LongTermStorage {
        room_object_data: StructureData {
            obj_id: storage.as_structure().id(),
        },
    }))
}

fn calc_terminal(
    terminal: screeps::StructureTerminal,
) -> Result<Option<ResourceProvider>, Box<dyn Error>> {
    Ok(Some(ResourceProvider::TerminalOverflow {
        room_object_data: StructureData {
            obj_id: terminal.as_structure().id(),
        },
    }))
}

fn calc_litter(
    room: &Room,
    litter: &screeps::Resource,
) -> Result<Option<ResourceProvider>, Box<dyn Error>> {
    let litter_pos = litter.pos();
    let sources = room.look_for_around(look::SOURCES, litter_pos, 1)?;
    if sources.len() > 0 {
        return Ok(Some(ResourceProvider::SourceDump {
            room_object_data: RoomObjectData::Litter {
                obj_id: litter.id(),
            },
        }));
    }
    Ok(None)
}

// fn calc_energy_farms(
//     room: &Room,
//     litter: &screeps::Resource,
// ) -> Result<Option<ResourceProvider>, Box<dyn Error>> {
//     let litter_pos = litter.pos();
//     let sources = room.find(find::SOURCES);
//     if sources.len() > 0 {
//         return Ok(Some(ResourceProvider::SourceDump {
//             room_object_data: RoomObjectData::Litter {
//                 obj_id: litter.id(),
//             },
//         }));
//     }
//     Ok(None)
// }

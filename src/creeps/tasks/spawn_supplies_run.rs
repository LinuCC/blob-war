use std::cmp::{self, Reverse};
use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;

use log::warn;
/// Fill extensions and spawns (hopefully) efficiently
use screeps::{
    game::{get_object_typed, rooms},
    FindOptions, HasStore, ObjectId, Path, Position, ResourceType, Room, RoomName, StructureSpawn,
};
use screeps::{Creep, HasId, HasPosition, RectStyle, RoomVisual, SharedCreepProperties};

use crate::constants::TERMINAL_TRADE_BUFFER;
use crate::rooms::extensions::StructureSpawnSupply;
use crate::rooms::resource_provider::{ResourceData, RoomObjectData, TakeResourceResult};
use crate::{
    creeps::races::{generic_calc_energy_resource_provider, OokRace, RepresentsCreep},
    rooms::{
        extensions::{ExtensionFillPath, SuppliersReachPoint},
        resource_provider::ResourceProvider,
        room_state::RoomState,
    },
    state::BWState,
};

use anyhow::{anyhow, bail, Result};

use super::{
    CalcResourceProviderResult, FetchesFromResourceProvider, OokTaskRunnable, OokTaskRunnableResult,
};

#[derive(Clone, Debug)]
pub enum Step {
    Created,
    GetEnergy {
        target: ResourceProvider,
    },
    /// TODO We could combine filling extensions and spawns
    FillSuppliers {
        open: Vec<SuppliersReachPoint>,
        done: Vec<SuppliersReachPoint>,
    },
}

#[derive(Clone, Debug)]
pub struct Task {
    target_room_name: RoomName,
    step: Step,
}

impl Task {
    pub fn new(target_room_name: RoomName, state: &mut BWState, race: &OokRace) -> Result<Self> {
        let mut task = Task {
            target_room_name,
            step: Step::Created,
        };
        task.precheck(state, race)?;
        Ok(task)
    }

    pub fn handling_supplier_points(&self) -> Result<Vec<SuppliersReachPoint>> {
        match &self.step {
            Step::FillSuppliers { open, done } => {
                let mut points = vec![];
                points.extend(open);
                points.extend(done);
                Ok(points.into_iter().cloned().collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn fill_suppliers(&mut self, state: &mut BWState, race: &OokRace) -> Result<()> {
        let room = rooms::get(self.target_room_name)
            .ok_or_else(|| anyhow!("Room not found {}", self.target_room_name))?;
        let room_state = state
            .room_states
            .get(&self.target_room_name)
            .ok_or_else(|| anyhow!("Room state not found"))?;
        let creep = race.creep()?;
        let creep_handling: Option<ObjectId<Creep>> =
            if let RoomState::Base(room_state) = room_state {
                let mut points: HashSet<SuppliersReachPoint> = HashSet::from_iter(
                    room_state
                        .get_open_suppliers_reach_points(&state)?
                        .into_iter(),
                );

                let mut pathed_points: Vec<SuppliersReachPoint> = Vec::new();
                let mut energy_left = creep.energy();
                let mut pos = creep.pos();
                while let Some((ext, energy_needed)) = Self::closest_suppliers_point(
                    &room,
                    pos,
                    Vec::from_iter(points.iter()),
                )? {
                    pos = ext.pos.clone();
                    points.remove(&ext);
                    pathed_points.push(ext);
                    let new_energy_left = energy_left as i32 - energy_needed as i32;
                    if new_energy_left <= 0 {
                        break;
                    } else {
                        energy_left = new_energy_left as u32;
                    }
                }

                self.step = Step::FillSuppliers {
                    open: pathed_points,
                    done: vec![],
                };
                Some(creep.id())
            } else {
                bail!(
                    "Trying to spawn_supplies_run on unhandled RoomState in {}",
                    room.name()
                );
            };
        let room_state = state
            .room_states
            .get_mut(&self.target_room_name)
            .ok_or_else(|| anyhow!("Room state not found"))?;
        if let Some(creep_id) = creep_handling {
            if let RoomState::Base(room_state) = room_state {
                room_state.creep_handles_filling_extensions(creep_id)?;
            } else {
                warn!("WTFBBQ");
            }
        }

        Ok(())
    }

    fn transfer_to_supplier(
        &self,
        creep: &Creep,
        point: &SuppliersReachPoint,
    ) -> Result<Option<u32>> {
        for supplier in point.suppliers.iter() {
            // TODO store whether we filled up a specific supplier or not
            match supplier {
                StructureSpawnSupply::Spawn(spawn_id) => {
                    if let Some(spawn) = get_object_typed(*spawn_id)? {
                        let free_cappa = spawn.store_free_capacity(Some(ResourceType::Energy));
                        if free_cappa > 0 {
                            let amount = cmp::min(creep.energy(), free_cappa as u32);
                            creep.transfer_amount(&spawn, ResourceType::Energy, amount);
                            creep.say("🚢", false);
                            return Ok(Some(amount));
                        }
                    } else {
                        warn!("Spawn not found: {}", spawn_id);
                        return Ok(None);
                    }
                }
                StructureSpawnSupply::Extension(extension_id) => {
                    if let Some(extension) = get_object_typed(*extension_id)? {
                        let free_cappa = extension.store_free_capacity(Some(ResourceType::Energy));
                        if free_cappa > 0 {
                            let amount = cmp::min(creep.energy(), free_cappa as u32);
                            creep.transfer_amount(&extension, ResourceType::Energy, amount);
                            creep.say("🚢", false);
                            return Ok(Some(amount));
                        }
                    } else {
                        warn!("extension not found: {}", extension_id);
                        return Ok(None);
                    }
                }
            }
        }
        Ok(None)
    }

    fn closest_suppliers_point(
        room: &Room,
        pos: Position,
        open_supplier_points: Vec<&SuppliersReachPoint>,
    ) -> Result<Option<(SuppliersReachPoint, u32)>> {
        let mut open_supplier_points = open_supplier_points.clone();
        open_supplier_points.sort_by_cached_key(|e| {
            match pos.find_path_to(&e.pos, FindOptions::default().ignore_creeps(true)) {
                Path::Serialized(p) => room.deserialize_path(&p),
                Path::Vectorized(p) => p,
            }
            .len()
        });
        match open_supplier_points.first() {
            Some(&open_supplier_point) => {
                let suppliers_ids = open_supplier_point.suppliers.clone();
                let needed_energy =
                    suppliers_ids
                        .into_iter()
                        .fold(0, |acc, supplier| match supplier {
                            StructureSpawnSupply::Spawn(spawn_id) => {
                                if let Ok(Some(spawn)) = get_object_typed(spawn_id) {
                                    acc + spawn.store_free_capacity(Some(ResourceType::Energy))
                                } else {
                                    warn!("Spawn not found or error");
                                    acc
                                }
                            }
                            StructureSpawnSupply::Extension(extension_id) => {
                                if let Ok(Some(Extension)) = get_object_typed(extension_id) {
                                    acc + Extension.store_free_capacity(Some(ResourceType::Energy))
                                } else {
                                    warn!("Extension not found or error");
                                    acc
                                }
                            }
                        });
                if needed_energy > 0 {
                    Ok(Some((open_supplier_point.clone(), needed_energy as u32)))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn precheck(
        &mut self,
        state: &mut BWState,
        race: &OokRace,
    ) -> Result<Option<OokTaskRunnableResult>> {
        let creep = race.creep()?;
        match &self.step {
            Step::Created => {
                let calc_result = self.calc_resource_provider(&state.room_states, race)?;
                match calc_result {
                    Some(calc_result) => {
                        creep.say("📦", false);
                        self.step = Step::GetEnergy {
                            target: calc_result.resource_provider,
                        };
                    }
                    None => {}
                }
                Ok(None)
            }
            Step::GetEnergy { .. } => {
                if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
                    self.fill_suppliers(state, race)?;
                    creep.say("📦✅", false);
                }
                Ok(None)
            }
            Step::FillSuppliers { open, .. } => {
                if creep.store_used_capacity(Some(ResourceType::Energy)) == 0 {
                    Ok(Some(OokTaskRunnableResult::CancelAndDoAnother))
                } else {
                    if open.len() == 0 {
                        Ok(Some(OokTaskRunnableResult::CancelAndDoAnother))
                    } else {
                        Ok(None)
                    }
                }
            }
        }
    }

    fn visualize(&self) {
        if let Step::FillSuppliers { open, done } = &self.step {
            if let Some(room) = rooms::get(self.target_room_name) {
                let vis = room.visual();
                for point in open.iter() {
                    vis.rect(
                        point.pos.x() as f32 - 0.5,
                        point.pos.y() as f32 - 0.5,
                        1.,
                        1.,
                        Some(RectStyle::default().fill("#ccaa33")),
                    );
                    // vis.text(pos.0 as f32, pos.1 as f32, num.to_string(), None);
                }
                for point in done.iter() {
                    vis.rect(
                        point.pos.x() as f32 - 0.5,
                        point.pos.y() as f32 - 0.5,
                        1.,
                        1.,
                        Some(RectStyle::default().fill("#aacc33")),
                    );
                    // vis.text(pos.0 as f32, pos.1 as f32, num.to_string(), None);
                }
            }
        }
    }
}

impl OokTaskRunnable for Task {
    fn run(&mut self, state: &mut BWState, race: &OokRace) -> Result<OokTaskRunnableResult> {
        if let Some(res) = self.precheck(state, &race)? {
            return Ok(res);
        }
        let creep = race.creep()?;
        let mut remove_point = false;
        let res = match &self.step {
            Step::Created => {
                // precheck didnt find any resource provider
                creep.say("...", false);
                Ok(OokTaskRunnableResult::Continue)
            }
            Step::GetEnergy { target } => {
                creep.say("sgx", false);
                let target_pos = target.pos()?;
                if creep.pos().is_near_to(&target_pos) {
                    match target.creep_get_resource(
                        &creep,
                        ResourceType::Energy,
                        creep.store_free_capacity(Some(ResourceType::Energy)) as u32,
                    )? {
                        TakeResourceResult::Withdraw { .. } => {
                            creep.say("⏫", false);
                            self.fill_suppliers(state, race);
                        }
                        TakeResourceResult::Harvest { return_code, .. } => {
                            // Continue harvest until we are full
                            match return_code {
                                screeps::ReturnCode::Ok => {}
                                screeps::ReturnCode::NotEnough => {
                                    creep.say("⏫", false);
                                    self.fill_suppliers(state, race);
                                }
                                _ => {
                                    warn!("Harvest unknown result_code {:?}", return_code);
                                }
                            }
                        }
                        TakeResourceResult::Pickup { .. } => {
                            creep.say("⏫", false);
                            self.fill_suppliers(state, race);
                        }
                    }
                } else {
                    creep.move_to(&target_pos);
                }
                Ok(OokTaskRunnableResult::Continue)
            }
            Step::FillSuppliers { open, .. } => {
                self.visualize();
                if let Some(next_point) = open.first() {
                    if creep.pos() == next_point.pos {
                        match self.transfer_to_supplier(&creep, next_point)? {
                            Some(_energy) => Ok(OokTaskRunnableResult::Continue),
                            None => {
                                remove_point = true;
                                if let Some(next_point) = open.get(1) {
                                    creep.move_to(&next_point.pos);
                                    Ok(OokTaskRunnableResult::Continue)
                                } else {
                                    Ok(OokTaskRunnableResult::CancelAndDoAnother)
                                }
                            }
                        }
                    } else {
                        creep.move_to(&next_point.pos);
                        Ok(OokTaskRunnableResult::Continue)
                    }
                } else {
                    Ok(OokTaskRunnableResult::Finish)
                }
            }
        };
        if remove_point {
            match &mut self.step {
                Step::FillSuppliers { open, done } => {
                    let point = open.remove(0);
                    done.push(point);
                }
                _ => warn!("watz?"),
            }
        }
        // Make sure to update room_state that this creep is done
        match res {
            Ok(OokTaskRunnableResult::CancelAndDoAnother) => {
                let room_state = state
                    .room_states
                    .get_mut(&self.target_room_name)
                    .ok_or_else(|| anyhow!("Room state not found"))?;
                if let RoomState::Base(room_state) = room_state {
                    room_state.creep_done_handling_filling_extensions(creep.id())?;
                } else {
                    warn!("WTFBBQv2");
                }
            },
            Ok(OokTaskRunnableResult::Finish) => {
                let room_state = state
                    .room_states
                    .get_mut(&self.target_room_name)
                    .ok_or_else(|| anyhow!("Room state not found"))?;
                if let RoomState::Base(room_state) = room_state {
                    room_state.creep_done_handling_filling_extensions(creep.id())?;
                } else {
                    warn!("WTFBBQv2");
                }
            },
            _ => {},
        }
        res
    }
}

impl<'a> FetchesFromResourceProvider<'a> for Task {
    fn calc_resource_provider(
        &self,
        rooms_state: &'a HashMap<screeps::RoomName, RoomState>,
        race: &'a OokRace,
    ) -> Result<Option<CalcResourceProviderResult>> {
        let creep = race.creep()?;
        let amount = creep.store_free_capacity(Some(ResourceType::Energy));
        let room = rooms::get(self.target_room_name)
            .ok_or_else(|| anyhow!("Room not found {}", self.target_room_name))?;
        let room_state = rooms_state
            .get(&self.target_room_name)
            .ok_or_else(|| anyhow!("Room state not found"));
        match room_state? {
            RoomState::Base(room_state) => carrier_calc_energy_resource_provider(
                &room_state.resource_providers,
                &creep,
                &room,
                amount as u32,
            ),
            RoomState::SetupBase(_) => {
                warn!("unhandled room: RoomState::SetupBase");
                Ok(None)
            }
        }
    }
}

pub fn carrier_calc_energy_resource_provider(
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
    let prioed = carrier_creep_fetch_from_provider_prio(&room, creep.pos(), working_providers)?;
    match prioed {
        Some(prov) => Ok(Some(CalcResourceProviderResult {
            resource_provider: prov.to_owned(),
            resource_type: ResourceType::Energy,
            amount: amount as u32,
        })),
        None => Ok(None),
    }
}

fn carrier_creep_fetch_from_provider_prio<'a>(
    room: &Room,
    creep_pos: Position,
    working_providers: Vec<&'a ResourceProvider>,
) -> anyhow::Result<Option<&'a ResourceProvider>> {
    let mut sorted = working_providers.clone();
    sorted.sort_by_cached_key(|a| {
        Reverse(
            carrier_working_providers_points(room, a, &creep_pos)
                .unwrap_or(Some(-10000))
                .unwrap_or(-10000),
        )
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
fn carrier_working_providers_points(
    room: &Room,
    prov: &ResourceProvider,
    for_pos: &Position,
) -> anyhow::Result<Option<i32>> {
    let mut points: i32 = 0;
    match prov {
        ResourceProvider::EnergyFarm { .. } => {
            points = -10000;
        }
        ResourceProvider::SourceDump { room_object_data } => {
            points += 200;
            // TODO Doesnt check which type of resoure yet
            let resource_amount = match room_object_data {
                RoomObjectData::StorageStructure { obj_id } => {
                    let obj = get_object_typed(*obj_id)?
                        .ok_or_else(|| anyhow!("object not found {}", *obj_id))?;
                    obj.as_has_store()
                        .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                        .unwrap_or(0)
                }
                RoomObjectData::Litter { obj_id } => {
                    let obj = get_object_typed(*obj_id)?
                        .ok_or_else(|| anyhow!("object not found {}", *obj_id))?;
                    obj.amount()
                }
            };
            // Poor man's curve
            if resource_amount == 0 {
                points = 0;
            } else if resource_amount < 200 {
                points = 50;
            } else if resource_amount < 500 {
                points -= 100 - ((resource_amount as f32 / 5.).round() as i32);
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
            points -= vec_path.len() as i32;
        }
        ResourceProvider::BufferControllerUpgrade { room_object_data } => {
            points += 50;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?
                .ok_or_else(|| anyhow!("object not found {}", room_object_data.obj_id))?;
            let resource_amount = obj
                .as_has_store()
                .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                .unwrap_or(0);
            // Poor man's curve
            if resource_amount == 0 {
                points = 0;
            } else if resource_amount < 100 {
                points -= 150 - resource_amount as i32;
            } else if resource_amount < 500 {
                points -= 100 - (resource_amount as f32 / 5.).round() as i32;
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
            points += 100;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?
                .ok_or_else(|| anyhow!("object not found {}", room_object_data.obj_id))?;
            let resource_amount = obj
                .as_has_store()
                .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                .unwrap_or(0);
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
            points += 100;
            // TODO Doesnt check which type of resoure yet
            let obj = get_object_typed(room_object_data.obj_id)?.ok_or_else(|| {
                anyhow!("object not found {}", room_object_data.obj_id)
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
                points += cmp::max((overflow_resource_amount as f32 / 1000.).round() as i32, 5);
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

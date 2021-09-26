pub mod resource_provider;
pub mod room_ext;
pub mod room_state;
pub mod extensions;

use std::collections::HashMap;

use log::{debug, warn};
use screeps::{
    creep,
    find::{self, SOURCES},
    game::rooms,
    ConstructionSite, FindOptions, HasId, HasPosition, LookResult, ObjectId, Part, Path, Position,
    RawObjectId, Room, RoomName, Source, Step, Structure, StructureSpawn,
};
use std::error::Error;
use anyhow::anyhow;

use crate::{
    constants::ROOM_ID_MAIN,
    game::{owned_rooms, OwnedBy},
    state::{BWContext, BWState}
};

#[derive(thiserror::Error, Debug)]
pub enum RoomError {
    #[error("room not found {0}")]
    RoomNotFound(String),
    #[error("Room {0} not configured!")]
    RoomNotConfigured(String),
    #[error("Queue not prioritized!")]
    RoomQueueNotPrioritized(),
    #[error("FarmPosition for source not found")]
    FarmPositionSourceNotFound(),
    #[error("FarmPosition of source not found")]
    FarmPositionForSourceNotFound(),
}

#[derive(Debug)]
pub struct RoomSettings {
    pub name: RoomName,
    pub spawns: Vec<ObjectId<StructureSpawn>>,
    pub target_creeps: RoomCreepSettings,
    pub maintenance: MaintenanceQueue,
    pub farm_positions: HashMap<ObjectId<Source>, Vec<FarmPosition>>,
}

pub fn get_room(room_ident: &str) -> anyhow::Result<Room> {
    let room =
        rooms::get(RoomName::new(room_ident)?).ok_or(RoomError::RoomNotFound(room_ident.into()))?;
    Ok(room)
}

impl RoomSettings {
    pub fn world() -> anyhow::Result<HashMap<MyRoom, RoomSettings>> {
        let owned_rooms = owned_rooms(OwnedBy::Me);
        let mut room_configs = HashMap::new();
        for (name, _room) in owned_rooms.into_iter() {
            let room_str = format!("{}", name);
            let my_room =
                MyRoom::by_name(&room_str).ok_or(RoomError::RoomNotConfigured(room_str));
            match my_room {
                Ok(my_room) => {room_configs.insert(my_room.clone(), MyRoom::config(my_room)?);},
                Err(err) => {warn!("Room not configured: {}", err);},
            }
        }
        // rooms.insert(
        //     main_room.name().clone(),
        //     RoomSettings {
        //         name: main_room.name().clone(),
        //         spawns: [String::from("Spawn1")].into(),
        //         creeps: RoomCreepSettings {
        //             builder: 1,
        //             farmer: [RoomFarmerSettings {
        //                 parts: [Part::Work, Part::Carry, Part::Move, Part::Move].into(),
        //             }]
        //             .into(),
        //         },
        //     },
        // );
        Ok(room_configs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MyRoom {
    Main,
}

impl MyRoom {
    pub fn name<'a>(room: MyRoom) -> &'a str {
        match room {
            MyRoom::Main => ROOM_ID_MAIN,
        }
    }

    pub fn by_name<'a>(room: &str) -> Option<MyRoom> {
        match room {
            ROOM_ID_MAIN => Some(MyRoom::Main),
            _ => None,
        }
    }

    pub fn by_room_name<'a>(room_name: RoomName) -> Option<MyRoom> {
        MyRoom::by_name(&format!("{}", room_name))
    }

    pub fn get(room: &MyRoom) -> anyhow::Result<Room> {
        get_room(MyRoom::name(room.clone()))
    }

    pub fn room(&self) -> anyhow::Result<Room> {
        get_room(MyRoom::name(self.clone()))
    }

    pub fn config(room_ident: MyRoom) -> anyhow::Result<RoomSettings> {
        let room_data = get_room(MyRoom::name(room_ident.clone()))?;
        match &room_ident {
            MyRoom::Main => main_room_config(room_ident, &room_data),
        }
    }
}

fn main_room_config(_room_ident: MyRoom, room: &Room) -> anyhow::Result<RoomSettings> {
    let spawns = room.find(find::MY_SPAWNS);
    let maintenance = match init_maintenance_queue(room) {
        Ok(m) => m,
        Err(err) => {
            warn!("Original Error: {}", err);
            warn!("Failed init-ing maintenance queue for room {}", room.name());
            MaintenanceQueue::Prioritized(vec![])
        }
    };
    debug!(
        "Maintenance: Discovered {} items for room {}",
        maintenance.items_len(),
        room.name()
    );
    let farm_positions = farm_positions(room.name())?;
    let farmers_positions = farmer_positions(&farm_positions)?;
    Ok(RoomSettings {
        name: room.name().clone(),
        spawns: spawns.iter().map(|spawn| spawn.id()).collect(),
        target_creeps: RoomCreepSettings {
            builder: [
                // RoomBuilderSettings {
                //     parts: [
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //     ]
                //     .into(),
                // },
                // RoomBuilderSettings {
                //     parts: [
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //     ]
                //     .into(),
                // },
            ]
            .into(),
            farmer: vec![].into(),
            // farmer: farmers_positions
            //     .iter()
            //     .map(|farm_pos| RoomFarmerSettings {
            //         parts: [
            //             Part::Work,
            //             Part::Work,
            //             Part::Work,
            //             Part::Work,
            //             Part::Work,
            //             Part::Work,
            //             Part::Move,
            //             Part::Move,
            //             Part::Move,
            //         ]
            //         .into(),
            //         farm_position: farm_pos.to_owned(),
            //     })
            //     .collect::<Vec<RoomFarmerSettings>>()
            //     .into(),
            runner: [
                RoomRunnerSettings {
                    parts: [
                        Part::Carry,
                        Part::Carry,
                        Part::Carry,
                        Part::Move,
                        Part::Move,
                        Part::Move,
                    ]
                    .into(),
                },
                // RoomRunnerSettings {
                //     parts: [
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //     ]
                //     .into(),
                // },
                // RoomRunnerSettings {
                //     parts: [
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Carry,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //     ]
                //     .into(),
                // },
            ]
            .into(),
            bitches: [
                // RoomBitchSettings {
                //     parts: []
                //     .into(),
                // },
                // RoomBitchSettings {
                //     parts: []
                //     .into(),
                // },
                // RoomBitchSettings {
                //     parts: []
                //     .into(),
                // },
                // RoomBitchSettings {
                //     parts: []
                //     .into(),
                // },
                // RoomBitchSettings {
                //     parts: [
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Work,
                //         Part::Carry,
                //         Part::Move,
                //         Part::Move,
                //         Part::Move,
                //     ]
                //     .into(),
                // },
            ]
            .into(),
            claimers: [
                // RoomClaimerSettings {
                //     parts: [
                //         Part::Claim,
                //         Part::Move,
                //     ]
                //     .into(),
                //     target_room: RoomName::new("W12N15")?,
                // },
            ]
            .into(),
        },
        maintenance,
        farm_positions,
    })
}

fn farmer_positions(
    sources_with_farm_pos: &HashMap<ObjectId<Source>, Vec<FarmPosition>>,
) -> anyhow::Result<Vec<FarmPosition>> {
    let mut farm_positions_for_farmers = vec![];
    for (_id, farm_positions) in sources_with_farm_pos.iter() {
        let prio_pos = prioritized_farm_positions(&farm_positions);
        farm_positions_for_farmers.push(
            prio_pos
                .first()
                .ok_or(Box::new(RoomError::FarmPositionForSourceNotFound()))?
                .to_owned(),
        );
    }
    Ok(farm_positions_for_farmers)
}

pub fn bootstrap_room(state: &mut BWState, target_room_name: RoomName, helper_room_name: RoomName) -> anyhow::Result<()> {
    let helper_room = rooms::get(helper_room_name).ok_or(anyhow!("Helper room not found"))?;
    let target_room = rooms::get(target_room_name);


    Ok(())
}

pub fn update_maintenance(room_ident: MyRoom) -> Result<(), Box<dyn Error>> {
    let room = room_ident.room()?;
    let maintenance = match init_maintenance_queue(&room) {
        Ok(m) => m,
        Err(err) => {
            warn!("Original Error: {}", err);
            warn!("Failed init-ing maintenance queue for room {}", room.name());
            MaintenanceQueue::Prioritized(vec![])
        }
    };
    BWContext::update_state(move |state: &mut BWState| -> Result<(), Box<dyn Error>> {
        let room_config =
            state
                .room_settings
                .get_mut(&room_ident)
                .ok_or(Box::new(RoomError::RoomNotFound(
                    MyRoom::name(room_ident.to_owned()).into(),
                )))?;
        room_config.maintenance = maintenance.clone();
        Ok(())
    })
}

fn init_maintenance_queue(room: &Room) -> Result<MaintenanceQueue, Box<dyn Error>> {
    let construction_sites = room.find(find::CONSTRUCTION_SITES);
    Ok(MaintenanceQueue::Prioritized(
        construction_sites
            .into_iter()
            .map(|site| RoomMaintenance::NewBuild {
                object_id: site.id(),
            })
            .collect(),
    ))
}

fn sources_closest_to_controller(room: &Room) -> Vec<Source> {
    let sources = room.find(SOURCES);
    if let Some(controller) = room.controller() {
        let mut pathed: Vec<(Path, Source)> = sources
            .into_iter()
            .map(|source: Source| {
                (
                    room.find_path(&source.pos(), &controller.pos(), FindOptions::new()),
                    source,
                )
            })
            .collect();
        pathed.sort_by(|a, b| {
            let a_path = a.0.vectorized().unwrap();
            let b_path = b.0.vectorized().unwrap();
            return a_path.len().cmp(&b_path.len());
        });
        return pathed.into_iter().map(|(_, source)| source).collect();
    }
    return [].into();
}

pub trait PathOptionUnwrapper {
    fn vectorized(&self) -> Option<Vec<Step>>;
}

impl PathOptionUnwrapper for Path {
    // If we know that we get a vectorized path back, we safe the
    // `match` by just doing `.vectorized().unwrap()` instead.
    fn vectorized(&self) -> Option<Vec<Step>> {
        if let Path::Vectorized(c) = self {
            Some(c.clone())
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct RoomCreepSettings {
    pub builder: Vec<RoomBuilderSettings>,
    pub farmer: Vec<RoomFarmerSettings>,
    pub bitches: Vec<RoomBitchSettings>,
    pub runner: Vec<RoomRunnerSettings>,
    pub claimers: Vec<RoomClaimerSettings>,
}

#[derive(Debug)]
pub struct RoomBuilderSettings {
    pub parts: Vec<creep::Part>,
}

#[derive(Debug)]
pub struct RoomFarmerSettings {
    pub parts: Vec<creep::Part>,
    pub farm_position: FarmPosition,
    // target_source: ObjectId,
}

#[derive(Debug)]
pub struct RoomRunnerSettings {
    pub parts: Vec<creep::Part>,
}

#[derive(Debug)]
pub struct RoomBitchSettings {
    pub parts: Vec<creep::Part>,
}

#[derive(Debug)]
pub struct RoomClaimerSettings {
    pub parts: Vec<creep::Part>,
    pub target_room: RoomName,
}


#[derive(Debug, Clone, PartialEq)]
pub enum RoomMaintenance {
    NewBuild {
        object_id: ObjectId<ConstructionSite>,
    },
    Repair {
        object_id: RawObjectId,
    },
}

impl RoomMaintenance {
    pub fn object_id(&self) -> RawObjectId {
        use RoomMaintenance::*;
        match self {
            NewBuild { object_id } => object_id.to_owned().into(),
            Repair { object_id } => object_id.to_owned(),
            // Arbeitsbeschaffungsmassnahme
            // BuildUp { object_id } => object_id.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MaintenanceQueue {
    /// Queue is prioritized right now
    Prioritized(Vec<RoomMaintenance>),
    /// Queue is currently unsorted
    Unsorted(Vec<RoomMaintenance>),
    // UnderAttack(Vec<RoomMaintenance>),
}

impl MaintenanceQueue {
    pub fn items_len(&self) -> usize {
        match &self {
            MaintenanceQueue::Prioritized(i) => i.len(),
            MaintenanceQueue::Unsorted(i) => i.len(),
        }
    }

    pub fn priority_item(&self) -> Result<Option<&RoomMaintenance>, Box<dyn Error>> {
        match self {
            MaintenanceQueue::Prioritized(items) => Ok(items.first()),
            MaintenanceQueue::Unsorted(_) => Err(Box::new(RoomError::RoomQueueNotPrioritized())),
        }
    }

    pub fn remove_item(&mut self, raw_object_id: RawObjectId) -> Result<(), Box<dyn Error>> {
        let items = match self {
            MaintenanceQueue::Prioritized(items) => items,
            MaintenanceQueue::Unsorted(items) => items,
        };
        items.retain(|item| item.object_id() != raw_object_id);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum FarmPosition {
    /// Directly drops the resource it farms, so it doesnt need to transport anything
    /// Also a `Running` farm position if needed
    Dropping(FarmPositionData),
    /// Takes resource some way away
    Running(FarmPositionData),
}

impl FarmPosition {
    pub fn from_basic(
        pos_x: u32,
        pos_y: u32,
        source_id: ObjectId<Source>,
        room: Room,
    ) -> FarmPosition {
        let tile = room.look_at_xy(pos_x, pos_y);
        let is_dropper = terrain_is_dropper(&tile);
        if is_dropper {
            FarmPosition::Dropping(FarmPositionData {
                position: Position::new(pos_x, pos_y, room.name()),
                for_source: source_id,
            })
        } else {
            FarmPosition::Running(FarmPositionData {
                position: Position::new(pos_x, pos_y, room.name()),
                for_source: source_id,
            })
        }
    }

    pub fn position(&self) -> Position {
        match self {
            FarmPosition::Dropping(data) => data.position,
            FarmPosition::Running(data) => data.position,
        }
    }

    pub fn for_source(&self) -> ObjectId<Source> {
        match self {
            FarmPosition::Dropping(data) => data.for_source,
            FarmPosition::Running(data) => data.for_source,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FarmPositionData {
    position: Position,
    for_source: ObjectId<Source>,
}

pub fn prioritized_farm_positions(farm_positions: &Vec<FarmPosition>) -> Vec<FarmPosition> {
    let mut sorted_farm_positions = farm_positions.clone();
    // Prioritize farm positions by dropping first
    sorted_farm_positions.sort_by(|pos_a, pos_b| {
        let pos_a_val = match pos_a {
            FarmPosition::Dropping(_) => 0,
            _ => 1,
        };
        let pos_b_val = match pos_b {
            FarmPosition::Dropping(_) => 0,
            _ => 1,
        };
        pos_a_val.cmp(&pos_b_val)
    });
    sorted_farm_positions
}

fn terrain_is_walkable(tile: &Vec<LookResult>) -> bool {
    tile.iter().any(|look| match look {
        LookResult::Terrain(screeps::Terrain::Plain) => true,
        LookResult::Terrain(screeps::Terrain::Swamp) => true,
        _ => false,
    })
}

fn terrain_is_dropper(tile: &Vec<LookResult>) -> bool {
    tile.iter().any(|look| match look {
        LookResult::Structure(Structure::Container(_)) => true,
        _ => false,
    })
}

pub fn farm_positions(
    room_name: RoomName,
) -> anyhow::Result<HashMap<ObjectId<Source>, Vec<FarmPosition>>> {
    let room =
        rooms::get(room_name).ok_or(Box::new(RoomError::RoomNotFound(format!("{}", room_name))))?;
    let sources = room.find(find::SOURCES);

    let mut positions: HashMap<ObjectId<Source>, Vec<FarmPosition>> = HashMap::new();
    for source in sources.iter() {
        let source_pos = source.pos();
        for pos_x in (source_pos.x() - 1)..(source_pos.x() + 2) {
            for pos_y in (source_pos.y() - 1)..(source_pos.y() + 2) {
                let tile = room.look_at_xy(pos_x, pos_y);
                let is_walkable = terrain_is_walkable(&tile);
                if is_walkable {
                    let is_dropper = terrain_is_dropper(&tile);
                    let new_position = if is_dropper {
                        FarmPosition::Dropping(FarmPositionData {
                            position: Position::new(pos_x, pos_y, room_name),
                            for_source: source.id(),
                        })
                    } else {
                        FarmPosition::Running(FarmPositionData {
                            position: Position::new(pos_x, pos_y, room_name),
                            for_source: source.id(),
                        })
                    };
                    if let Some(positions_list) = positions.get_mut(&source.id()) {
                        positions_list.push(new_position)
                    } else {
                        positions.insert(source.id(), vec![new_position]);
                    }
                }
            }
        }
    }

    Ok(positions)
}

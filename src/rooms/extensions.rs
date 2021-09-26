use std::{cmp::Reverse, collections::{HashMap, HashSet}};

use screeps::{HasId, HasPosition, ObjectId, Position, Room, RoomVisual, Structure, StructureExtension, StructureSpawn, find};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

use super::room_ext::RoomExt;

#[derive(Clone, Debug, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum StructureSpawnSupply {
    Spawn(ObjectId<StructureSpawn>),
    Extension(ObjectId<StructureExtension>),
}


impl From<StructureSpawnSupplyForCalc> for StructureSpawnSupply {
    fn from(c: StructureSpawnSupplyForCalc) -> Self {
        match c {
            StructureSpawnSupplyForCalc::Spawn(spawn) => StructureSpawnSupply::Spawn(spawn.id()),
            StructureSpawnSupplyForCalc::Extension(extension) => StructureSpawnSupply::Extension(extension.id()),
        }
    }
}

#[derive(Clone)]
enum StructureSpawnSupplyForCalc {
    Spawn(StructureSpawn),
    Extension(StructureExtension),
}

impl HasPosition for StructureSpawnSupplyForCalc {
    fn pos(&self) -> Position {
        match self {
            StructureSpawnSupplyForCalc::Spawn(spawn) => spawn.pos(),
            StructureSpawnSupplyForCalc::Extension(extension) => extension.pos(),
        }
    }
}

js_serializable!(StructureSpawnSupply);
js_deserializable!(StructureSpawnSupply);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuppliersReachPoint {
    pub suppliers: Vec<StructureSpawnSupply>,
    pub pos: Position,
}


impl Hash for SuppliersReachPoint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.pos.hash(state);
    }
}

js_serializable!(SuppliersReachPoint);
js_deserializable!(SuppliersReachPoint);

#[derive(Clone, Debug)]
pub struct ExtensionFillPath {
    pub points: Vec<SuppliersReachPoint>,
}

impl Default for ExtensionFillPath {
    fn default() -> Self {
        ExtensionFillPath {
            points: vec![]
        }
    }
}

impl ExtensionFillPath {
    pub fn best_for_room(room: &Room) -> ExtensionFillPath {
        let structures = room.find(find::STRUCTURES);
        let walkable_tiles = room_walkable_tiles(room);
        // let extensions: Vec<&StructureExtension> = structures.iter().filter_map(|s| match s {
        //     screeps::Structure::Extension(e) => Some(e),
        //     _ => None,
        // }).collect();

        let spawn_suppliers: Vec<StructureSpawnSupplyForCalc> = structures.into_iter().filter_map(|s| match s {
            Structure::Extension(e) => Some(StructureSpawnSupplyForCalc::Extension(e)),
            Structure::Spawn(e) => Some(StructureSpawnSupplyForCalc::Spawn(e)),
            _ => None,
        }).collect();

        let positioned_suppliers: HashMap<(u8, u8), StructureSpawnSupply> = spawn_suppliers.iter().map(|supplier| {
            match supplier {
                StructureSpawnSupplyForCalc::Spawn(spawn) => {
                    (spawn.pos().into(), StructureSpawnSupply::Spawn(spawn.id()))
                },
                StructureSpawnSupplyForCalc::Extension(extension) => {
                    (extension.pos().into(), StructureSpawnSupply::Extension(extension.id()))
                },
            }
        }).collect();
        let mut positions: HashMap<(u8, u8), u8> = HashMap::new();
        for supplier in spawn_suppliers.iter() {
            let (range_x, range_y) =
                Room::bounded_pos_area_range((supplier.pos().x() as u8, supplier.pos().y() as u8), 1, true);
            for x in range_x {
                for y in range_y.clone() {
                    if walkable_tiles.get(&(x, y)).map(|v| *v).unwrap_or(false) {
                        positions.entry((x, y)).and_modify(|e| *e += 1).or_insert(1);
                    }
                }
            }
        }

        let mut unreached_extensions: HashSet<StructureSpawnSupply> = spawn_suppliers.into_iter().map(StructureSpawnSupply::from).collect();
        let mut sorted_positions: Vec<((u8, u8), u8)> = positions.clone().iter().map(|(pos, num_ext)| (*pos, *num_ext)).collect();
        let mut final_positions: HashMap<(u8, u8), SuppliersReachPoint> = HashMap::new();
        sorted_positions.sort_by_key(|( _pos, num_ext )| Reverse(*num_ext));
        for position in sorted_positions {
            if unreached_extensions.len() == 0 {
                break;
            }
            let (range_x, range_y) =
                Room::bounded_pos_area_range(position.0, 1, true);
            for x in range_x {
                for y in range_y.clone() {
                    if let Some(positioned_extension) = positioned_suppliers.get(&(x, y)) {
                        if unreached_extensions.remove(positioned_extension) {
                            final_positions.entry(position.0)
                                .and_modify(|ext_point| ext_point.suppliers.push(positioned_extension.to_owned()))
                                .or_insert(SuppliersReachPoint {
                                    suppliers: vec![positioned_extension.to_owned()],
                                    pos: Position::new(position.0.0 as u32, position.0.1 as u32, room.name()),
                                });
                        }
                    }
                }
            }
        }

        ExtensionFillPath {
            points: final_positions.into_iter().map(|(_pos, ext)| ext).collect(),
        }
    }
}

fn room_walkable_tiles(room: &Room) -> HashMap<(u8, u8), bool> {
    let look_result = room.look_at_area(0, 0, 49, 49);
    let mut tile_info = HashMap::new();
    for tile in look_result {
        let walkable = match tile.look_result {
            screeps::LookResult::Creep(_) => true,
            screeps::LookResult::Energy(_) => true,
            screeps::LookResult::Resource(_) => true,
            screeps::LookResult::Source(_) => false,
            screeps::LookResult::Mineral(_) => false,
            screeps::LookResult::Deposit(_) => true,
            screeps::LookResult::Structure(s) => match s {
                Structure::Container(_) => true,
                Structure::Rampart(_) => true,
                Structure::Road(_) => true,
                _ => false
            },
            screeps::LookResult::Flag(_) => true,
            screeps::LookResult::ConstructionSite(_) => false,
            screeps::LookResult::Nuke(_) => true,
            screeps::LookResult::Terrain(t) => match t {
                screeps::Terrain::Plain => true,
                screeps::Terrain::Wall => false,
                screeps::Terrain::Swamp => true,
            },
            screeps::LookResult::Tombstone(_) => true,
            screeps::LookResult::PowerCreep(_) => true,
            screeps::LookResult::Ruin(_) => true,
        };
        tile_info.entry((tile.x as u8, tile.y as u8)).and_modify(|w| *w = *w && walkable).or_insert(walkable);
    }
    tile_info
}

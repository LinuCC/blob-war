use std::collections::HashMap;

use log::warn;
use screeps::{
    find, game::get_object_typed, look, HasId, HasPosition, LookResult, ObjectId, Position, Room,
    RoomObjectProperties, SharedCreepProperties, Source, Structure, StructureContainer,
    StructureLink,
};

use crate::{
    creeps::races::{OokRace, RepresentsCreep},
    rooms::room_ext::RoomExt,
    state::BWState,
    utils::{AnyhowOptionExt, ResultOptionExt},
};
use anyhow::Result;

use super::{OokTaskRunnable, OokTaskRunnableResult};

#[derive(Clone, Debug)]
pub enum Step {
    Walk { target: FarmPosition },
    Harvest { target: FarmPosition },
}

impl<'a> Step {
    fn target(&'a self) -> &'a FarmPosition {
        match self {
            Step::Walk { target } => target,
            Step::Harvest { target } => target,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    step: Step,
}

impl Task {
    pub fn new(target: &Source, state: &BWState, race: &OokRace) -> Result<Self> {
        let room = target.room().anyhow(&format!(
            "room not found for farm task target {}",
            target.id()
        ))?;
        let farm_positions = farm_positions(&room)?;
        let farm_positions = farm_positions
            .get(&target.id())
            .anyhow("farm position not found")?;
        let prioed = prioritized_farm_positions(farm_positions);
        let farm_position = prioed
            .first()
            .anyhow(&format!("no farm position found for {}", target.id()))?;
        let mut task = Task {
            step: Step::Walk {
                target: farm_position.to_owned(),
            },
        };
        task.precheck(state, race)?;
        Ok(task)
    }

    fn precheck(&mut self, _state: &BWState, race: &OokRace) -> Result<()> {
        let creep = race.creep()?;
        match &self.step {
            Step::Harvest { .. } => {
                // Checking `harvest` result should be enough
            }
            Step::Walk { target } => {
                if creep.pos() == target.position() {
                    self.step = Step::Harvest {
                        target: target.to_owned(),
                    };
                }
            }
        };
        Ok(())
    }
}

impl OokTaskRunnable for Task {
    fn run(&mut self, state: &mut BWState, race: &OokRace) -> Result<OokTaskRunnableResult> {
        self.precheck(state, &race)?;
        let creep = race.creep()?;
        match &mut self.step {
            Step::Harvest { target } => {
                let source = get_object_typed(target.for_source())
                    .err_or_none("source for target not found")?;
                match creep.harvest(&source) {
                    screeps::ReturnCode::Ok => {}
                    screeps::ReturnCode::NotInRange => {
                        self.step = Step::Walk {
                            target: target.to_owned(),
                        };
                    }
                    code => {
                        warn!("farm task harvest unhandled code: {:?}", code);
                    }
                }
                Ok(OokTaskRunnableResult::Continue)
            }
            Step::Walk { target } => {
                creep.move_to(&target.position());
                Ok(OokTaskRunnableResult::Continue)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum FarmPosition {
    /// Directly drops the resource it farms, so it doesnt need to transport anything
    /// Also a `Running` farm position if needed
    Dropping(FarmPositionData),
    /// Takes resource some way away
    Running(FarmPositionData),
    /// Farms and puts resource into a container / link next to it
    Shifting(FarmShiftPositionData),
}

impl FarmPosition {
    pub fn from_basic(
        pos_x: u32,
        pos_y: u32,
        source_id: ObjectId<Source>,
        room: Room,
    ) -> FarmPosition {
        let tile = room.look_at_xy(pos_x, pos_y);
        let structures_around = room.look_for_around(
            look::STRUCTURES,
            Position::new(pos_x, pos_y, room.name()),
            1,
        );
        let is_dropper = terrain_is_dropper(&tile);
        if let Ok(Some(shifter_pos)) =
            get_shifter_farm_position(&room, source_id, Position::new(pos_x, pos_y, room.name()))
        {
            shifter_pos
        } else if is_dropper {
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
            FarmPosition::Shifting(data) => data.position,
        }
    }

    pub fn for_source(&self) -> ObjectId<Source> {
        match self {
            FarmPosition::Dropping(data) => data.for_source,
            FarmPosition::Running(data) => data.for_source,
            FarmPosition::Shifting(data) => data.for_source,
        }
    }
}

fn get_shifter_farm_position(
    room: &Room,
    source_id: ObjectId<Source>,
    pos: Position,
) -> anyhow::Result<Option<FarmPosition>> {
    let structures_around = room.look_for_around(look::STRUCTURES, pos, 1);
    for structure in structures_around? {
        if structure.pos() == pos {
            continue; // Container directly on pos is FarmPosition::Dropping
        }
        let shift_target = match structure {
            Structure::Container(container) => Some(FarmShiftTarget::Container(container.id())),
            Structure::Link(link) => Some(FarmShiftTarget::Link(link.id())),
            _ => None,
        };
        if let Some(shift_target) = shift_target {
            return Ok(Some(FarmPosition::Shifting(FarmShiftPositionData {
                position: pos,
                for_source: source_id,
                shift_target,
            })));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone)]
pub struct FarmPositionData {
    position: Position,
    for_source: ObjectId<Source>,
}

#[derive(Debug, Clone)]
pub struct FarmShiftPositionData {
    position: Position,
    for_source: ObjectId<Source>,
    shift_target: FarmShiftTarget,
}

#[derive(Debug, Clone)]
pub enum FarmShiftTarget {
    Container(ObjectId<StructureContainer>),
    Link(ObjectId<StructureLink>),
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

pub fn farm_positions(room: &Room) -> anyhow::Result<HashMap<ObjectId<Source>, Vec<FarmPosition>>> {
    let room_name = room.name();
    let sources = room.find(find::SOURCES);

    let mut positions: HashMap<ObjectId<Source>, Vec<FarmPosition>> = HashMap::new();
    for source in sources.iter() {
        let source_pos = source.pos();
        for pos_x in (source_pos.x() - 1)..(source_pos.x() + 2) {
            for pos_y in (source_pos.y() - 1)..(source_pos.y() + 2) {
                let tile = room.look_at_xy(pos_x, pos_y);
                let is_walkable = terrain_is_walkable(&tile);
                if is_walkable {
                    let shifting_target = get_shifter_farm_position(
                        room,
                        source.id(),
                        Position::new(pos_x, pos_y, room_name),
                    );
                    let is_dropper = terrain_is_dropper(&tile);
                    let new_position = if is_dropper {
                        FarmPosition::Dropping(FarmPositionData {
                            position: Position::new(pos_x, pos_y, room_name),
                            for_source: source.id(),
                        })
                    } else if let Ok(Some(shifting_target)) = shifting_target {
                        shifting_target
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

use screeps::{Bodypart, CREEP_SPAWN_TIME, Part, creep};

use crate::state::UniqId;

use super::races::OokRaceKind;

pub fn create_creep_name(race: &OokRaceKind) -> String {
    format!(
        "{}-{}",
        race,
        UniqId::new(),
    )
}

pub fn get_bodyparts_cost(parts: Vec<creep::Part>) -> u32 {
    let mut val = 0;
    for part in parts {
        val += part.cost();
    }
    val
}

pub trait SpawnableTimer {
    fn get_spawn_time(&self) -> usize;
}

impl SpawnableTimer for Vec<Part> {
    fn get_spawn_time(&self) -> usize {
        self.len() * CREEP_SPAWN_TIME as usize
    }
}

impl SpawnableTimer for Vec<&Part> {
    fn get_spawn_time(&self) -> usize {
        self.len() * CREEP_SPAWN_TIME as usize
    }
}

impl SpawnableTimer for Vec<Bodypart> {
    fn get_spawn_time(&self) -> usize {
        self.len() * CREEP_SPAWN_TIME as usize
    }
}

impl SpawnableTimer for Vec<&Bodypart> {
    fn get_spawn_time(&self) -> usize {
        self.len() * CREEP_SPAWN_TIME as usize
    }
}


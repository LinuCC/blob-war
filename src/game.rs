
use std::collections::HashMap;

use screeps::{OwnedStructureProperties, Room, RoomName};

use crate::constants::MY_USERNAME;

pub enum OwnedBy {
    Me,
    Guy(String),
}

pub fn owned_rooms(player_name: OwnedBy) -> HashMap<RoomName, Room> {
    let username = match player_name {
        OwnedBy::Guy(name) => name,
        OwnedBy::Me => MY_USERNAME.into(),
    };
    let rooms = screeps::game::rooms::hashmap();
    rooms.into_iter()
        .filter(|(_, room)| {
            match room.controller() {
                Some(controller) => controller.owner_name() == Some(username.clone()),
                None => false
            }
        }).collect()
}

use log::warn;
use screeps::{Creep, HasPosition, HasStore, ResourceType, ReturnCode, RoomObjectProperties, SharedCreepProperties, find};

pub fn run_harvester(creep: Creep) {
    if creep.memory().bool("harvesting") {
        if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
            creep.memory().set("harvesting", false);
        }
    } else {
        creep.say("ᕕ( ᐛ )ᕗ", true);
        if creep.store_used_capacity(None) == 0 {
            creep.memory().set("harvesting", true);
        }
    }

    if creep.memory().bool("harvesting") {
        let source = &creep
            .room()
            .expect("room is not visible to you")
            .find(find::SOURCES)[0];
        if creep.pos().is_near_to(source) {
            let r = creep.harvest(source);
            if r != ReturnCode::Ok {
                warn!("couldn't harvest: {:?}", r);
            }
        } else {
            creep.move_to(source);
        }
    } else {
        if let Some(c) = creep
            .room()
            .expect("room is not visible to you")
            .controller()
        {
            let r = creep.upgrade_controller(&c);
            if r == ReturnCode::NotInRange {
                creep.move_to(&c);
            } else if r != ReturnCode::Ok {
                warn!("couldn't upgrade: {:?}", r);
            }
        } else {
            warn!("creep room has no controller!");
        }
    }
}

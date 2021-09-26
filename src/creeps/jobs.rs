use std::collections::HashMap;

use anyhow::{anyhow, bail, Context};
use screeps::{memory::MemoryReference, traits::TryFrom, ObjectId, RoomName, Source};
use serde::{Deserialize, Serialize};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_EXTENSION_ROOM, MEM_JOB_DATA, MEM_JOB_KIND, MEM_TARGET_ROOM},
    utils::ResultOptionExt,
};

// TODO update tasks to use `OokTaskRunnableResult` so that jobs can reassign

#[derive(Debug, Clone)]
pub enum OokCreepJobKind {
    UpgradeController = 1,
    RoomLogistics = 2,
    FarmSource = 3,
    FarmExtensionRoom = 4,
    LogisticsExtensionRoom = 5,
    MaintainStructures = 6,
    ClaimRoom = 7,
    BootstrapRoom = 8,
}

impl TryFrom<i32> for OokCreepJobKind {
    type Error = anyhow::Error;

    fn try_from(i: i32) -> Result<Self, Self::Error> {
        use OokCreepJobKind::*;
        Ok(match i {
            1 => UpgradeController,
            2 => RoomLogistics,
            3 => FarmSource,
            4 => FarmExtensionRoom,
            5 => LogisticsExtensionRoom,
            6 => MaintainStructures,
            7 => ClaimRoom,
            8 => BootstrapRoom,
            _ => bail!("Unknown creep job kind {}", i),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FarmSource {
    pub target_room: RoomName,
    pub target_source: ObjectId<Source>,
}

js_serializable!(FarmSource);
js_deserializable!(FarmSource);

/// Identifies which citizen takes which tasks for himself.
///
/// Higher view on the things a creep does.
/// A job could mean that always the same task is being done, but it could also
/// switch between tasks based on which one is more important.
///
#[derive(Debug, Clone)]
pub enum OokCreepJob {
    UpgradeController {
        target_room: RoomName,
    },
    /// Carry energy between things, etc
    RoomLogistics {
        target_room: RoomName,
    },
    FarmSource(FarmSource),
    FarmExtensionRoom {
        target_room: RoomName,
    },
    LogisticsExtensionRoom {
        extension_room: RoomName,
        target_room: RoomName,
    },
    /// Repair existing stuff, build new structures
    MaintainStructures {
        target_room: RoomName,
    },
    ClaimRoom {
        target_room: RoomName,
    },
    BootstrapRoom {
        target_room: RoomName,
    },
}

impl OokCreepJob {
    pub fn kind(&self) -> OokCreepJobKind {
        match self {
            OokCreepJob::UpgradeController { .. } => OokCreepJobKind::UpgradeController,
            OokCreepJob::RoomLogistics { .. } => OokCreepJobKind::RoomLogistics,
            OokCreepJob::FarmSource(FarmSource { .. }) => OokCreepJobKind::FarmSource,
            OokCreepJob::FarmExtensionRoom { .. } => OokCreepJobKind::FarmExtensionRoom,
            OokCreepJob::LogisticsExtensionRoom { .. } => OokCreepJobKind::LogisticsExtensionRoom,
            OokCreepJob::MaintainStructures { .. } => OokCreepJobKind::MaintainStructures,
            OokCreepJob::ClaimRoom { .. } => OokCreepJobKind::ClaimRoom,
            OokCreepJob::BootstrapRoom { .. } => OokCreepJobKind::BootstrapRoom,
        }
    }

    pub fn target_room(&self) -> RoomName {
        match self {
            OokCreepJob::UpgradeController { target_room, .. } => target_room,
            OokCreepJob::RoomLogistics { target_room, .. } => target_room,
            OokCreepJob::FarmSource(FarmSource { target_room, .. }) => target_room,
            OokCreepJob::FarmExtensionRoom { target_room, .. } => target_room,
            OokCreepJob::LogisticsExtensionRoom { target_room, .. } => target_room,
            OokCreepJob::MaintainStructures { target_room, .. } => target_room,
            OokCreepJob::ClaimRoom { target_room, .. } => target_room,
            OokCreepJob::BootstrapRoom { target_room, .. } => target_room,
        }
        .to_owned()
    }
}

pub trait StorableJob<'a, T> {
    fn to_js_serialize(&'a self) -> HashMap<String, Box<dyn JsSerialize + 'a>>;
    fn from_js_serialize(mem: &MemoryReference) -> anyhow::Result<Option<T>>;
}

impl<'a> StorableJob<'a, Self> for OokCreepJob {
    fn to_js_serialize(&'a self) -> HashMap<String, Box<dyn JsSerialize + 'a>> {
        let mut map: HashMap<String, Box<dyn JsSerialize + 'a>> = HashMap::new();
        map.insert(MEM_JOB_KIND.to_string(), Box::new(self.kind() as i32));
        match self {
            OokCreepJob::UpgradeController { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::RoomLogistics { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::FarmSource(job_data) => {
                map.insert(MEM_JOB_DATA.to_string(), Box::new(job_data));
            }
            OokCreepJob::FarmExtensionRoom { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::LogisticsExtensionRoom {
                extension_room,
                target_room,
            } => {
                map.insert(MEM_EXTENSION_ROOM.to_string(), Box::new(extension_room));
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::MaintainStructures { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::ClaimRoom { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
            OokCreepJob::BootstrapRoom { target_room } => {
                map.insert(MEM_TARGET_ROOM.to_string(), Box::new(target_room));
            }
        }
        map
    }

    fn from_js_serialize(memory: &MemoryReference) -> anyhow::Result<Option<Self>> {
        let job_kind: OokCreepJobKind = OokCreepJobKind::try_from(
            memory
                .i32(MEM_JOB_KIND)
                .context("loading mem job_kind")?
                .ok_or(anyhow!("mem job_kind missing"))?,
        )
        .context("loading mem job_kind")?;

        Ok(match job_kind {
            OokCreepJobKind::UpgradeController => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::UpgradeController { target_room })
            }
            OokCreepJobKind::RoomLogistics => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::RoomLogistics { target_room })
            }
            OokCreepJobKind::FarmSource => {
                let job_data: FarmSource = memory
                    .get(MEM_JOB_DATA)
                    .err_or_none("unable to get job data for farm source")?;
                Some(OokCreepJob::FarmSource(job_data))
            }
            OokCreepJobKind::FarmExtensionRoom => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::FarmExtensionRoom { target_room })
            }
            OokCreepJobKind::LogisticsExtensionRoom => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                let extension_room = RoomName::new(
                    &memory
                        .string(MEM_EXTENSION_ROOM)
                        .context("loading mem extension_room")?
                        .ok_or(anyhow!("mem extension_room missing"))?,
                )
                .context("loading mem extension_room")?;
                Some(OokCreepJob::LogisticsExtensionRoom {
                    target_room,
                    extension_room,
                })
            }
            OokCreepJobKind::MaintainStructures => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::MaintainStructures { target_room })
            }
            OokCreepJobKind::ClaimRoom => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::ClaimRoom { target_room })
            }
            OokCreepJobKind::BootstrapRoom => {
                let target_room = RoomName::new(
                    &memory
                        .string(MEM_TARGET_ROOM)
                        .context("loading mem target_room")?
                        .ok_or(anyhow!("mem target_room missing"))?,
                )
                .context("loading mem target_room")?;
                Some(OokCreepJob::BootstrapRoom { target_room })
            }
        })
    }
}

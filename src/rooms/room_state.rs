pub mod base;
pub mod setup_base;

use std::cmp;
use std::collections::HashMap;

use crate::creeps::jobs::{FarmSource, OokCreepJob};
use crate::creeps::races::carrier::{OokCreepCarrier, TrySpawnCarrierOptions};
use crate::creeps::races::worker::{OokCreepWorker, TrySpawnWorkerOptions};
use crate::creeps::races::{OokRaceBodyComposition, OokRaceKind};
use crate::creeps::{Spawnable, TrySpawnOptions, TrySpawnResult, TrySpawnResultData};
use crate::state::requests::{self, Request, RequestData};
use crate::state::{RequestHandledOpts, UniqId};
use crate::utils::AnyhowOptionExt;
use anyhow::{anyhow, bail, Context};
use log::{info, warn};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use screeps::game::rooms;
use screeps::{find, HasId, ReturnCode, Room};
use screeps::{memory::MemoryReference, RoomName};
use serde::{Deserialize, Serialize};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_OOK_ROOMS, MEM_OOK_ROOMS_DATA, MEM_ROOM_STATE_KIND},
    game::{owned_rooms, OwnedBy},
    rooms::room_state::base::BaseState,
    state::BWState,
    utils::ResultOptionExt,
};

pub use self::setup_base::{SetupBaseState, SetupBaseStateVisibility};

use super::resource_provider::ResourceProvider;

#[derive(thiserror::Error, Debug)]
pub enum RoomStateError {
    #[error("[RoomStateError] MyRoomNotFound not found {0}")]
    MyRoomNotFound(String),
}

pub trait RoomStateLifecycle<T> {
    fn new(room_name: RoomName) -> anyhow::Result<T>;
    /// Checks the events happening in this room
    ///
    /// Returns the requests that were done
    fn handle_events(&mut self, state: &mut BWState) -> anyhow::Result<Vec<Request>>;

    /// Handles stuff
    fn run(&self, state: &BWState) -> anyhow::Result<Vec<Request>>;

    /// Checks and updates itself
    fn update(
        &mut self,
        handled_requests: &HashMap<u32, HashMap<UniqId, Request>>,
    ) -> anyhow::Result<RoomStateChange>;

    fn request_logged(&mut self, request_id: UniqId);
}

pub trait RoomStatePersistable<T> {
    fn to_memory(&self) -> anyhow::Result<HashMap<String, Box<dyn JsSerialize>>>;
    fn load_from_memory(memory: &MemoryReference) -> anyhow::Result<T>;
    fn update_from_memory(&mut self, memory: &MemoryReference) -> anyhow::Result<()>;
}

#[derive(Clone, Debug, FromPrimitive, ToPrimitive)]
pub enum RoomStateKind {
    Base = 0,
    SetupBase = 1,
}

#[derive(Clone, Debug)]
pub enum RoomState {
    Base(BaseState),
    SetupBase(SetupBaseState),
    // Extension(SetupBaseState),
}

pub enum RoomStateChange {
    /// Room has finished `SetupBase`
    FinishSetup,
    /// Room needs help to setup itself
    Helpless,
    None,
}

impl RoomState {
    pub fn room_name(&self) -> RoomName {
        match self {
            RoomState::Base(state) => state.room_name,
            RoomState::SetupBase(state) => state.room_name,
        }
    }

    pub fn resource_provider(&self, id: &str) -> Option<&ResourceProvider> {
        match self {
            RoomState::Base(state) => state.resource_providers.get(id),
            RoomState::SetupBase(state) => match state.state {
                SetupBaseStateVisibility::Visible {
                    ref resource_providers,
                    ..
                } => resource_providers.get(id),
                _ => None,
            }, // RoomState::OwnBootstrapping(state) => state.resource_providers.get(id),
        }
    }
}

impl RoomStatePersistable<Self> for RoomState {
    fn to_memory(&self) -> anyhow::Result<HashMap<String, Box<dyn JsSerialize>>> {
        match self {
            RoomState::Base(state) => state.to_memory(),
            RoomState::SetupBase(state) => state.to_memory(),
        }
    }

    fn load_from_memory(memory: &MemoryReference) -> anyhow::Result<RoomState> {
        let state_kind = memory
            .i32(MEM_ROOM_STATE_KIND)
            .context("loading mem room_state_kind")?
            .ok_or(anyhow!("missing mem room_state_kind"))?;
        Ok(
            match RoomStateKind::from_i32(state_kind).anyhow("load from mem: room missing")? {
                RoomStateKind::Base => RoomState::Base(BaseState::load_from_memory(memory)?),
                RoomStateKind::SetupBase => {
                    RoomState::SetupBase(SetupBaseState::load_from_memory(memory)?)
                }
            },
        )
    }

    fn update_from_memory(&mut self, memory: &MemoryReference) -> anyhow::Result<()> {
        match self {
            RoomState::Base(state) => {
                state.update_from_memory(memory)?;
            }
            RoomState::SetupBase(state) => {
                state.update_from_memory(memory)?;
            }
        }
        Ok(())
    }
}

pub fn legacy_init_room_states() -> anyhow::Result<HashMap<RoomName, RoomState>> {
    warn!("Legacy room handling!!");
    let mut room_states: HashMap<RoomName, RoomState> = HashMap::new();
    let owned_rooms = owned_rooms(OwnedBy::Me);
    for (name, room) in owned_rooms.iter() {
        let state = BaseState::new(room.name());
        match state {
            Ok(state) => {
                room_states.insert(*name, RoomState::Base(state));
            }
            Err(err) => {
                warn!("Unable to init room state for {}: {}", name, err);
            }
        };
    }
    Ok(room_states)
}

pub fn init_room_states() -> anyhow::Result<HashMap<RoomName, RoomState>> {
    let mut room_states: HashMap<RoomName, RoomState> = HashMap::new();
    match screeps::memory::root().dict(MEM_OOK_ROOMS_DATA) {
        Ok(Some(data)) => {
            let rooms = data
                .dict(MEM_OOK_ROOMS)
                .err_or_none("rooms dict missing".to_string())?;
            for room_name in rooms.keys() {
                let room = rooms
                    .get(&room_name)
                    .err_or_none("wtf, could not get room for key")?;
                match RoomState::load_from_memory(&room) {
                    Ok(room_state) => {
                        room_states.insert(room_state.room_name(), room_state);
                    }
                    Err(err) => warn!("Could not load mem for room: {}", err),
                }
            }
        }
        _ => {
            warn!("Failed loading rooms mem");
            return legacy_init_room_states();
        }
    }
    Ok(room_states)
}

/// If I manually changed something in the memory, it should be taken into account
pub fn update_room_states_from_memory(state: &mut BWState) -> anyhow::Result<()> {
    let room_states = &mut state.room_states;
    let rooms_data = screeps::memory::root()
        .dict_or_create(MEM_OOK_ROOMS_DATA)
        .map_err(|e| anyhow!("Could not get mem rooms_state: {}", e))?
        .dict_or_create(MEM_OOK_ROOMS)
        .map_err(|e| anyhow!("Could not get mem rooms: {}", e))?;
    for (room_name, room_state) in room_states {
        match rooms_data.get(&room_name.to_string()) {
            Ok(Some(mem)) => {
                room_state.update_from_memory(&mem)?;
            }
            Ok(None) => {
                warn!("Missing gettign rooms data for {}", room_name);
            }
            Err(err) => {
                warn!("Error gettign rooms data: {}", err);
            }
        }
    }
    Ok(())
}

pub fn persist_room_states(state: &BWState) -> anyhow::Result<()> {
    let mut new_rooms: HashMap<RoomName, Box<HashMap<String, Box<dyn JsSerialize>>>> =
        HashMap::new();
    let room_states = &state.room_states;
    let rooms_data = screeps::memory::root()
        .dict_or_create(MEM_OOK_ROOMS_DATA)
        .map_err(|e| anyhow!("Could not get mem rooms_state: {}", e))?;

    for (room_name, room_state) in room_states {
        // if *room_name == RoomName::new("W11N16")? {
        //     continue;
        // }

        let mem = room_state.to_memory();
        match mem {
            Ok(mem) => {
                // let x = mem.iter()
                //     .map(|(i, v)| (i.clone(), &**v))
                //     .collect::<HashMap<String, &dyn JsSerialize>>();
                new_rooms.insert(room_name.to_owned(), Box::new(mem));
            }
            Err(_) => {
                warn!("Could not create data to room {}", room_name);
            }
        }
    }
    rooms_data.set(
        MEM_OOK_ROOMS,
        new_rooms
            .iter()
            .map(|(room_name, room_data)| {
                (
                    room_name.to_string(),
                    room_data
                        .iter()
                        .map(|(i, v)| (i.clone(), &**v))
                        .collect::<HashMap<String, &dyn JsSerialize>>(),
                )
            })
            .collect::<HashMap<String, HashMap<String, &dyn JsSerialize>>>(),
    );
    Ok(())
}

// pub enum AssignedRequest { }

// NOTE Perhaps, instead of returning a HashMap, use an enum so requests can be handled by
//   something else than rooms?
// NOTE Later on we might want to handle _all_ spawning with these requests
pub fn assign_requests(state: &mut BWState) -> anyhow::Result<HashMap<RoomName, Request>> {
    let mut request_handlers: HashMap<RoomName, Request> = HashMap::new();
    for (id, request) in &state.requests {
        match request {
            Request {
                data:
                    RequestData::BootstrapWorkerCitizen(requests::BootstrapWorkerCitizen {
                        target_room_name,
                        ..
                    }),
                ..
            } => {
                if let Some(target_room) = state.room_states.get(target_room_name) {
                    match target_room {
                        RoomState::Base(room_state) => {
                            request_handlers.insert(room_state.room_name, request.to_owned());
                        }
                        RoomState::SetupBase(room_state) => {
                            // TODO use the get_helping_room_for_request from below if we
                            //   cant spawn the creeps we need
                            request_handlers.insert(room_state.room_name, request.to_owned());
                        }
                    }
                } else {
                    // We dont see the room, so there's nothing in there from us, so it needs help
                    // from another room
                    match get_helping_room_for_request(state, request) {
                        Ok(Some(closest_room)) => {
                            request_handlers.insert(closest_room, request.to_owned());
                        }
                        Ok(None) => {}
                        Err(err) => {
                            warn!("error get_helping_room_for_request: {}", err);
                        }
                    }
                }
            }
            Request {
                data:
                    RequestData::Citizen(requests::Citizen {
                        target_room_name, ..
                    }),
                ..
            } => {
                if let Some(target_room) = state.room_states.get(target_room_name) {
                    match target_room {
                        RoomState::Base(room_state) => {
                            request_handlers.insert(room_state.room_name, request.to_owned());
                        }
                        RoomState::SetupBase(room_state) => {
                            // HACK just use a regular vec instead or prioritize correctly
                            if let Some(rh) = request_handlers.get(&room_state.room_name) {
                                let existing_important = match &rh.data {
                                    RequestData::Citizen(data) => {
                                        data.resolve_panic
                                    },
                                    _ => false,
                                };
                                if !existing_important {
                                    request_handlers.insert(room_state.room_name, request.to_owned());
                                }
                            } else {
                                request_handlers.insert(room_state.room_name, request.to_owned());
                            }
                        }
                    }
                } else {
                    warn!("room for room_state {} is invisible ayy", target_room_name);
                }
            }
        }
    }
    Ok(request_handlers)
}

fn get_helping_room_for_request(
    state: &BWState,
    request: &Request,
) -> anyhow::Result<Option<RoomName>> {
    match request {
        Request {
            data:
                RequestData::BootstrapWorkerCitizen(requests::BootstrapWorkerCitizen {
                    target_room_name,
                    ..
                }),
            ..
        } => {
            let mut rooms_able_to_help: Vec<RoomName> = state
                .room_states
                .iter()
                .filter_map(|(room_name, state)| {
                    match state {
                        // TODO Only get out "free" bases (those that are not currently
                        //   preoccupied spawning stuff
                        RoomState::Base(_) => Some(*room_name),
                        RoomState::SetupBase(_) => None,
                    }
                })
                .collect();
            rooms_able_to_help.sort_unstable_by_key(|&a| {
                let (x_diff, y_diff) = *target_room_name - a;
                let linear_len = ((x_diff * x_diff + y_diff * y_diff) as f32).sqrt().round() as i32;
                linear_len
            });
            Ok(rooms_able_to_help.first().map(|r| r.to_owned()))
        }
        Request {
            data: RequestData::Citizen(requests::Citizen { .. }),
            ..
        } => Ok(None),
    }
}

pub fn dummy_handle_requests(
    state: &mut BWState,
    requests: HashMap<RoomName, Request>,
) -> anyhow::Result<()> {
    for (room_name, request) in requests {
        match &request {
            Request {
                request_id,
                data: RequestData::BootstrapWorkerCitizen(request_data),
            } => {
                let source_room = rooms::get(room_name);
                if let Some(source_room) = source_room {
                    let room_energy = source_room.energy_available();
                    let target_spawn_energy: u32 = source_room.energy_capacity_available();

                    match OokCreepWorker::try_spawn(
                        &TrySpawnOptions {
                            assumed_job: OokCreepJob::BootstrapRoom {
                                target_room: request_data.target_room_name.to_owned(),
                            },
                            available_spawns: source_room
                                .find(find::MY_SPAWNS)
                                .iter()
                                .map(|s| s.id())
                                .collect(),
                            force_spawn: false,
                            race: OokRaceKind::Worker,
                            spawn_room: &source_room,
                            target_energy_usage: target_spawn_energy,
                            request_id: Some(request_id.to_owned()),
                            preset_parts: None,
                        },
                        &TrySpawnWorkerOptions {
                            post_ident: "XXX".into(),
                            base_room: request_data.target_room_name.to_owned(),
                        },
                    ) {
                        Ok(TrySpawnResult::Spawned(TrySpawnResultData {
                            return_code: ReturnCode::Ok,
                            creep_name,
                            ..
                        })) => {
                            let mut request_data = request_data.to_owned();
                            request_data.spawning_creep_name = Some(creep_name);
                            let request = Request {
                                request_id: request_id.to_owned(),
                                data: RequestData::BootstrapWorkerCitizen(request_data),
                            };
                            state.request_handled(
                                request,
                                RequestHandledOpts::DelayHandleForOneTick,
                            )?;
                        }
                        Ok(TrySpawnResult::ForceSpawned(TrySpawnResultData {
                            return_code: ReturnCode::Ok,
                            creep_name,
                            ..
                        })) => {
                            let mut request_data = request_data.to_owned();
                            request_data.spawning_creep_name = Some(creep_name);
                            let request = Request {
                                request_id: request_id.to_owned(),
                                data: RequestData::BootstrapWorkerCitizen(request_data),
                            };
                            state.request_handled(
                                request,
                                RequestHandledOpts::DelayHandleForOneTick,
                            )?;
                        }
                        Ok(_) => {
                            info!("Could not spawn for request {:?}", request);
                        }
                        Err(err) => warn!("err hurrdurur {}", err),
                    }
                } else {
                    warn!(
                        "Could not fulfill request {:?} cuz room {} is not visible",
                        request, room_name
                    );
                }
            }
            Request {
                request_id,
                data: RequestData::Citizen(request_data),
            } => {
                let source_room = rooms::get(room_name);
                if let Some(source_room) = source_room {
                    match spawn_citizen(&source_room, request_id.to_owned(), request_data) {
                        Ok(TrySpawnResult::Spawned(TrySpawnResultData {
                            return_code: ReturnCode::Ok,
                            creep_name,
                            ..
                        })) => {
                            let mut request_data = request_data.to_owned();
                            request_data.spawning_creep_name = Some(creep_name);
                            let request = Request {
                                request_id: request_id.to_owned(),
                                data: RequestData::Citizen(request_data),
                            };
                            state.request_handled(
                                request,
                                RequestHandledOpts::DelayHandleForOneTick,
                            )?;
                        }
                        Ok(TrySpawnResult::ForceSpawned(TrySpawnResultData {
                            return_code: ReturnCode::Ok,
                            creep_name,
                            ..
                        })) => {
                            let mut request_data = request_data.to_owned();
                            request_data.spawning_creep_name = Some(creep_name);
                            let request = Request {
                                request_id: request_id.to_owned(),
                                data: RequestData::Citizen(request_data),
                            };
                            state.request_handled(
                                request,
                                RequestHandledOpts::DelayHandleForOneTick,
                            )?;
                        }
                        Ok(TrySpawnResult::Skipped) => {}
                        Ok(_) => {
                            info!("Could not spawn for request {:?}", request);
                        }
                        Err(err) => {
                            warn!(
                                "Error spawning citizen for request {} : {}",
                                request_id, err
                            );
                        }
                    }
                } else {
                    warn!(
                        "Could not fulfill request {:?} cuz room {} is not visible",
                        request, room_name
                    );
                }
            }
        }
    }
    Ok(())
}

fn spawn_citizen(
    source_room: &Room,
    request_id: UniqId,
    request_data: &requests::Citizen,
) -> anyhow::Result<TrySpawnResult> {
    let room_energy = source_room.energy_available();
    let mut target_spawn_energy: u32 = source_room.energy_capacity_available();
    if request_data.resolve_panic {
        target_spawn_energy = cmp::max(room_energy, 300);
    }
    let (race_kind, parts) = if let Some(spawn_data) =
        creep_spawn_options_from_job(&request_data.initial_job, target_spawn_energy)?
    {
        spawn_data
    } else {
        // Not enough energy
        return Ok(TrySpawnResult::Skipped);
    };

    match race_kind {
        OokRaceKind::Worker => OokCreepWorker::try_spawn(
            &TrySpawnOptions {
                assumed_job: request_data.initial_job.to_owned(),
                available_spawns: source_room
                    .find(find::MY_SPAWNS)
                    .iter()
                    .map(|s| s.id())
                    .collect(),
                force_spawn: false,
                race: race_kind,
                spawn_room: &source_room,
                target_energy_usage: target_spawn_energy,
                request_id: Some(request_id.to_owned()),
                preset_parts: Some(parts),
            },
            &TrySpawnWorkerOptions {
                post_ident: "XXX".into(),
                base_room: request_data.target_room_name.to_owned(),
            },
        ),
        OokRaceKind::StaticWorker => {
            bail!("TODO spawn_citizen does not handle {:?} yet", race_kind)
        }
        OokRaceKind::Carrier => OokCreepCarrier::try_spawn(
            &TrySpawnOptions {
                assumed_job: request_data.initial_job.to_owned(),
                available_spawns: source_room
                    .find(find::MY_SPAWNS)
                    .iter()
                    .map(|s| s.id())
                    .collect(),
                force_spawn: false,
                race: race_kind,
                spawn_room: &source_room,
                target_energy_usage: target_spawn_energy,
                request_id: Some(request_id.to_owned()),
                // TODO Check if room has it covered with roads and adjust move parts to that
                preset_parts: Some(parts),
            },
            &TrySpawnCarrierOptions {
                post_ident: "XXX".into(),
                base_room: request_data.target_room_name.to_owned(),
            },
        ),
        OokRaceKind::Attacker => bail!("TODO spawn_citizen does not handle {:?} yet", race_kind),
        OokRaceKind::CloseCombatDefender => {
            bail!("TODO spawn_citizen does not handle {:?} yet", race_kind)
        }
        OokRaceKind::Claimer => bail!("TODO spawn_citizen does not handle {:?} yet", race_kind),
    }
}

fn creep_spawn_options_from_job(
    job: &OokCreepJob,
    target_energy_usage: u32,
) -> anyhow::Result<Option<(OokRaceKind, Vec<screeps::Part>)>> {
    match job {
        OokCreepJob::UpgradeController { .. } => {
            // TODO check for roads to improve comp
            // TODO check for container / link to improve comp
            // TODO better composition handling, see OokRace::Worker, different
            // target_energy_usage should use different kinds of composition
            let comp = OokRaceBodyComposition {
                mov: 1,
                carry: 1,
                work: 1,
                attack: 0,
                ranged_attack: 0,
                heal: 0,
                tough: 0,
                claim: 0,
            }
            .parts_for_x_energy(target_energy_usage);
            if let Some((parts, _energy)) = comp {
                Ok(Some((OokRaceKind::Worker, parts)))
            } else {
                Ok(None)
            }
        }
        OokCreepJob::RoomLogistics { .. } => {
            let comp = OokRaceBodyComposition {
                mov: 1,
                carry: 2,
                work: 0,
                attack: 0,
                ranged_attack: 0,
                heal: 0,
                tough: 0,
                claim: 0,
            }
            .parts_for_x_energy(target_energy_usage);
            if let Some((parts, _energy)) = comp {
                Ok(Some((OokRaceKind::Carrier, parts)))
            } else {
                Ok(None)
            }
        }
        OokCreepJob::FarmSource(FarmSource { .. }) => {
            // TODO check for roads to improve comp
            // TODO check for container / link to improve comp
            // TODO better composition handling, see OokRace::Worker, different
            // target_energy_usage should use different kinds of composition
            let limit_work = cmp::min(target_energy_usage, 900);
            let comp = OokRaceBodyComposition {
                mov: 1,
                carry: 0,
                work: 2,
                attack: 0,
                ranged_attack: 0,
                heal: 0,
                tough: 0,
                claim: 0,
            }
            .parts_for_x_energy(limit_work);
            if let Some((parts, _energy)) = comp {
                Ok(Some((OokRaceKind::Worker, parts)))
            } else {
                Ok(None)
            }
        }
        OokCreepJob::FarmExtensionRoom { .. } => {
            bail!("Unhandled job to create spawn options {:?}", job)
        }
        OokCreepJob::LogisticsExtensionRoom { .. } => {
            bail!("Unhandled job to create spawn options {:?}", job)
        }
        OokCreepJob::MaintainStructures { .. } => {
            bail!("Unhandled job to create spawn options {:?}", job)
        }
        OokCreepJob::ClaimRoom { .. } => bail!("Unhandled job to create spawn options {:?}", job),
        OokCreepJob::BootstrapRoom { .. } => {
            // TODO check for roads to improve comp
            // TODO check for container / link to improve comp
            // TODO better composition handling, see OokRace::Worker, different
            // target_energy_usage should use different kinds of composition
            let comp = OokRaceBodyComposition {
                mov: 1,
                carry: 1,
                work: 1,
                attack: 0,
                ranged_attack: 0,
                heal: 0,
                tough: 0,
                claim: 0,
            }
            .parts_for_x_energy(target_energy_usage);
            if let Some((parts, _energy)) = comp {
                Ok(Some((OokRaceKind::Worker, parts)))
            } else {
                Ok(None)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetSpawns {
    carrier: u8,
    #[deprecated]
    farmer: u8,
    worker: u8,
}

#[derive(Clone, Debug)]
pub enum TargetSpawnKind {
    Carrier = 0,
    Farmer = 1,
    Worker = 2,
}

/// Should only be used if you pass the initial job
impl From<OokCreepJob> for TargetSpawnKind {
    fn from(job: OokCreepJob) -> Self {
        match job {
            OokCreepJob::UpgradeController { .. } => TargetSpawnKind::Worker,
            OokCreepJob::RoomLogistics { .. } => TargetSpawnKind::Carrier,
            OokCreepJob::FarmSource(FarmSource { .. }) => TargetSpawnKind::Farmer,
            OokCreepJob::FarmExtensionRoom { .. } => TargetSpawnKind::Farmer,
            OokCreepJob::LogisticsExtensionRoom { .. } => TargetSpawnKind::Carrier,
            OokCreepJob::MaintainStructures { .. } => TargetSpawnKind::Worker,
            OokCreepJob::ClaimRoom { .. } => TargetSpawnKind::Worker, // TODO
            OokCreepJob::BootstrapRoom { .. } => TargetSpawnKind::Worker,
        }
    }
}

/// Should only be used if you pass the initial job
impl From<&OokCreepJob> for TargetSpawnKind {
    fn from(job: &OokCreepJob) -> Self {
        match job {
            OokCreepJob::UpgradeController { .. } => TargetSpawnKind::Worker,
            OokCreepJob::RoomLogistics { .. } => TargetSpawnKind::Carrier,
            OokCreepJob::FarmSource(FarmSource { .. }) => TargetSpawnKind::Farmer,
            OokCreepJob::FarmExtensionRoom { .. } => TargetSpawnKind::Farmer,
            OokCreepJob::LogisticsExtensionRoom { .. } => TargetSpawnKind::Carrier,
            OokCreepJob::MaintainStructures { .. } => TargetSpawnKind::Worker,
            OokCreepJob::ClaimRoom { .. } => TargetSpawnKind::Worker, // TODO
            OokCreepJob::BootstrapRoom { .. } => TargetSpawnKind::Worker,
        }
    }
}

js_serializable!(TargetSpawns);
js_deserializable!(TargetSpawns);

impl Default for TargetSpawns {
    fn default() -> Self {
        TargetSpawns {
            farmer: 0,
            worker: 0,
            carrier: 0,
        }
    }
}

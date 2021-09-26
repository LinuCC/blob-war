use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
};

use anyhow::{anyhow, bail, Context};

use log::{info, warn};
use screeps::{
    find,
    game::{self, creeps, get_object_typed, rooms},
    memory::MemoryReference,
    Creep, EventType, HasId, HasStore, ObjectId, ResourceType, RoomName, Source, Structure,
    StructureTower,
};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_BASE_DATA, MEM_ROOM_NAME, MEM_ROOM_STATE_KIND},
    creeps::{
        get_prio_repair_target,
        jobs::{self, OokCreepJob},
        races::{worker::OokCreepWorker, OokRace, RepresentsCreep},
        RepairTarget,
    },
    rooms::room_state::TargetSpawns,
    state::{
        requests::{self, Request, RequestData},
        BWState, UniqId,
    },
    utils::AnyhowOptionExt,
};

use super::{
    super::{
        resource_provider::{calc_resource_providers, ResourceProvider},
        room_state::{RoomStateKind, RoomStateLifecycle, RoomStatePersistable},
    },
    RoomStateChange, TargetSpawnKind,
};

const PANIC_THRESHOLD_TICKS: u32 = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetupBaseData {
    pub helping_citizens: Vec<ObjectId<Creep>>,
    pub target_spawns: TargetSpawns,
}

js_serializable!(SetupBaseData);
js_deserializable!(SetupBaseData);

impl Default for SetupBaseData {
    fn default() -> Self {
        SetupBaseData {
            helping_citizens: vec![],
            target_spawns: Default::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SetupBaseState {
    pub room_name: RoomName,

    pub state: SetupBaseStateVisibility,

    /// Data that also gets persisted
    pub data: SetupBaseData,

    /// Stores all open requests from this room so it doesnt request things twice
    open_requests: Vec<UniqId>,

    pub panic_countdown: Option<u32>,
}

impl SetupBaseState {
    fn spawn_citizens_up_to_target(&self, state: &BWState) -> anyhow::Result<Vec<Request>> {
        let mut requests: Vec<Request> = vec![];

        let mut current_spawns = TargetSpawns {
            farmer: 0,
            worker: 0,
            carrier: 0,
        };
        let mut unhandled_sources: HashSet<ObjectId<Source>> =
            if let SetupBaseStateVisibility::Visible { sources, .. } = &self.state {
                sources.iter().cloned().collect()
            } else {
                HashSet::new()
            };
        for id in &self.data.helping_citizens {
            match state.citizens.get(id) {
                Some(OokRace::Worker(OokCreepWorker {
                    job: OokCreepJob::FarmSource(jobs::FarmSource { target_source, .. }),
                    ..
                })) => {
                    unhandled_sources.remove(target_source);
                }
                Some(OokRace::Worker(_)) => current_spawns.worker += 1,
                Some(OokRace::Claimer(_)) => {}
                Some(OokRace::Carrier(_)) => current_spawns.carrier += 1,
                None => {
                    warn!("Missing citizen for helping citizen {}", id);
                }
            }
        }

        let mut open_request_spawns = TargetSpawns {
            farmer: 0,
            worker: 0,
            carrier: 0,
        };
        let open_requests: Vec<Request> = self
            .open_requests
            .iter()
            .filter_map(|req| {
                state
                    .get_current_or_old_request(req.to_owned())
                    .map(|(req, _id)| req)
            })
            .collect();
        for open_request in &open_requests {
            match &open_request.data {
                RequestData::BootstrapWorkerCitizen(requests::BootstrapWorkerCitizen {
                    target_room_name,
                    ..
                }) => {
                    if *target_room_name == self.room_name {
                        open_request_spawns.worker += 1;
                    }
                }
                RequestData::Citizen(requests::Citizen {
                    target_room_name,
                    initial_job,
                    ..
                }) => {
                    if let OokCreepJob::FarmSource(jobs::FarmSource { target_source, .. }, ..) =
                        initial_job
                    {
                        unhandled_sources.remove(target_source);
                    } else if *target_room_name == self.room_name {
                        match TargetSpawnKind::from(initial_job) {
                            TargetSpawnKind::Carrier => open_request_spawns.carrier += 1,
                            TargetSpawnKind::Farmer => {}
                            TargetSpawnKind::Worker => open_request_spawns.worker += 1,
                        }
                    }
                }
            }
        }

        if self.panicing() {
            requests.extend(self.spawn_panicing_citizens(state, &open_requests)?);
        }

        // TODO Fix Bootstrap worker

        // Prioritize requests; if we have open requests for farmers / carriers, dont start
        // to spawn workers
        for unhandled_source in &unhandled_sources {
            let target_room_name = self.room_name;
            let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                target_room_name,
                spawning_creep_name: None,
                initial_job: OokCreepJob::FarmSource(jobs::FarmSource {
                    target_room: target_room_name,
                    target_source: unhandled_source.clone(),
                }),
                resolve_panic: false,
            }));
            requests.push(new_request);
        }
        if unhandled_sources.len() == 0 && requests.len() == 0 {
            if current_spawns.carrier + open_request_spawns.carrier
                < self.data.target_spawns.carrier
            {
                let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                    target_room_name: self.room_name,
                    spawning_creep_name: None,
                    initial_job: OokCreepJob::RoomLogistics {
                        target_room: self.room_name,
                    },
                    resolve_panic: false,
                }));
                requests.push(new_request);
            }
        }
        if unhandled_sources.len() == 0 && open_request_spawns.carrier == 0 && requests.len() == 0 {
            if current_spawns.worker + open_request_spawns.worker < self.data.target_spawns.worker {
                let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                    target_room_name: self.room_name,
                    spawning_creep_name: None,
                    initial_job: OokCreepJob::BootstrapRoom {
                        target_room: self.room_name,
                    },
                    resolve_panic: false,
                }));
                requests.push(new_request);
            }
        }


        Ok(requests)
    }

    fn spawn_panicing_citizens(
        &self,
        state: &BWState,
        open_requests: &Vec<Request>,
    ) -> anyhow::Result<Vec<Request>> {
        let mut requests: Vec<Request> = vec![];
        let mut have_farmer = false;
        let mut have_carrier = false;

        for id in &self.data.helping_citizens {
            match state.citizens.get(id) {
                Some(OokRace::Worker(OokCreepWorker {
                    job: OokCreepJob::FarmSource(jobs::FarmSource { .. }),
                    ..
                })) => {
                    have_farmer = true;
                }
                Some(OokRace::Carrier(_)) => have_carrier = true,
                Some(_) => {}
                None => {
                    warn!("Missing citizen for helping citizen {}", id);
                }
            }
        }
        for open_request in open_requests {
            match &open_request.data {
                RequestData::Citizen(requests::Citizen {
                    target_room_name,
                    initial_job,
                    resolve_panic: true,
                    ..
                }) => {
                    if let OokCreepJob::FarmSource(jobs::FarmSource { .. }, ..) = initial_job {
                        have_farmer = true;
                    } else if *target_room_name == self.room_name {
                        match TargetSpawnKind::from(initial_job) {
                            TargetSpawnKind::Carrier => have_carrier = true,
                            TargetSpawnKind::Farmer => {}
                            TargetSpawnKind::Worker => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let target_room_name = self.room_name;
        if !have_farmer {
            if let SetupBaseStateVisibility::Visible { sources, .. } = &self.state {
                if let Some(source) = sources.first() {
                    let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                        target_room_name,
                        spawning_creep_name: None,
                        initial_job: OokCreepJob::FarmSource(jobs::FarmSource {
                            target_room: target_room_name,
                            /// TODO closest source to spawn?
                            target_source: *source,
                        }),
                        resolve_panic: true,
                    }));
                    requests.push(new_request);
                }
            }
        }
        if !have_carrier {
            let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                target_room_name: self.room_name,
                spawning_creep_name: None,
                initial_job: OokCreepJob::RoomLogistics {
                    target_room: self.room_name,
                },
                resolve_panic: true,
            }));
            requests.push(new_request);
        }
        Ok(requests)
    }

    fn handle_towers(&self) -> anyhow::Result<()> {
        let room = rooms::get(self.room_name).anyhow("handle_towers room not found")?;
        let structures = room.find(find::STRUCTURES);
        let towers: Vec<StructureTower> = structures
            .into_iter()
            .filter_map(|s| match s {
                Structure::Tower(t) => {
                    if t.store_used_capacity(Some(ResourceType::Energy)) > 500 {
                        Some(t)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        let enemies = room.find(find::HOSTILE_CREEPS);
        if enemies.len() > 0 {
            let structures = room.find(find::STRUCTURES);
            let towers: Vec<StructureTower> = structures
                .into_iter()
                .filter_map(|s| match s {
                    Structure::Tower(t) => {
                        if t.store_used_capacity(Some(ResourceType::Energy)) > 0 {
                            Some(t)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect();
            info!("tw {:?}", towers.len());
            if let Some(target) = enemies.first() {
                for (i, tower) in towers.iter().enumerate() {
                    if i == 0 {
                        if let Some(target) = enemies.last() {
                            tower.attack(target);
                        }
                    } else {
                        tower.attack(target);
                    }
                    warn!("Attacking {}", target.id());
                }
            }
        } else {
            match get_prio_repair_target(&room) {
                Ok(Some(RepairTarget::Important { target })) => towers.iter().for_each(|t| {
                    t.repair(&target);
                }),
                _ => {}
            }
        }

        Ok(())
    }

    pub fn check_room_status(
        &mut self,
        all_citizens: &HashMap<ObjectId<Creep>, OokRace>,
    ) -> anyhow::Result<()> {
        let mut farmer_exist = false;
        let mut runner_exist = false;
        for id in &self.data.helping_citizens {
            match all_citizens.get(id) {
                Some(OokRace::Worker(OokCreepWorker {
                    job: OokCreepJob::FarmSource(jobs::FarmSource { target_source, .. }),
                    ..
                })) => {
                    farmer_exist = true;
                }
                Some(OokRace::Carrier(_)) => runner_exist = true,
                Some(_) => {}
                None => {
                    warn!("Missing citizen for helping citizen {}", id);
                }
            }
        }
        if let Some(panic_countdown) = &mut self.panic_countdown {
            if farmer_exist && runner_exist {
                self.panic_countdown = None;
            } else {
                if *panic_countdown <= PANIC_THRESHOLD_TICKS {
                    info!("Increasing panic counter {}", panic_countdown);
                    *panic_countdown += 1;
                }
            }
        } else {
            if !farmer_exist || !runner_exist {
                self.panic_countdown = Some(1);
            }
        }
        Ok(())
    }

    fn panicing(&self) -> bool {
        if let Some(panic_countdown) = self.panic_countdown {
            warn!("Panicing in room {}", self.room_name);
            panic_countdown > PANIC_THRESHOLD_TICKS
        } else {
            false
        }
    }
}

impl RoomStateLifecycle<SetupBaseState> for SetupBaseState {
    fn new(room_name: RoomName) -> anyhow::Result<SetupBaseState> {
        match rooms::get(room_name) {
            Some(room) => {
                let resource_providers: HashMap<_, _> = calc_resource_providers(&room)?
                    .into_iter()
                    .map(|prov| (prov.ident(), prov))
                    .collect();
                Ok(SetupBaseState {
                    room_name: room.name(),
                    state: SetupBaseStateVisibility::Visible {
                        resource_providers,
                        sources: room
                            .find(find::SOURCES)
                            .into_iter()
                            .map(|s| s.id())
                            .collect(),
                    },
                    open_requests: Default::default(),
                    data: Default::default(),
                    panic_countdown: None,
                })
            }
            None => Ok(SetupBaseState {
                room_name,
                state: SetupBaseStateVisibility::NotVisible {},
                open_requests: Default::default(),
                data: Default::default(),
                panic_countdown: None,
            }),
        }
    }

    // TODO use dis
    fn handle_events(&mut self, state: &mut BWState) -> anyhow::Result<Vec<Request>> {
        if let Some(room) = rooms::get(self.room_name) {
            for event in room.get_event_log() {
                let object_id = event.object_id;
                match event.event {
                    EventType::Attack(_) => {}
                    EventType::ObjectDestroyed(ev) => {
                        warn!("Object destroyed: id: {:?} // {:?}", object_id, ev);
                    }
                    EventType::AttackController => {}
                    EventType::Build(_) => {}
                    EventType::Harvest(_) => {}
                    EventType::Heal(_) => {}
                    EventType::Repair(_) => {}
                    EventType::ReserveController(_) => {}
                    EventType::UpgradeController(_) => {}
                    EventType::Exit(_) => {}
                    EventType::Power(_) => {}
                    EventType::Transfer(_) => {}
                }
            }
        }
        todo!()
    }

    fn run(&self, state: &BWState) -> anyhow::Result<Vec<Request>> {
        if let Err(err) = self.handle_towers() {
            warn!("Error executing handle_towers: {}", err);
        }
        let spawn_requests = match self.spawn_citizens_up_to_target(state) {
            Ok(spawn_requests) => spawn_requests,
            Err(err) => {
                warn!(
                    "Unable to create spawn citizen requests for room '{}': {}",
                    self.room_name, err
                );
                vec![]
            }
        };
        Ok(spawn_requests)
    }

    fn update(
        &mut self,
        handled_requests: &HashMap<u32, HashMap<UniqId, Request>>,
    ) -> anyhow::Result<RoomStateChange> {
        let room = rooms::get(self.room_name);
        if let Some(room) = room {
            // FIXME Only update things that need to be updated
            let providers: HashMap<_, _> = calc_resource_providers(&room)?
                .into_iter()
                .map(|prov| (prov.ident(), prov))
                .collect();
            self.state = SetupBaseStateVisibility::Visible {
                resource_providers: providers,
                sources: room
                    .find(find::SOURCES)
                    .into_iter()
                    .map(|s| s.id())
                    .collect(),
            };
        } else {
            self.state = SetupBaseStateVisibility::NotVisible {};
        }

        let mut gone_citizens: Vec<usize> = vec![];
        for (i, id) in self.data.helping_citizens.iter().enumerate() {
            match get_object_typed(*id)? {
                Some(_) => {}
                None => {
                    warn!("Removing Helping citizen {}", id);
                    gone_citizens.push(i);
                }
            }
        }

        for to_del in gone_citizens {
            self.data.helping_citizens.remove(to_del);
        }

        match handled_requests.get(&game::time()) {
            Some(handled_requests) => {
                info!("handled requests found!");
                let mut closed_requests = vec![];
                for (i, open_request) in self.open_requests.iter_mut().enumerate() {
                    match handled_requests.get(&open_request) {
                        Some(handled_req) => match &handled_req.data {
                            RequestData::BootstrapWorkerCitizen(
                                requests::BootstrapWorkerCitizen {
                                    target_room_name,
                                    spawning_creep_name: Some(spawning_creep_name),
                                },
                            ) => {
                                info!("Closing request");
                                closed_requests.push(i);
                                if *target_room_name != self.room_name {
                                    warn!(
                                        "WTF, handling different target room?! {} // {}",
                                        self.room_name, target_room_name
                                    );
                                }
                                match creeps::get(&spawning_creep_name)
                                    .map(|c| OokRace::try_from(&c))
                                {
                                    Some(Ok(creep)) => {
                                        self.data.helping_citizens.push(creep.creep()?.id());
                                        info!("OKOKOKOKO push");
                                    }
                                    Some(Err(err)) => {
                                        warn!("Couldnt convert creep for handled request! {}", err);
                                    }
                                    None => {
                                        warn!("Couldnt find creep for handled request! Creep name: {}", spawning_creep_name);
                                    }
                                }
                            }
                            RequestData::BootstrapWorkerCitizen(
                                requests::BootstrapWorkerCitizen {
                                    spawning_creep_name: None,
                                    ..
                                },
                            ) => {
                                warn!(
                                    "Handled request for BootstrapWorkerCitizen ha sno spawning_creep_name"
                                );
                            }
                            RequestData::Citizen(requests::Citizen {
                                target_room_name,
                                spawning_creep_name: Some(spawning_creep_name),
                                ..
                            }) => {
                                info!("Closing request");
                                closed_requests.push(i);
                                if *target_room_name != self.room_name {
                                    warn!(
                                        "WTF, handling different target room?! {} // {}",
                                        self.room_name, target_room_name
                                    );
                                }
                                match creeps::get(&spawning_creep_name)
                                    .map(|c| OokRace::try_from(&c))
                                {
                                    Some(Ok(creep)) => {
                                        self.data.helping_citizens.push(creep.creep()?.id());
                                        info!("OKOKOKOKO push");
                                    }
                                    Some(Err(err)) => {
                                        warn!("Couldnt convert creep for handled request! {}", err);
                                    }
                                    None => {
                                        warn!("Couldnt find creep for handled request! Creep name: {}", spawning_creep_name);
                                    }
                                }
                            }
                            RequestData::Citizen(requests::Citizen {
                                spawning_creep_name: None,
                                ..
                            }) => {
                                warn!("Handled request for Citizen has no spawning_creep_name");
                            }
                        },
                        None => {}
                    }
                }
                for closed_request in closed_requests {
                    self.open_requests.remove(closed_request);
                }
            }
            None => {}
        }

        info!(
            "SetupBase: helping: {:?} // open_requests: {}",
            self.data.helping_citizens,
            self.open_requests.len()
        );
        Ok(RoomStateChange::None)
    }

    fn request_logged(&mut self, request_id: UniqId) {
        self.open_requests.push(request_id);
    }
}

impl RoomStatePersistable<Self> for SetupBaseState {
    fn to_memory(&self) -> anyhow::Result<HashMap<String, Box<dyn JsSerialize>>> {
        let mut map: HashMap<String, Box<dyn JsSerialize>> = HashMap::new();
        map.insert(
            MEM_ROOM_STATE_KIND.to_string(),
            Box::new(RoomStateKind::SetupBase as i32),
        );
        map.insert(
            MEM_ROOM_NAME.to_string(),
            Box::new(self.room_name.to_string()),
        );
        map.insert(MEM_BASE_DATA.to_string(), Box::new(self.data.clone()));
        Ok(map)
    }

    fn load_from_memory(memory: &MemoryReference) -> anyhow::Result<SetupBaseState> {
        let state_kind = memory
            .i32(MEM_ROOM_STATE_KIND)
            .context("loading mem room_state_kind")?
            .ok_or(anyhow!("missing mem room_state_kind"))?;
        if state_kind != RoomStateKind::SetupBase as i32 {
            bail!("Expected RoomStateKind::SetupBase, got {:?}", state_kind);
        }
        let room_name = RoomName::new(
            &memory
                .string(MEM_ROOM_NAME)
                .context("loading mem room_name")?
                .ok_or(anyhow!("missing mem room_name"))?,
        )?;
        let data: Option<SetupBaseData> = memory
            .get(MEM_BASE_DATA)
            .context("failed loading mem base_data")?;

        Ok(SetupBaseState {
            room_name,
            state: SetupBaseStateVisibility::NotVisible {},
            open_requests: Default::default(),
            data: data.unwrap_or_default(),
            panic_countdown: None,
        })
    }

    fn update_from_memory(&mut self, memory: &MemoryReference) -> anyhow::Result<()> {
        let state_kind = memory
            .i32(MEM_ROOM_STATE_KIND)
            .context("loading mem room_state_kind")?
            .ok_or(anyhow!("missing mem room_state_kind"))?;
        if state_kind != RoomStateKind::SetupBase as i32 {
            bail!(
                "Expected RoomStateKind::SetupBase, got {:?} for room {}",
                state_kind,
                self.room_name
            );
        }
        let room_name = RoomName::new(
            &memory
                .string(MEM_ROOM_NAME)
                .context("loading mem room_name")?
                .ok_or(anyhow!("missing mem room_name"))?,
        )?;
        let data: Option<SetupBaseData> = memory
            .get(MEM_BASE_DATA)
            .context("failed loading mem target_spawns")?;

        self.room_name = room_name;
        if let Some(data) = data {
            self.data.target_spawns = data.target_spawns;
            // dont update helping_citizens, dont wanna manually update them
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum SetupBaseStateVisibility {
    Visible {
        resource_providers: HashMap<String, ResourceProvider>,
        sources: Vec<ObjectId<Source>>,
    },
    NotVisible {},
}

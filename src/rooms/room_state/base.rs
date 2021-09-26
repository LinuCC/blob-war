use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    iter::FromIterator,
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
use serde::{Deserialize, Serialize};
use stdweb::JsSerialize;

use crate::{
    constants::{MEM_BASE_DATA, MEM_ROOM_NAME, MEM_ROOM_STATE_KIND},
    creeps::{
        get_prio_repair_target,
        jobs::{self, OokCreepJob},
        races::{carrier::OokCreepCarrier, worker::OokCreepWorker, OokRace, RepresentsCreep},
        tasks::OokCreepTask,
        RepairTarget,
    },
    rooms::{
        extensions::{ExtensionFillPath, StructureSpawnSupply, SuppliersReachPoint},
        room_state::{TargetSpawnKind, TargetSpawns},
    },
    state::{
        requests::{self, Request, RequestData},
        BWState, UniqId,
    },
    trade,
    utils::AnyhowOptionExt,
};

use super::{
    super::{
        resource_provider::{calc_resource_providers, ResourceProvider},
        room_state::{RoomStateKind, RoomStateLifecycle, RoomStatePersistable},
    },
    RoomStateChange,
};

const PANIC_THRESHOLD_TICKS: u32 = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaseData {
    pub helping_citizens: Vec<ObjectId<Creep>>,
    pub target_spawns: TargetSpawns,
    /// Creeps filling extensions & spawns right now
    pub supplier_fillers: Vec<ObjectId<Creep>>,
}

js_serializable!(BaseData);
js_deserializable!(BaseData);

impl Default for BaseData {
    fn default() -> Self {
        BaseData {
            helping_citizens: vec![],
            target_spawns: Default::default(),
            supplier_fillers: vec![],
        }
    }
}

#[derive(Clone, Debug)]
pub struct BaseState {
    pub room_name: RoomName,
    pub resource_providers: HashMap<String, ResourceProvider>,
    pub sources: Vec<ObjectId<Source>>,

    /// Data that also gets persistedook_rooms_data.ook_rooms.W12N15
    pub data: BaseData,

    /// Stores all open requests from this room so it doesnt request things twice
    open_requests: Vec<UniqId>,

    suppliers_fill_path: ExtensionFillPath,
    suppliers_to_fill: Vec<SuppliersReachPoint>,

    pub panic_countdown: Option<u32>,
}

impl BaseState {
    pub fn get_open_suppliers_reach_points(
        &self,
        state: &BWState,
    ) -> anyhow::Result<Vec<SuppliersReachPoint>> {
        let mut supplier_points: HashSet<SuppliersReachPoint> =
            self.suppliers_to_fill.iter().cloned().collect();
        info!("Suppliiiies: {:?}", self.suppliers_to_fill);
        for id in &self.data.supplier_fillers {
            match state.citizens.get(id) {
                Some(OokRace::Carrier(carrier)) => match &carrier.task {
                    Some(OokCreepTask::SpawnSuppliesRun(task)) => {
                        let handled_points = task.handling_supplier_points()?;
                        for point in handled_points {
                            if !supplier_points.remove(&point) {
                                warn!("Oops! looks like supplier points are being handled multiple times");
                            }
                        }
                    }
                    Some(_) => {
                        info!("extension filler {} has different task", id);
                    }
                    None => {
                        info!("extension filler {} has no task", id);
                    }
                },
                Some(citizen) => {
                    warn!(
                        "Unknown citizen {:?} for get_open_extension_reach_points {}",
                        citizen, id
                    );
                }
                None => {
                    warn!("Missing citizen for get_open_extension_reach_points {}", id);
                }
            }
        }
        Ok(Vec::from_iter(supplier_points))
    }

    pub fn creep_handles_filling_extensions(
        &mut self,
        creep_id: ObjectId<Creep>,
    ) -> anyhow::Result<()> {
        self.data.supplier_fillers.push(creep_id);
        Ok(())
    }

    pub fn creep_done_handling_filling_extensions(
        &mut self,
        creep_id: ObjectId<Creep>,
    ) -> anyhow::Result<()> {
        self.data.supplier_fillers.retain(|s| *s != creep_id);
        Ok(())
    }
}

impl BaseState {
    fn spawn_citizens_up_to_target(&self, state: &BWState) -> anyhow::Result<Vec<Request>> {
        let mut requests: Vec<Request> = vec![];
        let mut current_spawns = TargetSpawns {
            farmer: 0,
            worker: 0,
            carrier: 0,
        };
        let mut unhandled_sources: HashSet<ObjectId<Source>> =
            self.sources.iter().cloned().collect();
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
        for open_request in open_requests {
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
        if current_spawns.carrier + open_request_spawns.carrier < self.data.target_spawns.carrier {
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

        for unhandled_source in unhandled_sources {
            let target_room_name = self.room_name;
            let new_request = Request::new(RequestData::Citizen(requests::Citizen {
                target_room_name,
                spawning_creep_name: None,
                initial_job: OokCreepJob::FarmSource(jobs::FarmSource {
                    target_room: target_room_name,
                    target_source: unhandled_source,
                }),
                resolve_panic: false,
            }));
            requests.push(new_request);
        }

        Ok(requests)
    }

    pub fn check_supplier_fillers(&mut self, citizens: &HashMap<ObjectId<Creep>, OokRace>) -> () {
        let mut to_remove = vec![];
        for (i, id) in self.data.supplier_fillers.iter().enumerate() {
            match citizens.get(id) {
                Some(OokRace::Carrier(OokCreepCarrier {
                    task: Some(OokCreepTask::SpawnSuppliesRun(_)),
                    ..
                })) => {}
                _ => {
                    to_remove.push(i);
                }
            }
        }
        for i in to_remove.into_iter().rev() {
            self.data.supplier_fillers.remove(i);
        }
    }

    pub fn update_suppliers(&mut self) -> anyhow::Result<()> {
        let mut suppliers_to_fill = HashSet::new();
        for point in &self.suppliers_fill_path.points {
            for supplier in &point.suppliers {
                match supplier {
                    StructureSpawnSupply::Spawn(spawn_id) => {
                        if let Some(spawn) = get_object_typed(*spawn_id)? {
                            if spawn.store_free_capacity(Some(ResourceType::Energy)) != 0 {
                                suppliers_to_fill.insert(point.to_owned());
                                continue;
                            }
                        }
                    }
                    StructureSpawnSupply::Extension(extension_id) => {
                        if let Some(extension) = get_object_typed(*extension_id)? {
                            if extension.store_free_capacity(Some(ResourceType::Energy)) != 0 {
                                suppliers_to_fill.insert(point.to_owned());
                                continue;
                            }
                        }
                    }
                }
            }
        }
        self.suppliers_to_fill = Vec::from_iter(suppliers_to_fill);
        Ok(())
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

    fn initial_sources(&self) -> anyhow::Result<Vec<ObjectId<Source>>> {
        Ok(rooms::get(self.room_name)
            .anyhow("initial_sources room not found")?
            .find(find::SOURCES)
            .into_iter()
            .map(|s| s.id())
            .collect())
    }

    fn visualize(&self) {
        if let Some(room) = rooms::get(self.room_name) {
            let vis = room.visual();
            for point in self.suppliers_fill_path.points.iter() {
                vis.rect(
                    point.pos.x() as f32 - 0.5,
                    point.pos.y() as f32 - 0.5,
                    1.,
                    1.,
                    None,
                );
                // vis.text(pos.0 as f32, pos.1 as f32, num.to_string(), None);
            }
        }
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
                    job: OokCreepJob::FarmSource(jobs::FarmSource { .. }),
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
                if *panic_countdown < PANIC_THRESHOLD_TICKS {
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

    fn trade(&self) {
        if let Some(room) = rooms::get(self.room_name) {
            trade::get_energy(&room);
        }
    }
}

impl RoomStateLifecycle<BaseState> for BaseState {
    fn new(room_name: RoomName) -> anyhow::Result<BaseState> {
        let room = rooms::get(room_name).ok_or(anyhow!("Room not found to create BaseState"))?;
        // let my_room = MyRoom::by_room_name(room.name())
        //     .ok_or_else(|| Box::new(RoomStateError::MyRoomNotFound(format!("{}", room.name()))))?;
        let resource_providers: HashMap<_, _> = calc_resource_providers(&room)?
            .into_iter()
            .map(|prov| (prov.ident(), prov))
            .collect();
        Ok(BaseState {
            room_name,
            resource_providers,
            sources: room
                .find(find::SOURCES)
                .into_iter()
                .map(|s| s.id())
                .collect(),
            open_requests: Default::default(),
            data: Default::default(),
            suppliers_fill_path: ExtensionFillPath::best_for_room(&room),
            suppliers_to_fill: vec![],
            panic_countdown: None,
        })
    }

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
        // self.visualize();
        self.trade();
        Ok(spawn_requests)
    }

    fn update(
        &mut self,
        handled_requests: &HashMap<u32, HashMap<UniqId, Request>>,
    ) -> anyhow::Result<RoomStateChange> {
        let room = rooms::get(self.room_name);
        let mut state_change = RoomStateChange::None;
        if let Some(room) = room {
            // FIXME Only update things that need to be updated
            let providers: HashMap<_, _> = calc_resource_providers(&room)?
                .into_iter()
                .map(|prov| (prov.ident(), prov))
                .collect();
            self.resource_providers = providers;
            self.sources = room
                .find(find::SOURCES)
                .into_iter()
                .map(|s| s.id())
                .collect();
            if room.find(find::MY_SPAWNS).len() < 1 {
                state_change = RoomStateChange::Helpless;
            }
        } else {
            self.resource_providers = HashMap::new();
            // Cant see room, e.g. nothing in there
            state_change = RoomStateChange::Helpless;
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
                let mut should_update_suppliers = false;
                for (i, open_request) in self.open_requests.iter().enumerate() {
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
                                should_update_suppliers = true;
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
                                should_update_suppliers = true;
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
                                should_update_suppliers = true;
                                warn!("Handled request for Citizen has no spawning_creep_name");
                            }
                        },
                        None => {}
                    }
                }
                for closed_request in closed_requests {
                    self.open_requests.remove(closed_request);
                }
                if should_update_suppliers {
                    self.update_suppliers()?;
                }
            }
            None => {}
        }
        info!(
            "Base: helping: {:?} // open_requests: {}",
            self.data.helping_citizens,
            self.open_requests.len()
        );
        Ok(state_change)
    }

    fn request_logged(&mut self, request_id: UniqId) {
        self.open_requests.push(request_id);
    }
}

impl RoomStatePersistable<Self> for BaseState {
    fn to_memory(&self) -> anyhow::Result<HashMap<String, Box<dyn JsSerialize>>> {
        let mut map: HashMap<String, Box<dyn JsSerialize>> = HashMap::new();
        map.insert(
            MEM_ROOM_NAME.to_string(),
            Box::new(self.room_name.to_string()),
        );
        map.insert(
            MEM_ROOM_STATE_KIND.to_string(),
            Box::new(RoomStateKind::Base as i32),
        );
        map.insert(MEM_BASE_DATA.to_string(), Box::new(self.data.clone()));
        Ok(map)
    }

    fn load_from_memory(memory: &MemoryReference) -> anyhow::Result<BaseState> {
        let state_kind = memory
            .i32(MEM_ROOM_STATE_KIND)
            .context("loading mem room_state_kind")?
            .ok_or(anyhow!("missing mem room_state_kind"))?;
        if state_kind != RoomStateKind::Base as i32 {
            bail!("Expected RoomStateKind::Base, got {:?}", state_kind);
        }
        let room_name = RoomName::new(
            &memory
                .string(MEM_ROOM_NAME)
                .context("loading mem room_name")?
                .ok_or(anyhow!("missing mem room_name"))?,
        )?;
        let room = rooms::get(room_name).anyhow("load_from_mem room not found")?;
        let data: Option<BaseData> = memory
            .get(MEM_BASE_DATA)
            .context("failed loading mem target_spawns")?;
        let mut state = BaseState {
            room_name,
            resource_providers: HashMap::new(),
            data: data.unwrap_or_default(),
            open_requests: Default::default(),
            sources: room
                .find(find::SOURCES)
                .into_iter()
                .map(|s| s.id())
                .collect(),
            suppliers_fill_path: ExtensionFillPath::best_for_room(&room),
            suppliers_to_fill: vec![],
            panic_countdown: None,
        };
        state.update_suppliers()?;
        Ok(state)
    }

    fn update_from_memory(&mut self, memory: &MemoryReference) -> anyhow::Result<()> {
        let state_kind = memory
            .i32(MEM_ROOM_STATE_KIND)
            .context("loading mem room_state_kind")?
            .ok_or(anyhow!("missing mem room_state_kind"))?;
        if state_kind != RoomStateKind::Base as i32 {
            bail!("Expected RoomStateKind::Base, got {:?}", state_kind);
        }
        let room_name = RoomName::new(
            &memory
                .string(MEM_ROOM_NAME)
                .context("loading mem room_name")?
                .ok_or(anyhow!("missing mem room_name"))?,
        )?;
        let data: Option<BaseData> = memory
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

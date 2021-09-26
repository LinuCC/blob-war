pub mod build_target;
pub mod requests;

use core::fmt;
use lazy_static::lazy_static;
use log::{info, warn};
use screeps::{game, ObjectId, RoomName};
use std::{
    collections::HashMap,
    error::Error,
    sync::{atomic::AtomicUsize, Mutex, MutexGuard},
};

use crate::{
    creeps::{races::OokRace, CreepKind},
    rooms::{room_state::RoomState, MyRoom, RoomSettings},
};

use anyhow::anyhow;

use self::requests::{Request, RequestData, BootstrapWorkerCitizen};

lazy_static! {
    pub static ref CONTEXT: Mutex<BWContext> = Mutex::new(BWContext::Initializing);
}

static IN_TICK_UNIQUE_ID: AtomicUsize = AtomicUsize::new(0);

/// Returns a number guaranteed to be unique in this tick
#[inline]
pub fn get_in_tick_unique_id() -> usize {
    IN_TICK_UNIQUE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(thiserror::Error, Debug)]
pub enum ContextError {
    #[error("Context has not been initialized yet! In: {0}")]
    ContextNotInitialized(String),
}

pub enum BWContext {
    Initialized(BWContextInitialized),
    Initializing,
}

impl BWContext {
    pub fn get_mutex() -> &'static Mutex<BWContext> {
        return &*CONTEXT;
    }

    pub fn get() -> MutexGuard<'static, BWContext> {
        // unwrap should only fail if something panic-ed already and poisoned the mutex (aka here
        // never)
        return CONTEXT.lock().unwrap();
    }

    pub fn initialize(state: BWState) -> anyhow::Result<()> {
        lazy_static::initialize(&CONTEXT);
        *(CONTEXT).lock().unwrap() = BWContext::Initialized(BWContextInitialized { state });
        Ok(())
    }

    pub fn state(&self) -> anyhow::Result<&BWState> {
        match self {
            BWContext::Initialized(context) => Ok(&context.state),
            _ => Err(ContextError::ContextNotInitialized("BWContext.state".into()).into()),
        }
    }

    pub fn mut_state(&mut self) -> Result<&mut BWState, Box<dyn std::error::Error>> {
        match self {
            BWContext::Initialized(context) => Ok(&mut context.state),
            _ => Err(Box::new(ContextError::ContextNotInitialized(
                "BWContext.state".into(),
            ))),
        }
    }

    /// Update the global state stored in context
    ///
    /// Example:
    /// ```
    ///   BWContext::update_state(|state| {
    ///       state.ticks_since_init = state.ticks_since_init + 1;
    ///   })?;
    /// ```
    ///
    /// Alternative:
    ///   
    /// ```
    ///   let mut context = BWContext::get()?.lock()?;
    ///   let mut state = context.mut_state()?;
    ///   state.ticks_since_init = state.ticks_since_init + 1;
    /// ```
    pub fn update_state<F>(updater: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnOnce(&mut BWState) -> Result<(), Box<dyn Error>>,
    {
        let mut context = CONTEXT.lock()?;
        let mut state = context.mut_state()?;
        updater(&mut state)?;
        Ok(())
    }

    pub fn update_state_self<F>(&mut self, updater: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: Fn(&mut BWState) -> (),
    {
        let mut state = self.mut_state()?;
        updater(&mut state);
        Ok(())
    }
}

pub struct BWContextInitialized {
    pub state: BWState,
}

#[derive(Debug)]
pub struct BWState {
    pub ticks_since_init: i32,
    pub room_settings: HashMap<MyRoom, RoomSettings>,
    pub room_states: HashMap<RoomName, RoomState>,
    #[deprecated]
    pub kinded_creeps: HashMap<ObjectId<screeps::Creep>, CreepKind>,
    pub citizens: HashMap<ObjectId<screeps::Creep>, OokRace>,
    pub requests: HashMap<UniqId, Request>,
    /// Requests handled in Game Ticks -> RequestId
    pub handled_requests: HashMap<u32, HashMap<UniqId, Request>>,
    // Fast access for cached data at Room -> x -> y
    // pub pois: HashMap<RoomName, HashMap<u32, HashMap<u32, PoisAt>>>,
}

// /// Caches
// pub struct PoisAt {
//     repairables: Vec<PoiStructure>,
//     /// Extensions & spawn
//     spawners: Vec<PoiSpawners>,
// }
//
// /// Caches what creeps can do while on important routes (streets, refilling, etc.)
// pub struct PoisAround {
//     repairables: HashMap<(u8, u8), Vec<PoiStructure>>,
//     /// Extensions & spawn
//     spawners: HashMap<(u8, u8), Vec<PoiSpawn>>,
// }

impl BWState {
    pub fn next_tick(&mut self) {
        self.ticks_since_init = self.ticks_since_init + 1;
        IN_TICK_UNIQUE_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn add_request(&mut self, request: Request) -> anyhow::Result<()> {
        match request {
            Request{ data: RequestData::BootstrapWorkerCitizen(BootstrapWorkerCitizen { .. }), ..} => {
                info!("Inserting request : {:?}", request);
                self.requests.insert(request.request_id.to_owned(), request);
                Ok(())
            }
            Request{ data: RequestData::Citizen(requests::Citizen { .. }), ..} => {
                info!("Inserting request : {:?}", request);
                self.requests.insert(request.request_id.to_owned(), request);
                Ok(())
            }
        }
    }

    /// 
    ///
    /// opts - If you do something which result can only be checked after a tick (for example
    ///   spawn a minion), set this to DelayHandleForOneTick.
    ///   Handlers will update themselfes only then.
    // pub fn request_handled<>(&mut self, request_id: UniqId,  opts: RequestHandledOpts) -> anyhow::Result<()> {
    pub fn request_handled(&mut self, request_data: Request, opts: RequestHandledOpts) -> anyhow::Result<()> {
        let mut tick_handled = game::time();
        match opts {
            RequestHandledOpts::DelayHandleForOneTick => {
                tick_handled += 1;
            },
            RequestHandledOpts::None => {},
        }
        match self.requests.remove(&request_data.request_id) {
            Some(_old_request) => {
                self.handled_requests.entry(tick_handled).or_default().insert(request_data.request_id.to_owned(), request_data);
            },
            None => {
                warn!("called request_handled with id {}, but request is not queued anymore", request_data.request_id);
            },
        }
        Ok(())
    }

    pub fn get_current_or_old_request(&self, request_id: UniqId) -> Option<(Request, Option<u32>)> {
        if let Some(current_request) = self.requests.get(&request_id) {
            Some((current_request.to_owned(), None))
        } else {
            for (tick, old_requests) in &self.handled_requests {
                if let Some(old_request) = old_requests.get(&request_id) {
                    return Some((old_request.to_owned(), Some(*tick)));
                }
            }
            None
        }
    }
}

pub enum RequestHandledOpts {
    DelayHandleForOneTick,
    None
}

/// TODO Make this sexy and not use the heap
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct UniqId {
    val: String,
}

impl UniqId {
    pub fn new() -> UniqId {
        UniqId {
            val: format!("{:x}-{:02x}", game::time(), get_in_tick_unique_id()),
        }
    }
}

impl fmt::Display for UniqId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.val)
    }
}

impl From<&str> for UniqId {
    fn from(s: &str) -> Self {
        UniqId {val: s.to_string()}
    }
}

impl From<String> for UniqId {
    fn from(s: String) -> Self {
        UniqId {val: s}
    }
}

use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    error::Error,
};

use creeps::{
    get_prio_repair_target, harvesting::run_harvester, races::OokRace, CreepKind, RepairTarget,
};
use log::*;
use rooms::{
    room_state::{RoomState, RoomStateLifecycle},
    update_maintenance, MyRoom, RoomSettings,
};
use screeps::{
    find, game::cpu, prelude::*, ObjectId, ResourceType, ReturnCode, RoomName, SpawnOptions,
    Structure, StructureTower,
};
use state::{BWContext, BWState};
use stdweb::js;

use crate::{creeps::{
        jobs::OokCreepJob,
        races::{
            claimer::{OokCreepClaimer, TrySpawnClaimerOptions},
            get_all_citizens_from_creeps,
            worker::{OokCreepWorker, TrySpawnWorkerOptions},
            DynamicTasked, RoomBound,
        },
        CreepBuilder, CreepFarmer, CreepRunner, Spawnable, TrySpawnOptions,
    }, rooms::room_state::{RoomStateChange, SetupBaseState, assign_requests, base::BaseState, dummy_handle_requests, init_room_states, persist_room_states, update_room_states_from_memory}, state::requests::Request};

use anyhow::bail;

#[macro_use]
extern crate stdweb;

mod constants;
mod creeps;
mod game;
mod logging;
mod rooms;
mod state;
mod utils;
mod trade;

#[derive(thiserror::Error, Debug)]
pub enum MainError {
    #[error("Could not find roomsettings for {0}")]
    RoomSettingsNotFound(String),
}

fn main() {
    let mut aborted = 0;
    match main_handled() {
        Ok(_) => {}
        Err(err) => {
            error!("ABORTING Initialization! Unhandled Error occured: {}", err);
            aborted += 1;
            if aborted > 100 {
                error!("Aborted above 100 times! HALTing CPU");
                js! {
                    Game.cpu.halt();
                }
            }
            js! {
                module.exports.loop = function() {
                    console_error("resetting VM next tick.");
                    // reset the VM since we don't know if everything was cleaned up and don't
                    // want an inconsistent state.
                    module.exports.loop = wasm_initialize;
                }
            }
        }
    };
}

fn main_handled() -> Result<(), Box<dyn Error>> {
    logging::setup_logging(logging::Info);
    construct_context()?;
    js! {
        var game_loop = @{game_loop};

        module.exports.loop = function() {
            // Provide actual error traces.
            try {
                game_loop();
            } catch (error) {
                // console_error function provided by 'screeps-game-api'
                console_error("caught exception:", error);
                if (error.stack) {
                    console_error("stack trace:", error.stack);
                }
                console_error("resetting VM next tick.");
                // reset the VM since we don't know if everything was cleaned up and don't
                // want an inconsistent state.
                module.exports.loop = wasm_initialize;
            }
        }
    }
    Ok(())
}

fn game_loop() {
    match run() {
        Ok(_) => {}
        Err(err) => {
            error!("ABORTING Tick! Unhandled Error occured: {}", err);
        }
    };
}

fn run() -> Result<(), Box<dyn Error>> {
    debug!("loop starting! CPU: {}", screeps::game::cpu::get_used());
    BWContext::update_state(|state| {
        state.next_tick();
        Ok(())
    })?;
    debug!("Maintaining Rooms");
    let mut citizens = {
        let context = BWContext::get();
        let state = context.state()?;

        get_all_citizens_from_creeps(screeps::game::creeps::values(), &state.citizens)
            .unwrap_or_else(|err| {
                error!("Couldnt get citizens: {}", err);
                HashMap::new()
            })
    };

    {
        let mut context = BWContext::get();
        let state = context.mut_state()?;

        if let Err(err) = update_room_states_from_memory(state) {
            warn!("Error updating mem of room states {}", err);
        }
    };



    maintain_room(&MyRoom::Main, &citizens)?;

    let mut room_requests: HashMap<RoomName, Request> = HashMap::new();
    {
        let context = BWContext::get();
        let state = context.state()?;
        for (id, room_state) in &state.room_states {
            match room_state {
                RoomState::Base(room_state) => {
                    let requests = room_state.run(&state)?;
                    for request in requests {
                        room_requests.insert(*id, request);
                    }
                }
                RoomState::SetupBase(room_state) => {
                    let requests = room_state.run(&state)?;
                    for request in requests {
                        room_requests.insert(*id, request);
                    }
                }
            }
        }
    }
    BWContext::update_state(|state| {
        for (room_name, request) in room_requests {
            info!("Adding for room: {} // request: {:?}", room_name, request);
            match state.add_request(request.to_owned()) {
                Ok(_) => {
                    if let Some(room_data) = state.room_states.get_mut(&room_name) {
                        match room_data {
                            RoomState::Base(ref mut room_state) => {
                                room_state.request_logged(request.request_id.to_owned());
                            }
                            RoomState::SetupBase(ref mut room_state) => {
                                room_state.request_logged(request.request_id.to_owned());
                            }
                        }
                    }
                }
                Err(err) => warn!("Error adding request {:?} // {}", request, err),
            }
        }
        Ok(())
    })?;

    debug!("running creeps");
    for creep in screeps::game::creeps::values() {
        let name = creep.name();
        debug!("running creep {}", name);
        match (BWContext::get().state()?.ticks_since_init + 500) % 1000 {
            3 => {
                creep.say("Did", true);
            }
            4 => {
                creep.say("you", true);
            }
            5 => {
                creep.say("ever", true);
            }
            6 => {
                creep.say("hear", true);
            }
            7 => {
                creep.say("the", true);
            }
            8 => {
                creep.say("tragedy", true);
            }
            9 => {
                creep.say("of", true);
            }
            10 => {
                creep.say("Darth", true);
            }
            11 => {
                creep.say("Plagueis", true);
            }
            12 => {
                creep.say("The", true);
            }
            13 => {
                creep.say("Wise?", true);
            }
            14 => {
                creep.say("I", true);
            }
            15 => {
                creep.say("thought", true);
            }
            16 => {
                creep.say("not.", true);
            }
            17 => {
                creep.say("It’s", true);
            }
            18 => {
                creep.say("not", true);
            }
            19 => {
                creep.say("a", true);
            }
            20 => {
                creep.say("story", true);
            }
            21 => {
                creep.say("the", true);
            }
            22 => {
                creep.say("Jedi", true);
            }
            23 => {
                creep.say("would", true);
            }
            24 => {
                creep.say("tell", true);
            }
            25 => {
                creep.say("you.", true);
            }
            26 => {
                creep.say("It’s", true);
            }
            27 => {
                creep.say("a", true);
            }
            28 => {
                creep.say("Sith", true);
            }
            29 => {
                creep.say("legend.", true);
            }
            30 => {
                creep.say("Darth", true);
            }
            31 => {
                creep.say("Plagueis", true);
            }
            32 => {
                creep.say("was", true);
            }
            33 => {
                creep.say("a", true);
            }
            34 => {
                creep.say("Dark", true);
            }
            35 => {
                creep.say("Lord", true);
            }
            36 => {
                creep.say("of", true);
            }
            37 => {
                creep.say("the", true);
            }
            38 => {
                creep.say("Sith,", true);
            }
            39 => {
                creep.say("so", true);
            }
            40 => {
                creep.say("powerful", true);
            }
            41 => {
                creep.say("and", true);
            }
            42 => {
                creep.say("so", true);
            }
            43 => {
                creep.say("wise", true);
            }
            44 => {
                creep.say("he", true);
            }
            45 => {
                creep.say("could", true);
            }
            46 => {
                creep.say("use", true);
            }
            47 => {
                creep.say("the", true);
            }
            48 => {
                creep.say("Force", true);
            }
            49 => {
                creep.say("to", true);
            }
            50 => {
                creep.say("influence", true);
            }
            51 => {
                creep.say("the", true);
            }
            52 => {
                creep.say("midichlorians", true);
            }
            53 => {
                creep.say("to", true);
            }
            54 => {
                creep.say("create", true);
            }
            55 => {
                creep.say("life…", true);
            }
            56 => {
                creep.say("He", true);
            }
            57 => {
                creep.say("had", true);
            }
            58 => {
                creep.say("such", true);
            }
            59 => {
                creep.say("a", true);
            }
            60 => {
                creep.say("knowledge", true);
            }
            61 => {
                creep.say("of", true);
            }
            62 => {
                creep.say("the", true);
            }
            63 => {
                creep.say("dark", true);
            }
            64 => {
                creep.say("side", true);
            }
            65 => {
                creep.say("that", true);
            }
            66 => {
                creep.say("he", true);
            }
            67 => {
                creep.say("could", true);
            }
            68 => {
                creep.say("even", true);
            }
            69 => {
                creep.say("keep", true);
            }
            70 => {
                creep.say("the", true);
            }
            71 => {
                creep.say("ones", true);
            }
            72 => {
                creep.say("he", true);
            }
            73 => {
                creep.say("cared", true);
            }
            74 => {
                creep.say("about", true);
            }
            75 => {
                creep.say("from", true);
            }
            76 => {
                creep.say("dying.", true);
            }
            77 => {
                creep.say("The", true);
            }
            78 => {
                creep.say("dark", true);
            }
            79 => {
                creep.say("side", true);
            }
            80 => {
                creep.say("of", true);
            }
            81 => {
                creep.say("the", true);
            }
            82 => {
                creep.say("Force", true);
            }
            83 => {
                creep.say("is", true);
            }
            84 => {
                creep.say("a", true);
            }
            85 => {
                creep.say("pathway", true);
            }
            86 => {
                creep.say("to", true);
            }
            87 => {
                creep.say("many", true);
            }
            88 => {
                creep.say("abilities", true);
            }
            89 => {
                creep.say("some", true);
            }
            90 => {
                creep.say("consider", true);
            }
            91 => {
                creep.say("to", true);
            }
            92 => {
                creep.say("be", true);
            }
            93 => {
                creep.say("unnatural.", true);
            }
            94 => {
                creep.say("He", true);
            }
            95 => {
                creep.say("became", true);
            }
            96 => {
                creep.say("so", true);
            }
            97 => {
                creep.say("powerful…", true);
            }
            98 => {
                creep.say("the", true);
            }
            99 => {
                creep.say("only", true);
            }
            100 => {
                creep.say("thing", true);
            }
            101 => {
                creep.say("he", true);
            }
            102 => {
                creep.say("was", true);
            }
            103 => {
                creep.say("afraid", true);
            }
            104 => {
                creep.say("of", true);
            }
            105 => {
                creep.say("was", true);
            }
            106 => {
                creep.say("losing", true);
            }
            107 => {
                creep.say("his", true);
            }
            108 => {
                creep.say("power,", true);
            }
            109 => {
                creep.say("which", true);
            }
            110 => {
                creep.say("eventually,", true);
            }
            111 => {
                creep.say("of", true);
            }
            112 => {
                creep.say("course,", true);
            }
            113 => {
                creep.say("he", true);
            }
            114 => {
                creep.say("did.", true);
            }
            115 => {
                creep.say("Unfortunately,", true);
            }
            116 => {
                creep.say("he", true);
            }
            117 => {
                creep.say("taught", true);
            }
            118 => {
                creep.say("his", true);
            }
            119 => {
                creep.say("apprentice", true);
            }
            120 => {
                creep.say("everything", true);
            }
            121 => {
                creep.say("he", true);
            }
            122 => {
                creep.say("knew,", true);
            }
            123 => {
                creep.say("then", true);
            }
            124 => {
                creep.say("his", true);
            }
            125 => {
                creep.say("apprentice", true);
            }
            126 => {
                creep.say("killed", true);
            }
            127 => {
                creep.say("him", true);
            }
            128 => {
                creep.say("in", true);
            }
            129 => {
                creep.say("his", true);
            }
            130 => {
                creep.say("sleep.", true);
            }
            131 => {
                creep.say("Ironic.", true);
            }
            132 => {
                creep.say("He", true);
            }
            133 => {
                creep.say("could", true);
            }
            134 => {
                creep.say("save", true);
            }
            135 => {
                creep.say("others", true);
            }
            136 => {
                creep.say("from", true);
            }
            137 => {
                creep.say("death,", true);
            }
            138 => {
                creep.say("but", true);
            }
            139 => {
                creep.say("not", true);
            }
            140 => {
                creep.say("himself.", true);
            }

            500 => {
                creep.say("they", true);
            }
            501 => {
                creep.say("destroy", true);
            }
            502 => {
                creep.say("we", true);
            }
            503 => {
                creep.say("rebuild", true);
            }
            _ => {}
        };

        if creep.spawning() {
            continue;
        }
        if creep.memory().string("kind")?.is_none() && creep.memory().i32("race")?.is_none() {
            run_harvester(creep);
        }
    }

    {
        let mut context = BWContext::get();
        let mut state = context.mut_state()?;
        for (_id, citizen) in &mut citizens {
            match citizen {
                OokRace::Carrier(ref mut carrier) => match (*carrier).do_job(&mut state) {
                    Ok(_) => {}
                    Err(err) => warn!("Failed do_job: {} // for {:?}:", err, carrier),
                },
                OokRace::Worker(ref mut worker) => match (*worker).do_job(&mut state) {
                    Ok(_) => {}
                    Err(err) => warn!("Failed do_job: {} // for {:?}:", err, worker),
                },
                OokRace::Claimer(ref mut claimer) => {
                    match (*claimer).do_job(&mut state) {
                        Ok(_) => {}
                        Err(err) => warn!("Failed do_job: {} // for {:?}:", err, claimer),
                    }
                    info!("claim");
                }
            }
        }
    }

    {
        let mut context = BWContext::get();
        let mut state = context.mut_state()?;
        match assign_requests(state) {
            Ok(assigned_requests) => {
                dummy_handle_requests(state, assigned_requests)?;
            }
            Err(err) => warn!("Could not assign requests {}", err),
        }
    }

    BWContext::update_state(|state| {
        let mut room_state_updates: HashMap<RoomName, RoomState> = HashMap::new();
        for (room_name, room_state) in state.room_states.iter_mut() {
            match room_state {
                RoomState::Base(room_state) => {
                    room_state.check_room_status(&state.citizens)?;
                    room_state.check_supplier_fillers(&state.citizens);
                    if screeps::game::time() % 10 - 5 == 0 {
                        // HACK find out why dis not work sometimes
                        room_state.update_suppliers();
                    }
                    match room_state.update(&state.handled_requests)? {
                        RoomStateChange::FinishSetup => {} // Shouldnt happen
                        RoomStateChange::Helpless => match SetupBaseState::new(*room_name) {
                            Ok(state) => {
                                room_state_updates.insert(*room_name, RoomState::SetupBase(state));
                            }
                            Err(err) => {
                                warn!("Error creating SetupBaseState {}", err);
                            }
                        },
                        RoomStateChange::None => {}
                    }
                }
                RoomState::SetupBase(room_state) => {
                    room_state.check_room_status(&state.citizens)?;
                    room_state.update(&state.handled_requests)?;
                }
            }
        }

        for (room_name, new_state) in room_state_updates {
            state.room_states.insert(room_name, new_state);
        }
        Ok(())
    })?;

    BWContext::update_state(move |state| {
        state.citizens = citizens;
        Ok(())
    })?;

    {
        let mut context = BWContext::get();
        let state = context.mut_state()?;
        if let Err(err) = persist_room_states(state) {
            warn!("Error persisting room states {}", err);
        }
    }

    let time = screeps::game::time();

    if time % 32 == 3 {
        info!("running memory cleanup");
        cleanup_memory().expect("expected Memory.creeps format to be a regular memory object");
    }

    if cpu::bucket() >= 10000 {
        cpu::generate_pixel();
    }

    {
        let context = BWContext::get();
        let state = context.state()?;
        info!(
            "🚀 🦍 🚀 🦍 🚀 🦍 done! cpu: {}; Ticks since last update: {}, Requests: {} & Handled: {} 🍁 🍁 🍁 ",
            screeps::game::cpu::get_used(),
            state.ticks_since_init,
            state.requests.len(),
            state.handled_requests.len(),
        );
    }
    Ok(())
}

fn maintain_room_spawn(
    room_ident: &MyRoom,
    kinded_creeps: &Vec<(screeps::objects::Creep, CreepKind)>,
    citizens: &HashMap<ObjectId<screeps::Creep>, OokRace>,
) -> Result<(), Box<dyn Error>> {
    let room = MyRoom::get(room_ident)?;
    let context = BWContext::get();
    let state = context.state()?;
    let room_settings =
        state
            .room_settings
            .get(&room_ident)
            .ok_or(Box::new(MainError::RoomSettingsNotFound(
                MyRoom::name(room_ident.clone()).into(),
            )))?;
    let room_energy = room.energy_available();
    let target_spawn_energy: u32 = room.energy_capacity_available();

    // Check if all builder posts are staffed
    let builders = &room_settings.target_creeps.builder;
    'builder_settings: for (i, builder) in builders.iter().enumerate() {
        // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
        //   probably should be a hashmap instead
        let expected_post = i.to_string();
        for (_creep, kinded_creep) in kinded_creeps.iter() {
            if let CreepKind::Builder(kinded_builder) = kinded_creep {
                if kinded_builder.post == expected_post {
                    continue 'builder_settings;
                }
            }
        }
        info!("Missing builder for post {}", expected_post);
        let body = builder.parts.clone();
        // No creep with that `post` exists, create it
        for spawn in room.find(find::MY_SPAWNS) {
            if room_energy >= body.iter().map(|p| p.cost()).sum() {
                info!("Spawning builder for post {}", expected_post.clone());
                // create a unique name, spawn.
                let name_base = screeps::game::time();
                let mut additional = 0;
                let res = loop {
                    let name = format!(
                        "{}-{}-{}",
                        CreepBuilder::name_prefix(),
                        name_base,
                        additional
                    );
                    let memory = CreepBuilder::memory_for_spawn(expected_post.clone());
                    let mut options = SpawnOptions::new();
                    options = options.memory(memory);
                    let res = spawn.spawn_creep_with_options(&body, &name, &options);

                    if res == ReturnCode::NameExists {
                        additional += 1;
                    } else {
                        break res;
                    }
                };

                if res != ReturnCode::Ok {
                    warn!("couldn't spawn: {:?}", res);
                }
            }
        }
    }

    // Check if all runner posts are staffed
    let runners = &room_settings.target_creeps.runner;
    'runner_settings: for (i, runner) in runners.iter().enumerate() {
        // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
        //   probably should be a hashmap instead
        let expected_post = i.to_string();
        for (_creep, kinded_creep) in kinded_creeps.iter() {
            if let CreepKind::Runner(kinded_runner) = kinded_creep {
                if kinded_runner.post == expected_post {
                    continue 'runner_settings;
                }
            }
        }
        info!("Missing runner for post {}", expected_post);
        let body = runner.parts.clone();
        // No creep with that `post` exists, create it
        for spawn in room.find(find::MY_SPAWNS) {
            if room_energy >= body.iter().map(|p| p.cost()).sum() {
                info!("Spawning runner for post {}", expected_post);
                // create a unique name, spawn.
                let name_base = screeps::game::time();
                let mut additional = 0;
                let res = loop {
                    let name = format!(
                        "{}-{}-{}",
                        CreepRunner::name_prefix(),
                        name_base,
                        additional
                    );
                    let memory = CreepRunner::memory_for_spawn(expected_post.clone());
                    let mut options = SpawnOptions::new();
                    options = options.memory(memory);
                    let res = spawn.spawn_creep_with_options(&body, &name, &options);

                    if res == ReturnCode::NameExists {
                        additional += 1;
                    } else {
                        break res;
                    }
                };

                if res != ReturnCode::Ok {
                    warn!("couldn't spawn: {:?}", res);
                }
            }
        }
    }

    // Check if all farmer posts are staffed
    // 'runner_settings: for (i, farmer) in runners.iter().enumerate() {
    //     // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
    //     //   probably should be a hashmap instead
    //     for spawn in screeps::game::spawns::values() {
    //         if room_energy >= body.iter().map(|p| p.cost()).sum() {
    //             info!("Spawning farmer for post {}", expected_post);
    //             // create a unique name, spawn.
    //             let name_base = screeps::game::time();
    //             let mut additional = 0;
    //             let res = loop {
    //                 let name = format!(
    //                     "{}-{}-{}",
    //                     CreepFarmer::name_prefix(),
    //                     name_base,
    //                     additional
    //                 );
    //                 let memory =
    //                     CreepFarmer::memory_for_spawn(expected_post.clone(), &farmer.farm_position);
    //                 let mut options = SpawnOptions::new();
    //                 options = options.memory(memory);
    //                 let res = spawn.spawn_creep_with_options(&body, &name, &options);
    //
    //                 if res == ReturnCode::NameExists {
    //                     additional += 1;
    //                 } else {
    //                     break res;
    //                 }
    //             };
    //
    //             if res != ReturnCode::Ok {
    //                 warn!("couldn't spawn: {:?}", res);
    //             }
    //         }
    //     }
    // }

    let farmers = &room_settings.target_creeps.farmer;
    'farmer_settings: for (i, farmer) in farmers.iter().enumerate() {
        // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
        //   probably should be a hashmap instead
        let expected_post = i.to_string();
        for (_creep, kinded_creep) in kinded_creeps.iter() {
            if let CreepKind::Farmer(kinded_farmer) = kinded_creep {
                if kinded_farmer.post == expected_post {
                    continue 'farmer_settings;
                }
            }
        }
        info!("Missing farmer for post {}", expected_post);
        let body = farmer.parts.clone();
        // No creep with that `post` exists, create it
        for spawn in room.find(find::MY_SPAWNS) {
            if room_energy >= body.iter().map(|p| p.cost()).sum() {
                info!("Spawning farmer for post {}", expected_post);
                // create a unique name, spawn.
                let name_base = screeps::game::time();
                let mut additional = 0;
                let res = loop {
                    let name = format!(
                        "{}-{}-{}",
                        CreepFarmer::name_prefix(),
                        name_base,
                        additional
                    );
                    let memory =
                        CreepFarmer::memory_for_spawn(expected_post.clone(), &farmer.farm_position);
                    let mut options = SpawnOptions::new();
                    options = options.memory(memory);
                    let res = spawn.spawn_creep_with_options(&body, &name, &options);

                    if res == ReturnCode::NameExists {
                        additional += 1;
                    } else {
                        break res;
                    }
                };

                if res != ReturnCode::Ok {
                    warn!("couldn't spawn: {:?}", res);
                }
            }
        }
    }

    // Check if all bitch posts are staffed
    let bitches = &room_settings.target_creeps.bitches;
    // 'bitch_settings: for (i, bitch) in bitches.iter().enumerate() {
    //     // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
    //     //   probably should be a hashmap instead
    //     let expected_post = i.to_string();
    //     for (_creep, kinded_creep) in kinded_creeps.iter() {
    //         if let CreepKind::Bitch(kinded_bitch) = kinded_creep {
    //             if kinded_bitch.post == expected_post {
    //                 continue 'bitch_settings;
    //             }
    //         }
    //     }
    //     info!("Missing bitch for post {}", expected_post);
    //     let body = bitch.parts.clone();
    //     // No creep with that `post` exists, create it
    //     for spawn in screeps::game::spawns::values() {
    //         if room_energy >= body.iter().map(|p| p.cost()).sum() {
    //             info!("Spawning bitch for post {}", expected_post);
    //             // create a unique name, spawn.
    //             let name_base = screeps::game::time();
    //             let mut additional = 0;
    //             let res = loop {
    //                 let name = format!("{}-{}-{}", CREEP_ID_BITCH, name_base, additional);
    //                 let memory = CreepBitch::memory_for_spawn(expected_post.clone());
    //                 let mut options = SpawnOptions::new();
    //                 options = options.memory(memory);
    //                 let res = spawn.spawn_creep_with_options(&body, &name, &options);
    //
    //                 if res == ReturnCode::NameExists {
    //                     additional += 1;
    //                 } else {
    //                     break res;
    //                 }
    //             };
    //
    //             if res != ReturnCode::Ok {
    //                 warn!("couldn't spawn: {:?}", res);
    //             }
    //         }
    //     }
    // }
    'bitch_settings: for (i, _bitch) in bitches.iter().enumerate() {
        // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
        //   probably should be a hashmap instead
        let expected_post = i.to_string();
        for (_creep, citizen) in citizens.iter() {
            match citizen {
                OokRace::Worker(worker) => {
                    if worker.post_ident()?.to_string() == expected_post {
                        continue 'bitch_settings;
                    }
                }
                _ => {}
            }
        }
        info!(
            "Missing worker for post {}, {}",
            expected_post,
            citizens.len()
        );

        let res = OokCreepWorker::try_spawn(
            &TrySpawnOptions {
                race: creeps::races::OokRaceKind::Worker,
                assumed_job: OokCreepJob::UpgradeController {
                    target_room: room.name(),
                },
                available_spawns: room.find(find::MY_SPAWNS).iter().map(|s| s.id()).collect(),
                force_spawn: false,
                target_energy_usage: target_spawn_energy,
                spawn_room: &room,
                request_id: None,
                preset_parts: None,
            },
            &TrySpawnWorkerOptions {
                post_ident: expected_post,
                base_room: room.name(),
            },
        )?;

        // let body = builder.parts.clone();
        // No creep with that `post` exists, create it
        // for spawn in screeps::game::spawns::values() {
        //     if room_energy >= body.iter().map(|p| p.cost()).sum() {
        //         info!("Spawning builder for post {}", expected_post.clone());
        //         // create a unique name, spawn.
        //         let name_base = screeps::game::time();
        //         let mut additional = 0;
        //         let res = loop {
        //             let name = format!(
        //                 "{}-{}-{}",
        //                 CreepBuilder::name_prefix(),
        //                 name_base,
        //                 additional
        //             );
        //             let memory = CreepBuilder::memory_for_spawn(expected_post.clone());
        //             let mut options = SpawnOptions::new();
        //             options = options.memory(memory);
        //             let res = spawn.spawn_creep_with_options(&body, &name, &options);
        //
        //             if res == ReturnCode::NameExists {
        //                 additional += 1;
        //             } else {
        //                 break res;
        //             }
        //         };
        //
        //         if res != ReturnCode::Ok {
        //             warn!("couldn't spawn: {:?}", res);
        //         }
        //     }
        // }
    }

    let claimers = &room_settings.target_creeps.claimers;
    'claimer_settings: for (i, claimer) in claimers.iter().enumerate() {
        // TODO atm the position in the Vec<RoomBuilderSettings> is used as `post` id,
        //   probably should be a hashmap instead
        let expected_post = i.to_string();
        for (_creep, citizen) in citizens.iter() {
            match citizen {
                OokRace::Claimer(claimer) => {
                    if claimer.post_ident()?.to_string() == expected_post {
                        continue 'claimer_settings;
                    }
                }
                _ => {}
            }
        }
        info!("Missing claimer {}", expected_post);

        let res = OokCreepClaimer::try_spawn(
            &TrySpawnOptions {
                race: creeps::races::OokRaceKind::Claimer,
                assumed_job: OokCreepJob::ClaimRoom {
                    target_room: claimer.target_room,
                },
                available_spawns: room.find(find::MY_SPAWNS).iter().map(|s| s.id()).collect(),
                force_spawn: false,
                target_energy_usage: target_spawn_energy,
                spawn_room: &room,
                request_id: None,
                preset_parts: None,
            },
            &TrySpawnClaimerOptions {
                post_ident: expected_post,
            },
        )?;
        info!("Le spawn: {:?}", res);
    }

    Ok(())
}

fn defend_room(_room_ident: &MyRoom, room: &screeps::Room) -> Result<(), Box<dyn Error>> {
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
    }

    // HACK
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
    match get_prio_repair_target(room) {
        Ok(Some(RepairTarget::Important { target })) => towers.iter().for_each(|t| {
            t.repair(&target);
        }),
        _ => {}
    }

    Ok(())
}

fn maintain_room(
    room_ident: &MyRoom,
    citizens: &HashMap<ObjectId<screeps::Creep>, OokRace>,
) -> Result<(), Box<dyn Error>> {
    let state_kinded_creeps = {
        let context = BWContext::get();
        let state = context.state()?;
        &state.kinded_creeps.clone()
    };
    let room = MyRoom::get(room_ident)?;
    let creeps = room.find(find::MY_CREEPS);
    let kinded_creeps: Vec<(screeps::objects::Creep, CreepKind)> = creeps
        .into_iter()
        .filter_map(|creep| {
            let id = creep.id().clone();
            let kinded = if let Some(state_kinded) = state_kinded_creeps.get(&id) {
                let mut new_state_kinded = (*state_kinded).to_owned();
                // TODO This is stupid, the creepKind should just not contain the creep, just its
                // id
                new_state_kinded.set_creep(creep.clone());
                Ok(new_state_kinded)
            } else {
                let kinded = CreepKind::try_from(creep.clone());
                if let Ok(kinded) = kinded.as_ref() {
                    let update_res = BWContext::update_state(|state| {
                        state.kinded_creeps.insert(creep.id(), kinded.clone());
                        Ok(())
                    });
                    if let Err(err) = update_res {
                        warn!("Failed updating kinded_creeps state: {}", err);
                    }
                }
                kinded
            };
            match kinded {
                Ok(cr) => Some((creep, cr)),
                Err(err) => {
                    // warn!("Could not read creep {}: {}", id, err);
                    None
                }
            }
        })
        .collect();

    update_maintenance(room_ident.to_owned())?;
    maintain_room_spawn(room_ident, &kinded_creeps, citizens)?;
    defend_room(room_ident, &room)?;

    for (creep, kind_data) in kinded_creeps.into_iter() {
        match kind_data {
            CreepKind::Builder(mut builder_data) => {
                match builder_data.harvest_check() {
                    Ok(_) => {}
                    Err(err) => info!("Failed harvest_check builder: {}", err),
                }
                if builder_data.harvesting {
                    match builder_data.harvest() {
                        Ok(_) => {}
                        Err(err) => info!("Failed harvest builder: {}", err),
                    }
                } else {
                    match builder_data.build() {
                        Ok(_) => {}
                        Err(err) => info!("Failed build builder: {}", err),
                    }
                }
                // Whats happening here:
                //
                // 1. Somewhere else:
                //   1. Load creep from State kinded_creeps
                //   2. If that does not exist, `try_from` creep and store in State
                // 2. **CLONE** KindedCreep and use that here
                // 3. KindedCreep.run / .build / ... updates the cloned entry only
                // 4. Manually copy the cloned entry back
                //
                // There should be a better way instead of cloning and updating back? Cell or
                // something?
                BWContext::update_state(|state| {
                    let kinded = state.kinded_creeps.get_mut(&creep.id());
                    if let Some(builder) = kinded {
                        *builder = CreepKind::Builder(builder_data.clone());
                    }
                    Ok(())
                })?;
            }
            CreepKind::Farmer(mut farmer_data) => {
                farmer_data.harvest()?;
            }
            CreepKind::Runner(mut runner_data) => {
                match runner_data.run() {
                    Ok(_) => {}
                    Err(err) => info!("Failed running runner: {}", err),
                }
                BWContext::update_state(|state| {
                    let kinded = state.kinded_creeps.get_mut(&creep.id());
                    if let Some(runner) = kinded {
                        *runner = CreepKind::Runner(runner_data.clone());
                    }
                    Ok(())
                })?;
            }
            CreepKind::Bitch(mut bitch_data) => match bitch_data.run() {
                Ok(_) => {}
                Err(err) => info!("Failed running bitch: {}", err),
            },
            _ => {}
        }
    }

    Ok(())
}

fn construct_context() -> anyhow::Result<()> {
    let room_settings = match RoomSettings::world() {
        Ok(world) => world,
        Err(err) => {
            error!("Failed to initialize the world!");
            error!("Error: {}", err);
            return Err(err);
        }
    };

    let citizens = get_all_citizens_from_creeps(screeps::game::creeps::values(), &HashMap::new())
        .unwrap_or_else(|err| {
            error!("Couldnt get citizens: {}", err);
            HashMap::new()
        });
    let mut room_states: HashMap<RoomName, RoomState> = match init_room_states() {
        Ok(room_states) => room_states,
        Err(err) => {
            bail!("Failed initing room states: {}", err);
        },
    };
    if room_states.get(&RoomName::new("W12N16")?).is_none() {
        warn!("ITS GONE AGAIN?!");
        room_states.insert(
            RoomName::new("W12N16")?,
            RoomState::Base(BaseState::new(RoomName::new("W12N16")?)?),
        );
    }
    info!("{:?}", room_states);
    BWContext::initialize(BWState {
        ticks_since_init: 0,
        room_settings,
        room_states,
        kinded_creeps: HashMap::new(),
        citizens,
        requests: Default::default(),
        handled_requests: Default::default(),
    })?;
    info!("init done");
    Ok(())
}

fn cleanup_memory() -> Result<(), Box<dyn std::error::Error>> {
    let alive_creeps: HashSet<String> = screeps::game::creeps::keys().into_iter().collect();

    let screeps_memory = match screeps::memory::root().dict("creeps")? {
        Some(v) => v,
        None => {
            warn!("not cleaning game creep memory: no Memory.creeps dict");
            return Ok(());
        }
    };

    for mem_name in screeps_memory.keys() {
        if !alive_creeps.contains(&mem_name) {
            debug!("cleaning up creep memory of dead creep {}", mem_name);
            screeps_memory.del(&mem_name);
        }
    }

    Ok(())
}

use screeps::RoomName;

use crate::creeps::jobs::OokCreepJob;

use super::UniqId;
//
// pub enum RequestKind {
//     BootstrapWorkerCitizen = 0,
// }
//
// pub trait KindedRequest {
//     fn kind(&self) -> RequestKind;
// }
//
// pub trait HandleableRequest<T> {
//     type Handled;
//
//     fn handled(&self, additional_data: Self::Handled) -> HandledRequest;
// }

#[derive(Clone, Debug)]
pub struct Request {
    pub request_id: UniqId,
    pub data: RequestData,
}

impl Request {
    pub fn new(data: RequestData) -> Request {
        Request {
            request_id: UniqId::new(),
            data,
        }
    }
}

#[derive(Clone, Debug)]
pub enum RequestData {
    BootstrapWorkerCitizen(BootstrapWorkerCitizen),
    Citizen(Citizen),
}

#[derive(Clone, Debug)] 
pub struct BootstrapWorkerCitizen { 
    pub target_room_name: RoomName,
    pub spawning_creep_name: Option<String>,
}

#[derive(Clone, Debug)] 
pub struct Citizen { 
    pub target_room_name: RoomName,
    pub spawning_creep_name: Option<String>,
    pub initial_job: OokCreepJob,
    pub resolve_panic: bool,
}

// #[derive(Clone, Debug)]
// pub struct HandledRequest {
//     pub request_id: UniqId,
//     pub data: HandledRequestData,
// }
//
// #[derive(Clone, Debug)]
// pub enum HandledRequestData {
//     BootstrapWorkerCitizen(HandledWorkerCitizen)
// }
//
// pub fn handled_request_from_data(data: )
// //
// // impl<T> HandledRequestData where T: KindedRequest {
// //     fn from(data: T) -> Self {
// //         match data.kind() {
// //             RequestKind::BootstrapWorkerCitizen => HandledRequestData::BootstrapWorkerCitizen(data),
// //         }
// //     }
// // }
//
// #[derive(Clone, Debug)]
// pub struct HandledWorkerCitizen {
//     pub target_room_name: RoomName,
//     pub spawning_creep_name: String 
// }
// impl KindedRequest for HandledWorkerCitizen {
//     fn kind(&self) -> RequestKind {
//         RequestKind::BootstrapWorkerCitizen
//     }
// }
//
// pub fn handled(req) -> handled_req {
//
// }

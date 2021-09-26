use core::fmt;
use std::error::Error;

use screeps::{ObjectId, RawObjectId, StructureController};

use anyhow::anyhow;

#[derive(thiserror::Error, Debug)]
pub enum UtilError {
    #[error("object not found {0}")]
    ObjectNotFound(String),
}

pub trait HexStr {
    fn to_hex_string(&self) -> String;
    fn from_hex_string(hex_str: &str) -> Result<RawObjectId, Box<dyn Error>>;
}

impl HexStr for RawObjectId {
    fn to_hex_string(&self) -> String {
        let num = self.to_u128();
        format!("{:X}", num)
    }

    fn from_hex_string(hex_str: &str) -> Result<Self, Box<dyn Error>> {
        use screeps::traits::TryFrom;
        let num = u128::from_str_radix(hex_str, 16)?;
        Ok(Self::try_from(num)?)
    }
}

pub fn as_object_id<T>(num: u128) -> Result<ObjectId<T>, Box<(dyn Error)>> {
    use screeps::traits::TryFrom;
    Ok(ObjectId::from(RawObjectId::try_from(num)?))
}

pub trait ResultOptionExt<T, M> {
    fn err_or_none(self, msg: M) -> anyhow::Result<T>;
}

impl<T, E> ResultOptionExt<T, String> for std::result::Result<Option<T>, E> 
    where E: std::error::Error + Send + Sync + 'static {
    #[inline]
    fn err_or_none(self, msg: String) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::Error::new(e)).and_then(|o| o.ok_or_else(|| anyhow!(msg)))
    }
}

impl<T, E> ResultOptionExt<T, &str> for std::result::Result<Option<T>, E> 
    where E: std::error::Error + Send + Sync + 'static {
    #[inline]
    fn err_or_none(self, msg: &str) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::Error::new(e)).and_then(|o| o.ok_or_else(|| anyhow!(msg.to_owned())))
    }
}

pub trait AnyhowOptionExt<'a, T> {
    fn anyhow(self, msg: &'a str) -> anyhow::Result<T>;
}

impl<'a, T> AnyhowOptionExt<'a, T> for Option<T> {
    #[inline]
    fn anyhow(self, msg: &'a str) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow!(msg.to_string()))
    }
}

// impl<T> ResultOptionExt<T> for anyhow::Result<Option<T>>  {
//     #[inline]
//     fn err_none(self, msg: String) -> anyhow::Result<T> {
//         self.and_then(|o| o.ok_or_else(|| anyhow!(msg)))
//     }
// }

// impl<T> ResultOptionExt<std::result::Result<T, anyhow::Error>> for std::result::Result<Option<T>, anyhow::Error> {
//     #[inline]
//     fn err_none(self, msg: String) -> anyhow::Result<T> {
//         self.and_then(|o| o.ok_or_else(|| anyhow!(msg)))
//     }
// }




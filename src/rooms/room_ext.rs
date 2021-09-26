use std::{cmp, ops::Range};

use screeps::{LookConstant, Position};

pub trait RoomExt {
    fn look_for_around<T: LookConstant>(
        &self,
        look_for: T,
        pos: Position,
        steps: u8,
    ) -> anyhow::Result<Vec<T::Item>>;

    fn bounded_pos_area_range(pos: (u8, u8), steps: u8, include_end: bool) -> (Range<u8>, Range<u8>);
}

impl RoomExt for screeps::Room {
    /// Does not handle world coords!
    fn look_for_around<T: LookConstant>(
        &self,
        look_for: T,
        pos: Position,
        steps: u8,
    ) -> anyhow::Result<Vec<T::Item>> {
        let (range_x, range_y) =
            Self::bounded_pos_area_range((pos.x() as u8, pos.y() as u8), steps, false);
        Ok(self.look_for_at_area(look_for, range_x, range_y))
    }

    /// gets range for x and y to look around without overstepping rooms bounds
    ///
    /// Takes the position as the center
    /// Use `include_end: true` if you iterate over the ranges (e.g. in a `for..in`)
    /// Use `include_end: false` if you pass values to `look_for_at_area` (internal implementation
    ///   already includes end val)
    fn bounded_pos_area_range((x, y): (u8, u8), steps: u8, include_end: bool) -> (Range<u8>, Range<u8>) {
        let include_end_val: u8 = if include_end {
            1
        } else {
            0
        };
        (
            cmp::max(x - steps, 0) as u8..cmp::min(x + steps + include_end_val, 49) as u8,
            cmp::max(y - steps, 0) as u8..cmp::min(y + steps + include_end_val, 49) as u8,
        )
    }

    // TODO bunch of functions that cache for the tick
    // if they arent already optimized
    // Like `room.find(find::STRUCTURES)`
}

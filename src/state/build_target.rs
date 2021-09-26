use screeps::{ConstructionSite, HasId, HasPosition, ObjectId, Position, RoomName};

#[derive(Clone, Debug)]
pub struct BuildTarget {
    pub pos: Position,
    pub id: ObjectId<ConstructionSite>,
}

impl From<ConstructionSite> for BuildTarget {
    fn from(c: ConstructionSite) -> Self {
        BuildTarget {
            pos: c.pos(),
            id: c.id(),
        }
    }
}

impl From<&ConstructionSite> for BuildTarget {
    fn from(c: &ConstructionSite) -> Self {
        BuildTarget {
            pos: c.pos(),
            id: c.id(),
        }
    }
}

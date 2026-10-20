/// A change-detection timestamp. Wraparound handling arrives with the change
/// detection milestone; for now it is only stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tick(u32);

impl Tick {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

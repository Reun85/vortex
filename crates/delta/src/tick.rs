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

    /// Whether `self` happened after `last_run`, using wrapping arithmetic
    /// so it stays correct while the two are less than half the range apart.
    pub const fn is_newer_than(self, last_run: Tick) -> bool {
        self.0.wrapping_sub(last_run.0).wrapping_sub(1) < u32::MAX / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_than_handles_wraparound() {
        assert!(Tick::new(5).is_newer_than(Tick::new(4)));
        assert!(!Tick::new(4).is_newer_than(Tick::new(4)));
        assert!(!Tick::new(3).is_newer_than(Tick::new(4)));
        assert!(Tick::new(2).is_newer_than(Tick::new(u32::MAX - 1)));
        assert!(!Tick::new(u32::MAX - 1).is_newer_than(Tick::new(2)));
    }
}

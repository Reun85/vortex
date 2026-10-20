use std::cell::UnsafeCell;

/// How often (in ticks) the world clamps stored ticks, see
/// [`World::check_change_ticks`](crate::World::check_change_ticks).
pub const CHECK_TICK_THRESHOLD: u32 = 518_400_000;

/// The oldest age a stored tick is allowed to keep. Anything older is
/// clamped to this, so ages never exceed half the tick range and wrapping
/// subtraction stays unambiguous.
pub const MAX_CHANGE_AGE: u32 = u32::MAX - (2 * CHECK_TICK_THRESHOLD - 1);

/// A change-detection timestamp: the value of the world's change tick at the
/// moment something happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tick(u32);

impl Tick {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    /// `self - other` with wraparound: how long ago `other` was, seen from
    /// `self`.
    pub const fn relative_to(self, other: Tick) -> Tick {
        Tick(self.0.wrapping_sub(other.0))
    }

    /// Whether `self` happened after `last_run`, judged at `this_run`.
    ///
    /// Compares ages (clamped to [`MAX_CHANGE_AGE`]) rather than raw values,
    /// so it stays correct across `u32` wraparound as long as stored ticks
    /// are kept within range by [`check_tick`](Self::check_tick).
    pub const fn is_newer_than(self, last_run: Tick, this_run: Tick) -> bool {
        let since_change = min(this_run.relative_to(self).0, MAX_CHANGE_AGE);
        let since_system =
            min(this_run.relative_to(last_run).0, MAX_CHANGE_AGE);
        since_system > since_change
    }

    /// Clamps `self` if it is older than [`MAX_CHANGE_AGE`] at `now`.
    /// Returns whether it was changed.
    pub fn check_tick(&mut self, now: Tick) -> bool {
        if now.relative_to(*self).0 > MAX_CHANGE_AGE {
            *self = now.relative_to(Tick(MAX_CHANGE_AGE));
            true
        } else {
            false
        }
    }
}

const fn min(a: u32, b: u32) -> u32 {
    if a < b { a } else { b }
}

/// A [`Tick`] that can be stamped through a shared reference, which is how a
/// `Mut<T>` marks one row changed while the column stays shared.
#[repr(transparent)]
pub struct TickCell(UnsafeCell<Tick>);

// SAFETY: concurrent access is ruled out by the declared access sets of
// systems, the same discipline that protects the component data itself.
unsafe impl Sync for TickCell {}
// SAFETY: it is just a `u32`.
unsafe impl Send for TickCell {}

impl TickCell {
    pub const fn new(tick: Tick) -> Self {
        Self(UnsafeCell::new(tick))
    }

    /// # Safety
    /// No concurrent [`write`](Self::write).
    pub unsafe fn read(&self) -> Tick {
        // SAFETY: per contract.
        unsafe { *self.0.get() }
    }

    /// # Safety
    /// No concurrent access of any kind.
    pub unsafe fn write(&self, tick: Tick) {
        // SAFETY: per contract.
        unsafe { *self.0.get() = tick }
    }

    pub fn set_mut(&mut self, tick: Tick) {
        *self.0.get_mut() = tick;
    }

    pub fn get_mut(&mut self) -> &mut Tick {
        self.0.get_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_than_plain() {
        let now = Tick::new(10);
        assert!(Tick::new(5).is_newer_than(Tick::new(4), now));
        assert!(!Tick::new(4).is_newer_than(Tick::new(4), now));
        assert!(!Tick::new(3).is_newer_than(Tick::new(4), now));
        // Stamped during the current run is newer than the last run.
        assert!(now.is_newer_than(Tick::new(9), now));
    }

    #[test]
    fn newer_than_across_wraparound() {
        let now = Tick::new(5); // wrapped past u32::MAX
        let before = Tick::new(u32::MAX - 3);
        assert!(Tick::new(2).is_newer_than(before, now));
        assert!(!before.is_newer_than(Tick::new(2), now));
        assert!(!before.is_newer_than(before, now));
    }

    #[test]
    fn check_tick_clamps_only_old_ticks() {
        let now = Tick::new(MAX_CHANGE_AGE + 100);
        let mut fresh = Tick::new(now.get() - 10);
        assert!(!fresh.check_tick(now));
        assert_eq!(fresh, Tick::new(now.get() - 10));

        let mut stale = Tick::new(0);
        assert!(stale.check_tick(now));
        assert_eq!(now.relative_to(stale).get(), MAX_CHANGE_AGE);
        // Clamping is stable.
        assert!(!stale.check_tick(now));
    }

    #[test]
    fn clamped_ticks_never_look_newer() {
        // A system that has not run for a very long time sees a component
        // that was changed even longer ago as unchanged, never as new.
        let now = Tick::new(3_000_000_000);
        let mut old_change = Tick::new(0);
        old_change.check_tick(now);
        let mut old_run = Tick::new(1);
        old_run.check_tick(now);
        assert!(!old_change.is_newer_than(old_run, now));
        // A recent change is still seen by a long-idle system.
        assert!(Tick::new(now.get() - 1).is_newer_than(old_run, now));
    }
}

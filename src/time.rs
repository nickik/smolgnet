use core::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant(u64);

impl Instant {
    pub const ZERO: Self = Self(0);
    pub const fn from_millis(millis: u64) -> Self { Self(millis) }
    pub const fn total_millis(self) -> u64 { self.0 }
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.0.checked_add(duration.0) { Some(v) => Some(Self(v)), None => None }
    }
}

impl From<u64> for Instant {
    fn from(value: u64) -> Self { Self::from_millis(value) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(u64);

impl Duration {
    pub const ZERO: Self = Self(0);
    pub const fn from_millis(millis: u64) -> Self { Self(millis) }
    pub const fn from_secs(seconds: u64) -> Self { Self(seconds.saturating_mul(1_000)) }
    pub const fn total_millis(self) -> u64 { self.0 }
    pub const fn total_secs(self) -> u64 { self.0 / 1_000 }
}

impl From<u64> for Duration {
    fn from(value: u64) -> Self { Self::from_millis(value) }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, rhs: Duration) -> Self::Output { Instant(self.0.saturating_add(rhs.0)) }
}
impl AddAssign<Duration> for Instant { fn add_assign(&mut self, rhs: Duration) { *self = *self + rhs; } }
impl Sub<Duration> for Instant {
    type Output = Instant;
    fn sub(self, rhs: Duration) -> Self::Output { Instant(self.0.saturating_sub(rhs.0)) }
}
impl SubAssign<Duration> for Instant { fn sub_assign(&mut self, rhs: Duration) { *self = *self - rhs; } }
impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, rhs: Instant) -> Self::Output { Duration(self.0.saturating_sub(rhs.0)) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollAt { Now, Time(Instant), Ingress }

impl PollAt {
    pub fn delay_from(self, now: Instant) -> Option<Duration> {
        match self {
            Self::Now => Some(Duration::ZERO),
            Self::Time(at) => Some(at.saturating_duration_since(now)),
            Self::Ingress => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arithmetic_is_saturating() {
        let now = Instant::from_millis(100);
        assert_eq!((now + Duration::from_millis(25)).total_millis(), 125);
        assert_eq!((now - Duration::from_millis(150)).total_millis(), 0);
        assert_eq!((Instant::from_millis(180) - now).total_millis(), 80);
    }
}

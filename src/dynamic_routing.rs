use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RouterId(pub u64);

impl RouterId {
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(Error::InvalidField);
        }
        Ok(Self(value))
    }

    #[cfg(feature = "std")]
    pub fn random() -> core::result::Result<Self, getrandom::Error> {
        loop {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes)?;
            let value = u64::from_be_bytes(bytes);
            if value != 0 {
                return Ok(Self(value));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(pub u64);

impl LinkId {
    pub const fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(Error::InvalidField);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RouteOrigin {
    Connected = 0,
    Learned = 1,
    Static = 2,
    Escape = 3,
}

impl RouteOrigin {
    pub const fn from_wire(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Connected),
            1 => Ok(Self::Learned),
            2 => Ok(Self::Static),
            3 => Ok(Self::Escape),
            _ => Err(Error::InvalidField),
        }
    }

    pub const fn administrative_preference(self) -> u8 {
        match self {
            Self::Connected => 0,
            Self::Static => 10,
            Self::Learned => 100,
            Self::Escape => 255,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteMetric(pub u32);

impl RouteMetric {
    pub const DEFAULT_LINK: Self = Self(100);

    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

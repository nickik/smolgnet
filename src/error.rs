use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidLength,
    InvalidField,
    InvalidCrc,
    InvalidAddressForm,
    InvalidSizeClass,
    Unsupported,
    NonCanonical,
    BufferFull,
    NoCredit,
    CreditViolation,
    LinkDown,
    Desynchronized,
    UnknownService,
    UnknownTunnel,
    UnknownStream,
    InvalidState,
    DirectionViolation,
    ProfileViolation,
    MessageTooLarge,
    WouldBlock,
}

pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

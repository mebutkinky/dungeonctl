use uuid::Uuid;

/// The result type returned by this library.
pub type Result<T> = std::result::Result<T, Error>;

/// The error type returned by this library.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// The device is missing a BLE characteristic.
    ///
    /// This should never occur using an original device.
    MissingCharacteristic(Uuid),
    /// An error returned by [`btleplug`].
    Btleplug(btleplug::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::MissingCharacteristic(uuid) => {
                write!(f, "missing device characteristic '{uuid}'")
            }
            Error::Btleplug(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::MissingCharacteristic(_) => None,
            Error::Btleplug(e) => Some(e),
        }
    }
}

impl From<btleplug::Error> for Error {
    fn from(e: btleplug::Error) -> Self {
        Self::Btleplug(e)
    }
}

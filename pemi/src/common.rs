#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The provided buffer is too short. For use with the `octets` module.
    BufferTooShort,

    /// This package is not in the usual QUIC format.
    MayNotQUIC,

    /// The operation cannot be completed because the connection is in an
    /// invalid state.
    InvalidState,

    /// other errors
    Other(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl std::convert::From<octets::BufferTooShortError> for Error {
    fn from(_err: octets::BufferTooShortError) -> Self {
        Error::BufferTooShort
    }
}

// support conversion to String
impl std::convert::From<Error> for String {
    fn from(err: Error) -> Self {
        format!("{}", err)
    }
}

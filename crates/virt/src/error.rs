use std::fmt;

/// An error from libvirt or from interpreting its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// libvirt reported an error.
    Libvirt { code: i32, message: String },
    /// libvirt returned XML that could not be parsed.
    Xml(String),
    /// The requested object does not exist (any more).
    NotFound(String),
    /// The input was rejected before reaching libvirt.
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The libvirt error code for "no such domain" and friends.
    pub fn is_not_found(&self) -> bool {
        // VIR_ERR_NO_DOMAIN = 42, VIR_ERR_NO_NETWORK = 43,
        // VIR_ERR_NO_STORAGE_POOL = 49, VIR_ERR_NO_STORAGE_VOL = 50.
        matches!(self, Self::NotFound(_)) || matches!(self, Self::Libvirt { code: 42 | 43 | 49 | 50, .. })
    }

    /// The error text to show to a person.
    pub fn message(&self) -> &str {
        match self {
            Self::Libvirt { message, .. } | Self::Xml(message) | Self::NotFound(message) | Self::Invalid(message) => {
                message
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

impl From<virt::error::Error> for Error {
    fn from(e: virt::error::Error) -> Self {
        Self::Libvirt { code: e.code() as i32, message: e.message().trim().to_owned() }
    }
}

impl From<vmng_xml::ParseError> for Error {
    fn from(e: vmng_xml::ParseError) -> Self {
        Self::Xml(e.to_string())
    }
}

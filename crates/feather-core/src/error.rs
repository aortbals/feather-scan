use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The scanner sent something we did not expect.
    Protocol(String),
    /// A SCSI command failed; carries the decoded sense.
    Scsi(String),
    /// The feeder ran out of paper before the first sheet.
    NoPaper,
    /// Jam, double feed, cover open and so on.
    Feed(String),
    Ocr(String),
    Llm(String),
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "network error: {e}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
            Error::Scsi(m) => write!(f, "scanner error: {m}"),
            Error::NoPaper => write!(f, "no paper in the feeder"),
            Error::Feed(m) => write!(f, "{m}"),
            Error::Ocr(m) => write!(f, "OCR failed: {m}"),
            Error::Llm(m) => write!(f, "title generation failed: {m}"),
            Error::Config(m) => write!(f, "config: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub(crate) fn proto(msg: impl Into<String>) -> Error {
    Error::Protocol(msg.into())
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("authentication failed (check username/password)")]
    Unauthorized,
    /// The endpoint does not exist on this Sunshine version. Callers should
    /// treat the feature as unavailable rather than failing outright.
    #[error("{0} is not supported by this Sunshine version")]
    Unsupported(String),
    #[error(
        "server certificate does not match the pinned fingerprint; if the host's certificate was regenerated, re-trust it with `sun-control hosts trust`"
    )]
    CertificateMismatch,
    #[error("Sunshine reported an error: {0}")]
    Server(String),
    #[error("HTTP {status} from {path}: {body}")]
    Http {
        status: u16,
        path: String,
        body: String,
    },
    #[error("could not reach Sunshine at {url}")]
    Connect {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("unexpected response from {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, ApiError>;

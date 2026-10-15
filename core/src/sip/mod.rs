pub mod dialog;
pub mod message;
pub mod parser;
pub mod register;
pub mod sdp;
pub mod transport;

pub use dialog::{Dialog, DialogState};
pub use message::{SipMessage, SipResponse};
pub use parser::SipParser;
pub use transport::SipSocket;

/// SIP User-Agent 標頭（版本取自 Cargo.toml）
pub const USER_AGENT: &str = concat!("sipress/", env!("CARGO_PKG_VERSION"));

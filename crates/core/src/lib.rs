//! bellwether's brain logic, platform-neutral (distribution plan D1, "Cargo layout"): the
//! parts of brain/*.mjs that need no network or runtime. crates/brain serves them over
//! HTTP; the CLI and the conformance commands call them directly.

pub mod json;
pub mod jsstr;
pub mod redact;
pub mod normalise;
pub mod config;
pub mod flock;
pub mod fixture;
pub mod cliargs;
pub mod envfile;
pub mod mask;
pub mod outfile;
pub mod persist;
pub mod publisher;
pub mod replay;
pub mod shellstate;
pub mod status;
pub mod token;

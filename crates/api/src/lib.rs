//! Home apiserver: admission, watch bus, reconcilers. The only process that opens api.db.

mod admission;
mod controllers;
mod serve;

pub use serve::{ApiError, serve_api};

use std::path::PathBuf;

/// Paths the api binary binds.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub socket: PathBuf,
    pub api_db: PathBuf,
}

//! Estado compartilhado entre os handlers.

use std::sync::Arc;

use crate::cache::SeiCache;
use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub http: reqwest::Client,
    /// Cache em memória das respostas do SEI (ver `cache.rs`).
    pub cache: Arc<SeiCache>,
}

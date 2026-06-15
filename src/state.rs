//! Estado compartilhado entre os handlers.

use std::sync::Arc;

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub http: reqwest::Client,
}

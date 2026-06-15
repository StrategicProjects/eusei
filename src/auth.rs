//! Middleware de autenticação por Bearer token estático.

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, state::AppState};

pub async fn require_bearer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);

    match token {
        Some(t) if state.cfg.tokens.iter().any(|valid| valid == t) => Ok(next.run(req).await),
        _ => Err(AppError::Unauthorized),
    }
}

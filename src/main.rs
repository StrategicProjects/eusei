//! eusei — API HTTP/JSON para os Web Services SOAP do SEI.
//! Roda no servidor de aplicação (único host com acesso liberado ao SEI) e
//! expõe consultas read-only espelhando o pacote R `rsei`.

mod auth;
mod config;
mod docs;
mod error;
mod routes;
mod sei;
mod soap;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{middleware, routing::get, Json, Router};
use config::AppConfig;
use serde_json::json;
use state::AppState;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    // Em dev carregamos um .env; em produção o systemd injeta via EnvironmentFile.
    let _ = dotenvy::dotenv();

    let cfg = match AppConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Erro de configuração: {e}");
            std::process::exit(1);
        }
    };

    tracing_subscriber::registry()
        .with(EnvFilter::new(&cfg.log_filter))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let addr: SocketAddr = cfg
        .bind
        .parse()
        .unwrap_or_else(|e| panic!("EUSEI_BIND inválido ({}): {e}", cfg.bind));

    let state = AppState {
        cfg: Arc::new(cfg),
        http: reqwest::Client::new(),
    };

    let protected = routes::router()
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer));

    let app = Router::new()
        .route("/health", get(health))
        // landing page (Tailwind v4, pública)
        .route("/", get(docs::index))
        .route("/tailwind.css", get(docs::tailwind))
        // documentação online (pública, Tailwind)
        .route("/__docs__", get(docs::docs))
        .route("/__docs__/", get(docs::docs))
        .route("/__docs__/openapi.json", get(docs::openapi))
        .route("/__docs__/tailwind.css", get(docs::tailwind))
        .route("/__docs__/fraunces.woff2", get(docs::font_fraunces))
        .route("/__docs__/splinesans.woff2", get(docs::font_spline))
        .route("/openapi.json", get(docs::openapi))
        .route("/fraunces.woff2", get(docs::font_fraunces))
        .route("/splinesans.woff2", get(docs::font_spline))
        .nest("/v1", protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    tracing::info!(%addr, sei_url = %state.cfg.sei.url, "eusei iniciando");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("não foi possível fazer bind em {addr}: {e}"));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("falha ao servir");
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "eusei", "version": env!("CARGO_PKG_VERSION") }))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("encerrando eusei");
}

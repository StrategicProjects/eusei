//! Cache em memória das respostas do SEI (read-only).
//!
//! Fica no funil `sei::call_with` / `sei::sip_call`, então beneficia todos os
//! endpoints de uma vez (e cada lote do andamentos). Características:
//!
//! - **Single-flight**: requisições idênticas concorrentes coalescem numa única
//!   chamada ao SEI (via `moka::entry().or_try_insert_with`).
//! - **TTL por classe de operação** (estático / semi / dinâmico), via `Expiry`.
//! - **Teto de memória** (peso = tamanho do JSON), via `weigher`.
//! - **Bypass** por `Cache-Control: no-cache`/`no-store` na requisição.
//! - **Erros não são cacheados** (negative caching fica para uma fase futura).
//! - **`X-Cache: HIT|MISS|PARTIAL`** na resposta + contadores em `/health`.
//!
//! A chave de cache deriva de operação + parâmetros **não-secretos** (a chave de
//! acesso do SEI é injetada depois, no funil, e nunca entra na chave).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moka::future::Cache;
use moka::Expiry;
use serde_json::{json, Value};

use crate::config::CacheConfig;
use crate::error::AppError;
use crate::soap::envelope::Param;

/// Valor guardado no cache: o JSON, seu TTL (para o `Expiry`) e o peso em bytes.
#[derive(Clone)]
pub struct CacheValue {
    value: Value,
    ttl: Duration,
    bytes: u32,
}

/// Expiração por entrada: cada valor expira conforme o TTL da sua classe.
struct PerEntryExpiry;
impl Expiry<String, CacheValue> for PerEntryExpiry {
    fn expire_after_create(&self, _key: &String, v: &CacheValue, _now: Instant) -> Option<Duration> {
        Some(v.ttl)
    }
}

/// Cache + contadores globais (para observabilidade no `/health`).
pub struct SeiCache {
    inner: Cache<String, CacheValue>,
    enabled: bool,
    hits: AtomicU64,
    misses: AtomicU64,
    ttl_estatico: Duration,
    ttl_semi: Duration,
    ttl_dinamico: Duration,
}

impl SeiCache {
    pub fn new(cfg: &CacheConfig) -> Arc<Self> {
        let inner = Cache::builder()
            .max_capacity(cfg.max_bytes)
            .weigher(|_k: &String, v: &CacheValue| v.bytes)
            .expire_after(PerEntryExpiry)
            .build();
        Arc::new(SeiCache {
            inner,
            enabled: cfg.enabled,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            ttl_estatico: cfg.ttl_estatico,
            ttl_semi: cfg.ttl_semi,
            ttl_dinamico: cfg.ttl_dinamico,
        })
    }

    /// TTL da operação conforme sua classe (estático / semi / dinâmico).
    pub fn ttl_for(&self, operation: &str) -> Duration {
        const ESTATICOS: &[&str] = &[
            "listarPaises",
            "listarEstados",
            "listarCidades",
            "listarSeries",
            "listarTiposProcedimento",
            "listarTiposProcedimentoOuvidoria",
            "listarTiposConferencia",
            "listarCargos",
            "listarHipotesesLegais",
            "listarExtensoesPermitidas",
            "listarUnidades",
            "listarMarcadoresUnidade",
            "listarFeriados",
        ];
        const SEMI: &[&str] = &["listarUsuarios", "listarContatos", "listarPermissao"];
        if ESTATICOS.contains(&operation) {
            self.ttl_estatico
        } else if SEMI.contains(&operation) {
            self.ttl_semi
        } else {
            self.ttl_dinamico
        }
    }

    /// Devolve o valor cacheado para `key` ou executa `init` (uma vez, mesmo sob
    /// concorrência) e cacheia o resultado. Erros são propagados sem cachear.
    pub async fn get_or_fetch<F>(&self, key: String, ttl: Duration, init: F) -> Result<Value, AppError>
    where
        F: std::future::Future<Output = Result<Value, AppError>> + Send + 'static,
    {
        if !self.enabled {
            return init.await;
        }

        // Bypass (Cache-Control: no-cache): busca fresco e atualiza o cache.
        if ctx_bypass() {
            let v = init.await?;
            self.inner.insert(key, CacheValue::new(v.clone(), ttl)).await;
            self.note(false);
            return Ok(v);
        }

        let entry = self
            .inner
            .entry_by_ref(&key)
            .or_try_insert_with(async move { init.await.map(|v| CacheValue::new(v, ttl)) })
            .await
            .map_err(|e: Arc<AppError>| (*e).clone())?;

        // `is_fresh()` => acabou de ser computado (MISS); senão veio do cache (HIT).
        self.note(!entry.is_fresh());
        Ok(entry.into_value().value)
    }

    /// Registra hit/miss nos contadores globais e na estatística da requisição.
    fn note(&self, hit: bool) {
        if hit {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        let _ = REQ.try_with(|s| {
            let counter = if hit { &s.stats.hits } else { &s.stats.misses };
            counter.fetch_add(1, Ordering::Relaxed);
        });
    }

    /// Estatísticas para o `/health`.
    pub fn stats_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "entradas": self.inner.entry_count(),
            "bytes": self.inner.weighted_size(),
            "hits": self.hits.load(Ordering::Relaxed),
            "misses": self.misses.load(Ordering::Relaxed),
        })
    }
}

impl CacheValue {
    fn new(value: Value, ttl: Duration) -> Self {
        // Peso ~= tamanho do JSON serializado (serializa uma vez, no miss).
        let bytes = serde_json::to_string(&value)
            .map(|s| s.len())
            .unwrap_or(0)
            .min(u32::MAX as usize) as u32;
        CacheValue { value, ttl, bytes }
    }
}

/// Chave de cache para uma operação do SEI. Inclui só parâmetros não-secretos
/// (`SiglaSistema`/`IdentificacaoServico`/`IdUnidade` são injetados depois).
pub fn key_sei(operation: &str, include_unidade: bool, extra: &[(&str, Param)]) -> String {
    format!("sei:{operation}:{}:{}", include_unidade, encode_params(extra))
}

/// Chave de cache para uma operação do SIP.
pub fn key_sip(operation: &str, extra: &[(&str, Param)]) -> String {
    format!("sip:{operation}:{}", encode_params(extra))
}

/// Serializa os parâmetros de forma determinística (ordenados por chave).
fn encode_params(extra: &[(&str, Param)]) -> String {
    let mut parts: Vec<String> = extra
        .iter()
        .map(|(k, v)| match v {
            Param::Scalar(s) => format!("{k}=S:{s}"),
            Param::Array(a) => format!("{k}=A:{}", a.join(",")),
        })
        .collect();
    parts.sort();
    parts.join("\u{1f}")
}

// --- Contexto por requisição (bypass + estatística para o header X-Cache) ------

#[derive(Default)]
struct ReqStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

struct ReqCtx {
    bypass: bool,
    stats: Arc<ReqStats>,
}

tokio::task_local! {
    static REQ: ReqCtx;
}

fn ctx_bypass() -> bool {
    REQ.try_with(|c| c.bypass).unwrap_or(false)
}

/// Middleware: lê `Cache-Control` (bypass), abre o escopo do contexto por toda a
/// execução do handler e, ao final, anota `X-Cache: HIT|MISS|PARTIAL`.
pub async fn middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header::CACHE_CONTROL, HeaderValue};

    let bypass = req
        .headers()
        .get(CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("no-cache") || s.contains("no-store")
        })
        .unwrap_or(false);

    let stats = Arc::new(ReqStats::default());
    let ctx = ReqCtx {
        bypass,
        stats: stats.clone(),
    };
    let mut resp = REQ.scope(ctx, next.run(req)).await;

    let h = stats.hits.load(Ordering::Relaxed);
    let m = stats.misses.load(Ordering::Relaxed);
    let label = match (h, m) {
        (0, 0) => None,
        (_, 0) => Some("HIT"),
        (0, _) => Some("MISS"),
        _ => Some("PARTIAL"),
    };
    if let Some(l) = label {
        resp.headers_mut()
            .insert("x-cache", HeaderValue::from_static(l));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chave_ignora_ordem_e_distingue_operacao() {
        let a = key_sei("listarSeries", false, &[("IdUnidade", Param::Scalar("1".into())), ("IdSerie", Param::Scalar("2".into()))]);
        let b = key_sei("listarSeries", false, &[("IdSerie", Param::Scalar("2".into())), ("IdUnidade", Param::Scalar("1".into()))]);
        assert_eq!(a, b, "ordem dos params não deve mudar a chave");
        let c = key_sei("listarPaises", false, &[]);
        assert_ne!(a, c);
        // include_unidade faz parte da chave
        let d = key_sei("listarSeries", true, &[("IdUnidade", Param::Scalar("1".into())), ("IdSerie", Param::Scalar("2".into()))]);
        assert_ne!(a, d);
    }

    #[test]
    fn chave_sei_e_sip_nao_colidem() {
        let s = key_sei("listarPermissao", false, &[("X", Param::Scalar("1".into()))]);
        let p = key_sip("listarPermissao", &[("X", Param::Scalar("1".into()))]);
        assert_ne!(s, p);
    }
}

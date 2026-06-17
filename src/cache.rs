//! Cache em memória das respostas do SEI (read-only).
//!
//! Fica no funil `sei::call_with` / `sei::sip_call`, então beneficia todos os
//! endpoints de uma vez (e cada lote do andamentos). Características:
//!
//! - **Single-flight**: requisições idênticas concorrentes coalescem numa única
//!   chamada ao SEI (via `moka::entry().or_try_insert_with`).
//! - **TTL por classe de operação** (estático / semi / dinâmico), via `Expiry`.
//! - **Serve-stale-on-error**: se o SEI falha (indisponível/timeout/HTTP/parse), o
//!   último valor bom é servido (`X-Cache: STALE`) por uma janela maior. Isso usa
//!   um segundo cache (`stale`) de retenção longa; o valor é `Arc<Value>`, então os
//!   dois caches **compartilham** a mesma memória (não duplica o payload).
//! - **Teto de memória** (peso = tamanho do JSON), via `weigher`.
//! - **Bypass** por `Cache-Control: no-cache`/`no-store` na requisição.
//! - **Erros não são cacheados** (negative caching fica para uma fase futura).
//! - **`X-Cache: HIT|MISS|STALE|PARTIAL`** na resposta + contadores em `/health`.
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

/// Valor guardado no cache. O JSON vai em `Arc` para que os caches `fresh` e
/// `stale` compartilhem a mesma alocação.
#[derive(Clone)]
pub struct CacheValue {
    value: Arc<Value>,
    ttl: Duration,
    bytes: u32,
}

/// Expiração por entrada (cache `fresh`): cada valor expira no TTL da sua classe.
struct PerEntryExpiry;
impl Expiry<String, CacheValue> for PerEntryExpiry {
    fn expire_after_create(&self, _key: &String, v: &CacheValue, _now: Instant) -> Option<Duration> {
        Some(v.ttl)
    }
}

/// Resultado de uma busca, para fins de contagem / header `X-Cache`.
#[derive(Clone, Copy)]
enum Hit {
    Fresh,
    Miss,
    Stale,
}

/// Cache + contadores globais (para observabilidade no `/health`).
pub struct SeiCache {
    /// Cache de frescor: TTL curto por classe; é nele que o single-flight ocorre.
    fresh: Cache<String, CacheValue>,
    /// Cache de retenção longa para serve-stale (None se desligado).
    stale: Option<Cache<String, CacheValue>>,
    enabled: bool,
    hits: AtomicU64,
    misses: AtomicU64,
    stales: AtomicU64,
    ttl_estatico: Duration,
    ttl_semi: Duration,
    ttl_dinamico: Duration,
}

impl SeiCache {
    pub fn new(cfg: &CacheConfig) -> Arc<Self> {
        let fresh = Cache::builder()
            .max_capacity(cfg.max_bytes)
            .weigher(|_k: &String, v: &CacheValue| v.bytes)
            .expire_after(PerEntryExpiry)
            .build();
        let stale = if cfg.stale_ttl.is_zero() {
            None
        } else {
            Some(
                Cache::builder()
                    .max_capacity(cfg.max_bytes)
                    .weigher(|_k: &String, v: &CacheValue| v.bytes)
                    .time_to_live(cfg.stale_ttl)
                    .build(),
            )
        };
        Arc::new(SeiCache {
            fresh,
            stale,
            enabled: cfg.enabled,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stales: AtomicU64::new(0),
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
    /// concorrência) e cacheia o resultado. Em falha de infraestrutura do SEI,
    /// serve o último valor bom (serve-stale). Erros são propagados sem cachear.
    pub async fn get_or_fetch<F>(&self, key: String, ttl: Duration, init: F) -> Result<Value, AppError>
    where
        F: std::future::Future<Output = Result<Value, AppError>> + Send + 'static,
    {
        if !self.enabled {
            return init.await;
        }

        // Bypass (Cache-Control: no-cache): busca fresco e atualiza os caches; em
        // erro, propaga (respeita o pedido explícito do cliente por dado fresco).
        if ctx_bypass() {
            let v = init.await?;
            let cv = CacheValue::new(v, ttl);
            self.store(&key, &cv).await;
            self.note(Hit::Miss);
            return Ok((*cv.value).clone());
        }

        let key2 = key.clone();
        let res = self
            .fresh
            .entry_by_ref(&key)
            .or_try_insert_with(async move { init.await.map(|v| CacheValue::new(v, ttl)) })
            .await;

        match res {
            Ok(entry) => {
                let recem = entry.is_fresh();
                let cv = entry.into_value();
                if recem {
                    // MISS: acabou de buscar — espelha no cache de stale.
                    if let Some(st) = &self.stale {
                        st.insert(key2, cv.clone()).await;
                    }
                    self.note(Hit::Miss);
                } else {
                    self.note(Hit::Fresh);
                }
                Ok((*cv.value).clone())
            }
            Err(err) => {
                // Falha de infraestrutura: tenta servir o último valor bom.
                if err.permite_stale() {
                    if let Some(st) = &self.stale {
                        if let Some(cv) = st.get(&key2).await {
                            self.note(Hit::Stale);
                            return Ok((*cv.value).clone());
                        }
                    }
                }
                self.note(Hit::Miss);
                Err((*err).clone())
            }
        }
    }

    /// Escreve nos caches `fresh` e `stale` (compartilhando o `Arc`).
    async fn store(&self, key: &str, cv: &CacheValue) {
        self.fresh.insert(key.to_string(), cv.clone()).await;
        if let Some(st) = &self.stale {
            st.insert(key.to_string(), cv.clone()).await;
        }
    }

    /// Registra o resultado nos contadores globais e na estatística da requisição.
    fn note(&self, hit: Hit) {
        let global = match hit {
            Hit::Fresh => &self.hits,
            Hit::Miss => &self.misses,
            Hit::Stale => &self.stales,
        };
        global.fetch_add(1, Ordering::Relaxed);
        let _ = REQ.try_with(|s| {
            let counter = match hit {
                Hit::Fresh => &s.stats.hits,
                Hit::Miss => &s.stats.misses,
                Hit::Stale => &s.stats.stales,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        });
    }

    /// Estatísticas para o `/health`.
    pub fn stats_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "entradas": self.fresh.entry_count(),
            "bytes": self.fresh.weighted_size(),
            "hits": self.hits.load(Ordering::Relaxed),
            "misses": self.misses.load(Ordering::Relaxed),
            "stale": self.stales.load(Ordering::Relaxed),
            "stale_habilitado": self.stale.is_some(),
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
        CacheValue {
            value: Arc::new(value),
            ttl,
            bytes,
        }
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
    stales: AtomicU64,
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
/// execução do handler e, ao final, anota `X-Cache: HIT|MISS|STALE|PARTIAL`.
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
    let s = stats.stales.load(Ordering::Relaxed);
    let total = h + m + s;
    let label = if total == 0 {
        None
    } else if h == total {
        Some("HIT")
    } else if m == total {
        Some("MISS")
    } else if s == total {
        Some("STALE")
    } else {
        Some("PARTIAL")
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

    fn cfg_para_teste(ttl_dinamico: Duration, stale_ttl: Duration) -> CacheConfig {
        CacheConfig {
            enabled: true,
            max_bytes: 8 * 1024 * 1024,
            ttl_estatico: Duration::from_secs(60),
            ttl_semi: Duration::from_secs(60),
            ttl_dinamico,
            stale_ttl,
        }
    }

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

    #[tokio::test]
    async fn serve_stale_em_falha_de_infra_mas_propaga_fault() {
        let ttl = Duration::from_millis(50);
        let c = SeiCache::new(&cfg_para_teste(ttl, Duration::from_secs(60)));

        // 1) sucesso: popula fresh + stale
        let v1 = c
            .get_or_fetch("k".into(), ttl, async { Ok(json!({"x": 1})) })
            .await
            .unwrap();
        assert_eq!(v1, json!({"x": 1}));

        // espera o fresh expirar
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 2) falha de infra (Timeout) -> serve o último valor bom (stale)
        let v2 = c
            .get_or_fetch("k".into(), ttl, async { Err(AppError::Timeout) })
            .await
            .unwrap();
        assert_eq!(v2, json!({"x": 1}), "deve servir o último valor bom");

        // 3) SOAP Fault NÃO serve stale -> propaga o erro
        tokio::time::sleep(Duration::from_millis(150)).await;
        let r3 = c
            .get_or_fetch("k".into(), ttl, async {
                Err(AppError::SoapFault { code: "1".into(), string: "x".into() })
            })
            .await;
        assert!(r3.is_err(), "fault não deve ser mascarado por stale");
    }

    #[tokio::test]
    async fn stale_desligado_propaga_erro() {
        let ttl = Duration::from_millis(50);
        let c = SeiCache::new(&cfg_para_teste(ttl, Duration::ZERO)); // stale off
        let _ = c.get_or_fetch("k".into(), ttl, async { Ok(json!(1)) }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let r = c.get_or_fetch("k".into(), ttl, async { Err(AppError::SeiUnavailable) }).await;
        assert!(r.is_err(), "sem stale, erro de infra propaga");
    }
}

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

/// Conteúdo de uma entrada de cache: uma resposta boa ou um SOAP Fault lembrado
/// (negative caching). O JSON bom vai em `Arc` para que `fresh` e `stale`
/// compartilhem a mesma alocação.
#[derive(Clone)]
enum Payload {
    Ok(Arc<Value>),
    Fault { code: String, string: String },
}

/// Entrada do cache: conteúdo + TTL próprio (frescor ou negative) + idade + peso.
#[derive(Clone)]
pub struct CacheEntry {
    payload: Payload,
    ttl: Duration,
    bytes: u32,
    /// Quando este valor foi obtido do SEI (para calcular o header `Age`).
    created: Instant,
}

/// Expiração por entrada (cache `fresh`): cada valor expira no seu próprio TTL
/// (frescor para respostas boas; negative-TTL para SOAP Faults).
struct PerEntryExpiry;
impl Expiry<String, CacheEntry> for PerEntryExpiry {
    fn expire_after_create(&self, _key: &String, v: &CacheEntry, _now: Instant) -> Option<Duration> {
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
    fresh: Cache<String, CacheEntry>,
    /// Cache de retenção longa para serve-stale (None se desligado). Só guarda
    /// respostas boas (faults nunca vão para o stale).
    stale: Option<Cache<String, CacheEntry>>,
    enabled: bool,
    /// TTL do negative caching (SOAP Fault); ZERO desliga.
    neg_ttl: Duration,
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
            .weigher(|_k: &String, v: &CacheEntry| v.bytes)
            .expire_after(PerEntryExpiry)
            .build();
        let stale = if cfg.stale_ttl.is_zero() {
            None
        } else {
            Some(
                Cache::builder()
                    .max_capacity(cfg.max_bytes)
                    .weigher(|_k: &String, v: &CacheEntry| v.bytes)
                    .time_to_live(cfg.stale_ttl)
                    .build(),
            )
        };
        Arc::new(SeiCache {
            fresh,
            stale,
            enabled: cfg.enabled,
            neg_ttl: cfg.neg_ttl,
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

        // Bypass: busca fresco; em erro, propaga (respeita o pedido por dado
        // fresco). `no-cache` ainda atualiza os caches; `no-store` NÃO persiste.
        let modo = ctx_bypass();
        if modo != Bypass::Off {
            let v = init.await?;
            if modo == Bypass::NoCache {
                self.store(&key, &CacheEntry::ok(v.clone(), ttl)).await;
            }
            self.note(Hit::Miss, ttl, Duration::ZERO);
            return Ok(v);
        }

        // O init mapeia o resultado: resposta boa -> entrada `Ok`; SOAP Fault ->
        // entrada `Fault` (negative caching, se habilitado), também cacheada; erro
        // de infraestrutura -> `Err` (não cacheia; cai no serve-stale).
        let key2 = key.clone();
        let neg_ttl = self.neg_ttl;
        let res = self
            .fresh
            .entry_by_ref(&key)
            .or_try_insert_with(async move {
                match init.await {
                    Ok(v) => Ok(CacheEntry::ok(v, ttl)),
                    Err(AppError::SoapFault { code, string }) if !neg_ttl.is_zero() => {
                        Ok(CacheEntry::fault(code, string, neg_ttl))
                    }
                    Err(e) => Err(e),
                }
            })
            .await;

        match res {
            Ok(entry) => {
                let recem = entry.is_fresh();
                let entry = entry.into_value();
                let age = if recem { Duration::ZERO } else { entry.idade() };
                self.note(if recem { Hit::Miss } else { Hit::Fresh }, entry.ttl, age);
                match &entry.payload {
                    Payload::Ok(v) => {
                        // MISS de resposta boa -> espelha no cache de stale.
                        if recem {
                            if let Some(st) = &self.stale {
                                st.insert(key2, entry.clone()).await;
                            }
                        }
                        Ok((**v).clone())
                    }
                    // Fault cacheado (negativo): devolve o mesmo erro, sem ir ao SEI.
                    Payload::Fault { code, string } => Err(AppError::SoapFault {
                        code: code.clone(),
                        string: string.clone(),
                    }),
                }
            }
            Err(err) => {
                // Falha de infraestrutura: tenta servir o último valor bom.
                if err.permite_stale() {
                    if let Some(st) = &self.stale {
                        if let Some(entry) = st.get(&key2).await {
                            if let Payload::Ok(v) = &entry.payload {
                                self.note(Hit::Stale, entry.ttl, entry.idade());
                                return Ok((**v).clone());
                            }
                        }
                    }
                }
                self.note(Hit::Miss, ttl, Duration::ZERO);
                Err((*err).clone())
            }
        }
    }

    /// Escreve uma resposta boa nos caches `fresh` e `stale` (compartilhando o `Arc`).
    async fn store(&self, key: &str, entry: &CacheEntry) {
        self.fresh.insert(key.to_string(), entry.clone()).await;
        if let Some(st) = &self.stale {
            st.insert(key.to_string(), entry.clone()).await;
        }
    }

    /// Registra o resultado nos contadores globais e na estatística da requisição,
    /// incluindo o menor TTL e a maior idade vistos (para `Cache-Control`/`Age`).
    fn note(&self, hit: Hit, ttl: Duration, age: Duration) {
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
            s.stats.min_ttl.fetch_min(ttl.as_secs(), Ordering::Relaxed);
            s.stats.max_age.fetch_max(age.as_secs(), Ordering::Relaxed);
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

impl CacheEntry {
    fn ok(value: Value, ttl: Duration) -> Self {
        // Peso ~= tamanho do JSON serializado (serializa uma vez, no miss).
        let bytes = serde_json::to_string(&value)
            .map(|s| s.len())
            .unwrap_or(0)
            .min(u32::MAX as usize) as u32;
        CacheEntry {
            payload: Payload::Ok(Arc::new(value)),
            ttl,
            bytes,
            created: Instant::now(),
        }
    }

    fn fault(code: String, string: String, ttl: Duration) -> Self {
        let bytes = (code.len() + string.len()).min(u32::MAX as usize) as u32;
        CacheEntry {
            payload: Payload::Fault { code, string },
            ttl,
            bytes,
            created: Instant::now(),
        }
    }

    /// Há quanto tempo este valor foi obtido do SEI.
    fn idade(&self) -> Duration {
        Instant::now().saturating_duration_since(self.created)
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

struct ReqStats {
    hits: AtomicU64,
    misses: AtomicU64,
    stales: AtomicU64,
    /// Menor TTL (s) entre as operações cacheáveis tocadas (u64::MAX = nenhuma).
    min_ttl: AtomicU64,
    /// Maior idade (s) de um valor servido do cache (para o header `Age`).
    max_age: AtomicU64,
}

impl Default for ReqStats {
    fn default() -> Self {
        ReqStats {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stales: AtomicU64::new(0),
            min_ttl: AtomicU64::new(u64::MAX),
            max_age: AtomicU64::new(0),
        }
    }
}

/// Diretiva de cache da requisição (do header `Cache-Control`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bypass {
    /// Sem bypass: usa o cache normalmente.
    Off,
    /// `no-cache`: busca fresco e **atualiza** o cache.
    NoCache,
    /// `no-store`: busca fresco e **não persiste** (dados não vão p/ fresh/stale).
    NoStore,
}

struct ReqCtx {
    bypass: Bypass,
    stats: Arc<ReqStats>,
}

tokio::task_local! {
    static REQ: ReqCtx;
}

fn ctx_bypass() -> Bypass {
    REQ.try_with(|c| c.bypass).unwrap_or(Bypass::Off)
}

/// Lê a diretiva de cache da requisição atual (`Off` se fora de escopo). Para
/// capturar antes de `tokio::spawn` e repassar via [`com_bypass`].
pub(crate) fn bypass_atual() -> Bypass {
    ctx_bypass()
}

/// Executa `fut` dentro de um contexto de cache com a diretiva dada. Necessário
/// para repassar o contexto a uma task spawnada — o task-local não cruza
/// `tokio::spawn` (usado pelo endpoint SSE de andamentos).
pub(crate) async fn com_bypass<F>(bypass: Bypass, fut: F) -> F::Output
where
    F: std::future::Future,
{
    let ctx = ReqCtx {
        bypass,
        stats: Arc::new(ReqStats::default()),
    };
    REQ.scope(ctx, fut).await
}

/// Middleware: lê `Cache-Control` (bypass), abre o escopo do contexto por toda a
/// execução do handler e, ao final, anota `X-Cache: HIT|MISS|STALE|PARTIAL`.
pub async fn middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{
        header::{CACHE_CONTROL, AGE},
        HeaderValue,
    };

    let bypass = req
        .headers()
        .get(CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let s = s.to_ascii_lowercase();
            // no-store é mais estrito que no-cache: tem prioridade.
            if s.contains("no-store") {
                Bypass::NoStore
            } else if s.contains("no-cache") {
                Bypass::NoCache
            } else {
                Bypass::Off
            }
        })
        .unwrap_or(Bypass::Off);

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

    // Cache-Control coerente com o TTL da operação (só quando houve operação
    // cacheável). max-age = TTL pleno + Age = idade já decorrida (o cliente
    // calcula o frescor restante — evita staleness composta).
    let min_ttl = stats.min_ttl.load(Ordering::Relaxed);
    if min_ttl != u64::MAX {
        let success = resp.status().is_success();
        let headers = resp.headers_mut();
        if bypass == Bypass::NoStore || !success {
            // no-store (respeita o pedido do cliente) ou erro: nada de cachear,
            // nem no cliente/proxy privado.
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        } else if s > 0 {
            // dado obsoleto servido: o cliente não deve recachear sem revalidar
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        } else {
            let age = stats.max_age.load(Ordering::Relaxed);
            if let Ok(cc) = HeaderValue::from_str(&format!("private, max-age={min_ttl}")) {
                headers.insert(CACHE_CONTROL, cc);
            }
            if let Ok(a) = HeaderValue::from_str(&age.to_string()) {
                headers.insert(AGE, a);
            }
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_para_teste(ttl_dinamico: Duration, stale_ttl: Duration) -> CacheConfig {
        cfg_neg(ttl_dinamico, stale_ttl, Duration::from_secs(30))
    }

    fn cfg_neg(ttl_dinamico: Duration, stale_ttl: Duration, neg_ttl: Duration) -> CacheConfig {
        CacheConfig {
            enabled: true,
            max_bytes: 8 * 1024 * 1024,
            ttl_estatico: Duration::from_secs(60),
            ttl_semi: Duration::from_secs(60),
            ttl_dinamico,
            stale_ttl,
            neg_ttl,
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
    async fn negative_caching_lembra_fault_mas_nao_erro_de_infra() {
        use std::sync::atomic::{AtomicUsize, Ordering as O};
        let ttl = Duration::from_secs(30);

        // (a) SOAP Fault é cacheado -> init roda só 1x; 2ª devolve o mesmo fault.
        let c = SeiCache::new(&cfg_neg(ttl, Duration::ZERO, Duration::from_secs(30)));
        let n = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let n2 = n.clone();
            let r = c
                .get_or_fetch("k".into(), ttl, async move {
                    n2.fetch_add(1, O::SeqCst);
                    Err(AppError::SoapFault { code: "1".into(), string: "inexistente".into() })
                })
                .await;
            assert!(matches!(r, Err(AppError::SoapFault { .. })));
        }
        assert_eq!(n.load(O::SeqCst), 1, "fault deve ser cacheado (init 1x)");

        // (b) erro de infra NÃO é cacheado -> init roda nas duas vezes.
        let c2 = SeiCache::new(&cfg_neg(ttl, Duration::ZERO, Duration::from_secs(30)));
        let m = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let m2 = m.clone();
            let _ = c2
                .get_or_fetch("k".into(), ttl, async move {
                    m2.fetch_add(1, O::SeqCst);
                    Err(AppError::Timeout)
                })
                .await;
        }
        assert_eq!(m.load(O::SeqCst), 2, "erro de infra não deve ser cacheado");

        // (c) negative caching desligado (neg_ttl=0) -> fault não é cacheado.
        let c3 = SeiCache::new(&cfg_neg(ttl, Duration::ZERO, Duration::ZERO));
        let p = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let p2 = p.clone();
            let _ = c3
                .get_or_fetch("k".into(), ttl, async move {
                    p2.fetch_add(1, O::SeqCst);
                    Err(AppError::SoapFault { code: "1".into(), string: "x".into() })
                })
                .await;
        }
        assert_eq!(p.load(O::SeqCst), 2, "com neg desligado, fault não é cacheado");
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

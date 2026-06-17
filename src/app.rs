//! Montagem do router HTTP. Separado de `main.rs` para permitir testes de rota
//! in-process (sem abrir porta para a aplicação).

use axum::{extract::State, middleware, routing::get, Json, Router};
use serde_json::json;

use crate::{auth, cache, docs, routes, state::AppState};

/// Monta o router completo: rotas públicas (landing, docs, assets, health) e as
/// protegidas sob `/v1` (Bearer obrigatório).
pub fn build_app(state: AppState) -> Router {
    let protected = routes::router()
        // cache: contexto por requisição (bypass + X-Cache), por dentro do auth
        .route_layer(middleware::from_fn(cache::middleware))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer));

    Router::new()
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
        .with_state(state)
}

pub async fn health(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "eusei",
        "version": env!("CARGO_PKG_VERSION"),
        "cache": s.cache.stats_json(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, SeiConfig, SipConfig};
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use serde_json::Value;
    use tower::ServiceExt; // para `oneshot`

    const TOKEN: &str = "token-de-teste";

    /// Sobe um SEI/SIP falso que responde envelopes SOAP conforme a operação
    /// presente no corpo da requisição. Devolve a URL base (`http://127.0.0.1:PORTA/`).
    async fn mock_sei() -> String {
        async fn handler(body: String) -> axum::response::Response {
            use axum::http::header;
            let resp = |xml: &str| {
                axum::response::Response::builder()
                    .header(header::CONTENT_TYPE, "text/xml")
                    .body(Body::from(xml.to_string()))
                    .unwrap()
            };
            // Falha sistêmica simulada para `consultarPublicacao` (HTTP 500 sem
            // SOAP Fault) — usada para testar a propagação em publicacoes-processo.
            if body.contains("consultarPublicacao") {
                return axum::response::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from("erro interno do SEI".to_string()))
                    .unwrap();
            }
            if body.contains("listarAndamentos") {
                return resp(SOAP_ANDAMENTOS);
            }
            if body.contains("listarPermissao") {
                return resp(SOAP_SIP_RETURN);
            }
            if body.contains("listarPaises") {
                return resp(SOAP_LISTA);
            }
            // consultarProcedimento e afins -> objeto
            resp(SOAP_OBJETO)
        }

        let app = Router::new().route("/", axum::routing::post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/")
    }

    const SOAP_OBJETO: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><parametros><ProcedimentoFormatado>0001/2022</ProcedimentoFormatado></parametros></Resp></soap:Body></soap:Envelope>"#;
    const SOAP_LISTA: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><parametros><item><Nome>BRASIL</Nome></item><item><Nome>PORTUGAL</Nome></item></parametros></Resp></soap:Body></soap:Envelope>"#;
    const SOAP_ANDAMENTOS: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><parametros><item><IdAndamento>13</IdAndamento><DataHora>30/06/2022 13:56:27</DataHora><Descricao>Gerado documento 84230597</Descricao></item></parametros></Resp></soap:Body></soap:Envelope>"#;
    const SOAP_SIP_RETURN: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><returnPermissoes><item><IdUsuario>42</IdUsuario></item></returnPermissoes></Resp></soap:Body></soap:Envelope>"#;

    /// Mock do SEI que conta quantas requisições POST recebeu (para testar cache).
    /// Responde sempre a lista de exemplo — basta para `/v1/paises`.
    async fn mock_sei_contado() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let app = Router::new().route(
            "/",
            axum::routing::post(move |_body: String| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    axum::response::Response::builder()
                        .header(axum::http::header::CONTENT_TYPE, "text/xml")
                        .body(Body::from(SOAP_LISTA.to_string()))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/"), count)
    }

    fn cache_cfg() -> crate::config::CacheConfig {
        crate::config::CacheConfig {
            enabled: true,
            max_bytes: 32 * 1024 * 1024,
            ttl_estatico: std::time::Duration::from_secs(3600),
            ttl_semi: std::time::Duration::from_secs(600),
            ttl_dinamico: std::time::Duration::from_secs(30),
            stale_ttl: std::time::Duration::from_secs(3600),
            neg_ttl: std::time::Duration::from_secs(30),
        }
    }

    fn state_com(sei_url: String, sip: SipConfig) -> AppState {
        let cc = cache_cfg();
        AppState {
            cfg: Arc::new(AppConfig {
                bind: "127.0.0.1:0".into(),
                tokens: vec![TOKEN.into()],
                sei: SeiConfig {
                    url: sei_url,
                    sigla_sistema: "TESTE".into(),
                    identificacao_servico: "chave".into(),
                    id_unidade: "1".into(),
                    timeout_secs: 5,
                    andamentos_lote: 10,
                    andamentos_conc: 4,
                },
                sip,
                cache: cc.clone(),
                log_filter: "off".into(),
            }),
            http: reqwest::Client::new(),
            cache: crate::cache::SeiCache::new(&cc),
        }
    }

    fn sip_off() -> SipConfig {
        SipConfig { url: "http://invalido/".into(), chave_acesso: "".into(), id_sistema: "".into() }
    }

    /// Envia uma requisição GET ao router e devolve (status, corpo JSON).
    async fn get(app: Router, uri: &str, auth: Option<&str>) -> (StatusCode, Value) {
        let mut req = Request::builder().uri(uri).method("GET");
        if let Some(t) = auth {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn health_publico_sem_token() {
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/health", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn protegida_sem_token_401() {
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/v1/paises", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["ok"], false);
        assert_eq!(body["codigo"], "nao_autorizado");
    }

    #[tokio::test]
    async fn protegida_token_errado_401() {
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, _) = get(app, "/v1/paises", Some("errado")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lista_com_token_ok() {
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let (status, body) = get(app, "/v1/paises", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["dados"][0]["Nome"], "BRASIL");
    }

    #[tokio::test]
    async fn procedimentos_sem_protocolos_400_em_json() {
        // #8: parâmetro obrigatório ausente cai no envelope JSON, não na
        // rejeição crua do extractor.
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/v1/procedimentos", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], false);
        assert_eq!(body["codigo"], "parametro_invalido");
    }

    #[tokio::test]
    async fn publicacao_param_vazio_400() {
        // #9: ?id_documento= (vazio) não conta como identificador.
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/v1/publicacao?id_documento=", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["codigo"], "parametro_invalido");
    }

    #[tokio::test]
    async fn flag_sn_invalida_400() {
        // #14: sinalizador S/N inválido -> 400 (em vez de coagir para S).
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let (status, body) = get(
            app,
            "/v1/procedimento?protocolo=0001&sin_retornar_assuntos=talvez",
            Some(TOKEN),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["codigo"], "parametro_invalido");
    }

    #[tokio::test]
    async fn ordenar_invalido_400() {
        // #8: ?ordenar=abc -> 400 no envelope, não rejeição do extractor de bool.
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/v1/andamentos?protocolo=0001&ordenar=abc", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["codigo"], "parametro_invalido");
    }

    #[tokio::test]
    async fn publicacoes_processo_propaga_erro_sistemico() {
        // #7: timeline tem 1 documento, mas consultarPublicacao falha (HTTP 500).
        // Deve propagar como 502, não responder ok:true com lista vazia.
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let (status, body) = get(app, "/v1/publicacoes-processo?protocolo=0001", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["ok"], false);
        assert_eq!(body["codigo"], "sei_erro_http");
    }

    #[tokio::test]
    async fn andamentos_fatiado_inclui_resumo_e_deduplica() {
        // #22: a linha do tempo completa é fatiada em lotes (default 1..200 ->
        // vários lotes). O mock devolve o mesmo IdAndamento em cada lote: a
        // resposta deve trazer `resumo` e deduplicar para 1 registro.
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let (status, body) = get(app, "/v1/andamentos?protocolo=0001", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert!(body["resumo"]["lotes"].as_u64().unwrap() > 1, "deve fatiar em vários lotes");
        assert_eq!(body["resumo"]["parciais"], false);
        // dedup por IdAndamento: todos os lotes trazem o mesmo id -> 1 registro
        assert_eq!(body["dados"].as_array().unwrap().len(), 1);
        assert_eq!(body["resumo"]["registros"], 1);
    }

    #[tokio::test]
    async fn andamentos_stream_emite_progresso_e_concluido() {
        // #22: o endpoint SSE emite eventos `progresso` e um `concluido` final.
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let req = Request::builder()
            .uri("/v1/andamentos/stream?protocolo=0001")
            .method("GET")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(ct.contains("text/event-stream"), "content-type SSE, veio: {ct}");
        let bytes = to_bytes(resp.into_body(), 4 << 20).await.unwrap();
        let texto = String::from_utf8_lossy(&bytes);
        // O nome do evento aparece numa linha `event:<nome>` (com ou sem espaço,
        // conforme a serialização do axum).
        assert!(texto.contains("progresso"), "deve emitir progresso; corpo: {texto}");
        assert!(texto.contains("concluido"), "deve emitir concluido; corpo: {texto}");
    }

    #[tokio::test]
    async fn cache_evita_segunda_chamada_e_marca_x_cache() {
        use std::sync::atomic::Ordering;
        let (url, count) = mock_sei_contado().await;
        let app = build_app(state_com(url, sip_off()));
        let pedir = |cc: Option<&str>| {
            let mut b = Request::builder()
                .uri("/v1/paises")
                .header("Authorization", format!("Bearer {TOKEN}"));
            if let Some(v) = cc {
                b = b.header("Cache-Control", v);
            }
            b.body(Body::empty()).unwrap()
        };
        // 1ª: MISS (vai ao SEI)
        let r1 = app.clone().oneshot(pedir(None)).await.unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        assert_eq!(r1.headers().get("x-cache").and_then(|v| v.to_str().ok()), Some("MISS"));
        // 2ª: HIT (servida do cache, sem nova chamada)
        let r2 = app.clone().oneshot(pedir(None)).await.unwrap();
        assert_eq!(r2.headers().get("x-cache").and_then(|v| v.to_str().ok()), Some("HIT"));
        assert_eq!(count.load(Ordering::SeqCst), 1, "SEI chamado uma única vez");
    }

    #[tokio::test]
    async fn cache_control_reflete_ttl_da_operacao() {
        let (url, _c) = mock_sei_contado().await;
        let app = build_app(state_com(url, sip_off()));
        let req = Request::builder()
            .uri("/v1/paises")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let r = app.oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let cc = r
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // listarPaises é classe estática -> ttl_estatico (3600 no cfg de teste)
        assert!(cc.contains("private"), "deve ser private; veio: {cc}");
        assert!(cc.contains("max-age=3600"), "max-age deve refletir o TTL; veio: {cc}");
        assert!(r.headers().get(axum::http::header::AGE).is_some(), "deve ter Age");
    }

    #[tokio::test]
    async fn cache_bypass_com_no_cache_revalida() {
        use std::sync::atomic::Ordering;
        let (url, count) = mock_sei_contado().await;
        let app = build_app(state_com(url, sip_off()));
        let auth = format!("Bearer {TOKEN}");
        // popula o cache
        let r1 = app
            .clone()
            .oneshot(Request::builder().uri("/v1/paises").header("Authorization", &auth).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        // Cache-Control: no-cache força nova ida ao SEI
        let r2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/paises")
                    .header("Authorization", &auth)
                    .header("Cache-Control", "no-cache")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.headers().get("x-cache").and_then(|v| v.to_str().ok()), Some("MISS"));
        assert_eq!(count.load(Ordering::SeqCst), 2, "no-cache revalida no SEI");
    }

    #[tokio::test]
    async fn procedimento_inclui_concluido_derivado() {
        // #31: a resposta de /v1/procedimento traz o campo derivado `concluido`.
        // O mock não retorna UnidadesProcedimentoAberto -> concluido indeterminado.
        let url = mock_sei().await;
        let app = build_app(state_com(url, sip_off()));
        let (status, body) = get(app, "/v1/procedimento?protocolo=0001", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["dados"]["ProcedimentoFormatado"], "0001/2022");
        assert!(body.as_object().unwrap().contains_key("concluido"), "deve emitir 'concluido'");
        assert!(body["concluido"].is_null(), "sem unidades -> indeterminado");
    }

    #[tokio::test]
    async fn sip_nao_configurado_400() {
        let app = build_app(state_com("http://nao-usado/".into(), sip_off()));
        let (status, body) = get(app, "/v1/permissao", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["codigo"], "parametro_invalido");
    }

    #[tokio::test]
    async fn sip_configurado_ok() {
        let url = mock_sei().await;
        let sip = SipConfig {
            url: url.clone(),
            chave_acesso: "chave-sip".into(),
            id_sistema: "SIP".into(),
        };
        let app = build_app(state_com(url, sip));
        let (status, body) = get(app, "/v1/permissao", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["dados"][0]["IdUsuario"], "42");
    }
}

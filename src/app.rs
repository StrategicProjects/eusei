//! Montagem do router HTTP. Separado de `main.rs` para permitir testes de rota
//! in-process (sem abrir porta para a aplicação).

use axum::{middleware, routing::get, Json, Router};
use serde_json::json;

use crate::{auth, docs, routes, state::AppState};

/// Monta o router completo: rotas públicas (landing, docs, assets, health) e as
/// protegidas sob `/v1` (Bearer obrigatório).
pub fn build_app(state: AppState) -> Router {
    let protected = routes::router()
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

pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "eusei", "version": env!("CARGO_PKG_VERSION") }))
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
    const SOAP_ANDAMENTOS: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><parametros><item><DataHora>30/06/2022 13:56:27</DataHora><Descricao>Gerado documento 84230597</Descricao></item></parametros></Resp></soap:Body></soap:Envelope>"#;
    const SOAP_SIP_RETURN: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Resp><returnPermissoes><item><IdUsuario>42</IdUsuario></item></returnPermissoes></Resp></soap:Body></soap:Envelope>"#;

    fn state_com(sei_url: String, sip: SipConfig) -> AppState {
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
                },
                sip,
                log_filter: "off".into(),
            }),
            http: reqwest::Client::new(),
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

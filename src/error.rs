//! Erros da aplicação e sua conversão em respostas JSON.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(thiserror::Error, Debug, Clone)]
pub enum AppError {
    #[error("não autorizado")]
    Unauthorized,

    #[error("{0}")]
    BadRequest(String),

    /// Não foi possível conectar ao SEI (rede/firewall/servidor fora do ar).
    #[error("SEI indisponível")]
    SeiUnavailable,

    /// O SEI demorou demais para responder.
    #[error("tempo de resposta do SEI esgotado")]
    Timeout,

    /// O SEI respondeu HTTP de erro (não SOAP Fault).
    #[error("falha ao acessar o SEI: {0}")]
    Upstream(String),

    /// O SEI retornou um SOAP Fault (ex.: protocolo inexistente, acesso negado).
    #[error("SOAP Fault [{code}]: {string}")]
    SoapFault { code: String, string: String },

    #[error("erro ao processar resposta do SEI: {0}")]
    Parse(String),
}

impl AppError {
    /// Indica se vale servir um valor cacheado obsoleto (serve-stale) diante deste
    /// erro: só para falhas de **infraestrutura** do SEI (indisponível/timeout/HTTP/
    /// parse), nunca para SOAP Fault (resposta semântica) nem erros de cliente.
    pub fn permite_stale(&self) -> bool {
        matches!(
            self,
            AppError::SeiUnavailable | AppError::Timeout | AppError::Upstream(_) | AppError::Parse(_)
        )
    }

    /// Código estável legível por máquina, para o cliente tratar.
    fn codigo(&self) -> &'static str {
        match self {
            AppError::Unauthorized => "nao_autorizado",
            AppError::BadRequest(_) => "parametro_invalido",
            AppError::SeiUnavailable => "sei_indisponivel",
            AppError::Timeout => "sei_timeout",
            AppError::Upstream(_) => "sei_erro_http",
            AppError::SoapFault { .. } => "sei_fault",
            AppError::Parse(_) => "resposta_invalida",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // (status, mensagem amigável ao cliente, detalhe técnico opcional)
        let (status, erro, detalhe): (StatusCode, String, Option<String>) = match &self {
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Não autorizado. Envie um token válido em 'Authorization: Bearer <token>'.".into(),
                None,
            ),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone(), None),
            AppError::SeiUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "O SEI está temporariamente indisponível. Não foi possível conectar ao \
                 servidor do SEI; tente novamente em alguns minutos."
                    .into(),
                None,
            ),
            AppError::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "O SEI demorou demais para responder. Tente novamente em alguns instantes.".into(),
                None,
            ),
            // SOAP Fault quase sempre indica entrada inválida (ex.: protocolo
            // inexistente); 400 com a faultstring legível.
            AppError::SoapFault { string, .. } => {
                (StatusCode::BAD_REQUEST, string.clone(), None)
            }
            AppError::Upstream(m) => (
                StatusCode::BAD_GATEWAY,
                "O SEI respondeu com um erro inesperado.".into(),
                Some(m.clone()),
            ),
            AppError::Parse(m) => (
                StatusCode::BAD_GATEWAY,
                "Não foi possível interpretar a resposta do SEI.".into(),
                Some(m.clone()),
            ),
        };

        let body = json!({
            "ok": false,
            "codigo": self.codigo(),
            "erro": erro,
            "detalhe": detalhe,
        });
        (status, Json(body)).into_response()
    }
}

//! Middleware de autenticação por Bearer token estático.

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConstantTimeEq};

use crate::{error::AppError, state::AppState};

/// Compara o token recebido com os tokens válidos em tempo constante.
/// Hasheia ambos com SHA-256 antes de comparar: a comparação é sobre digests
/// de tamanho fixo (32 bytes), o que evita tanto o vazamento por byte quanto o
/// vazamento de tamanho do segredo. Não usa short-circuit entre os tokens.
fn token_valido(tokens: &[String], candidato: &str) -> bool {
    let cand = Sha256::digest(candidato.as_bytes());
    let mut ok = Choice::from(0u8);
    for t in tokens {
        let d = Sha256::digest(t.as_bytes());
        ok |= d.ct_eq(cand.as_slice());
    }
    ok.into()
}

pub async fn require_bearer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    // O nome do esquema HTTP é case-insensitive (RFC 7235): aceita "Bearer",
    // "bearer", etc. O valor do token segue case-sensitive.
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, rest)| rest.trim());

    match token {
        Some(t) if token_valido(&state.cfg.tokens, t) => Ok(next.run(req).await),
        _ => Err(AppError::Unauthorized),
    }
}

#[cfg(test)]
mod tests {
    use super::token_valido;

    #[test]
    fn aceita_token_valido_e_rejeita_demais() {
        let tokens = vec!["alpha".to_string(), "bravo".to_string()];
        assert!(token_valido(&tokens, "alpha"));
        assert!(token_valido(&tokens, "bravo"));
        assert!(!token_valido(&tokens, "charlie"));
        assert!(!token_valido(&tokens, "")); // vazio
        assert!(!token_valido(&tokens, "alph")); // prefixo não vale
    }
}

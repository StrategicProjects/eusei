//! Handlers de consulta do SIP (Sistema de Permissões). Endpoint/namespace e
//! autenticação distintos do SEI (ver `sei::sip_call`). Requer `SEI_SIP_*`.

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::{json, Value};

use crate::{error::AppError, soap::envelope::Param, state::AppState};

type Resp = Result<Json<Value>, AppError>;

/// `GET /v1/permissao` — lista permissões no SIP (`listarPermissao`).
pub async fn permissao(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Resp {
    let pairs = [
        ("id_orgao_usuario", "IdOrgaoUsuario"),
        ("id_usuario", "IdUsuario"),
        ("id_origem_usuario", "IdOrigemUsuario"),
        ("id_orgao_unidade", "IdOrgaoUnidade"),
        ("id_unidade", "IdUnidade"),
        ("id_origem_unidade", "IdOrigemUnidade"),
        ("id_perfil", "IdPerfil"),
    ];
    let extra: Vec<(&str, Param)> = pairs
        .iter()
        .filter_map(|(qk, sip)| {
            q.get(*qk)
                .filter(|v| !v.is_empty())
                .map(|v| (*sip, Param::Scalar(v.clone())))
        })
        .collect();

    let dados = super::sip_call(&s, "listarPermissao", extra).await?;
    Ok(Json(json!({ "ok": true, "dados": dados })))
}

//! Operações do SEI sobre o motor SOAP. Cada rota é um wrapper fino sobre
//! `call()`, que injeta os parâmetros de autenticação e devolve JSON.

pub mod andamentos;
pub mod consultas;
pub mod listas;
pub mod sip;

use serde_json::Value;

use crate::{error::AppError, soap, soap::envelope::Param, state::AppState};

/// Executa uma operação read-only do SEI com parâmetros escalares.
///
/// Injeta `SiglaSistema` e `IdentificacaoServico` (e `IdUnidade` quando
/// `include_unidade`), acrescenta `extra` e devolve o `<parametros>` como JSON.
pub async fn call(
    state: &AppState,
    operation: &str,
    include_unidade: bool,
    extra: &[(&str, String)],
) -> Result<Value, AppError> {
    let params: Vec<(&str, Param)> = extra
        .iter()
        .map(|(k, v)| (*k, Param::Scalar(v.clone())))
        .collect();
    call_with(state, operation, include_unidade, params).await
}

/// Como [`call`], mas aceita parâmetros que podem ser arrays (`Param::Array`),
/// necessário para operações como `listarAndamentos`.
pub async fn call_with(
    state: &AppState,
    operation: &str,
    include_unidade: bool,
    extra: Vec<(&str, Param)>,
) -> Result<Value, AppError> {
    let cfg = &state.cfg.sei;
    let mut params: Vec<(&str, Param)> = vec![
        ("SiglaSistema", Param::Scalar(cfg.sigla_sistema.clone())),
        ("IdentificacaoServico", Param::Scalar(cfg.identificacao_servico.clone())),
    ];
    if include_unidade {
        params.push(("IdUnidade", Param::Scalar(cfg.id_unidade.clone())));
    }
    params.extend(extra);

    let body = soap::client::soap_call(
        &state.http,
        &cfg.url,
        cfg.timeout_secs,
        operation,
        &params,
        "sei",
        "Sei",
        "SeiAction",
    )
    .await?;
    soap::parse::parametros_to_json(&body)
}

/// Executa uma operação read-only do SIP (namespace "sip"/"sipns").
/// Injeta `ChaveAcesso` e `IdSistema`. Resposta vem em `<return*>` (não
/// `<parametros>`).
pub async fn sip_call(
    state: &AppState,
    operation: &str,
    extra: Vec<(&str, Param)>,
) -> Result<Value, AppError> {
    let sip = &state.cfg.sip;
    if !sip.configurado() {
        return Err(AppError::BadRequest(
            "Serviço SIP não configurado neste servidor (defina SEI_SIP_CHAVE_ACESSO e \
             SEI_SIP_ID_SISTEMA)."
                .into(),
        ));
    }
    let mut params: Vec<(&str, Param)> = vec![
        ("ChaveAcesso", Param::Scalar(sip.chave_acesso.clone())),
        ("IdSistema", Param::Scalar(sip.id_sistema.clone())),
    ];
    params.extend(extra);

    let body = soap::client::soap_call(
        &state.http,
        &sip.url,
        state.cfg.sei.timeout_secs,
        operation,
        &params,
        "sip",
        "sipns",
        "sipnsAction",
    )
    .await?;
    soap::parse::return_to_json(&body)
}

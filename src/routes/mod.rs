//! Roteador das rotas protegidas (montadas sob `/v1`).

use axum::{routing::get, Router};

use crate::sei::{andamentos, consultas, listas, sip};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // consultas — forma por query string (?protocolo=...) é a recomendada
        // atrás do nginx (a barra do protocolo não sobrevive no path via proxy).
        .route("/procedimento", get(consultas::procedimento_q))
        .route("/procedimento/{protocolo}", get(consultas::procedimento))
        .route("/procedimentos", get(consultas::procedimentos))
        .route("/procedimento-individual", get(consultas::procedimento_individual))
        .route("/documento", get(consultas::documento_q))
        .route("/documento/{protocolo}", get(consultas::documento))
        .route("/documentos", get(consultas::documentos))
        .route("/publicacao", get(consultas::publicacao))
        .route("/bloco", get(consultas::bloco_q))
        .route("/bloco/{id}", get(consultas::bloco))
        // andamentos (linha do tempo) e documentos/publicações do processo
        .route("/andamentos", get(andamentos::andamentos))
        // mesma consulta, via SSE (progresso lote a lote -> cliente acompanha)
        .route("/andamentos/stream", get(andamentos::andamentos_stream))
        .route("/documentos-processo", get(andamentos::documentos_processo))
        .route("/publicacoes-processo", get(andamentos::publicacoes_processo))
        // SIP (Sistema de Permissões) — requer SEI_SIP_*
        .route("/permissao", get(sip::permissao))
        // listas
        .route("/unidades", get(listas::unidades))
        .route("/series", get(listas::series))
        .route("/tipos-procedimento", get(listas::tipos_procedimento))
        .route("/tipos-procedimento-ouvidoria", get(listas::tipos_procedimento_ouvidoria))
        .route("/tipos-conferencia", get(listas::tipos_conferencia))
        .route("/paises", get(listas::paises))
        .route("/estados", get(listas::estados))
        .route("/cidades", get(listas::cidades))
        .route("/cargos", get(listas::cargos))
        .route("/hipoteses-legais", get(listas::hipoteses_legais))
        .route("/extensoes-permitidas", get(listas::extensoes_permitidas))
        .route("/marcadores-unidade", get(listas::marcadores_unidade))
        .route("/usuarios", get(listas::usuarios))
        .route("/feriados", get(listas::feriados))
        .route("/contatos", get(listas::contatos))
}

//! Handlers das operações `listar*` (read-only). Filtros opcionais entram pela
//! query string (snake_case) e são repassados ao SEI quando presentes.

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::{json, Value};

use crate::{error::AppError, state::AppState};

type Resp = Result<Json<Value>, AppError>;
type Q = Query<HashMap<String, String>>;

fn ok(dados: Value) -> Resp {
    Ok(Json(json!({ "ok": true, "dados": dados })))
}

/// Para cada (chave_query, ParamSEI), inclui o filtro se presente e não-vazio.
fn pick(map: &HashMap<String, String>, pairs: &[(&str, &'static str)]) -> Vec<(&'static str, String)> {
    pairs
        .iter()
        .filter_map(|(qk, sei)| {
            map.get(*qk)
                .filter(|v| !v.is_empty())
                .map(|v| (*sei, v.clone()))
        })
        .collect()
}

// Operações cujo `IdUnidade` NÃO é enviado (espelha include_unidade=FALSE no rsei).
pub async fn unidades(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_tipo_procedimento", "IdTipoProcedimento"), ("id_serie", "IdSerie")]);
    ok(super::call(&s, "listarUnidades", false, &e).await?)
}

pub async fn series(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_unidade", "IdUnidade"), ("id_tipo_procedimento", "IdTipoProcedimento")]);
    ok(super::call(&s, "listarSeries", false, &e).await?)
}

pub async fn tipos_procedimento(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_unidade", "IdUnidade"), ("id_serie", "IdSerie")]);
    ok(super::call(&s, "listarTiposProcedimento", false, &e).await?)
}

pub async fn tipos_procedimento_ouvidoria(State(s): State<AppState>) -> Resp {
    ok(super::call(&s, "listarTiposProcedimentoOuvidoria", false, &[]).await?)
}

pub async fn tipos_conferencia(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_unidade", "IdUnidade")]);
    ok(super::call(&s, "listarTiposConferencia", false, &e).await?)
}

// Operações que enviam `IdUnidade` (do config) por padrão.
pub async fn paises(State(s): State<AppState>) -> Resp {
    ok(super::call(&s, "listarPaises", true, &[]).await?)
}

pub async fn estados(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_pais", "IdPais")]);
    ok(super::call(&s, "listarEstados", true, &e).await?)
}

pub async fn cidades(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_pais", "IdPais"), ("id_estado", "IdEstado")]);
    ok(super::call(&s, "listarCidades", true, &e).await?)
}

pub async fn cargos(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_cargo", "IdCargo")]);
    ok(super::call(&s, "listarCargos", true, &e).await?)
}

pub async fn hipoteses_legais(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("nivel_acesso", "NivelAcesso")]);
    ok(super::call(&s, "listarHipotesesLegais", true, &e).await?)
}

pub async fn extensoes_permitidas(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_arquivo_extensao", "IdArquivoExtensao")]);
    ok(super::call(&s, "listarExtensoesPermitidas", true, &e).await?)
}

pub async fn marcadores_unidade(State(s): State<AppState>) -> Resp {
    ok(super::call(&s, "listarMarcadoresUnidade", true, &[]).await?)
}

pub async fn usuarios(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(&q, &[("id_usuario", "IdUsuario")]);
    ok(super::call(&s, "listarUsuarios", true, &e).await?)
}

pub async fn feriados(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(
        &q,
        &[("id_orgao", "IdOrgao"), ("data_inicial", "DataInicial"), ("data_final", "DataFinal")],
    );
    ok(super::call(&s, "listarFeriados", true, &e).await?)
}

pub async fn contatos(State(s): State<AppState>, Query(q): Q) -> Resp {
    let e = pick(
        &q,
        &[
            ("id_tipo_contato", "IdTipoContato"),
            ("pagina_registros", "PaginaRegistros"),
            ("pagina_atual", "PaginaAtual"),
            ("sigla", "Sigla"),
            ("nome", "Nome"),
            ("cpf", "CPF"),
            ("cnpj", "CNPJ"),
            ("matricula", "Matricula"),
        ],
    );
    ok(super::call(&s, "listarContatos", true, &e).await?)
}

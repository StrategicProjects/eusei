//! Handlers das consultas read-only de processos/documentos.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, state::AppState};

type Resp = Result<Json<Value>, AppError>;

fn ok(dados: Value) -> Resp {
    Ok(Json(json!({ "ok": true, "dados": dados })))
}

/// Resolve um sinalizador "S"/"N", default "S".
fn sn(opt: &Option<String>) -> String {
    match opt.as_deref() {
        Some("N") | Some("n") => "N".into(),
        _ => "S".into(),
    }
}

// ---- consultarProcedimento ------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ProcFlags {
    pub sin_retornar_assuntos: Option<String>,
    pub sin_retornar_interessados: Option<String>,
    pub sin_retornar_observacoes: Option<String>,
    pub sin_retornar_andamento_geracao: Option<String>,
    pub sin_retornar_andamento_conclusao: Option<String>,
    pub sin_retornar_ultimo_andamento: Option<String>,
    pub sin_retornar_unidades_procedimento_aberto: Option<String>,
    pub sin_retornar_procedimentos_relacionados: Option<String>,
    pub sin_retornar_procedimentos_anexados: Option<String>,
}

fn proc_params(protocolo: String, f: &ProcFlags) -> Vec<(&'static str, String)> {
    vec![
        ("ProtocoloProcedimento", protocolo),
        ("SinRetornarAssuntos", sn(&f.sin_retornar_assuntos)),
        ("SinRetornarInteressados", sn(&f.sin_retornar_interessados)),
        ("SinRetornarObservacoes", sn(&f.sin_retornar_observacoes)),
        ("SinRetornarAndamentoGeracao", sn(&f.sin_retornar_andamento_geracao)),
        ("SinRetornarAndamentoConclusao", sn(&f.sin_retornar_andamento_conclusao)),
        ("SinRetornarUltimoAndamento", sn(&f.sin_retornar_ultimo_andamento)),
        ("SinRetornarUnidadesProcedimentoAberto", sn(&f.sin_retornar_unidades_procedimento_aberto)),
        ("SinRetornarProcedimentosRelacionados", sn(&f.sin_retornar_procedimentos_relacionados)),
        ("SinRetornarProcedimentosAnexados", sn(&f.sin_retornar_procedimentos_anexados)),
    ]
}

pub async fn procedimento(
    State(s): State<AppState>,
    Path(protocolo): Path<String>,
    Query(f): Query<ProcFlags>,
) -> Resp {
    let dados = super::call(&s, "consultarProcedimento", true, &proc_params(protocolo, &f)).await?;
    ok(dados)
}

/// Variante por query string: `/v1/procedimento?protocolo=...`. Preferida atrás
/// do nginx, pois a barra do protocolo (`/`) na query não é normalizada como o
/// `%2F` no path seria.
#[derive(Debug, Deserialize)]
pub struct ProcOneQuery {
    pub protocolo: Option<String>,
    #[serde(flatten)]
    pub flags: ProcFlags,
}

pub async fn procedimento_q(State(s): State<AppState>, Query(q): Query<ProcOneQuery>) -> Resp {
    let protocolo = q
        .protocolo
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolo".into()))?;
    let dados = super::call(&s, "consultarProcedimento", true, &proc_params(protocolo, &q.flags)).await?;
    ok(dados)
}

/// Lote: `?protocolos=A,B,C`. Cada item recebe `{protocolo, dados|erro}`.
#[derive(Debug, Deserialize)]
pub struct ProcsQuery {
    pub protocolos: String,
    #[serde(flatten)]
    pub flags: ProcFlags,
}

pub async fn procedimentos(State(s): State<AppState>, Query(q): Query<ProcsQuery>) -> Resp {
    let mut itens = Vec::new();
    for p in q.protocolos.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()) {
        let params = proc_params(p.to_string(), &q.flags);
        match super::call(&s, "consultarProcedimento", true, &params).await {
            Ok(dados) => itens.push(json!({ "protocolo": p, "dados": dados, "erro": Value::Null })),
            Err(e) => itens.push(json!({ "protocolo": p, "dados": Value::Null, "erro": e.to_string() })),
        }
    }
    Ok(Json(json!({ "ok": true, "itens": itens })))
}

// ---- consultarDocumento ---------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct DocFlags {
    pub sin_retornar_andamento_geracao: Option<String>,
    pub sin_retornar_assinaturas: Option<String>,
    pub sin_retornar_publicacao: Option<String>,
    pub sin_retornar_campos: Option<String>,
}

fn doc_params(protocolo: String, f: &DocFlags) -> Vec<(&'static str, String)> {
    vec![
        ("ProtocoloDocumento", protocolo),
        ("SinRetornarAndamentoGeracao", sn(&f.sin_retornar_andamento_geracao)),
        ("SinRetornarAssinaturas", sn(&f.sin_retornar_assinaturas)),
        ("SinRetornarPublicacao", sn(&f.sin_retornar_publicacao)),
        ("SinRetornarCampos", sn(&f.sin_retornar_campos)),
    ]
}

pub async fn documento(
    State(s): State<AppState>,
    Path(protocolo): Path<String>,
    Query(f): Query<DocFlags>,
) -> Resp {
    let dados = super::call(&s, "consultarDocumento", true, &doc_params(protocolo, &f)).await?;
    ok(dados)
}

/// Lote de documentos: `?protocolos=A,B,C`. Cada item: `{protocolo, dados|erro}`.
#[derive(Debug, Deserialize)]
pub struct DocsQuery {
    pub protocolos: String,
    #[serde(flatten)]
    pub flags: DocFlags,
}

pub async fn documentos(State(s): State<AppState>, Query(q): Query<DocsQuery>) -> Resp {
    let mut itens = Vec::new();
    for p in q.protocolos.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()) {
        let params = doc_params(p.to_string(), &q.flags);
        match super::call(&s, "consultarDocumento", true, &params).await {
            Ok(dados) => itens.push(json!({ "protocolo": p, "dados": dados, "erro": Value::Null })),
            Err(e) => itens.push(json!({ "protocolo": p, "dados": Value::Null, "erro": e.to_string() })),
        }
    }
    Ok(Json(json!({ "ok": true, "itens": itens })))
}

/// Variante por query string: `/v1/documento?protocolo=...`.
#[derive(Debug, Deserialize)]
pub struct DocOneQuery {
    pub protocolo: Option<String>,
    #[serde(flatten)]
    pub flags: DocFlags,
}

pub async fn documento_q(State(s): State<AppState>, Query(q): Query<DocOneQuery>) -> Resp {
    let protocolo = q
        .protocolo
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolo".into()))?;
    let dados = super::call(&s, "consultarDocumento", true, &doc_params(protocolo, &q.flags)).await?;
    ok(dados)
}

// ---- consultarPublicacao --------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct PublicacaoQuery {
    pub id_publicacao: Option<String>,
    pub id_documento: Option<String>,
    pub protocolo_documento: Option<String>,
    pub sin_retornar_andamento: Option<String>,
    pub sin_retornar_assinaturas: Option<String>,
}

pub async fn publicacao(State(s): State<AppState>, Query(q): Query<PublicacaoQuery>) -> Resp {
    if q.id_publicacao.is_none() && q.id_documento.is_none() && q.protocolo_documento.is_none() {
        return Err(AppError::BadRequest(
            "informe ao menos um de: id_publicacao, id_documento ou protocolo_documento".into(),
        ));
    }
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let Some(v) = q.id_publicacao { extra.push(("IdPublicacao", v)); }
    if let Some(v) = q.id_documento { extra.push(("IdDocumento", v)); }
    if let Some(v) = q.protocolo_documento { extra.push(("ProtocoloDocumento", v)); }
    extra.push(("SinRetornarAndamento", sn(&q.sin_retornar_andamento)));
    extra.push(("SinRetornarAssinaturas", sn(&q.sin_retornar_assinaturas)));
    let dados = super::call(&s, "consultarPublicacao", true, &extra).await?;
    ok(dados)
}

// ---- consultarBloco -------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BlocoQuery {
    pub sin_retornar_protocolos: Option<String>,
}

fn bloco_params(id_bloco: String, sin_retornar_protocolos: &Option<String>) -> Vec<(&'static str, String)> {
    // default deste sinalizador é "N"
    let srp = match sin_retornar_protocolos.as_deref() {
        Some("S") | Some("s") => "S".to_string(),
        _ => "N".to_string(),
    };
    vec![("IdBloco", id_bloco), ("SinRetornarProtocolos", srp)]
}

pub async fn bloco(
    State(s): State<AppState>,
    Path(id_bloco): Path<String>,
    Query(q): Query<BlocoQuery>,
) -> Resp {
    let dados = super::call(&s, "consultarBloco", true, &bloco_params(id_bloco, &q.sin_retornar_protocolos)).await?;
    ok(dados)
}

/// Variante por query string: `/v1/bloco?id=...`.
#[derive(Debug, Deserialize)]
pub struct BlocoOneQuery {
    pub id: Option<String>,
    pub sin_retornar_protocolos: Option<String>,
}

pub async fn bloco_q(State(s): State<AppState>, Query(q): Query<BlocoOneQuery>) -> Resp {
    let id = q
        .id
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: id".into()))?;
    let dados = super::call(&s, "consultarBloco", true, &bloco_params(id, &q.sin_retornar_protocolos)).await?;
    ok(dados)
}

// ---- consultarProcedimentoIndividual --------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ProcIndividualQuery {
    pub id_orgao_procedimento: Option<String>,
    pub id_tipo_procedimento: Option<String>,
    pub id_orgao_usuario: Option<String>,
    pub sigla_usuario: Option<String>,
}

pub async fn procedimento_individual(
    State(s): State<AppState>,
    Query(q): Query<ProcIndividualQuery>,
) -> Resp {
    let req = |o: Option<String>, nome: &str| {
        o.filter(|v| !v.is_empty())
            .ok_or_else(|| AppError::BadRequest(format!("parâmetro obrigatório ausente: {nome}")))
    };
    let extra = vec![
        ("IdOrgaoProcedimento", req(q.id_orgao_procedimento, "id_orgao_procedimento")?),
        ("IdTipoProcedimento", req(q.id_tipo_procedimento, "id_tipo_procedimento")?),
        ("IdOrgaoUsuario", req(q.id_orgao_usuario, "id_orgao_usuario")?),
        ("SiglaUsuario", req(q.sigla_usuario, "sigla_usuario")?),
    ];
    let dados = super::call(&s, "consultarProcedimentoIndividual", true, &extra).await?;
    ok(dados)
}

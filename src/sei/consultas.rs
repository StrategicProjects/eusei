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

/// Deriva o estado de conclusão do processo a partir de `UnidadesProcedimentoAberto`
/// (unidades onde ainda está aberto). Mantém os nomes originais do SEI em `dados`
/// e expõe o derivado como campo irmão.
/// - `true`  — sem unidades abertas (campo nulo) ⇒ concluído em todas as unidades;
/// - `false` — há unidade(s) aberta(s);
/// - `null`  — indeterminado, quando `solicitadas == false` (o cliente passou
///   `sin_retornar_unidades_procedimento_aberto=N`). É necessário receber a flag
///   pois o SEI devolve o campo como `nil` mesmo quando não foi pedido — sem a
///   flag, "não pedido" seria indistinguível de "concluído".
fn concluido(dados: &Value, solicitadas: bool) -> Value {
    if !solicitadas {
        return Value::Null;
    }
    match dados.get("UnidadesProcedimentoAberto") {
        None => Value::Null,                       // pedido mas ausente -> desconhecido
        Some(Value::Null) => Value::Bool(true),    // nil -> nenhuma unidade aberta
        Some(Value::Array(a)) => Value::Bool(a.is_empty()),
        Some(_) => Value::Bool(false),             // objeto único -> 1 unidade aberta
    }
}

/// `true` se o handler pediu ao SEI as unidades em aberto (default `S`).
fn unidades_solicitadas(f: &ProcFlags) -> Result<bool, AppError> {
    Ok(sn(&f.sin_retornar_unidades_procedimento_aberto)? == "S")
}

/// Envelope de resposta de consulta de processo: `dados` + `concluido` derivado.
fn ok_proc(dados: Value, solicitadas: bool) -> Resp {
    let concl = concluido(&dados, solicitadas);
    Ok(Json(json!({ "ok": true, "dados": dados, "concluido": concl })))
}

/// Resolve um sinalizador "S"/"N" (default "S"); valor fora de S/N → 400.
fn sn(opt: &Option<String>) -> Result<String, AppError> {
    super::flag_sn(opt, true)
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

fn proc_params(protocolo: String, f: &ProcFlags) -> Result<Vec<(&'static str, String)>, AppError> {
    Ok(vec![
        ("ProtocoloProcedimento", protocolo),
        ("SinRetornarAssuntos", sn(&f.sin_retornar_assuntos)?),
        ("SinRetornarInteressados", sn(&f.sin_retornar_interessados)?),
        ("SinRetornarObservacoes", sn(&f.sin_retornar_observacoes)?),
        ("SinRetornarAndamentoGeracao", sn(&f.sin_retornar_andamento_geracao)?),
        ("SinRetornarAndamentoConclusao", sn(&f.sin_retornar_andamento_conclusao)?),
        ("SinRetornarUltimoAndamento", sn(&f.sin_retornar_ultimo_andamento)?),
        ("SinRetornarUnidadesProcedimentoAberto", sn(&f.sin_retornar_unidades_procedimento_aberto)?),
        ("SinRetornarProcedimentosRelacionados", sn(&f.sin_retornar_procedimentos_relacionados)?),
        ("SinRetornarProcedimentosAnexados", sn(&f.sin_retornar_procedimentos_anexados)?),
    ])
}

pub async fn procedimento(
    State(s): State<AppState>,
    Path(protocolo): Path<String>,
    Query(f): Query<ProcFlags>,
) -> Resp {
    let params = proc_params(protocolo, &f)?;
    let solic = unidades_solicitadas(&f)?;
    let dados = super::call(&s, "consultarProcedimento", true, &params).await?;
    ok_proc(dados, solic)
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
    let params = proc_params(protocolo, &q.flags)?;
    let solic = unidades_solicitadas(&q.flags)?;
    let dados = super::call(&s, "consultarProcedimento", true, &params).await?;
    ok_proc(dados, solic)
}

/// Lote: `?protocolos=A,B,C`. Cada item recebe `{protocolo, dados|erro}`.
/// `protocolos` é `Option<String>` (não `String`) para que a ausência caia no
/// envelope JSON da API em vez de uma rejeição crua do extractor do axum.
#[derive(Debug, Deserialize)]
pub struct ProcsQuery {
    pub protocolos: Option<String>,
    #[serde(flatten)]
    pub flags: ProcFlags,
}

pub async fn procedimentos(State(s): State<AppState>, Query(q): Query<ProcsQuery>) -> Resp {
    let protocolos = q
        .protocolos
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolos".into()))?;
    let solic = unidades_solicitadas(&q.flags)?;
    let mut itens = Vec::new();
    let mut sucessos = 0usize;
    let mut transitorio: Option<AppError> = None;
    for p in protocolos.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()) {
        let params = proc_params(p.to_string(), &q.flags)?;
        match super::call(&s, "consultarProcedimento", true, &params).await {
            Ok(dados) => {
                sucessos += 1;
                let concl = concluido(&dados, solic);
                itens.push(json!({ "protocolo": p, "dados": dados, "concluido": concl, "erro": Value::Null }));
            }
            Err(e) => {
                if e.permite_stale() && transitorio.is_none() {
                    transitorio = Some(e.clone());
                }
                itens.push(json!({ "protocolo": p, "dados": Value::Null, "concluido": Value::Null, "erro": e.to_string() }));
            }
        }
    }
    // Nenhum item teve sucesso e houve falha transitória (timeout/indisponível/…):
    // propaga o erro (não-2xx, não cacheável) em vez de um 200 com tudo falho.
    // SOAP Faults (ex.: todos inexistentes) seguem como 200 com erros por item.
    if sucessos == 0 {
        if let Some(e) = transitorio {
            return Err(e);
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

fn doc_params(protocolo: String, f: &DocFlags) -> Result<Vec<(&'static str, String)>, AppError> {
    Ok(vec![
        ("ProtocoloDocumento", protocolo),
        ("SinRetornarAndamentoGeracao", sn(&f.sin_retornar_andamento_geracao)?),
        ("SinRetornarAssinaturas", sn(&f.sin_retornar_assinaturas)?),
        ("SinRetornarPublicacao", sn(&f.sin_retornar_publicacao)?),
        ("SinRetornarCampos", sn(&f.sin_retornar_campos)?),
    ])
}

pub async fn documento(
    State(s): State<AppState>,
    Path(protocolo): Path<String>,
    Query(f): Query<DocFlags>,
) -> Resp {
    let dados = super::call(&s, "consultarDocumento", true, &doc_params(protocolo, &f)?).await?;
    ok(dados)
}

/// Lote de documentos: `?protocolos=A,B,C`. Cada item: `{protocolo, dados|erro}`.
/// `protocolos` é `Option<String>` (ver `ProcsQuery`) para validar no handler.
#[derive(Debug, Deserialize)]
pub struct DocsQuery {
    pub protocolos: Option<String>,
    #[serde(flatten)]
    pub flags: DocFlags,
}

pub async fn documentos(State(s): State<AppState>, Query(q): Query<DocsQuery>) -> Resp {
    let protocolos = q
        .protocolos
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolos".into()))?;
    let mut itens = Vec::new();
    let mut sucessos = 0usize;
    let mut transitorio: Option<AppError> = None;
    for p in protocolos.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()) {
        let params = doc_params(p.to_string(), &q.flags)?;
        match super::call(&s, "consultarDocumento", true, &params).await {
            Ok(dados) => {
                sucessos += 1;
                itens.push(json!({ "protocolo": p, "dados": dados, "erro": Value::Null }));
            }
            Err(e) => {
                if e.permite_stale() && transitorio.is_none() {
                    transitorio = Some(e.clone());
                }
                itens.push(json!({ "protocolo": p, "dados": Value::Null, "erro": e.to_string() }));
            }
        }
    }
    // todos falharam por erro transitório -> propaga (não-2xx, não cacheável)
    if sucessos == 0 {
        if let Some(e) = transitorio {
            return Err(e);
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
    let dados = super::call(&s, "consultarDocumento", true, &doc_params(protocolo, &q.flags)?).await?;
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
    // descarta valores vazios (`?id_documento=`) para não enviá-los ao SEI nem
    // contá-los como identificador presente.
    let naovazio = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let id_publicacao = naovazio(q.id_publicacao);
    let id_documento = naovazio(q.id_documento);
    let protocolo_documento = naovazio(q.protocolo_documento);
    if id_publicacao.is_none() && id_documento.is_none() && protocolo_documento.is_none() {
        return Err(AppError::BadRequest(
            "informe ao menos um de: id_publicacao, id_documento ou protocolo_documento".into(),
        ));
    }
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let Some(v) = id_publicacao { extra.push(("IdPublicacao", v)); }
    if let Some(v) = id_documento { extra.push(("IdDocumento", v)); }
    if let Some(v) = protocolo_documento { extra.push(("ProtocoloDocumento", v)); }
    extra.push(("SinRetornarAndamento", sn(&q.sin_retornar_andamento)?));
    extra.push(("SinRetornarAssinaturas", sn(&q.sin_retornar_assinaturas)?));
    let dados = super::call(&s, "consultarPublicacao", true, &extra).await?;
    ok(dados)
}

// ---- consultarBloco -------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BlocoQuery {
    pub sin_retornar_protocolos: Option<String>,
}

fn bloco_params(id_bloco: String, sin_retornar_protocolos: &Option<String>) -> Result<Vec<(&'static str, String)>, AppError> {
    // default deste sinalizador é "N"
    let srp = super::flag_sn(sin_retornar_protocolos, false)?;
    Ok(vec![("IdBloco", id_bloco), ("SinRetornarProtocolos", srp)])
}

pub async fn bloco(
    State(s): State<AppState>,
    Path(id_bloco): Path<String>,
    Query(q): Query<BlocoQuery>,
) -> Resp {
    let dados = super::call(&s, "consultarBloco", true, &bloco_params(id_bloco, &q.sin_retornar_protocolos)?).await?;
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
    let dados = super::call(&s, "consultarBloco", true, &bloco_params(id, &q.sin_retornar_protocolos)?).await?;
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

#[cfg(test)]
mod tests {
    use super::concluido;
    use serde_json::{json, Value};

    #[test]
    fn concluido_derivado_de_unidades_abertas() {
        // (solicitadas = true) sem unidades abertas (nil) -> concluído
        assert_eq!(concluido(&json!({ "UnidadesProcedimentoAberto": null }), true), json!(true));
        // lista vazia -> concluído
        assert_eq!(concluido(&json!({ "UnidadesProcedimentoAberto": [] }), true), json!(true));
        // unidade(s) aberta(s) -> não concluído
        assert_eq!(
            concluido(&json!({ "UnidadesProcedimentoAberto": [{ "IdUnidade": "1" }] }), true),
            json!(false)
        );
        // objeto único (uma unidade) -> não concluído
        assert_eq!(
            concluido(&json!({ "UnidadesProcedimentoAberto": { "IdUnidade": "1" } }), true),
            json!(false)
        );
        // campo ausente -> desconhecido
        assert_eq!(concluido(&json!({ "ProcedimentoFormatado": "x" }), true), Value::Null);
        // não solicitado (flag N) -> indeterminado, mesmo com o SEI devolvendo nil
        assert_eq!(concluido(&json!({ "UnidadesProcedimentoAberto": null }), false), Value::Null);
    }
}

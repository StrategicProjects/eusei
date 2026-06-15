//! Andamentos (linha do tempo) de um processo e a lista de documentos derivada.
//!
//! A operação `listarAndamentos` do SEI exige ao menos um filtro de
//! `Andamentos`/`Tarefas`/`TarefasModulos` (arrays). Quando nenhum é informado,
//! usamos um intervalo amplo de tarefas (1..=200), como `listar_andamentos_completo`
//! do `rsei`.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, soap::envelope::Param, state::AppState};

type Resp = Result<Json<Value>, AppError>;

fn ok(dados: Value) -> Resp {
    Ok(Json(json!({ "ok": true, "dados": dados })))
}

fn comma_list(s: &Option<String>) -> Vec<String> {
    s.as_deref()
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Converte "dd/mm/aaaa HH:MM:SS" em uma chave ordenável "aaaammddHHMMSS".
fn datahora_key(v: &Value) -> Option<String> {
    let s = v.get("DataHora")?.as_str()?;
    let (data, hora) = s.split_once(' ').unwrap_or((s, "00:00:00"));
    let p: Vec<&str> = data.split('/').collect();
    if p.len() != 3 {
        return None;
    }
    let hms: String = hora.chars().filter(|c| c.is_ascii_digit()).collect();
    Some(format!("{}{:0>2}{:0>2}{:0<6}", p[2], p[1], p[0], hms))
}

fn ordenar_por_datahora(arr: &mut [Value]) {
    arr.sort_by(|a, b| match (datahora_key(a), datahora_key(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

#[derive(Debug, Deserialize)]
pub struct AndamentosQuery {
    pub protocolo: Option<String>,
    /// Filtros (listas separadas por vírgula). Se nenhum for dado, usa tarefas 1..=200.
    pub tarefas: Option<String>,
    pub andamentos: Option<String>,
    pub tarefas_modulos: Option<String>,
    pub sin_retornar_atributos: Option<String>,
    /// Ordena por DataHora (mais antigo primeiro). Padrão: true.
    pub ordenar: Option<bool>,
}

/// Recupera a linha do tempo de um processo, aplicando os filtros e a ordenação.
async fn fetch_andamentos(state: &AppState, q: &AndamentosQuery) -> Result<Value, AppError> {
    let protocolo = q
        .protocolo
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolo".into()))?;

    let mut tarefas = comma_list(&q.tarefas);
    let andamentos = comma_list(&q.andamentos);
    let tarefas_modulos = comma_list(&q.tarefas_modulos);

    // sem nenhum filtro -> intervalo amplo de tarefas (linha do tempo completa)
    if tarefas.is_empty() && andamentos.is_empty() && tarefas_modulos.is_empty() {
        tarefas = (1..=200).map(|n| n.to_string()).collect();
    }

    let sra = match q.sin_retornar_atributos.as_deref() {
        Some("S") | Some("s") => "S".to_string(),
        _ => "N".to_string(),
    };

    let mut extra: Vec<(&str, Param)> = vec![
        ("ProtocoloProcedimento", Param::Scalar(protocolo)),
        ("SinRetornarAtributos", Param::Scalar(sra)),
    ];
    if !andamentos.is_empty() {
        extra.push(("Andamentos", Param::Array(andamentos)));
    }
    if !tarefas.is_empty() {
        extra.push(("Tarefas", Param::Array(tarefas)));
    }
    if !tarefas_modulos.is_empty() {
        extra.push(("TarefasModulos", Param::Array(tarefas_modulos)));
    }

    let mut dados = super::call_with(state, "listarAndamentos", true, extra).await?;

    if q.ordenar.unwrap_or(true) {
        if let Value::Array(arr) = &mut dados {
            ordenar_por_datahora(arr);
        }
    }
    Ok(dados)
}

pub async fn andamentos(State(s): State<AppState>, Query(q): Query<AndamentosQuery>) -> Resp {
    ok(fetch_andamentos(&s, &q).await?)
}

/// Extrai o primeiro número (>= 6 dígitos) que segue a palavra "documento".
fn numero_documento(descricao: &str) -> Option<String> {
    let lower = descricao.to_lowercase();
    let pos = lower.find("documento")? + "documento".len();
    let rest = &descricao[pos..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.len() >= 6 {
        Some(digits)
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
pub struct DocsProcessoQuery {
    pub protocolo: Option<String>,
}

/// Lista os documentos de um processo a partir dos andamentos (heurística).
/// O Web Service do SEI não possui operação nativa para isso; espelha
/// `listar_documentos_processo()` do `rsei`.
pub async fn documentos_processo(
    State(s): State<AppState>,
    Query(q): Query<DocsProcessoQuery>,
) -> Resp {
    let tl = fetch_andamentos(
        &s,
        &AndamentosQuery {
            protocolo: q.protocolo,
            tarefas: None,
            andamentos: None,
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: Some(true),
        },
    )
    .await?;

    let mut vistos: Vec<String> = Vec::new();
    let mut itens: Vec<Value> = Vec::new();
    if let Value::Array(arr) = &tl {
        for and in arr {
            let descricao = and.get("Descricao").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(num) = numero_documento(descricao) {
                if vistos.contains(&num) {
                    continue; // 1a ocorrência (geração)
                }
                vistos.push(num.clone());
                itens.push(json!({
                    "documento": num,
                    "DataHora": and.get("DataHora").cloned().unwrap_or(Value::Null),
                    "Unidade": and.get("Unidade").cloned().unwrap_or(Value::Null),
                    "Usuario": and.get("Usuario").cloned().unwrap_or(Value::Null),
                    "Andamento": descricao,
                }));
            }
        }
    }
    ok(Value::Array(itens))
}

/// `GET /v1/publicacoes-processo?protocolo=...` — publicações de um processo.
/// Heurística: descobre os documentos via timeline e consulta a publicação de
/// cada um, mantendo só os que de fato possuem publicação. Espelha
/// `listar_publicacoes_processo()` do `rsei`.
pub async fn publicacoes_processo(
    State(s): State<AppState>,
    Query(q): Query<DocsProcessoQuery>,
) -> Resp {
    let tl = fetch_andamentos(
        &s,
        &AndamentosQuery {
            protocolo: q.protocolo,
            tarefas: None,
            andamentos: None,
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: Some(true),
        },
    )
    .await?;

    // números de documento únicos, na ordem da timeline
    let mut nums: Vec<String> = Vec::new();
    if let Value::Array(arr) = &tl {
        for and in arr {
            let descricao = and.get("Descricao").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(n) = numero_documento(descricao) {
                if !nums.contains(&n) {
                    nums.push(n);
                }
            }
        }
    }

    // Uma consulta de publicação por documento — paralelizadas com concorrência
    // limitada (não martelar o SEI nem estourar o tempo em processos grandes).
    use futures::stream::StreamExt;
    let s_ref = &s;
    let mut resultados: Vec<(usize, Option<Value>)> =
        futures::stream::iter(nums.into_iter().enumerate())
            .map(|(i, num)| async move {
                let params = [
                    ("ProtocoloDocumento", num.clone()),
                    ("SinRetornarAndamento", "N".to_string()),
                    ("SinRetornarAssinaturas", "N".to_string()),
                ];
                // documentos sem publicação retornam <parametros xsi:nil> -> null
                match super::call(s_ref, "consultarPublicacao", true, &params).await {
                    Ok(d) if !d.is_null() => {
                        (i, Some(json!({ "documento": num, "publicacao": d })))
                    }
                    _ => (i, None),
                }
            })
            .buffer_unordered(10)
            .collect()
            .await;

    resultados.sort_by_key(|(i, _)| *i);
    let itens: Vec<Value> = resultados.into_iter().filter_map(|(_, v)| v).collect();
    ok(Value::Array(itens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrai_numero_documento() {
        assert_eq!(
            numero_documento("Gerado documento público 84230597 (Ofício)"),
            Some("84230597".to_string())
        );
        assert_eq!(numero_documento("Conclusão do processo na unidade"), None);
    }

    #[test]
    fn chave_datahora_ordenavel() {
        let a = json!({"DataHora": "30/06/2022 13:56:27"});
        let b = json!({"DataHora": "22/11/2024 14:47:07"});
        assert!(datahora_key(&a) < datahora_key(&b));
    }
}

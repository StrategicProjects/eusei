//! Andamentos (linha do tempo) de um processo e a lista de documentos derivada.
//!
//! A operação `listarAndamentos` do SEI exige ao menos um filtro de
//! `Andamentos`/`Tarefas`/`TarefasModulos` (arrays). Quando nenhum é informado,
//! usamos um intervalo amplo de tarefas (1..=200), como `listar_andamentos_completo`
//! do `rsei`.
//!
//! Processos enormes não cabem numa única chamada: pedir as 200 tarefas de uma vez
//! faz o `listarAndamentos` passar do limite (~45s) do gateway do próprio SEI, que
//! responde HTTP 504. Por isso, no caminho "só tarefas" (o default da linha do
//! tempo completa), a lista de `Tarefas` é **fatiada em lotes** consultados
//! concorrentemente (à la `publicacoes_processo`); os resultados são mesclados,
//! deduplicados por `IdAndamento` e ordenados por `DataHora`. Cada lote fica abaixo
//! da janela do SEI. O endpoint `/andamentos/stream` (SSE) expõe o avanço lote a
//! lote para o cliente acompanhar.

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::convert::Infallible;

use crate::{error::AppError, soap::envelope::Param, state::AppState};

type Resp = Result<Json<Value>, AppError>;

fn ok_com_resumo(dados: Value, resumo: Value) -> Resp {
    Ok(Json(json!({ "ok": true, "dados": dados, "resumo": resumo })))
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

/// Remove andamentos repetidos por `IdAndamento`, preservando a 1ª ocorrência.
/// Lotes de tarefas distintas devolvem conjuntos disjuntos, mas deduplicamos por
/// garantia (espelha o comportamento do `rsei`). Itens sem `IdAndamento` ficam.
fn dedup_por_id(arr: &mut Vec<Value>) {
    let mut vistos: HashSet<String> = HashSet::new();
    arr.retain(|v| match v.get("IdAndamento").and_then(|x| x.as_str()) {
        Some(id) => vistos.insert(id.to_string()),
        None => true,
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
    /// `Option<String>` (não `Option<bool>`) para que um valor inválido vire um
    /// 400 no envelope JSON, em vez de uma rejeição crua do extractor do axum.
    pub ordenar: Option<String>,
}

/// Interpreta o parâmetro booleano `ordenar` de forma tolerante; valor fora do
/// conjunto aceito vira `AppError::BadRequest` (400). Vazio/ausente -> default.
fn parse_ordenar(s: &Option<String>) -> Result<bool, AppError> {
    match s.as_deref().map(str::trim) {
        None | Some("") => Ok(true),
        Some("true") | Some("1") | Some("S") | Some("s") => Ok(true),
        Some("false") | Some("0") | Some("N") | Some("n") => Ok(false),
        Some(other) => Err(AppError::BadRequest(format!(
            "valor inválido para 'ordenar': '{other}' (use true ou false)"
        ))),
    }
}

/// Plano de execução de uma consulta de andamentos: o protocolo, o sinalizador de
/// atributos, a ordenação e os **lotes** de filtros a consultar (um por chamada
/// `listarAndamentos`). No caminho "só tarefas" há vários lotes; com filtros
/// avançados (`andamentos`/`tarefas_modulos`) há um único lote.
struct Plano {
    protocolo: String,
    sra: String,
    ordenar: bool,
    /// Tamanho do lote efetivamente usado (para o `resumo`).
    tarefas_por_lote: usize,
    /// Filtros por lote (chaves `String` para que os itens cruzem o `tokio::spawn`
    /// do stream sem referências emprestadas — evita a limitação de HRTB do rustc).
    lotes: Vec<Vec<(String, Param)>>,
}

/// Resolve a query num [`Plano`], validando os parâmetros (erros viram 400 no
/// envelope JSON, antes de qualquer chamada ao SEI ou abertura de stream).
/// `lote` é o tamanho de cada fatia de tarefas (config `SEI_ANDAMENTOS_LOTE`).
fn montar_plano(q: &AndamentosQuery, lote: usize) -> Result<Plano, AppError> {
    let protocolo = q
        .protocolo
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("parâmetro obrigatório ausente: protocolo".into()))?;

    let mut tarefas = comma_list(&q.tarefas);
    let andamentos = comma_list(&q.andamentos);
    let tarefas_modulos = comma_list(&q.tarefas_modulos);
    let so_tarefas = andamentos.is_empty() && tarefas_modulos.is_empty();

    // sem nenhum filtro -> intervalo amplo de tarefas (linha do tempo completa)
    if tarefas.is_empty() && so_tarefas {
        tarefas = (1..=200).map(|n| n.to_string()).collect();
    }

    let sra = super::flag_sn(&q.sin_retornar_atributos, false)?;
    let ordenar = parse_ordenar(&q.ordenar)?;

    // Caminho "só tarefas" com lista grande: fatiar em lotes. Os filtros avançados
    // (andamentos/tarefas_modulos) seguem numa única chamada — são consultas
    // estreitas que não estouram a janela do SEI, e combiná-los por lote mudaria
    // a semântica.
    let lotes: Vec<Vec<(String, Param)>> = if so_tarefas && tarefas.len() > lote {
        tarefas
            .chunks(lote)
            .map(|c| vec![("Tarefas".to_string(), Param::Array(c.to_vec()))])
            .collect()
    } else {
        let mut filtro: Vec<(String, Param)> = Vec::new();
        if !andamentos.is_empty() {
            filtro.push(("Andamentos".to_string(), Param::Array(andamentos)));
        }
        if !tarefas.is_empty() {
            filtro.push(("Tarefas".to_string(), Param::Array(tarefas)));
        }
        if !tarefas_modulos.is_empty() {
            filtro.push(("TarefasModulos".to_string(), Param::Array(tarefas_modulos)));
        }
        vec![filtro]
    };

    Ok(Plano {
        protocolo,
        sra,
        ordenar,
        tarefas_por_lote: lote,
        lotes,
    })
}

/// Executa um único lote (`listarAndamentos` com os filtros daquele lote).
async fn chamar_lote(
    state: &AppState,
    protocolo: &str,
    sra: &str,
    filtro: Vec<(String, Param)>,
) -> Result<Value, AppError> {
    let mut extra: Vec<(&str, Param)> = vec![
        ("ProtocoloProcedimento", Param::Scalar(protocolo.to_string())),
        ("SinRetornarAtributos", Param::Scalar(sra.to_string())),
    ];
    // `extra` toma emprestado as chaves de `filtro`, que vive até o fim do await.
    for (k, v) in &filtro {
        extra.push((k.as_str(), v.clone()));
    }
    super::call_with(state, "listarAndamentos", true, extra).await
}

/// Normaliza o retorno de um lote numa lista de andamentos.
fn lote_para_lista(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        outro => vec![outro],
    }
}

fn montar_resumo(
    lotes: usize,
    lotes_com_dados: usize,
    registros: usize,
    parciais: bool,
    tarefas_por_lote: usize,
) -> Value {
    json!({
        "lotes": lotes,
        "lotes_com_dados": lotes_com_dados,
        "registros": registros,
        "parciais": parciais,
        "tarefas_por_lote": tarefas_por_lote,
    })
}

/// Recupera a linha do tempo de um processo: consulta os lotes concorrentemente,
/// mescla, deduplica, ordena e devolve `(dados, resumo)`.
///
/// Política de falha: um SOAP Fault (ex.: protocolo inexistente, acesso negado)
/// propaga — vale para a requisição inteira. Falhas sistêmicas pontuais (timeout,
/// indisponível, HTTP) de um lote **não** derrubam tudo: o resultado vem parcial
/// (`resumo.parciais = true`), preservando os lotes que responderam.
async fn fetch_andamentos(state: &AppState, q: &AndamentosQuery) -> Result<(Value, Value), AppError> {
    let plano = montar_plano(q, state.cfg.sei.andamentos_lote)?;
    let n_lotes = plano.lotes.len();
    let lote = plano.tarefas_por_lote;
    let protocolo = plano.protocolo.as_str();
    let sra = plano.sra.as_str();

    let resultados: Vec<Result<Value, AppError>> = futures::stream::iter(plano.lotes.into_iter())
        .map(|filtro| chamar_lote(state, protocolo, sra, filtro))
        .buffer_unordered(state.cfg.sei.andamentos_conc)
        .collect()
        .await;

    let mut registros: Vec<Value> = Vec::new();
    let mut lotes_com_dados = 0usize;
    let mut parciais = false;
    let mut falhas = 0usize;
    let mut ultimo_erro: Option<AppError> = None;
    for r in resultados {
        match r {
            Ok(v) => {
                let arr = lote_para_lista(v);
                if !arr.is_empty() {
                    lotes_com_dados += 1;
                }
                registros.extend(arr);
            }
            // entrada inválida (protocolo inexistente etc.) -> propaga p/ a requisição
            Err(e @ AppError::SoapFault { .. }) => return Err(e),
            // falha sistêmica pontual -> resultado parcial, não derruba tudo
            Err(e) => {
                parciais = true;
                falhas += 1;
                ultimo_erro = Some(e);
            }
        }
    }

    // Se TODOS os lotes falharam sistemicamente, não há resultado: propaga o erro
    // em vez de devolver uma timeline vazia (que seria indistinguível de "sem
    // andamentos"). Com ao menos um lote bom, segue o resultado parcial.
    if falhas == n_lotes {
        return Err(ultimo_erro.unwrap_or(AppError::SeiUnavailable));
    }

    dedup_por_id(&mut registros);
    if plano.ordenar {
        ordenar_por_datahora(&mut registros);
    }
    let resumo = montar_resumo(n_lotes, lotes_com_dados, registros.len(), parciais, lote);
    Ok((Value::Array(registros), resumo))
}

pub async fn andamentos(State(s): State<AppState>, Query(q): Query<AndamentosQuery>) -> Resp {
    let (dados, resumo) = fetch_andamentos(&s, &q).await?;
    ok_com_resumo(dados, resumo)
}

// --- SSE: progresso lote a lote -------------------------------------------------

fn evento(nome: &str, dados: Value) -> Event {
    Event::default()
        .event(nome)
        .json_data(&dados)
        .unwrap_or_else(|_| Event::default().event(nome).data("{}"))
}

/// Roda os lotes e emite eventos no canal: um `progresso` a cada lote concluído e,
/// ao final, `concluido` com o payload completo (ou `erro` num SOAP Fault).
async fn stream_lotes(
    state: AppState,
    plano: Plano,
    mut tx: futures::channel::mpsc::Sender<Result<Event, Infallible>>,
) {
    let n_lotes = plano.lotes.len();
    let lote = plano.tarefas_por_lote;
    let conc = state.cfg.sei.andamentos_conc;
    let protocolo = plano.protocolo;
    let sra = plano.sra;

    // Cada futuro recebe suas próprias cópias (estado é Arc/Client — clone barato),
    // para que a task spawnada seja `'static` (sem empréstimos cruzando o spawn).
    let mut stream = futures::stream::iter(plano.lotes.into_iter())
        .map(|filtro| {
            let state = state.clone();
            let protocolo = protocolo.clone();
            let sra = sra.clone();
            async move { chamar_lote(&state, &protocolo, &sra, filtro).await }
        })
        .buffer_unordered(conc);

    let mut registros: Vec<Value> = Vec::new();
    let mut feitos = 0usize;
    let mut lotes_com_dados = 0usize;
    let mut parciais = false;
    let mut falhas = 0usize;
    let mut ultimo_erro: Option<String> = None;

    while let Some(r) = stream.next().await {
        feitos += 1;
        match r {
            Ok(v) => {
                let arr = lote_para_lista(v);
                if !arr.is_empty() {
                    lotes_com_dados += 1;
                }
                registros.extend(arr);
            }
            Err(AppError::SoapFault { string, .. }) => {
                let _ = tx
                    .send(Ok(evento("erro", json!({ "ok": false, "erro": string }))))
                    .await;
                return;
            }
            Err(e) => {
                parciais = true;
                falhas += 1;
                ultimo_erro = Some(e.to_string());
            }
        }
        let _ = tx
            .send(Ok(evento(
                "progresso",
                json!({
                    "lotes_concluidos": feitos,
                    "total_lotes": n_lotes,
                    "registros_ate_agora": registros.len(),
                    "parciais": parciais,
                }),
            )))
            .await;
    }

    // Todos os lotes falharam sistemicamente -> erro (não um `concluido` vazio).
    if falhas == n_lotes {
        let msg = ultimo_erro.unwrap_or_else(|| "SEI indisponível".to_string());
        let _ = tx
            .send(Ok(evento("erro", json!({ "ok": false, "erro": msg }))))
            .await;
        return;
    }

    dedup_por_id(&mut registros);
    if plano.ordenar {
        ordenar_por_datahora(&mut registros);
    }
    let resumo = montar_resumo(n_lotes, lotes_com_dados, registros.len(), parciais, lote);
    let _ = tx
        .send(Ok(evento(
            "concluido",
            json!({ "ok": true, "dados": registros, "resumo": resumo }),
        )))
        .await;
}

/// `GET /v1/andamentos/stream?protocolo=...` — mesma consulta de `/andamentos`,
/// porém via SSE (`text/event-stream`): emite `progresso` a cada lote e
/// `concluido` com o payload final, para o cliente exibir o avanço ao vivo.
pub async fn andamentos_stream(
    State(s): State<AppState>,
    Query(q): Query<AndamentosQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, AppError> {
    // validação de parâmetros antes de abrir o stream (erro -> 400 JSON normal)
    let plano = montar_plano(&q, s.cfg.sei.andamentos_lote)?;
    let (tx, rx) = futures::channel::mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(stream_lotes(s, plano, tx));
    Ok(Sse::new(rx).keep_alive(KeepAlive::default()))
}

// --- Documentos/publicações derivados da timeline ------------------------------

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

/// Timeline completa (default) a partir de um protocolo — base de
/// `documentos_processo`/`publicacoes_processo`. Devolve `(dados, resumo)`.
async fn timeline_completa(state: &AppState, protocolo: Option<String>) -> Result<(Value, Value), AppError> {
    fetch_andamentos(
        state,
        &AndamentosQuery {
            protocolo,
            tarefas: None,
            andamentos: None,
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: None, // default: ordena por DataHora
        },
    )
    .await
}

/// Lista os documentos de um processo a partir dos andamentos (heurística).
/// O Web Service do SEI não possui operação nativa para isso; espelha
/// `listar_documentos_processo()` do `rsei`. O `resumo` vem do fetch da timeline
/// (sinaliza se ela veio parcial, caso a lista de documentos esteja incompleta).
pub async fn documentos_processo(
    State(s): State<AppState>,
    Query(q): Query<DocsProcessoQuery>,
) -> Resp {
    let (tl, resumo) = timeline_completa(&s, q.protocolo).await?;

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
    ok_com_resumo(Value::Array(itens), resumo)
}

/// `GET /v1/publicacoes-processo?protocolo=...` — publicações de um processo.
/// Heurística: descobre os documentos via timeline e consulta a publicação de
/// cada um, mantendo só os que de fato possuem publicação. Espelha
/// `listar_publicacoes_processo()` do `rsei`.
pub async fn publicacoes_processo(
    State(s): State<AppState>,
    Query(q): Query<DocsProcessoQuery>,
) -> Resp {
    let (tl, resumo) = timeline_completa(&s, q.protocolo).await?;

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
    let s_ref = &s;
    let resultados: Vec<Result<(usize, Option<Value>), AppError>> =
        futures::stream::iter(nums.into_iter().enumerate())
            .map(|(i, num)| async move {
                let params = [
                    ("ProtocoloDocumento", num.clone()),
                    ("SinRetornarAndamento", "N".to_string()),
                    ("SinRetornarAssinaturas", "N".to_string()),
                ];
                match super::call(s_ref, "consultarPublicacao", true, &params).await {
                    // documento com publicação
                    Ok(d) if !d.is_null() => {
                        Ok((i, Some(json!({ "documento": num, "publicacao": d }))))
                    }
                    // documento sem publicação: <parametros xsi:nil> -> null
                    Ok(_) => Ok((i, None)),
                    // SOAP Fault: número heurístico inválido ou sem publicação -> ignora
                    Err(AppError::SoapFault { .. }) => Ok((i, None)),
                    // erro sistêmico (timeout, indisponível, http, parse) -> propaga,
                    // para não mascarar uma falha geral como "sem publicações".
                    Err(e) => Err(e),
                }
            })
            .buffer_unordered(10)
            .collect()
            .await;

    // Aborta no primeiro erro sistêmico (devolve o AppError pelo envelope JSON).
    let mut itens_idx: Vec<(usize, Option<Value>)> = Vec::with_capacity(resultados.len());
    for r in resultados {
        itens_idx.push(r?);
    }
    itens_idx.sort_by_key(|(i, _)| *i);
    let itens: Vec<Value> = itens_idx.into_iter().filter_map(|(_, v)| v).collect();
    ok_com_resumo(Value::Array(itens), resumo)
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
    fn ordenar_aceita_validos_e_rejeita_invalido() {
        assert!(parse_ordenar(&None).unwrap());
        assert!(parse_ordenar(&Some("".into())).unwrap());
        assert!(parse_ordenar(&Some("true".into())).unwrap());
        assert!(!parse_ordenar(&Some("false".into())).unwrap());
        assert!(!parse_ordenar(&Some("N".into())).unwrap());
        let err = parse_ordenar(&Some("abc".into())).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn chave_datahora_ordenavel() {
        let a = json!({"DataHora": "30/06/2022 13:56:27"});
        let b = json!({"DataHora": "22/11/2024 14:47:07"});
        assert!(datahora_key(&a) < datahora_key(&b));
    }

    #[test]
    fn dedup_mantem_primeira_ocorrencia_por_id() {
        let mut arr = vec![
            json!({"IdAndamento": "1", "x": "a"}),
            json!({"IdAndamento": "2", "x": "b"}),
            json!({"IdAndamento": "1", "x": "c"}),
            json!({"sem_id": true}),
        ];
        dedup_por_id(&mut arr);
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["x"], "a"); // 1ª ocorrência de id=1 preservada
        assert_eq!(arr[2]["sem_id"], true); // item sem id mantido
    }

    #[test]
    fn plano_fatiamento_so_tarefas() {
        // default (sem filtros) -> 1..200 fatiado em lotes de tamanho `lote`
        let q = AndamentosQuery {
            protocolo: Some("0001".into()),
            tarefas: None,
            andamentos: None,
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: None,
        };
        let plano = montar_plano(&q, 10).unwrap();
        assert_eq!(plano.lotes.len(), 200_usize.div_ceil(10));
    }

    #[test]
    fn plano_filtro_avancado_lote_unico() {
        // com andamentos/tarefas_modulos -> chamada única (sem fatiamento)
        let q = AndamentosQuery {
            protocolo: Some("0001".into()),
            tarefas: None,
            andamentos: Some("10,20,30".into()),
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: None,
        };
        let plano = montar_plano(&q, 10).unwrap();
        assert_eq!(plano.lotes.len(), 1);
    }

    #[test]
    fn plano_protocolo_ausente_400() {
        let q = AndamentosQuery {
            protocolo: None,
            tarefas: None,
            andamentos: None,
            tarefas_modulos: None,
            sin_retornar_atributos: None,
            ordenar: None,
        };
        assert!(matches!(montar_plano(&q, 10), Err(AppError::BadRequest(_))));
    }
}

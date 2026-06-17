//! Configuração do serviço, resolvida a partir de variáveis de ambiente
//! (carregadas de um `.env` em desenvolvimento ou de um EnvironmentFile no
//! systemd em produção). Espelha o `sei_config()` do pacote R `rsei`.

use std::env;
use std::fmt;

/// Mascara um segredo para logs: mostra só o tamanho, nunca o valor.
fn redigido(s: &str) -> String {
    if s.is_empty() {
        "\"\"".to_string()
    } else {
        format!("\"***\" ({} chars)", s.len())
    }
}

#[derive(Clone)]
pub struct AppConfig {
    /// Endereço de bind do servidor HTTP (ex.: "127.0.0.1:8088").
    pub bind: String,
    /// Tokens Bearer válidos para autenticar clientes do eusei.
    pub tokens: Vec<String>,
    /// Configuração de acesso ao SEI.
    pub sei: SeiConfig,
    /// Configuração de acesso ao SIP (opcional).
    pub sip: SipConfig,
    /// Filtro de log (RUST_LOG), guardado para referência.
    pub log_filter: String,
}

#[derive(Clone)]
pub struct SeiConfig {
    pub url: String,
    pub sigla_sistema: String,
    pub identificacao_servico: String,
    pub id_unidade: String,
    pub timeout_secs: u64,
    /// Quantas tarefas por chamada `listarAndamentos` ao fatiar a linha do tempo
    /// completa (processos grandes). Menor = cada chamada mais leve/rápida.
    pub andamentos_lote: usize,
    /// Quantos lotes de andamentos consultar em paralelo. Menor = menos pressão
    /// concorrente sobre o SEI (lotes batem menos no timeout do gateway dele).
    pub andamentos_conc: usize,
}

/// Configuração do SIP (Sistema de Permissões) — endpoint e autenticação
/// distintos do SEI. Opcional: requer chave de acesso própria do SIP.
#[derive(Clone)]
pub struct SipConfig {
    pub url: String,
    pub chave_acesso: String,
    pub id_sistema: String,
}

// `Debug` manual: nunca expõe tokens nem chaves de acesso (defesa contra
// vazamento acidental em logs/dumps `{:?}`).
impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("bind", &self.bind)
            .field("tokens", &format_args!("[{} token(s) ***]", self.tokens.len()))
            .field("sei", &self.sei)
            .field("sip", &self.sip)
            .field("log_filter", &self.log_filter)
            .finish()
    }
}

impl fmt::Debug for SeiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeiConfig")
            .field("url", &self.url)
            .field("sigla_sistema", &self.sigla_sistema)
            .field("identificacao_servico", &format_args!("{}", redigido(&self.identificacao_servico)))
            .field("id_unidade", &self.id_unidade)
            .field("timeout_secs", &self.timeout_secs)
            .field("andamentos_lote", &self.andamentos_lote)
            .field("andamentos_conc", &self.andamentos_conc)
            .finish()
    }
}

impl fmt::Debug for SipConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipConfig")
            .field("url", &self.url)
            .field("chave_acesso", &format_args!("{}", redigido(&self.chave_acesso)))
            .field("id_sistema", &self.id_sistema)
            .finish()
    }
}

impl SipConfig {
    pub fn configurado(&self) -> bool {
        !self.chave_acesso.is_empty() && !self.id_sistema.is_empty()
    }
}

fn get(key: &str, fallback: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

impl AppConfig {
    /// Carrega a configuração do ambiente. Falha cedo se faltar a chave de
    /// acesso do SEI ou se nenhum token for definido.
    pub fn from_env() -> Result<Self, String> {
        let tokens: Vec<String> = get("EUSEI_TOKENS", "")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if tokens.is_empty() {
            return Err("EUSEI_TOKENS vazio: defina ao menos um token Bearer.".into());
        }
        // Recusa subir com o token de exemplo/placeholder (defesa em profundidade).
        if tokens.iter().any(|t| t.to_uppercase().starts_with("TROQUE")) {
            return Err(
                "EUSEI_TOKENS contém um token placeholder ('TROQUE...'): defina um token real."
                    .into(),
            );
        }

        let identificacao_servico = get("SEI_IDENTIFICACAO_SERVICO", "");
        if identificacao_servico.is_empty() {
            return Err("SEI_IDENTIFICACAO_SERVICO ausente (chave de acesso do SEI).".into());
        }

        let timeout_secs = get("SEI_TIMEOUT_SECS", "60")
            .parse::<u64>()
            .map_err(|_| "SEI_TIMEOUT_SECS inválido (esperado inteiro).".to_string())?;

        let andamentos_lote = get("SEI_ANDAMENTOS_LOTE", "8")
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("SEI_ANDAMENTOS_LOTE inválido (esperado inteiro > 0).".to_string())?;
        let andamentos_conc = get("SEI_ANDAMENTOS_CONC", "3")
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("SEI_ANDAMENTOS_CONC inválido (esperado inteiro > 0).".to_string())?;

        Ok(AppConfig {
            bind: get("EUSEI_BIND", "127.0.0.1:18088"),
            tokens,
            sei: SeiConfig {
                url: get("SEI_URL", "https://sei.pe.gov.br/sei/ws/SeiWS.php"),
                sigla_sistema: get("SEI_SIGLA_SISTEMA", "HORTENSIAS"),
                identificacao_servico,
                id_unidade: get("SEI_ID_UNIDADE", ""),
                timeout_secs,
                andamentos_lote,
                andamentos_conc,
            },
            sip: SipConfig {
                url: get(
                    "SEI_SIP_URL",
                    "https://sei.pe.gov.br/sip/controlador_ws.php?servico=sip",
                ),
                chave_acesso: get("SEI_SIP_CHAVE_ACESSO", ""),
                id_sistema: get("SEI_SIP_ID_SISTEMA", ""),
            },
            log_filter: get("RUST_LOG", "eusei=info,tower_http=info"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_nao_vaza_segredos() {
        let cfg = AppConfig {
            bind: "127.0.0.1:18088".into(),
            tokens: vec!["super-secret-token".into()],
            sei: SeiConfig {
                url: "https://exemplo/ws".into(),
                sigla_sistema: "SIGLA".into(),
                identificacao_servico: "chave-sei-secreta".into(),
                id_unidade: "10".into(),
                timeout_secs: 60,
                andamentos_lote: 10,
                andamentos_conc: 4,
            },
            sip: SipConfig {
                url: "https://exemplo/sip".into(),
                chave_acesso: "chave-sip-secreta".into(),
                id_sistema: "SIP".into(),
            },
            log_filter: "eusei=info".into(),
        };
        let dump = format!("{cfg:?}");
        assert!(!dump.contains("super-secret-token"));
        assert!(!dump.contains("chave-sei-secreta"));
        assert!(!dump.contains("chave-sip-secreta"));
        // metadados não-sensíveis seguem visíveis
        assert!(dump.contains("127.0.0.1:18088"));
        assert!(dump.contains("SIGLA"));
    }
}

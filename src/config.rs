//! Configuração do serviço, resolvida a partir de variáveis de ambiente
//! (carregadas de um `.env` em desenvolvimento ou de um EnvironmentFile no
//! systemd em produção). Espelha o `sei_config()` do pacote R `rsei`.

use std::env;

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub struct SeiConfig {
    pub url: String,
    pub sigla_sistema: String,
    pub identificacao_servico: String,
    pub id_unidade: String,
    pub timeout_secs: u64,
}

/// Configuração do SIP (Sistema de Permissões) — endpoint e autenticação
/// distintos do SEI. Opcional: requer chave de acesso própria do SIP.
#[derive(Clone, Debug)]
pub struct SipConfig {
    pub url: String,
    pub chave_acesso: String,
    pub id_sistema: String,
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

        Ok(AppConfig {
            bind: get("EUSEI_BIND", "127.0.0.1:18088"),
            tokens,
            sei: SeiConfig {
                url: get("SEI_URL", "https://sei.pe.gov.br/sei/ws/SeiWS.php"),
                sigla_sistema: get("SEI_SIGLA_SISTEMA", "HORTENSIAS"),
                identificacao_servico,
                id_unidade: get("SEI_ID_UNIDADE", ""),
                timeout_secs,
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

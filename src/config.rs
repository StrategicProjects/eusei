//! Configuração do serviço, resolvida a partir de variáveis de ambiente
//! (carregadas de um `.env` em desenvolvimento ou de um EnvironmentFile no
//! systemd em produção). Espelha o `sei_config()` do pacote R `rsei`.

use std::env;
use std::fmt;
use std::time::Duration;

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
    /// Configuração do cache em memória das respostas do SEI.
    pub cache: CacheConfig,
    /// Filtro de log (RUST_LOG), guardado para referência.
    pub log_filter: String,
}

/// Cache em memória das respostas do SEI (read-only): TTL por classe de operação
/// + teto de memória. Reduz latência e protege o SEI (single-flight). Ver `cache.rs`.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Liga/desliga o cache por completo (`EUSEI_CACHE=off`).
    pub enabled: bool,
    /// Teto de memória do cache, em bytes (peso = tamanho do JSON).
    pub max_bytes: u64,
    /// TTL de listas quase-estáticas (países, séries, tipos…).
    pub ttl_estatico: Duration,
    /// TTL de dados semi-dinâmicos (usuários, contatos, permissões).
    pub ttl_semi: Duration,
    /// TTL de dados de processo (consultas, andamentos) — curto (frescor).
    pub ttl_dinamico: Duration,
    /// Janela de "serve-stale": por quanto tempo, após o TTL de frescor, o último
    /// valor bom pode ser servido quando o SEI falha (0 desliga o serve-stale).
    pub stale_ttl: Duration,
    /// TTL do "negative caching": por quanto tempo um SOAP Fault (ex.: protocolo
    /// inexistente) é lembrado, para não repetir a chamada ao SEI (0 desliga).
    pub neg_ttl: Duration,
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
            .field("cache", &self.cache)
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
        // Recusa subir com a chave de exemplo/placeholder (defesa em profundidade,
        // espelha a checagem do token). Evita iniciar e só falhar em runtime.
        let chave_up = identificacao_servico.to_uppercase();
        if ["COLOQUE", "TROQUE", "CHANGE", "ALTERE"]
            .iter()
            .any(|p| chave_up.starts_with(p))
            || chave_up.contains("CHAVE_DE_ACESSO")
        {
            return Err(
                "SEI_IDENTIFICACAO_SERVICO contém um placeholder (ex.: \
                 'COLOQUE_A_CHAVE_DE_ACESSO_AQUI'): defina a chave de acesso real do SEI."
                    .into(),
            );
        }

        let timeout_secs = get("SEI_TIMEOUT_SECS", "60")
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("SEI_TIMEOUT_SECS inválido (esperado inteiro > 0).".to_string())?;

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

        // Cache em memória. Desligado com EUSEI_CACHE em {off,0,false,no}.
        let cache_enabled = !matches!(
            get("EUSEI_CACHE", "on").to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        );
        // Valor inválido (não-numérico) falha cedo, em vez de cair silenciosamente
        // no default — escondia erro operacional. TTLs aceitam 0 (desligam stale/neg).
        let dur_secs = |key: &str, default: u64| -> Result<Duration, String> {
            get(key, &default.to_string())
                .parse::<u64>()
                .map(Duration::from_secs)
                .map_err(|_| format!("{key} inválido (esperado inteiro de segundos)."))
        };
        let cache_max_mb = get("EUSEI_CACHE_MAX_MB", "256")
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("EUSEI_CACHE_MAX_MB inválido (esperado inteiro de MB > 0).".to_string())?;
        let cache = CacheConfig {
            enabled: cache_enabled,
            max_bytes: cache_max_mb.saturating_mul(1024 * 1024),
            ttl_estatico: dur_secs("EUSEI_CACHE_TTL_ESTATICO_SECS", 21_600)?, // 6h
            ttl_semi: dur_secs("EUSEI_CACHE_TTL_SEMI_SECS", 600)?,            // 10min
            ttl_dinamico: dur_secs("EUSEI_CACHE_TTL_DINAMICO_SECS", 30)?,     // 30s
            stale_ttl: dur_secs("EUSEI_CACHE_STALE_SECS", 86_400)?,           // 24h
            neg_ttl: dur_secs("EUSEI_CACHE_NEG_SECS", 30)?,                   // 30s
        };

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
            cache,
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
            cache: CacheConfig {
                enabled: true,
                max_bytes: 64 * 1024 * 1024,
                ttl_estatico: Duration::from_secs(21_600),
                ttl_semi: Duration::from_secs(600),
                ttl_dinamico: Duration::from_secs(30),
                stale_ttl: Duration::from_secs(86_400),
                neg_ttl: Duration::from_secs(30),
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

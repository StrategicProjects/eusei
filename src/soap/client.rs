//! Execução da chamada SOAP via reqwest, com tratamento de SOAP Fault, timeout
//! e erros HTTP. Espelha `sei_call()` do `rsei`.

use std::time::Duration;

use crate::error::AppError;

use super::{
    envelope::{build_envelope, Param},
    parse::extract_fault,
};

/// Monta o envelope para a operação, envia ao endpoint SOAP e devolve o corpo
/// XML da resposta (já validado contra SOAP Fault e status HTTP). Genérico:
/// serve tanto ao SEI (ns "sei"/"Sei") quanto ao SIP (ns "sip"/"sipns").
#[allow(clippy::too_many_arguments)]
pub async fn soap_call(
    http: &reqwest::Client,
    url: &str,
    timeout_secs: u64,
    operation: &str,
    params: &[(&str, Param)],
    ns_prefix: &str,
    ns_uri: &str,
    soap_action: &str,
) -> Result<String, AppError> {
    let envelope = build_envelope(operation, params, ns_prefix, ns_uri);

    // Uma retentativa para blips transitórios de conexão (o acesso passa por
    // firewall/NAT e pode falhar pontualmente).
    let mut attempt = 0u8;
    let resp = loop {
        attempt += 1;
        let result = http
            .post(url)
            .header("Content-Type", "text/xml; charset=UTF-8")
            .header("SOAPAction", soap_action)
            .timeout(Duration::from_secs(timeout_secs))
            .body(envelope.clone())
            .send()
            .await;

        match result {
            Ok(r) => break r,
            Err(e) if e.is_timeout() => {
                tracing::warn!(operation, "timeout ao chamar o SEI");
                return Err(AppError::Timeout);
            }
            Err(e) => {
                // Falha de conexão (DNS/rota/firewall/servidor fora do ar).
                tracing::warn!(operation, attempt, error = %e, "falha de conexão com o SEI");
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    continue;
                }
                return Err(AppError::SeiUnavailable);
            }
        }
    };

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;

    // SOAP Fault pode vir mesmo em HTTP 500.
    if let Some((code, string)) = extract_fault(&body) {
        return Err(AppError::SoapFault { code, string });
    }

    if !status.is_success() {
        return Err(AppError::Upstream(format!(
            "HTTP {} do SEI na operação '{operation}'",
            status.as_u16()
        )));
    }

    Ok(body)
}

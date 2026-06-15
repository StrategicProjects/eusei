//! Montagem do envelope SOAP no formato esperado pelo SEI.
//! Para as consultas read-only todos os parâmetros são escalares
//! (`<Nome xsi:type="xsd:string">valor</Nome>`); estruturas/arrays de envio
//! (necessárias só em operações de escrita) ficam para uma fase futura.

/// Valor de um parâmetro do envelope: escalar ou array (`<item>` por elemento).
#[derive(Debug, Clone)]
pub enum Param {
    Scalar(String),
    Array(Vec<String>),
}

impl From<String> for Param {
    fn from(s: String) -> Self {
        Param::Scalar(s)
    }
}

/// Escapa os cinco caracteres reservados de XML. Espelha `sei_xml_escape()`.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_param(name: &str, value: &Param) -> String {
    match value {
        Param::Scalar(v) => {
            format!("<{name} xsi:type=\"xsd:string\">{}</{name}>", xml_escape(v))
        }
        Param::Array(items) => {
            let inner: String = items
                .iter()
                .map(|v| format!("<item xsi:type=\"xsd:string\">{}</item>", xml_escape(v)))
                .collect();
            format!("<{name}>{inner}</{name}>")
        }
    }
}

/// Monta o envelope SOAP 1.1 para `operation` com os `params` dados.
pub fn build_envelope(operation: &str, params: &[(&str, Param)], ns_prefix: &str, ns_uri: &str) -> String {
    let body: String = params
        .iter()
        .map(|(name, value)| render_param(name, value))
        .collect();

    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soapenv:Envelope xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:{ns_prefix}="{ns_uri}">
  <soapenv:Header/>
  <soapenv:Body>
    <{ns_prefix}:{operation} soapenv:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">{body}</{ns_prefix}:{operation}>
  </soapenv:Body>
</soapenv:Envelope>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapa_reservados() {
        assert_eq!(xml_escape("a & b < c"), "a &amp; b &lt; c");
    }

    #[test]
    fn envelope_contem_operacao_e_params() {
        let env = build_envelope(
            "listarUnidades",
            &[("SiglaSistema", Param::Scalar("HORTENSIAS".into()))],
            "sei",
            "Sei",
        );
        assert!(env.contains("<sei:listarUnidades"));
        assert!(env.contains("<SiglaSistema xsi:type=\"xsd:string\">HORTENSIAS</SiglaSistema>"));
        assert!(env.contains("xmlns:sei=\"Sei\""));
    }

    #[test]
    fn envelope_renderiza_array() {
        let env = build_envelope(
            "listarAndamentos",
            &[("Tarefas", Param::Array(vec!["1".into(), "2".into()]))],
            "sei",
            "Sei",
        );
        assert!(env.contains("<Tarefas><item xsi:type=\"xsd:string\">1</item><item xsi:type=\"xsd:string\">2</item></Tarefas>"));
    }
}

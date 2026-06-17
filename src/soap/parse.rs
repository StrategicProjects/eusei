//! Parsing genérico da resposta SOAP do SEI para JSON.
//!
//! Uma única função (`node_to_json`) cobre TODAS as operações read-only:
//!   - `xsi:nil="true"`                       -> null
//!   - nó com `arrayType`/`xsi:type=ArrayOf*` -> array (mapeia filhos `<item>`)
//!   - nó cujos filhos são todos `<item>`     -> array
//!   - nó com filhos nomeados                 -> objeto { nome: valor }
//!   - folha                                  -> string (texto, trim)
//!
//! Ignora namespaces comparando pelo nome local (como o `local-name()` do
//! `xml2`/`rsei`).

use roxmltree::{Document, Node};
use serde_json::{Map, Value};

use crate::error::AppError;

/// Extrai `faultcode`/`faultstring` de um corpo SOAP, ou `None` se não houver.
pub fn extract_fault(body: &str) -> Option<(String, String)> {
    let doc = Document::parse(body).ok()?;
    let fault = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Fault")?;
    let text_of = |tag: &str| {
        fault
            .descendants()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .unwrap_or("")
            .to_string()
    };
    Some((text_of("faultcode"), text_of("faultstring")))
}

/// Localiza o nó `<parametros>` da resposta e o converte em JSON.
pub fn parametros_to_json(body: &str) -> Result<Value, AppError> {
    let doc = Document::parse(body).map_err(|e| AppError::Parse(e.to_string()))?;
    let node = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "parametros")
        .ok_or_else(|| AppError::Parse("nó <parametros> não encontrado na resposta".into()))?;
    Ok(node_to_json(node))
}

/// Localiza o nó de retorno do SIP (`<return*>`, ex.: `returnPermissoes`) e o
/// converte em JSON. O SIP não usa `<parametros>` como o SEI.
pub fn return_to_json(body: &str) -> Result<Value, AppError> {
    let doc = Document::parse(body).map_err(|e| AppError::Parse(e.to_string()))?;
    let node = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name().starts_with("return"))
        .ok_or_else(|| AppError::Parse("nó de retorno do SIP não encontrado".into()))?;
    Ok(node_to_json(node))
}

fn is_nil(n: Node) -> bool {
    // xsi:nil é um booleano do XML Schema: aceita "true"/"false" e "1"/"0".
    n.attributes()
        .any(|a| a.name() == "nil" && matches!(a.value(), "true" | "1"))
}

fn is_array(n: Node, element_children: &[Node]) -> bool {
    let typed_array = n.attributes().any(|a| {
        a.name() == "arrayType" || (a.name() == "type" && a.value().contains("ArrayOf"))
    });
    let all_items = !element_children.is_empty()
        && element_children.iter().all(|c| c.tag_name().name() == "item");
    typed_array || all_items
}

fn node_to_json(node: Node) -> Value {
    if is_nil(node) {
        return Value::Null;
    }

    let children: Vec<Node> = node.children().filter(|n| n.is_element()).collect();

    if is_array(node, &children) {
        return Value::Array(
            children
                .iter()
                .filter(|c| c.tag_name().name() == "item")
                .map(|c| node_to_json(*c))
                .collect(),
        );
    }

    if children.is_empty() {
        let text = node.text().unwrap_or("").trim().to_string();
        return Value::String(text);
    }

    let mut map = Map::new();
    for c in children {
        let key = c.tag_name().name().to_string();
        let val = node_to_json(c);
        match map.get_mut(&key) {
            // chave repetida -> vira array
            Some(Value::Array(arr)) => arr.push(val),
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, val]);
            }
            None => {
                map.insert(key, val);
            }
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_detectado() {
        let body = r#"<env:Envelope xmlns:env="x"><env:Body><env:Fault>
            <faultcode>SOAP-ENV:Server</faultcode>
            <faultstring>Processo nao encontrado</faultstring>
            </env:Fault></env:Body></env:Envelope>"#;
        let f = extract_fault(body).unwrap();
        assert_eq!(f.1, "Processo nao encontrado");
    }

    #[test]
    fn procedimento_vira_objeto() {
        let xml = include_str!("../../tests/fixtures/consultarProcedimento.xml");
        let v = parametros_to_json(xml).unwrap();
        assert_eq!(v["ProcedimentoFormatado"], "0011108545.000056/2022-49");
        assert_eq!(v["TipoProcedimento"]["Nome"], "Licitação: Tomada de Preços");
        // AndamentoConclusao é xsi:nil -> null
        assert_eq!(v["AndamentoConclusao"], Value::Null);
    }

    #[test]
    fn nil_aceita_true_e_um() {
        // xsi:nil booleano: "true" e "1" viram null; "false"/"0" não.
        let xml = r#"<Resp xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><parametros><A xsi:nil="true"/><B xsi:nil="1"/><C xsi:nil="0">x</C></parametros></Resp>"#;
        let v = parametros_to_json(xml).unwrap();
        assert_eq!(v["A"], Value::Null);
        assert_eq!(v["B"], Value::Null);
        assert_eq!(v["C"], "x");
    }

    #[test]
    fn lista_vira_array() {
        let xml = include_str!("../../tests/fixtures/listarEstados.xml");
        let v = parametros_to_json(xml).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["Nome"], "FLORIDA");
        // Sigla é xsi:nil -> null
        assert_eq!(v[0]["Sigla"], Value::Null);
    }
}

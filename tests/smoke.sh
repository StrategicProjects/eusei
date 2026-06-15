#!/usr/bin/env bash
# eusei — testes rápidos via curl, com números de processo/documento reais.
#
# Uso:
#   EUSEI_TOKEN=seu-token bash tests/smoke.sh
#   EUSEI_TOKEN=... EUSEI_BASE=http://127.0.0.1:18088 bash tests/smoke.sh   # interno no boxdev
#
# Requer: curl. (jq é opcional — se existir, o JSON sai formatado.)

set -u
BASE="${EUSEI_BASE:-https://monitoramento.sepe.pe.gov.br/eusei}"
TOKEN="${EUSEI_TOKEN:?defina EUSEI_TOKEN=...}"
AUTH=(-H "Authorization: Bearer ${TOKEN}")

# dados reais (processo de licitação já validado)
PROTO="0011108545.000056/2022-49"
DOC="25833665"     # documento real desse processo (Anexo)

pp() { if command -v jq >/dev/null 2>&1; then jq .; else cat; echo; fi; }
hr() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

hr "health (público, sem token)"
curl -s "${BASE}/health" | pp

hr "consultar processo (real)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/procedimento" \
  --data-urlencode "protocolo=${PROTO}" | pp

hr "consultar processo — só o essencial (flags N)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/procedimento" \
  --data-urlencode "protocolo=${PROTO}" \
  --data-urlencode "sin_retornar_assuntos=N" \
  --data-urlencode "sin_retornar_interessados=N" \
  --data-urlencode "sin_retornar_observacoes=N" | pp

hr "lote de processos"
curl -s "${AUTH[@]}" --get "${BASE}/v1/procedimentos" \
  --data-urlencode "protocolos=${PROTO},0000000000.000000/2099-99" | pp

hr "andamentos (linha do tempo completa)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/andamentos" \
  --data-urlencode "protocolo=${PROTO}" | pp

hr "documentos do processo (heurística)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/documentos-processo" \
  --data-urlencode "protocolo=${PROTO}" | pp

hr "publicações do processo (pode levar alguns segundos)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/publicacoes-processo" \
  --data-urlencode "protocolo=${PROTO}" | pp

hr "consultar documento (real)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/documento" \
  --data-urlencode "protocolo=${DOC}" | pp

hr "listas (países)"
curl -s "${AUTH[@]}" "${BASE}/v1/paises" | pp

hr "listas (unidades)"
curl -s "${AUTH[@]}" "${BASE}/v1/unidades" | pp

# ---- casos de erro (falha graciosa) ----
hr "ERRO: sem token (espera 401)"
curl -s -o /dev/null -w "  http=%{http_code}\n" "${BASE}/v1/paises"

hr "ERRO: protocolo inexistente (espera 400 sei_fault)"
curl -s "${AUTH[@]}" --get "${BASE}/v1/procedimento" \
  --data-urlencode "protocolo=0000000000.000000/2099-99" | pp

hr "ERRO: SIP sem credenciais (espera 400)"
curl -s "${AUTH[@]}" "${BASE}/v1/permissao" | pp

echo
echo "fim."

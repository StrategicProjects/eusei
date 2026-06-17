#!/usr/bin/env bash
# eusei — rotação de segredos em /etc/eusei.env. Rode como:
#   sudo bash 03-rotate-secrets-sudo.sh
#
# O que faz (idempotente, com backup):
#   - SEMPRE: gera um novo EUSEI_TOKENS (Bearer token dos clientes) aleatório.
#   - OPCIONAL: se NEW_SEI_KEY estiver no ambiente, atualiza também
#     SEI_IDENTIFICACAO_SERVICO (a chave de acesso ao SEI — o valor novo precisa
#     ser emitido pelos administradores do SEI-PE; este script só o grava).
#   - faz backup do env, reescreve as chaves preservando o resto e reinicia o
#     serviço.
#
# Exemplos:
#   sudo bash 03-rotate-secrets-sudo.sh                 # só o Bearer token
#   sudo NEW_SEI_KEY='nova-chave' bash 03-rotate-secrets-sudo.sh   # + chave SEI

set -euo pipefail

ENV=/etc/eusei.env

if [[ $EUID -ne 0 ]]; then
  echo "Precisa de root. Rode: sudo bash $0" >&2
  exit 1
fi
[[ -f "$ENV" ]] || { echo "ERRO: $ENV não existe (rode o deploy primeiro)." >&2; exit 1; }

ts="$(date +%Y%m%d-%H%M%S 2>/dev/null || echo bak)"
cp -a "$ENV" "${ENV}.bak-rotate-${ts}"

# Token Bearer: 43 chars alfanuméricos (~256 bits de entropia). Gera 64 bytes
# antes do filtro para garantir os 43 chars (tr remove +,/,= do base64).
NEW_TOKEN="$(head -c 64 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-43)"

# Atualiza KEY=VALUE no env (ou acrescenta no fim se ausente), de forma segura:
# o valor é passado via ENVIRON para o awk, sem interpolação pelo shell.
set_kv() {
  local key="$1" val="$2" tmp
  tmp="$(mktemp)"
  KEY="$key" VAL="$val" awk -F= '
    BEGIN { k=ENVIRON["KEY"]; v=ENVIRON["VAL"]; done=0 }
    $1==k && !done { print k"="v; done=1; next }
    { print }
    END { if (!done) print k"="v }
  ' "$ENV" > "$tmp"
  cat "$tmp" > "$ENV"
  rm -f "$tmp"
}

set_kv EUSEI_TOKENS "$NEW_TOKEN"
echo "  + EUSEI_TOKENS rotacionado"

if [[ -n "${NEW_SEI_KEY:-}" ]]; then
  set_kv SEI_IDENTIFICACAO_SERVICO "$NEW_SEI_KEY"
  echo "  + SEI_IDENTIFICACAO_SERVICO atualizado"
fi

chown eusei:eusei "$ENV"
chmod 600 "$ENV"

systemctl restart eusei
echo "  + eusei reiniciado (backup: ${ENV}.bak-rotate-${ts})"

echo
echo "============================================================"
echo "  NOVO TOKEN (Bearer dos clientes) — anote agora:"
echo "      ${NEW_TOKEN}"
echo "  Guardado em ${ENV} (chmod 600). Não será exibido de novo."
echo "============================================================"
echo
echo "Valide:"
echo "  systemctl status eusei --no-pager"
echo "  curl -s -H 'Authorization: Bearer ${NEW_TOKEN}' http://127.0.0.1:18088/v1/paises | head -c 200"

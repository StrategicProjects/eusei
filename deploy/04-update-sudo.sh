#!/usr/bin/env bash
# eusei — atualização SEGURA do binário em produção (e, opcionalmente, do snippet
# nginx). Para a instalação inicial use 02-deploy-sudo.sh; este script só atualiza.
#
# Rode como:
#   sudo bash ~/eusei_dev/deploy/04-update-sudo.sh              # só o binário
#   sudo EUSEI_NGINX=1 bash ~/eusei_dev/deploy/04-update-sudo.sh # binário + nginx (p/ o SSE)
#
# O que faz (idempotente, seguro):
#   1. backup do binário atual em /opt/eusei/eusei.bak-<timestamp>;
#   2. instala o novo binário (compilado antes, sem sudo: cargo build --release);
#   3. (se EUSEI_NGINX=1) reinstala o snippet nginx e recarrega após `nginx -t`
#      — necessário para o endpoint SSE /v1/andamentos/stream (proxy_buffering off);
#   4. systemctl restart + healthcheck no /health;
#   5. se o healthcheck falhar, ROLLBACK automático para o binário anterior.

set -euo pipefail

[[ $EUID -eq 0 ]] || { echo "Precisa de root. Rode: sudo bash $0" >&2; exit 1; }

DEV_USER="${SUDO_USER:?execute via sudo (precisa de SUDO_USER)}"
DEV_HOME="$(eval echo "~${DEV_USER}")"
DEV_DIR="${EUSEI_DEV_DIR:-${DEV_HOME}/eusei_dev}"
BIN_SRC="${DEV_DIR}/target/release/eusei"
BIN_DST="/opt/eusei/eusei"
SNIPPET_SRC="${DEV_DIR}/deploy/eusei.nginx.conf"
HEALTH="http://127.0.0.1:18088/health"
ts="$(date +%Y%m%d-%H%M%S 2>/dev/null || echo bak)"

[[ -f "$BIN_SRC" ]] || { echo "ERRO: binário não encontrado: $BIN_SRC" >&2
  echo "      compile antes (sem sudo): cd ~/eusei_dev && cargo build --release --locked" >&2; exit 1; }
[[ -f "$BIN_DST" ]] || { echo "ERRO: $BIN_DST não existe — para a 1ª instalação use 02-deploy-sudo.sh." >&2; exit 1; }

echo "== eusei update (dev user: ${DEV_USER}) =="
echo "  versão em produção (antes): $(curl -s -m 3 "$HEALTH" 2>/dev/null | grep -oE '"version":"[^"]*"' || echo '?')"

# 1. backup do binário atual
BAK="${BIN_DST}.bak-${ts}"
cp -a "$BIN_DST" "$BAK"
echo "  + backup do binário atual: $BAK"

# 2. instala o novo binário
install -m 0755 "$BIN_SRC" "$BIN_DST"
echo "  + novo binário instalado em $BIN_DST"

# 3. (opcional) snippet nginx — para o SSE /v1/andamentos/stream
if [[ "${EUSEI_NGINX:-0}" == "1" ]]; then
  install -d -m 0755 /etc/nginx/snippets
  install -m 0644 "$SNIPPET_SRC" /etc/nginx/snippets/eusei.conf
  if nginx -t; then
    systemctl reload nginx
    echo "  + snippet nginx atualizado e nginx recarregado"
  else
    echo "  ! nginx -t FALHOU — snippet instalado mas nginx NÃO recarregado. Revise manualmente." >&2
  fi
fi

# 4. restart + healthcheck
systemctl restart eusei
ok=0
for _ in $(seq 1 10); do
  sleep 1
  if curl -fsS -m 3 "$HEALTH" >/dev/null 2>&1; then ok=1; break; fi
done

# 5. resultado / rollback
if [[ "$ok" == "1" ]]; then
  echo "  + serviço OK — versão agora: $(curl -s -m 3 "$HEALTH" | grep -oE '"version":"[^"]*"')"
  echo
  echo "== Pronto. Backup do binário anterior: $BAK =="
  echo "  Valide externamente: curl -s https://SEU-DOMINIO/eusei/health"
else
  echo "  ! healthcheck FALHOU após o restart — fazendo ROLLBACK para o binário anterior..." >&2
  install -m 0755 "$BAK" "$BIN_DST"
  systemctl restart eusei
  sleep 2
  if curl -fsS -m 3 "$HEALTH" >/dev/null 2>&1; then
    echo "  = rollback OK (versão anterior restaurada e no ar)." >&2
  else
    echo "  !! rollback também falhou — serviço fora do ar. Veja: journalctl -u eusei -n 50 --no-pager" >&2
  fi
  echo "== FALHOU (provável config inválida no /etc/eusei.env p/ a nova validação). Logs:" >&2
  echo "   journalctl -u eusei -n 50 --no-pager" >&2
  exit 1
fi

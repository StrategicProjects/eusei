#!/usr/bin/env bash
# eusei — instalação/atualização no servidor (Fase 5). Rode como:
#   sudo bash 02-deploy-sudo.sh
#
# Idempotente. O que faz:
#   1. cria o usuário de sistema `eusei` (sem login);
#   2. instala o binário release em /opt/eusei/eusei;
#   3. cria /etc/eusei.env (se ausente) com a config (chmod 600);
#   4. instala e ativa o serviço systemd `eusei` (bind 127.0.0.1:18088);
#   5. instala o snippet nginx e o inclui no server block de
#      monitoramento.sepe.pe.gov.br (backup + nginx -t antes de recarregar).
#
# Pré-requisito: ter compilado o release ANTES, como seu usuário (sem sudo):
#   cd ~/eusei_dev && ~/.cargo/bin/cargo build --release

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Precisa de root. Rode: sudo bash $0" >&2
  exit 1
fi

DEV_USER="${SUDO_USER:-andre.leite}"
DEV_HOME="$(eval echo "~${DEV_USER}")"
DEV_DIR="${DEV_HOME}/eusei_dev"
BIN_SRC="${DEV_DIR}/target/release/eusei"
SITE="/etc/nginx/sites-available/monitoramento.sepe.pe.gov.br"
SNIPPET_SRC="${DEV_DIR}/deploy/eusei.nginx.conf"
UNIT_SRC="${DEV_DIR}/deploy/eusei.service"

echo "== eusei deploy (dev user: ${DEV_USER}) =="

# --- 0. checagens ----------------------------------------------------------
for f in "$BIN_SRC" "$SNIPPET_SRC" "$UNIT_SRC"; do
  [[ -f "$f" ]] || { echo "ERRO: não encontrado: $f" >&2;
    echo "      compile o release primeiro: cd ~/eusei_dev && cargo build --release" >&2; exit 1; }
done
[[ -f "$SITE" ]] || { echo "ERRO: site nginx não encontrado: $SITE" >&2; exit 1; }

# --- 1. usuário de sistema -------------------------------------------------
if ! id -u eusei >/dev/null 2>&1; then
  useradd --system --no-create-home --shell /usr/sbin/nologin eusei
  echo "  + usuário 'eusei' criado"
else
  echo "  = usuário 'eusei' já existe"
fi

# --- 2. binário ------------------------------------------------------------
install -d -m 0755 /opt/eusei
install -m 0755 "$BIN_SRC" /opt/eusei/eusei
echo "  + binário instalado em /opt/eusei/eusei"

# --- 3. /etc/eusei.env -----------------------------------------------------
GEN_TOKEN=""
if [[ ! -f /etc/eusei.env ]]; then
  # Gera um token aleatório no install (sem janela de credencial padrão).
  GEN_TOKEN="$(head -c 32 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-43)"
  cat > /etc/eusei.env <<ENV
EUSEI_BIND=127.0.0.1:18088
EUSEI_TOKENS=${GEN_TOKEN}
SEI_URL=https://sei.pe.gov.br/sei/ws/SeiWS.php
SEI_SIGLA_SISTEMA=HORTENSIAS
SEI_IDENTIFICACAO_SERVICO=publicacao
SEI_ID_UNIDADE=
SEI_TIMEOUT_SECS=60
RUST_LOG=eusei=info,tower_http=warn
ENV
  chown eusei:eusei /etc/eusei.env
  chmod 600 /etc/eusei.env
  echo "  + /etc/eusei.env criado com um TOKEN gerado (anote — só aparece agora)."
else
  echo "  = /etc/eusei.env já existe (mantido)"
fi

# --- 4. systemd ------------------------------------------------------------
install -m 0644 "$UNIT_SRC" /etc/systemd/system/eusei.service
systemctl daemon-reload
systemctl enable eusei >/dev/null 2>&1 || true
systemctl restart eusei
echo "  + serviço systemd 'eusei' ativo"

# --- 5. nginx --------------------------------------------------------------
install -d -m 0755 /etc/nginx/snippets
install -m 0644 "$SNIPPET_SRC" /etc/nginx/snippets/eusei.conf
echo "  + snippet em /etc/nginx/snippets/eusei.conf"

if grep -q "snippets/eusei.conf" "$SITE"; then
  echo "  = include já presente no server block"
else
  ts="$(date +%Y%m%d-%H%M%S 2>/dev/null || echo bak)"
  cp -a "$SITE" "${SITE}.bak-eusei-${ts}"
  tmp="$(mktemp)"
  awk '
    /server_name[[:space:]]+monitoramento\.sepe\.pe\.gov\.br;/ && !done {
      print; print "\tinclude snippets/eusei.conf;"; done=1; next
    }
    { print }
  ' "$SITE" > "$tmp"
  if ! grep -q "snippets/eusei.conf" "$tmp"; then
    echo "ERRO: não achei a linha server_name para inserir o include." >&2
    echo "      Adicione manualmente 'include snippets/eusei.conf;' no server block." >&2
    rm -f "$tmp"; exit 1
  fi
  cp "$tmp" "$SITE"; rm -f "$tmp"
  echo "  + include adicionado (backup: ${SITE}.bak-eusei-${ts})"
fi

if nginx -t; then
  systemctl reload nginx
  echo "  + nginx recarregado"
else
  echo "ERRO: nginx -t falhou. Reverta com o backup .bak-eusei-* se necessário." >&2
  exit 1
fi

echo
if [[ -n "$GEN_TOKEN" ]]; then
  echo "============================================================"
  echo "  TOKEN gerado (Bearer dos clientes) — anote agora:"
  echo "      ${GEN_TOKEN}"
  echo "  Guardado em /etc/eusei.env (chmod 600). Não será exibido de novo."
  echo "============================================================"
  echo
fi
echo "== Pronto. Valide: =="
echo "  systemctl status eusei --no-pager"
echo "  curl -s http://127.0.0.1:18088/health"
echo "  curl -s -H 'Authorization: Bearer SEU-TOKEN' http://127.0.0.1:18088/v1/paises | head -c 200"
echo "  curl -s https://monitoramento.sepe.pe.gov.br/eusei/health   # via nginx corporativo"

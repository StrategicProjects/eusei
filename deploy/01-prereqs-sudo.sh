#!/usr/bin/env bash
# eusei — pré-requisitos de sistema no servidor (Fase 0).
# Rode no servidor como:  sudo bash 01-prereqs-sudo.sh
#
# O que faz (idempotente):
#   - Instala o toolchain de compilação C necessário para o linker do Rust
#     (build-essential / gcc), pkg-config, curl e ca-certificates.
#   - NÃO instala o Rust: o rustup é instalado SEM sudo, no home do
#     andre.leite (ver instrução abaixo).
#
# Observação: o eusei usa reqwest com rustls-tls, então NÃO precisamos de
# libssl-dev/openssl. Se algum crate exigir OpenSSL no futuro, acrescente
# "libssl-dev" à lista de pacotes.

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Este script precisa de root. Rode: sudo bash $0" >&2
  exit 1
fi

PKGS=(build-essential pkg-config curl ca-certificates)

if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -y
  apt-get install -y "${PKGS[@]}"
elif command -v dnf >/dev/null 2>&1; then
  dnf install -y gcc gcc-c++ make pkgconf-pkg-config curl ca-certificates
elif command -v yum >/dev/null 2>&1; then
  yum install -y gcc gcc-c++ make pkgconfig curl ca-certificates
else
  echo "Gerenciador de pacotes não reconhecido (sem apt/dnf/yum)." >&2
  echo "Instale manualmente: ${PKGS[*]}" >&2
  exit 1
fi

echo
echo "== Pré-requisitos de sistema instalados. =="
echo
echo "Próximo passo (SEM sudo, como andre.leite):"
echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
echo "  source \"\$HOME/.cargo/env\""
echo "  rustc --version && cargo --version"

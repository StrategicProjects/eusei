# eusei — notas para o Claude Code

API HTTP/JSON (Rust + axum) **read-only** sobre os Web Services SOAP do SEI.
Espelha as consultas do pacote R [`rsei`](https://github.com/StrategicProjects/rsei).
Veja `PLAN.md` (arquitetura/roadmap) e `README.md` (uso/instalação).

> Detalhes de deployment (host real, domínio público, comando de deploy com os
> valores concretos) **não ficam neste arquivo nem no repositório** — estão na
> memória privada do projeto (são específicos da instalação, não do código).

## Arquitetura (`src/`)

- `main.rs` — bootstrap: carrega config, monta o router (públicas: `/`, `/__docs__`,
  `/openapi.json`, assets; protegidas: `/v1/*`), sobe o axum.
- `config.rs` — `AppConfig::from_env` (SEI + SIP + tokens + bind). Recusa subir com
  token placeholder.
- `auth.rs` — middleware Bearer; comparação **constant-time** (SHA-256 + `subtle`).
- `error.rs` — `AppError` → JSON `{ ok:false, codigo, erro, detalhe }` + status.
- `soap/` — `envelope.rs` (monta o envelope; `Param::Scalar`/`Array`), `client.rs`
  (`soap_call` genérico p/ SEI e SIP, SOAP Fault + timeout + 1 retry), `parse.rs`
  (mapeador genérico XML→JSON; `parametros_to_json` p/ SEI, `return_to_json` p/ SIP).
- `sei/` — `consultas.rs`, `listas.rs`, `andamentos.rs` (timeline + docs/publicações
  de processo), `sip.rs`. Tudo wrapper fino sobre `sei::call`/`sei::sip_call`.
- `routes/mod.rs` — declara as rotas.
- `docs.rs` — serve landing (`static/index.html`), docs (`static/docs.html`),
  `tailwind.css`, fontes e openapi — todos `include_str!`/`include_bytes!`.

## Convenções importantes

- **Protocolo vai na query** (`?protocolo=...`), nunca no path: a barra do número
  não sobrevive como `%2F` no path através do nginx (→ 404). Rotas `/{protocolo}`
  existem só para uso interno em `127.0.0.1`.
- **Genérico**: nenhum domínio/host/órgão fixo no código. O endpoint do SEI é
  100% configurável por env (`SEI_URL`, `SEI_SIGLA_SISTEMA`,
  `SEI_IDENTIFICACAO_SERVICO`, `SEI_ID_UNIDADE`, `SEI_SIP_*`). Defaults apontam p/
  o SEI-PE mas são overridáveis. **Não reintroduzir** URLs/hosts específicos.
- **Sem segredos no repo**: token real e chave só em `/etc/eusei.env` no servidor.
  `.gitignore` cobre `.env`, `.claude/`, `.DS_Store`.
- JSON preserva os nomes originais do SEI; `xsi:nil` → `null`; arrays → listas.

## Build / test / deploy

- **Só compila/roda no servidor** (alias `ssh boxdev`) — é o único host com acesso
  liberado ao SEI (firewall por IP) e o alvo Linux do binário.
- Ciclo: editar local → `rsync` (use **caminho absoluto** na origem!) → no servidor
  `cargo build --release --locked` / `cargo test --locked`.
  - ⚠️ O cwd do shell **persiste entre chamadas**; um `rsync ... ./ ...` após um
    `cd /tmp/...` sincroniza a pasta errada e `--delete` apaga o repo remoto. Sempre
    `rsync -az --delete --exclude target --exclude .git /caminho/abs/eusei/ boxdev:~/eusei_dev/`.
- Testes efêmeros: porta dedicada (18097/18099); mate sobras com
  `pkill -u <devuser> -x eusei` (nunca `pkill -f target/release/eusei` — casa o
  próprio shell; nunca `pkill -x eusei` puro — mata a produção `/opt/eusei/eusei`).
- Deploy (privilegiado → script): `deploy/02-deploy-sudo.sh` (idempotente; cria
  usuário, gera `/etc/eusei.env` com token aleatório, systemd, inclui snippet nginx).
  Exige a env `EUSEI_NGINX_SITE`. Passos com sudo → entregar como script p/ o usuário.

## Frontend (Tailwind v4)

- `static/index.html` (landing) + `static/docs.html` (docs com diagramas SVG em
  `assets/`). Tema: papel + verde-pinheiro, fontes Fraunces + Spline Sans
  (vendorizadas em `static/*.woff2`).
- O `static/tailwind.css` é **gerado** (commitado). Para regenerar após mexer no
  HTML: `cd /tmp && npm i tailwindcss @tailwindcss/cli && cp <repo>/static/{tw-input.css,index.html,docs.html} . && npx @tailwindcss/cli -i tw-input.css -o tailwind.css --minify && cp tailwind.css <repo>/static/`.
- SVGs são inline (não geram classes Tailwind novas) — editar não exige regenerar CSS.

## Distribuição / CI

- **`.deb`** via `cargo-deb` (metadata em `Cargo.toml`, scripts em `deploy/debian/`).
- **Homebrew**: `Formula/eusei.rb` (build do fonte; `sha256` da tag).
- **CI** (`.github/workflows/ci.yml`): build+test em push/PR; nas tags `v*` gera
  `.deb`+binário e publica a release. Actions de terceiros **pinadas por SHA**;
  Dependabot (`.github/dependabot.yml`) acompanha actions + cargo.
- Próxima release: `git tag vX.Y.Z && git push origin vX.Y.Z` (CI faz o resto).

# Plano — `eusei`: API HTTP/JSON para os Web Services do SEI

Serviço em **Rust (axum)** que roda no servidor **servidor** (único com acesso
liberado ao SEI pelo firewall institucional), recebe consultas de processos por
HTTP e devolve **JSON**. Os endpoints espelham as funções de consulta do pacote R
[`rsei`](../rsei). Publicado externamente em
`https://monitoramento.sepe.pe.gov.br/eusei/...` via **nginx → systemd** no servidor.

## Decisões acordadas

| Tema | Decisão |
|------|---------|
| Stack | **Rust + axum** (binário único, sem runtime, footprint pequeno) |
| Escopo v1 | **Somente consultas read-only** (consultar_* + listar_*) |
| Autenticação | **Bearer tokens estáticos** (header `Authorization: Bearer <token>`) |
| Chave SEI | Fixa no servidor (`SEI_IDENTIFICACAO_SERVICO`), nunca exposta ao cliente |
| Deploy | **systemd nativo** no servidor + **nginx** como reverse proxy em `/eusei` |

## Como o SEI funciona (resumo do manual + do rsei)

- Protocolo: **SOAP 1.1**. Endpoint de produção (read-only):
  `https://sei.pe.gov.br/sei/ws/SeiWS.php`. Treino: `sei4treina.pe.gov.br`.
- Autenticação **no SEI**: `SiglaSistema` (ex.: `HORTENSIAS`) +
  `IdentificacaoServico` (a *chave de acesso* gerada no cadastro do serviço).
- Envelope correto (ver `rsei/R/core.R`):
  `xmlns:sei="Sei"`, operação com `soapenv:encodingStyle=".../soap/encoding/"`,
  parâmetros como `<Param xsi:type="xsd:string">valor</Param>`.
- Erros vêm como **SOAP Fault** (às vezes em HTTP 500) com `faultcode`/`faultstring`.
- **Acesso restrito por IP**: só o servidor consegue falar com o SEI. Por isso build
  e testes de integração rodam no servidor.

## Arquitetura (camadas — espelham o rsei)

```
eusei/
├── Cargo.toml
├── .env.example                # template de configuração
├── README.md
├── PLAN.md / CLAUDE.md
├── src/
│   ├── main.rs                 # bootstrap: carrega config, monta router, sobe axum
│   ├── config.rs               # AppConfig a partir de env (SEI + tokens + bind)
│   ├── auth.rs                 # middleware Bearer token (401 se inválido)
│   ├── error.rs                # AppError -> resposta JSON {erro, detalhe} + status
│   ├── soap/
│   │   ├── envelope.rs         # build_envelope + xml_escape  (porta de core.R)
│   │   ├── client.rs           # sei_call: POST reqwest, timeout, SOAP Fault
│   │   └── parse.rs            # localizar <parametros>, helpers nó->JSON (local-name)
│   ├── sei/
│   │   ├── consultas.rs        # consultar_procedimento/documento/publicacao/bloco/individual
│   │   ├── listas.rs           # listar_* (unidades, series, tipos, andamentos, ...)
│   │   └── models.rs           # structs serde (Procedimento, Documento, Unidade, ...)
│   └── routes/
│       ├── procedimento.rs     # GET /procedimento/{protocolo}, batch
│       ├── documento.rs
│       ├── publicacao.rs
│       ├── bloco.rs
│       └── listas.rs
├── deploy/
│   ├── eusei.service           # unit systemd (User, EnvironmentFile, ExecStart)
│   └── nginx-eusei.conf        # location /eusei { proxy_pass 127.0.0.1:18088 }
└── tests/
    └── fixtures/               # XMLs reais capturados do servidor (offline)
```

### Crates
- `axum` + `tokio` — servidor HTTP assíncrono.
- `reqwest` (rustls-tls) — cliente HTTP para o POST SOAP.
- `roxmltree` — navegação XML por `tag_name().name()` ignorando namespace
  (equivalente ao `xml_find_all(..., local-name())` do rsei).
- `serde` + `serde_json` — serialização JSON de saída.
- `tower-http` (TraceLayer, CorsLayer opcional) + `tracing` — logs/observabilidade.
- `thiserror` — erros tipados; `dotenvy` — carregar `.env`.

## Endpoints (espelhando o `rsei`)

Prefixo de versão `/v1`. Externamente: `https://monitoramento.sepe.pe.gov.br/eusei/v1/...`
(nginx encaminha `/eusei/` → `127.0.0.1:18088/`). Todos exigem `Authorization: Bearer`,
exceto `/health`.

| Método | Rota | Função rsei | Observações |
|--------|------|-------------|-------------|
| GET | `/health` | — | liveness, sem auth |
| GET | `/v1/procedimento/{protocolo}` | `consultar_procedimento` | flags `sin_retornar_*` via query (default `S`) |
| GET | `/v1/procedimentos?protocolo=A&protocolo=B` | `consultar_procedimentos` | lote; `erro` por item |
| GET | `/v1/procedimento-individual` | `consultar_procedimento_individual` | params: id_orgao_*, id_tipo_*, sigla_usuario |
| GET | `/v1/documento/{protocolo}` | `consultar_documento` | flags `sin_retornar_*` |
| GET | `/v1/documentos?protocolo=...` | `consultar_documentos` | lote |
| GET | `/v1/publicacao` | `consultar_publicacao` | um de: `id_publicacao`/`id_documento`/`protocolo_documento` |
| GET | `/v1/bloco/{id}` | `consultar_bloco` | `sin_retornar_protocolos` (default `N`) |
| GET | `/v1/unidades` | `listar_unidades` | listas read-only |
| GET | `/v1/series` · `/tipos-procedimento` · `/andamentos` · `/cargos` · `/cidades` · `/estados` · `/paises` · `/contatos` · `/hipoteses-legais` · `/marcadores` · `/extensoes-permitidas` · `/feriados` · `/tipos-conferencia` · `/usuarios` | `listar_*` | mesma forma |

Formato de saída JSON (exemplo `procedimento`):
```json
{ "ok": true, "dados": { "ProcedimentoFormatado": "...", "Especificacao": "...", ... } }
```
Erros (SOAP Fault, validação, upstream): `{ "ok": false, "erro": "...", "detalhe": "..." }`
com status HTTP apropriado (400/401/502/504).

## Configuração (`.env` / EnvironmentFile do systemd)

```
EUSEI_BIND=127.0.0.1:18088
EUSEI_TOKENS=tok-cliente-1,tok-cliente-2     # tokens válidos (Bearer)
SEI_URL=https://sei.pe.gov.br/sei/ws/SeiWS.php
SEI_SIGLA_SISTEMA=HORTENSIAS
SEI_IDENTIFICACAO_SERVICO=<chave-de-acesso>  # secreto, só no servidor
SEI_ID_UNIDADE=
SEI_TIMEOUT_SECS=60
RUST_LOG=eusei=info,tower_http=info
```

## Deploy (servidor)

1. **nginx** (host) — `location /eusei/ { proxy_pass http://127.0.0.1:18088/; ... }`
   com `proxy_set_header` padrão; TLS terminado no domínio
   `monitoramento.sepe.pe.gov.br` (já existente / a confirmar com infra).
2. **systemd** — `eusei.service`: `EnvironmentFile=/etc/eusei.env`,
   `ExecStart=/opt/eusei/eusei`, `User=eusei`, `Restart=on-failure`. Bind em
   `127.0.0.1` (só nginx acessa de fora).
3. Binário em `/opt/eusei/eusei`; segredos em `/etc/eusei.env` (chmod 600).

## Ciclo de desenvolvimento (igual ao rsei)

Só o servidor fala com o SEI **e** compila o binário Linux de destino:
1. Editar local (mac).
2. `rsync` do código → `servidor:~/eusei_dev/` (excluindo `target/`, `.git`).
3. No servidor: `cargo build --release` e `cargo test` (rustup necessário no servidor).
4. Testes de integração read-only contra `sei.pe.gov.br`; unit tests offline com
   fixtures (XMLs reais já presentes no home do servidor: `unidades.xml`, `text2.xml`,
   `consultaProcd.xml`, `consultaPublic.xml`, `consultaDocument.xml`).
5. Deploy: copiar `target/release/eusei` → `/opt/eusei/`, `systemctl restart eusei`.

## Roadmap por fases

| Fase | Entrega | Validação | Status |
|------|---------|-----------|--------|
| 0 | Scaffold Cargo + axum + `/health` + config por env + rsync→servidor | `cargo run` no servidor; `curl /health` | ✅ |
| 1 | `soap::envelope` + `soap::client` (SOAP Fault, timeout) | unit test envelope; chamada read-only | ✅ |
| 2 | `soap::parse` (mapeador genérico XML→JSON) + parser de `consultarProcedimento` | unit tests com fixtures reais → JSON | ✅ |
| 3 | Endpoints `/procedimento(s)`, `/documento`, `/publicacao`, `/bloco`, `/procedimento-individual` + auth Bearer | integração read-only no servidor | ✅ |
| 4 | Endpoints `listar_*` | integração read-only (listarPaises ok) | ✅ |
| 5 | `deploy/eusei.service` + `deploy/nginx-eusei.conf` + README de operação | `systemctl` ativo; acesso por `/eusei` | ✅ |
| 6 | `listarAndamentos` (params array no envelope) + `documentos-processo` + lote de documentos + OpenAPI/Redoc em `/__docs__` | 8/8 testes; docs validadas live | ✅* |

\* Fase 6: código completo, 8/8 testes offline, docs (`/__docs__`, `/openapi.json`,
`/redoc.standalone.js` vendorizado) validadas em produção. A validação **live** de
`andamentos`/`documentos-processo` ficou pendente porque o acesso servidor→SEI foi
bloqueado pelo firewall institucional no meio da sessão (ver memory
`servidor-sei-firewall`) — não é bug do eusei; endpoints SEI-independentes seguem ok.

### Read-only — todos implementados ✅
- `publicacoes-processo` (heurística sobre documentos do processo; consultas
  paralelizadas, conc. 10).
- `permissao` (SIP — endpoint/namespace `?servico=sip`; requer `SEI_SIP_*`,
  retorna 400 gracioso se não configurado).

### Frontend
- Landing em **Tailwind CSS v4** (`/`) + referência **Redoc** (`/__docs__`),
  ambos vendorizados no binário (sem CDN).

**Validado em produção (2026-06-15, servidor → sei.pe.gov.br):** 401 sem token;
`/v1/paises` (array, UTF-8 ok); `/v1/procedimento/{real}` (objeto completo,
`xsi:nil`→null, arrays vazias→`[]`); protocolo inexistente → SOAP Fault → HTTP 400
com `detalhe` legível.

## Riscos / a confirmar

- **Rust no servidor**: instalar `rustup`/toolchain (ou cross-compilar musl do mac —
  mais trabalhoso). Pré-requisito da Fase 0.
- **TLS e DNS de `monitoramento.sepe.pe.gov.br/eusei`**: quem controla o nginx/
  certificado e se o path `/eusei` está livre — alinhar com a infra.
- **Encoding**: round-trip latin1/utf-8 nas respostas do SEI (o rsei força utf-8).
- **Mapeamento XML→JSON**: decidir saída "fiel ao SEI" (nomes originais) vs
  normalizada (snake_case); proposta: manter nomes originais do SEI na v1.
- **Segredo da chave de acesso**: manter só em `/etc/eusei.env` (600), fora do git.
```

I plan to: (1) confirm this plan with you, then (2) scaffold the Cargo
project and Fase 0–2 if approved.

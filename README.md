<p align="center">
  <img src="assets/logo.svg" alt="eusei" width="220"/>
</p>

<p align="center">
  <strong>API HTTP/JSON (Rust + axum) para os Web Services do SEI.</strong><br/>
  Consultas read-only de processos, documentos e andamentos — em JSON limpo.
</p>

<p align="center">
  <img src="https://github.com/StrategicProjects/eusei/actions/workflows/ci.yml/badge.svg" alt="CI"/>
  <img src="https://img.shields.io/badge/rust-1.96-0b5848" alt="rust"/>
  <img src="https://img.shields.io/badge/api-read--only-0e6e5c" alt="read-only"/>
  <img src="https://img.shields.io/badge/license-GPL--3.0-555" alt="license"/>
</p>

<p align="center">
  <img src="assets/fluxo.svg" alt="Fluxo de uma requisição" width="720"/>
</p>

A **eusei** traduz os Web Services SOAP do SEI (Sistema Eletrônico de Informações)
em JSON. Roda em um servidor com acesso liberado ao SEI e espelha as consultas do
pacote R [`rsei`](https://github.com/StrategicProjects/rsei). Atrás de um nginx,
fica acessível em `https://SEU-DOMINIO/eusei/`.

Veja [`PLAN.md`](PLAN.md) para a arquitetura e o roadmap.

## Destaques

- **Read-only completo**: processos, documentos, publicações, blocos, andamentos e 15 listas auxiliares.
- **Auto-contido**: um binário único serve a API, a landing (Tailwind v4) e a documentação — sem CDN.
- **Falha graciosa**: erros com `codigo` estável e mensagens claras (incl. “SEI indisponível”).
- **Seguro**: Bearer token; a chave do SEI nunca é exposta ao cliente.

## Instalação

Os artefatos saem prontos em cada [release](https://github.com/StrategicProjects/eusei/releases)
(o CI gera e anexa automaticamente nas tags `v*`):

| Artefato | Para quê |
|----------|----------|
| `eusei_<versão>_amd64.deb` | Debian/Ubuntu amd64 (glibc ≥ 2.35) — produção |
| `eusei-linux-x86_64` | binário avulso (Ubuntu 22.04) |
| fórmula Homebrew | macOS/Linux (compila do fonte) — uso local/dev |

### 1. Debian/Ubuntu — pacote `.deb` (recomendado)

```sh
# baixar o .deb da última release (precisa do gh CLI)
gh release download --repo StrategicProjects/eusei --pattern '*.deb'
# … ou baixe manualmente em /releases/latest

sudo apt install ./eusei_*_amd64.deb
```

O pacote, via `postinst`, **cria o usuário de sistema** `eusei`, **gera
`/etc/eusei.env` com um token aleatório** (impresso uma vez) e **habilita** o
serviço (sem iniciar). Em seguida:

```sh
sudo nano /etc/eusei.env      # defina SEI_IDENTIFICACAO_SERVICO (e SEI_URL/SIGLA se não for o PE)
sudo systemctl start eusei
curl -s http://127.0.0.1:18088/health
```

Conteúdo instalado: binário em `/usr/bin/eusei`, unit `eusei.service`, e os
modelos `eusei.env.example` / `eusei.nginx.conf` em `/usr/share/eusei/`. Para
expor publicamente, inclua o snippet do nginx
([`deploy/eusei.nginx.conf`](deploy/eusei.nginx.conf)).

Atualizar: baixe o `.deb` novo e `sudo apt install ./eusei_*_amd64.deb` (o
`/etc/eusei.env` é preservado). Remover: `sudo apt remove eusei` (ou `purge`).

### 2. Homebrew (macOS / Linux) — uso local/dev

```sh
brew tap StrategicProjects/eusei https://github.com/StrategicProjects/eusei
brew install eusei
EUSEI_TOKENS=meu-token SEI_IDENTIFICACAO_SERVICO=minha-chave eusei
```

A fórmula compila do fonte (precisa do Rust, instalado como dependência de build).
É voltada a execução manual/dev — para servidor de produção, prefira o `.deb`.

### 3. Binário avulso

Baixe `eusei-linux-x86_64` da release, coloque em `/usr/local/bin/eusei`, crie
`/etc/eusei.env` (modelo em [`.env.example`](.env.example)) e rode. O script
[`deploy/02-deploy-sudo.sh`](deploy/02-deploy-sudo.sh) automatiza usuário, env,
systemd e o proxy nginx (defina `EUSEI_NGINX_SITE`).

## Desenvolvimento

Só o servidor fala com o SEI e compila o binário Linux de destino. Ciclo:

```sh
# 1. editar local (mac)
# 2. sincronizar para o servidor — use SEMPRE caminho ABSOLUTO na origem;
#    com `./` e um cwd "sujo", o --delete pode apagar a pasta errada no destino.
rsync -az --delete --exclude target --exclude .git /caminho/abs/eusei/ servidor:~/eusei_dev/
# 3. no servidor
ssh servidor 'cd ~/eusei_dev && ~/.cargo/bin/cargo build && cp .env.example .env && ~/.cargo/bin/cargo run'
```

## Configuração

Veja [`.env.example`](.env.example). Em produção, as variáveis vêm de
`/etc/eusei.env` via systemd. A chave de acesso do SEI
(`SEI_IDENTIFICACAO_SERVICO`) fica só no servidor e nunca é exposta ao cliente.

> **Funciona com qualquer instância do SEI.** O endpoint, a sigla e a chave são
> 100% configuráveis (`SEI_URL`, `SEI_SIGLA_SISTEMA`, `SEI_IDENTIFICACAO_SERVICO`,
> `SEI_ID_UNIDADE` e `SEI_SIP_*`). Os defaults apontam para o SEI de Pernambuco,
> mas o envelope SOAP e as operações são padrão do SEI — basta apontar `SEI_URL`
> para outra instalação. Nada de específico do PE está embutido no código.

## Autenticação

Todas as rotas (exceto `/health`) exigem `Authorization: Bearer <token>`, validado
contra `EUSEI_TOKENS`.

## Uso

Base pública: `https://SEU-DOMINIO/eusei/`.

> **Importante:** consulte processo/documento/bloco pela **query string**
> (`?protocolo=...`), não pelo path. A barra (`/`) do número do processo não
> sobrevive como `%2F` no path através do nginx; na query ela passa intacta.

```sh
# consultar um processo (forma recomendada)
curl -H 'Authorization: Bearer SEU-TOKEN' \
  'https://SEU-DOMINIO/eusei/v1/procedimento?protocolo=0011108545.000056/2022-49'

# listas
curl -H 'Authorization: Bearer SEU-TOKEN' \
  'https://SEU-DOMINIO/eusei/v1/paises'
```

### Endpoints

| Rota | Descrição |
|------|-----------|
| `GET /health` | liveness (sem auth) |
| `GET /v1/procedimento?protocolo=` | consulta um processo (flags `sin_retornar_*` opcionais) |
| `GET /v1/procedimentos?protocolos=A,B` | lote (cada item com `dados`/`erro`) |
| `GET /v1/procedimento-individual?...` | processo individual mais recente |
| `GET /v1/documento?protocolo=` | consulta um documento |
| `GET /v1/publicacao?...` | consulta publicação (id_publicacao/id_documento/protocolo_documento) |
| `GET /v1/bloco?id=` | consulta um bloco |
| `GET /v1/{paises,estados,cidades,unidades,series,tipos-procedimento,...}` | listas read-only |

As variantes com path (`/v1/procedimento/{protocolo}`) existem para uso interno
(`127.0.0.1:18088`), onde o `%2F` é preservado.

### Formato da resposta

Sucesso devolve `{ ok, dados }`, com os nomes originais do SEI (`xsi:nil` → `null`,
arrays como listas JSON). Abaixo, a estrutura de um processo:

<p align="center">
  <img src="assets/resposta.svg" alt="Anatomia da resposta de um processo" width="380"/>
</p>

### Erros (falha graciosa)

Erros vêm como JSON `{ "ok": false, "codigo", "erro", "detalhe" }` com o status
HTTP apropriado. O campo `codigo` é estável para tratamento pelo cliente:

| `codigo` | HTTP | Quando |
|----------|------|--------|
| `nao_autorizado` | 401 | token ausente/inválido |
| `parametro_invalido` | 400 | falta um parâmetro obrigatório |
| `sei_fault` | 400 | o SEI rejeitou (ex.: processo inexistente) — `erro` traz a mensagem do SEI |
| `sei_indisponivel` | 503 | **SEI fora do ar / sem conexão** (após 1 retentativa) |
| `sei_timeout` | 504 | o SEI demorou demais |
| `sei_erro_http` | 502 | o SEI respondeu HTTP de erro |
| `resposta_invalida` | 502 | resposta do SEI não pôde ser interpretada |

Exemplo (SEI fora do ar):

```json
{ "ok": false, "codigo": "sei_indisponivel",
  "erro": "O SEI está temporariamente indisponível. Não foi possível conectar ao servidor do SEI; tente novamente em alguns minutos.",
  "detalhe": null }
```

## Frontend

- **Landing**: `https://SEU-DOMINIO/eusei/` — página inicial em
  **Tailwind CSS v4** (CSS gerado/tree-shaken e embutido no binário, sem CDN), com
  hero, início rápido e cards de endpoints.
- **Referência**: `…/eusei/__docs__` — documentação custom em Tailwind v4
  (cards por endpoint, diagramas SVG de fluxo e da resposta, navegação lateral).

Tudo é servido pelo próprio binário (Tailwind e fontes vendorizados, sem
dependência de rede externa). O CSS do Tailwind (cobre `index.html` e `docs.html`)
é regenerado com:

```sh
cd /tmp && npm i tailwindcss @tailwindcss/cli && \
  cp <repo>/static/{tw-input.css,index.html,docs.html} . && \
  npx @tailwindcss/cli -i tw-input.css -o tailwind.css --minify && \
  cp tailwind.css <repo>/static/tailwind.css
```

//! Frontend e documentação: landing + página de docs (ambas Tailwind v4),
//! spec OpenAPI e fontes — tudo vendorizado no binário, sem CDN.
//! Rotas públicas (sem autenticação).

use axum::{body::Bytes, http::header, response::Html};

/// Spec OpenAPI embutido (machine-readable; também servido em /openapi.json).
const OPENAPI: &str = include_str!("../static/openapi.json");

/// Fontes vendorizadas (variáveis, subset latin — cobre acentuação do português).
const FRAUNCES: &[u8] = include_bytes!("../static/fraunces.woff2");
const SPLINE: &[u8] = include_bytes!("../static/splinesans.woff2");

/// Páginas (Tailwind v4) e o CSS gerado, embutidos no binário.
const INDEX_HTML: &str = include_str!("../static/index.html");
const DOCS_HTML: &str = include_str!("../static/docs.html");
const TAILWIND_CSS: &str = include_str!("../static/tailwind.css");

/// Fontes são versionadas de fato pelo conteúdo (nome fixo, bytes fixos): cache
/// longo + `immutable`.
const FONT_CACHE: &str = "public, max-age=31536000, immutable";
/// O CSS é gerado e seu conteúdo muda entre deploys sem mudar o nome do arquivo.
/// Sem `immutable` e com `must-revalidate` para o navegador não servir CSS
/// velho por um ano após um deploy.
const CSS_CACHE: &str = "public, max-age=3600, must-revalidate";

pub async fn openapi() -> ([(header::HeaderName, &'static str); 1], &'static str) {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        OPENAPI,
    )
}

pub async fn tailwind() -> ([(header::HeaderName, &'static str); 2], &'static str) {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, CSS_CACHE),
        ],
        TAILWIND_CSS,
    )
}

pub async fn font_fraunces() -> ([(header::HeaderName, &'static str); 2], Bytes) {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, FONT_CACHE),
        ],
        Bytes::from_static(FRAUNCES),
    )
}

pub async fn font_spline() -> ([(header::HeaderName, &'static str); 2], Bytes) {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, FONT_CACHE),
        ],
        Bytes::from_static(SPLINE),
    )
}

pub async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

pub async fn docs() -> Html<&'static str> {
    Html(DOCS_HTML)
}

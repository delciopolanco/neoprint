//! HTTP surface. Loopback-only server — no auth layer. The 127.0.0.1
//! bind IS the trust boundary: only processes on the operator's machine
//! can reach the endpoints.

use axum::{
    extract::{DefaultBodyLimit, Request},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{origin, print};

/// A4 PDFs with embedded fonts base64-encode well past axum's 2 MB default.
const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;

/// Refuse browser requests from origins that are not allowlisted. A request
/// with no `Origin` (curl, native tools) is inside the loopback trust boundary.
async fn origin_guard(req: Request, next: Next) -> Result<Response, StatusCode> {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let allowed = origin.to_str().map(origin::is_allowed).unwrap_or(false);
        if !allowed {
            tracing::warn!(?origin, "rejected request from a non-allowlisted origin");
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(next.run(req).await)
}

pub fn router() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            |origin: &HeaderValue, _| origin.to_str().map(origin::is_allowed).unwrap_or(false),
        ))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
        // Chrome's Private Network Access: a public HTTPS page reaching a
        // loopback server sends this preflight and requires the opt-in reply.
        .allow_private_network(true);

    Router::new()
        .route("/health", get(health))
        .route("/printers", get(list_printers))
        .route("/print/raw", post(print_raw))
        .route("/print/pdf", post(print_pdf))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(origin_guard))
        .layer(cors)
}

fn ensure_printer(name: &str) -> Result<(), (StatusCode, String)> {
    if print::printer_exists(name) {
        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, format!("printer not found: {name}")))
    }
}

#[derive(Serialize)]
pub struct HealthResponse {
    ok: bool,
    version: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
pub struct PrintersResponse {
    /// Display names (kept as-is for existing consumers).
    printers: Vec<String>,
    /// Same printers with the format each accepts (`escpos` | `pdf` | `unknown`).
    devices: Vec<print::PrinterDevice>,
}

pub async fn list_printers() -> Json<PrintersResponse> {
    let devices = print::list_devices();
    Json(PrintersResponse {
        printers: devices.iter().map(|d| d.name.clone()).collect(),
        devices,
    })
}

#[derive(Deserialize)]
pub struct RawRequest {
    printer: String,
    #[serde(rename = "bytesBase64")]
    bytes_base64: String,
}

pub async fn print_raw(
    Json(body): Json<RawRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_printer(&body.printer)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.bytes_base64.as_bytes())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad base64: {e}")))?;
    print::send_raw(&body.printer, &bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PdfRequest {
    printer: String,
    #[serde(rename = "pdfBase64")]
    pdf_base64: String,
}

pub async fn print_pdf(
    Json(body): Json<PdfRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_printer(&body.printer)?;
    let pdf = base64::engine::general_purpose::STANDARD
        .decode(body.pdf_base64.as_bytes())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad base64: {e}")))?;
    print::send_pdf(&body.printer, &pdf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn get_req(path: &str, origin: Option<&str>) -> axum::http::Request<Body> {
        let mut b = axum::http::Request::builder().method("GET").uri(path);
        if let Some(o) = origin {
            b = b.header("origin", o);
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn foreign_origin_is_forbidden_even_for_a_simple_get() {
        let res = router()
            .oneshot(get_req("/printers", Some("https://evil.example")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn foreign_origin_cannot_print() {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/print/raw")
            .header("origin", "https://evil.example")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"printer":"x","bytesBase64":"G0A="}"#))
            .unwrap();
        let res = router().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allowlisted_origin_is_served_with_cors_headers() {
        let res = router()
            .oneshot(get_req("/health", Some("https://app.neogeniux.com")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "https://app.neogeniux.com"
        );
    }

    #[tokio::test]
    async fn no_origin_header_is_allowed_for_native_callers() {
        let res = router().oneshot(get_req("/health", None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn preflight_answers_private_network_access() {
        let req = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/print/raw")
            .header("origin", "https://app.neogeniux.com")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .header("access-control-request-private-network", "true")
            .body(Body::empty())
            .unwrap();
        let res = router().oneshot(req).await.unwrap();
        assert!(res.status().is_success());
        assert_eq!(
            res.headers()
                .get("access-control-allow-private-network")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test]
    async fn unknown_printer_is_404_before_anything_is_decoded_or_spawned() {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/print/pdf")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"printer":"x'; Remove-Item -Recurse; '","pdfBase64":"AAAA"}"#,
            ))
            .unwrap();
        let res = router().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}

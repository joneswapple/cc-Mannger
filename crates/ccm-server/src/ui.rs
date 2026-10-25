//! The web admin UI (`dist/`), embedded at build time and served at `/`.

use axum::http::{header, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CCM_UI_DIR"]
struct Assets;

pub fn router() -> Router {
    Router::new().fallback(serve)
}

async fn serve(method: Method, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // API namespaces never fall through to the SPA.
    if method != Method::GET && method != Method::HEAD
        || ["v1/", "admin/api/", "oauth/"]
            .iter()
            .any(|p| path.starts_with(p))
    {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "not found"})),
        )
            .into_response();
    }
    let (file, name) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (f, path),
        // Unknown paths get the SPA shell (client-side navigation).
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        file.data,
    )
        .into_response()
}

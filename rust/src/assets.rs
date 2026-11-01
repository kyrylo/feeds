use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::Response,
};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub async fn serve(uri: Uri) -> Response {
    match ASSETS.iter().find(|(path, _, _)| *path == uri.path()) {
        Some((path, mime, bytes)) => {
            let mut response = Response::new(Body::from(*bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(if path.starts_with("/assets/") {
                    "public, max-age=31536000, immutable"
                } else {
                    "public, max-age=3600"
                }),
            );
            response
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_original_imports_resolve_to_embedded_files() {
        let map: serde_json::Value = serde_json::from_str(IMPORTMAP).unwrap();
        for (name, url) in map["imports"].as_object().unwrap() {
            assert!(
                ASSETS
                    .iter()
                    .any(|(path, _, _)| Some(*path) == url.as_str()),
                "{name}"
            );
        }
        for name in [
            "application",
            "controllers",
            "@hotwired/turbo-rails",
            "@hotwired/stimulus",
            "@hotwired/stimulus-loading",
            "sortablejs",
        ] {
            assert!(map["imports"][name].is_string(), "{name}");
        }
        assert!(!STYLES.is_empty());
    }
}

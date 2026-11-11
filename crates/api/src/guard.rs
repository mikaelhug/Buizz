use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::Failure;

const FETCH_SITE: &str = "sec-fetch-site";
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
    frame-ancestors 'none'";

fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers.get(FETCH_SITE) {
        return site == "same-origin";
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, host)| host);
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    origin_host.is_some_and(|origin_host| Some(origin_host) == host)
}

pub(crate) async fn reject_cross_site(request: Request, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if safe || same_origin(request.headers()) {
        next.run(request).await
    } else {
        Failure::new(StatusCode::FORBIDDEN, "cross-site request refused").into_response()
    }
}

pub(crate) async fn spine_host_only(
    State(spine_ip): State<IpAddr>,
    request: Request,
    next: Next,
) -> Response {
    let from_spine_host = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| {
            let peer = peer.ip().to_canonical();
            peer.is_loopback() || peer == spine_ip.to_canonical()
        });
    if from_spine_host {
        next.run(request).await
    } else {
        Failure::new(
            StatusCode::FORBIDDEN,
            "bots connect from the spine's host only",
        )
        .into_response()
    }
}

pub(crate) async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::middleware::from_fn;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;

    async fn post_status(headers: &[(&str, &str)]) -> StatusCode {
        let router = Router::new()
            .route("/api/bots/x/live", post(|| async { StatusCode::OK }))
            .layer(from_fn(reject_cross_site));
        let mut request =
            Request::post("/api/bots/x/live").header(header::HOST, "buizz.example.se");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(Body::empty()).expect("valid request");
        router
            .oneshot(request)
            .await
            .expect("infallible router")
            .status()
    }

    #[tokio::test]
    async fn posts_from_a_sibling_subdomain_are_refused() {
        let sibling = [
            (FETCH_SITE, "same-site"),
            ("origin", "https://other.example.se"),
        ];
        assert_eq!(post_status(&sibling).await, StatusCode::FORBIDDEN);
        let sibling_without_fetch_metadata = [("origin", "https://other.example.se")];
        assert_eq!(
            post_status(&sibling_without_fetch_metadata).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn same_origin_and_non_browser_posts_pass() {
        let browser = [
            (FETCH_SITE, "same-origin"),
            ("origin", "https://buizz.example.se"),
        ];
        assert_eq!(post_status(&browser).await, StatusCode::OK);
        let older_browser = [("origin", "https://buizz.example.se")];
        assert_eq!(post_status(&older_browser).await, StatusCode::OK);
        assert_eq!(post_status(&[]).await, StatusCode::OK);
    }
}

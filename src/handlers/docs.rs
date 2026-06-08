use axum::response::{Html, IntoResponse, Response};
use axum::http::header;

pub async fn api_docs() -> Html<&'static str> {
    Html(include_str!("../../static/api-docs.html"))
}

pub async fn openapi_json() -> Response {
    let json = include_str!("../../static/openapi.json");
    (
        [(header::CONTENT_TYPE, "application/json")],
        json,
    ).into_response()
}

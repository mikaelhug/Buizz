#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("BUIZZ_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!("fixture API on http://{bind}");
    axum::serve(listener, buizz_api::fixture_router()).await
}

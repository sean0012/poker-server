mod app;
mod log;
mod models;
mod service;
mod state;

#[tokio::main]
async fn main() {
    app::run().await;
}

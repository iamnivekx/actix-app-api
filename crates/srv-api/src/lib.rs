#![feature(async_closure)]

use actix_web::{web, App, HttpServer};
use actix_web_opentelemetry::{RequestMetrics, RequestTracing};
use srv_storage::init_db;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use tracing_actix_web::TracingLogger;

mod handlers;

mod errors;
mod shutdown;
use shutdown::shutdown;
use srv_tracing::init_logging;
use std::time::Duration;

pub async fn init_api() -> std::io::Result<()> {
    dotenvy::dotenv().expect(".env file not found");
    let port: u16 = std::env::var("PORT")
        .expect("Expected PORT to be set")
        .parse()
        .expect("Expected PORT to be a number");
    let addr = format!("0.0.0.0:{}", port);
    let guard = init_logging("app".to_string(), "debug".to_string());

    debug!(target: "init", "Initializing database...");
    let database_url = std::env::var("DATABASE_URL").expect("Expected DATABASE_URL to be set");
    #[cfg(feature = "sync")]
    let pool = init_db(database_url.as_str());

    #[cfg(feature = "async")]
    let pool = init_db(database_url.as_str()).await;
    debug!(target: "init", "Database connected.");

    let srv: actix_web::dev::Server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .wrap(RequestTracing::new())
            .wrap(RequestMetrics::default())
            .wrap(TracingLogger::default())
            .service(handlers::health::get_health)
            .service(handlers::signatures::add_signature)
            .service(handlers::signatures::query_signature)
            .service(handlers::tokens::add_tokens)
            .service(handlers::tokens::query_token)
    })
    .disable_signals()
    .bind(addr)?
    .run();

    let srv_handle = srv.handle();
    let server_task = tokio::spawn(srv);

    let shutdown_handle = shutdown(async move {
        srv_handle.stop(true).await;
        drop(guard);
        let (tx, rx) = oneshot::channel();
        tokio::task::spawn_blocking(|| {
            debug!("shutting down the tracer provider.");
            opentelemetry::global::shutdown_tracer_provider();
            debug!("shutdown the tracer provider.");
            let _ = tx.send(());
        })
        .await
        .expect("shutdown tracer provider failed.");

        // Wrap the future with a `Timeout` set to expire in 10 seconds.
        if tokio::time::timeout(Duration::from_secs(10), rx).await.is_err() {
            warn!("timed out while shutting down tracing, exiting anyway");
        };
    });

    let shutdown_task = tokio::spawn(shutdown_handle);
    let _ = tokio::try_join!(server_task, shutdown_task).expect("unable to join tasks");

    Ok(())
}

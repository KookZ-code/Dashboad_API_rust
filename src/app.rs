use axum::{
    http::{HeaderValue, Method},
    middleware,
    routing::get,
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::{
    config::Config,
    db::{DbPool, MssqlPool},
    handlers::{
        da, docs::{api_docs, openapi_json}, downtime, health::health_check, inventory, items,
        master, overview, tech, utilization, wb, AppState,
    },
    middleware::api_key::require_api_key,
};

pub fn create_app(sqlite: DbPool, mssql: MssqlPool, config: Config) -> Router {
    let state = AppState::new(sqlite, mssql, config.clone());
    let cors  = build_cors(&config.frontend_origin, config.is_production());

    // Apply API key middleware only to authenticated routes
    let auth_routes = authenticated_routes()
        .layer(middleware::from_fn_with_state(state.config.clone(), require_api_key));

    Router::new()
        .route("/docs", get(api_docs))              // no auth
        .route("/openapi.json", get(openapi_json))  // no auth
        .route("/api/v1/health", get(health_check)) // no auth
        .nest("/api/v1", auth_routes)
        .with_state(state)
        .layer(CompressionLayer::new())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

fn authenticated_routes() -> Router<AppState> {
    Router::new()
        // ─── Master ───────────────────────────────────────────────────────────
        .route("/areas",           get(master::get_areas))
        .route("/machines",        get(master::get_machines))
        .route("/machines/detail", get(master::get_machine_detail))
        .route("/machines/records",get(master::get_machine_records))
        // ─── Overview ─────────────────────────────────────────────────────────
        .route("/overview",          get(overview::get_overview))
        .route("/overview/open-jobs",get(overview::get_open_jobs))
        // ─── Utilization ──────────────────────────────────────────────────────
        .route("/utilization/detail",    get(utilization::get_detail))
        .route("/utilization/by-machine",get(utilization::get_by_machine))
        .route("/utilization/attention", get(utilization::get_attention))
        // ─── Downtime ─────────────────────────────────────────────────────────
        .route("/downtime/detail",  get(downtime::get_detail))
        .route("/downtime/machines",get(downtime::get_machines))
        .route("/downtime/events",  get(downtime::get_events))
        // ─── Inventory ────────────────────────────────────────────────────────
        .route("/inventory/machines",get(inventory::get_machines))
        .route("/inventory/downtime",get(inventory::get_downtime))
        // ─── Tech ─────────────────────────────────────────────────────────────
        .route("/tech/metrics",get(tech::get_metrics))
        .route("/tech/list",   get(tech::get_list))
        // ─── WB ───────────────────────────────────────────────────────────────
        .route("/wb/packages",get(wb::get_packages))
        .route("/wb/report",  get(wb::get_report))
        // ─── DA ───────────────────────────────────────────────────────────────
        .route("/da/packages",get(da::get_packages))
        .route("/da/report",  get(da::get_report))
        // ─── Items CRUD (SQLite) ───────────────────────────────────────────────
        .route("/items",     get(items::list_items).post(items::create_item))
        .route("/items/{id}",get(items::get_item).put(items::update_item).delete(items::delete_item))
}

fn build_cors(frontend_origin: &str, is_production: bool) -> CorsLayer {
    let origin = frontend_origin
        .parse::<HeaderValue>()
        .expect("Invalid FRONTEND_ORIGIN in config");
    let base = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);
    if is_production { base.allow_origin(origin) } else { base.allow_origin(Any) }
}

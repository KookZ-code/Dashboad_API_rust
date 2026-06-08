# CLAUDE.md — Rust + Axum Professional Backend

Behavioral + project-specific guidelines for AI coding agents.

---

## Behavioral Guidelines

### 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them — don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

### 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code (no traits/generics for one impl).
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior Rust engineer say this is overcomplicated?" If yes, simplify.

### 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it — don't delete it.

When your changes create orphans:
- Remove imports/functions/types that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

Every changed line should trace directly to the user's request.

### 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure `cargo test` passes before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Always verify with `cargo check`, `cargo clippy`, and `cargo test` before declaring done.

### 5. Module-First Thinking

**Before writing code, ask: should this be its own module?**

- If logic runs more than once → extract into a function or module — don't copy-paste.
- If a helper already exists in `src/` → use it, don't recreate it.
- If SQL exists in `repositories/` → reuse the query function, don't inline new SQL.
- Prefer composing small, named functions over large `match` arms or nested closures.
- Minimum code: if a one-liner solves it, don't write ten lines.

### 6. No Hardcoding

**All environment-specific values belong in `.env` and the typed `Config` struct.**

- Never hardcode URLs, ports, DB connection strings, secrets, or feature flags in source code.
- Read from `Config::from_env()` only — never call `std::env::var()` directly inside handlers/repositories.
- Add every new env variable to `.env.example` with a placeholder value AND to the `Config` struct with proper typing.
- If a value might change between dev / staging / prod → it must be an env var.

### 7. Requirements Discovery First

**Before starting any non-trivial task, align on requirements.**

- Ask **ONE question at a time** until you fully understand the requirements. Don't batch multiple questions.
- Once requirements are clear, write a design summary (200–300 words) and ask: *"Does this match what you want?"* Wait for approval before writing code.
- Every significant decision or plan must be written as a Markdown file:
  - `/docs/plan/` — implementation plans, feature breakdowns
  - `/docs/discussion/` — tradeoff analyses, architecture decisions, rejected alternatives

### 8. Comment Complex Functions

**Long or non-obvious functions must explain their intent.**

- If a function is longer than ~30 lines or contains non-obvious logic, add a `///` doc comment explaining: *what it does*, *why it exists*, and *any tricky edge cases* (lock ordering, retry policy, atomicity guarantees).
- Inline comments for logic that isn't self-evident from variable/function names.
- Don't comment obvious code (`// increment counter` above `count += 1` is noise).
- Document `unsafe` blocks **always** — explain the invariant the caller must uphold.

### 9. Use Existing Agents First

**Before creating new tooling, check what agents already exist.**

- Review the available agent list before building custom logic: `architect`, `planner`, `tdd-guide`, `security-reviewer`, `database-reviewer`, `rust-reviewer`, `rust-build-resolver`.
- Match the agent to the task: planning → `planner`, build/clippy errors → `rust-build-resolver`, SQL/migration review → `database-reviewer`.
- Only create new agents/tools if no existing agent covers the need.

### 10. Layered Architecture Discipline

**Every line of code lives in exactly one layer. Zero exceptions.**

The backend follows a strict 4-layer architecture. Mixing layers makes the code untestable and brittle.

- **Handlers** (`src/handlers/`) — HTTP boundary only. Validate input, call repositories, shape response. **No SQL. No business rules.**
- **Repositories** (`src/repositories/`) — Data access only. Compile-time-checked SQL via `sqlx::query!`. **No HTTP types. No business logic.**
- **Models** (`src/models/`) — Pure data structs that map to DB rows + request/response DTOs. **No methods that hit the DB.**
- **Errors** (`src/errors/`) — Single `AppError` enum that implements `IntoResponse`. **All errors flow through here — never return `String` errors directly.**

If a handler imports `sqlx::*` directly, it's wrong. If a repository imports `axum::*`, it's wrong. Stop and refactor.

---

## Project Overview

Professional Rust backend for HTTP APIs. Target stack:

- **Rust** — stable channel, edition 2021
- **Axum 0.8** — HTTP router + middleware
- **Tokio** — async runtime (`#[tokio::main]`)
- **SQLx 0.8** — async SQL with **compile-time checked queries** (`query!`, `query_as!`, `query_scalar!`)
- **SQLite** for dev / **PostgreSQL** for production (swap via `DATABASE_URL` + `DbPool` type alias)
- **tracing** + `tracing-subscriber` — structured logging (NEVER `println!`)
- **thiserror** + **anyhow** — error handling (`thiserror` for typed `AppError`, `anyhow` for top-level `main`)
- **dotenvy** — `.env` loading at startup only
- **validator** — request input validation via derive macros
- **chrono** + **uuid** — timestamps and IDs
- Deployed on **Windows Server** as a **Windows Service** via NSSM, fronted by IIS reverse proxy.

---

## Architecture

> **Before every session**, scan the actual filesystem to get the current structure.
> Do **not** rely on a static tree here — the project evolves and the file below may be stale.
> Use the file-search / list-dir tools to load the real layout before editing.

Expected module roles (may grow over time):

| Path | Purpose |
|---|---|
| `src/main.rs` | Bootstrap **only** — load config, init pool, run migrations, start server. **No business logic.** |
| `src/app.rs` | Router setup + middleware stack. Single `create_app(pool, config) -> Router`. |
| `src/config/mod.rs` | `Config` struct + `Config::from_env() -> Result<Config>`. Single source of typed env access. |
| `src/db/mod.rs` | `DbPool` type alias + `create_pool()` + `run_migrations()`. |
| `src/errors/mod.rs` | `AppError` enum + `IntoResponse` impl + `AppResult<T>` type alias. |
| `src/middleware/mod.rs` | CORS, request-id, tracing layer builders. **No domain logic.** |
| `src/models/mod.rs` | Domain structs: `Item`, `CreateItemRequest`, `UpdateItemRequest`, `PaginationParams`. |
| `src/handlers/` | Per-resource handler modules. Thin adapters HTTP ↔ repository. |
| `src/repositories/` | Per-resource SQL modules. Compile-time-checked queries only. |
| `migrations/` | Timestamped `.sql` files run by `sqlx::migrate!()` (embedded into binary at compile time). |

### Request flow

```
HTTP Request
  → Middleware (CORS, RequestId, Trace)
  → Router (axum::Router)
  → Handler (validate input, call repo)
  → Repository (SQL query via sqlx)
  → Database
  ↓
HTTP Response (via IntoResponse on AppError or success type)
```

`AppState { pool: DbPool, config: Config }` is the only state passed through `with_state` — extract via `State(state): State<AppState>` in handlers.

---

## Database Discipline

### Compile-time checked queries — always

```rust
// ✅ Right — query! / query_as! / query_scalar! verify the SQL at compile time
let items = sqlx::query_as!(Item,
    "SELECT id, name, created_at FROM items WHERE id = $1",
    item_id
).fetch_all(&pool).await?;
```

```rust
// ❌ Wrong — sqlx::query() (no bang) is a runtime-only string, no schema check
sqlx::query("SELECT * FROM items").execute(&pool).await?;
```

The DB file (`dev.db` for SQLite) MUST exist at compile time so the macros can connect — run `sqlx database create && sqlx migrate run` before `cargo build` on a fresh checkout.

### SQL rules

- **Never `SELECT *`** — list every column explicitly. Schema drift breaks queries silently otherwise.
- **Never hard `DELETE`** — use `UPDATE ... SET deleted_at = NOW()` (soft delete + audit trail).
- **Always parameterize** — `$1`, `$2`, ... never string-format values into SQL.
- **One transaction per multi-step write** — use `pool.begin() -> tx -> tx.commit()`.
- **Index columns used in `WHERE` / `ORDER BY` / `JOIN`** — add to a migration when adding new query patterns.

### Migrations

- One file per change: `YYYYMMDDHHMMSS_description.sql`.
- Idempotent: use `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`.
- Reversible if possible: include a `-- down` comment block describing the rollback.
- After adding a migration, run `sqlx migrate run` then `cargo check` to refresh the macro cache.

---

## Error Handling

- All fallible operations return `Result<T, AppError>`.
- Convert library errors via `#[from]` on `AppError` variants — never `.unwrap()` or `.expect()` outside of `main.rs` startup.
- `AppError` implements `IntoResponse` — Axum will render the JSON response automatically.
- Internal error details (SQL errors, panics) are **never** sent to the client — log them via `tracing::error!` and return a generic message.

```rust
// ✅ Right
pub async fn get_item(...) -> Result<Json<ItemDto>, AppError> {
    let item = repo::find_by_id(&pool, &id).await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(item.into()))
}
```

```rust
// ❌ Wrong — .unwrap() panics, returns 500 with stack trace, leaks internals
pub async fn get_item(...) -> Json<ItemDto> {
    let item = repo::find_by_id(&pool, &id).await.unwrap().unwrap();
    Json(item.into())
}
```

---

## API Response Contract

Every endpoint returns one of two shapes — backend and frontend rely on this contract.

```json
// Success
{ "data": <T>, "error": null }

// Failure
{ "data": null, "error": { "code": "ERROR_CODE", "message": "Human-readable" } }
```

Return `204 No Content` for successful deletes (no body).

---

## Deployment (Windows Service + IIS)

- Build release binary: `cargo build --release --target x86_64-pc-windows-msvc`.
- Migrations are **embedded at compile time** via `sqlx::migrate!("./migrations")` — no need to copy the `migrations/` folder to the server.
- Register `backend.exe` as a Windows Service via `deploy/install-service.ps1` (uses NSSM).
- Service binds to `127.0.0.1:PORT` (loopback only — IIS reverse-proxies external traffic).
- IIS forwards `/<base>/api/*` → `127.0.0.1:PORT/api/*` via URL Rewrite + ARR.
- `.env` file lives next to `backend.exe` in the working directory — `dotenvy::dotenv()` loads it at startup.

```env
# .env (production example)
DATABASE_URL=sqlite://C:/services/backend/prod.db?mode=rwc
PORT=8080
ENVIRONMENT=production
FRONTEND_ORIGIN=https://yourdomain.com
RUST_LOG=warn
```

CORS `FRONTEND_ORIGIN` must match the public URL exactly (scheme + host, no path).

---

## Testing

- **Unit tests** — inline `#[cfg(test)] mod tests` blocks in each module for pure functions and small helpers.
- **Integration tests** — `tests/` directory at the crate root. Use `axum-test` to drive the actual `Router`. Hit a real SQLite test DB (in-memory: `sqlite::memory:`) — never mock the DB.
- **Migration tests** — every PR that adds a migration should include an integration test that exercises the new schema.

```rust
#[tokio::test]
async fn create_then_get_item() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let server = TestServer::new(app::create_app(pool, test_config())).unwrap();
    let response = server.post("/api/items")
        .json(&json!({"name": "test"}))
        .await;
    response.assert_status_ok();
}
```

Always run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` before committing.

---

## Do Not

- Do not use `unwrap()` or `expect()` outside of `main.rs` startup — use `?` or `.map_err()`.
- Do not call `std::env::var()` outside `Config::from_env()` — env access has one entry point.
- Do not write `SELECT *` — list every column explicitly.
- Do not hard-`DELETE` rows — soft-delete with `deleted_at`.
- Do not put SQL in handlers — SQL lives in `repositories/` only.
- Do not put HTTP types (`StatusCode`, `Json`, `Response`) in repositories or models — they belong in handlers/errors.
- Do not use `println!` / `eprintln!` — use `tracing::info!` / `error!` / `warn!` / `debug!`.
- Do not open a connection by hand — use the `DbPool` from `AppState`.
- Do not use `sqlx::query()` (string-only) — use `query!` / `query_as!` macros for compile-time checking.
- Do not commit `.env` — only `.env.example`.
- Do not commit the `target/` directory — it's in `.gitignore`.
- Do not commit `dev.db` or any local DB file — gitignore it.
- Do not use `unsafe` without a `// SAFETY: ...` comment explaining the invariant.
- Do not return `String` errors from public functions — use the typed `AppError` enum.
- Do not bypass `cargo clippy` warnings — fix them or `#[allow(...)]` with a comment explaining why.
- Do not introduce a new dependency without justifying it — keep `Cargo.toml` lean.
- Do not assume the project structure is what CLAUDE.md shows — always scan the filesystem first.
- Do not write inline logic that could be a named, reusable function.
- Do not add caching layers unless there is a measured bottleneck and the invalidation strategy is agreed upfront.

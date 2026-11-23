#![allow(dead_code)]
pub mod library;
pub mod mix_keys;

use aether_api::AppState;
use aether_db::PgPool;
use aether_db::users::{self, Registration, User};
use axum_test::TestServer;
use uuid::Uuid;

pub const SECRET: &str = "test-secret";

pub fn state(pool: PgPool) -> AppState {
    AppState::new(pool, SECRET)
}

pub fn server(pool: PgPool) -> TestServer {
    TestServer::new(aether_api::router(state(pool))).unwrap()
}

/// A user created straight in the DB (password "password1"), plus an access token for them.
pub async fn user_with_token(state: &AppState, name: &str) -> (User, String) {
    sqlx::query("UPDATE system_settings SET registration_open = true")
        .execute(&state.pool)
        .await
        .unwrap();
    let hash = aether_core::auth::hash_password("password1");
    let Registration::Created(user) = users::register(&state.pool, Uuid::now_v7(), name, &hash)
        .await
        .unwrap()
    else {
        panic!("registration failed")
    };
    let token = state.jwt.issue_access(
        user.account(),
        user.main_profile(),
        &user.username,
        &user.role,
        user.token_version,
    );
    (user, token)
}

#[allow(unused_imports)]
pub use aether_test_support::require_ffmpeg;

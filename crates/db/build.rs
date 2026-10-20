//! Rebuild when a migration is added or changed: `sqlx::migrate!` embeds
//! `migrations/` at compile time, and cargo does not track that directory
//! on its own.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

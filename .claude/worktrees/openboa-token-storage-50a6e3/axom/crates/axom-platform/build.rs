// Rebuild when platform migrations change (`sqlx::migrate!` embeds them).
fn main() {
    println!("cargo:rerun-if-changed=../../../supabase/platform_migrations");
}

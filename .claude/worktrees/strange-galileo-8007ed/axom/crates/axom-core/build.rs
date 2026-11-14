// Rebuild when migrations change: `sqlx::migrate!` embeds them at compile
// time, and Cargo does not otherwise notice new files in that directory.
fn main() {
    println!("cargo:rerun-if-changed=../../../supabase/migrations");
}

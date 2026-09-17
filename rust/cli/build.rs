#[path = "../build_support/native_revision.rs"]
mod native_revision;

fn main() {
    native_revision::emit();
}

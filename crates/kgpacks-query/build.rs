// Build script for `kgpacks-query`.
//
// The retrieval read path loads LadybugDB's `vector` and `fts` extensions at
// query time (`Connection::load_extension`). Per the `lbug` crate docs, a binary
// (including test/example/bench targets) that loads an extension must be linked
// with `-rdynamic`, otherwise the dynamically-loaded extension fails to resolve
// the engine symbols it links against ("undefined symbol" at LOAD time). The M2
// `kgpacks-db` tests deliberately avoided the extensions, so this is the first
// crate in the workspace that needs it.
//
// `cargo:rustc-link-arg` applies the flag to every binary target built for this
// crate (its integration-test binaries in particular), which is exactly the set
// that exercises extension loading.
fn main() {
    println!("cargo:rustc-link-arg=-rdynamic");
}

// Build script for `kgpacks-cli`.
//
// The `query` and `ask` commands load LadybugDB's `vector` / `fts` extensions at
// query time (`Connection::load_extension`, via `kgpacks-query`'s retriever).
// Per the `lbug` crate — and mirroring `kgpacks-query`'s build.rs — a binary
// (including integration-test binaries) that loads an extension must be linked
// with `-rdynamic`, otherwise the dynamically-loaded extension cannot resolve
// the engine symbols it references and loading fails at runtime with an
// "undefined symbol" error. This flag applies to the `kgpacks` binary and the
// crate's test binaries, which are exactly the targets that load extensions.
fn main() {
    println!("cargo:rustc-link-arg=-rdynamic");
}

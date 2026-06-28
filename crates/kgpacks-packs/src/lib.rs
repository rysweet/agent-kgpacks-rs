//! `kgpacks-packs` — knowledge-pack manifests, registry and install.
//!
//! Rust port of `@kgpacks/packs`. The M1 scaffold models pack identity; the
//! registry, tarball install and version resolution land across M2-M5.

/// Metadata describing an installable knowledge pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    /// Pack name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
}

impl PackManifest {
    /// Build a manifest from a name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }

    /// Canonical `name@version` identifier.
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_pack_id() {
        let m = PackManifest::new("cve", "1.0.0");
        assert_eq!(m.id(), "cve@1.0.0");
    }
}

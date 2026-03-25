use std::collections::HashMap;
use std::path::Path;

/// Information about a dependency extracted from Cargo.toml.
#[derive(Debug, Clone)]
pub struct DepMeta {
    /// The local alias for this dependency (the key in [dependencies.X]).
    /// If package renaming is used, this differs from the crate name.
    pub local_name: String,
    /// The actual package name on crates.io.
    pub package_name: String,
    /// Whether this dependency is declared `optional = true`.
    pub optional: bool,
    /// Whether this dependency is platform-conditional (under [target.'cfg(...)'.dependencies]).
    pub platform_conditional: bool,
    /// Whether this is a build-dependency.
    pub build_dep: bool,
}

/// All dependency metadata extracted from a crate's Cargo.toml.
#[derive(Debug, Clone)]
pub struct CrateDepInfo {
    /// Map from package_name -> DepMeta for each dependency.
    pub deps: HashMap<String, DepMeta>,
}

impl CrateDepInfo {
    pub fn empty() -> Self {
        Self {
            deps: HashMap::new(),
        }
    }

    /// Parse a Cargo.toml file and extract dependency metadata.
    pub fn from_manifest(manifest_path: &Path) -> Self {
        let content = match std::fs::read_to_string(manifest_path) {
            Ok(c) => c,
            Err(_) => return Self::empty(),
        };

        // Use basic toml parsing via serde_json roundtrip won't work.
        // We'll parse with a simple line-based approach for the key fields we need.
        Self::parse_toml_content(&content)
    }

    fn parse_toml_content(content: &str) -> Self {
        let mut deps = HashMap::new();

        // Track which section we're in.
        let mut current_section = Section::None;
        let mut current_dep_name: Option<String> = None;
        let mut current_package: Option<String> = None;
        let mut current_optional = false;
        let mut is_build = false;
        let mut is_platform = false;

        let flush = |deps: &mut HashMap<String, DepMeta>,
                     dep_name: &Option<String>,
                     pkg: &Option<String>,
                     optional: bool,
                     build: bool,
                     platform: bool| {
            if let Some(local_name) = dep_name {
                let package_name = pkg
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| local_name.clone());
                deps.insert(
                    package_name.clone(),
                    DepMeta {
                        local_name: local_name.clone(),
                        package_name,
                        optional,
                        platform_conditional: platform,
                        build_dep: build,
                    },
                );
            }
        };

        for line in content.lines() {
            let trimmed = line.trim();

            // Detect section headers.
            if trimmed.starts_with('[') {
                // Flush previous dep if any.
                flush(
                    &mut deps,
                    &current_dep_name,
                    &current_package,
                    current_optional,
                    is_build,
                    is_platform,
                );
                current_dep_name = None;
                current_package = None;
                current_optional = false;

                let header = trimmed.trim_matches(|c| c == '[' || c == ']').trim();

                if header == "dependencies" {
                    current_section = Section::Dependencies;
                    is_build = false;
                    is_platform = false;
                } else if header == "build-dependencies" {
                    current_section = Section::Dependencies;
                    is_build = true;
                    is_platform = false;
                } else if header.starts_with("dependencies.") {
                    current_section = Section::SpecificDep;
                    is_build = false;
                    is_platform = false;
                    current_dep_name = Some(
                        header
                            .strip_prefix("dependencies.")
                            .unwrap_or("")
                            .to_string(),
                    );
                } else if header.starts_with("build-dependencies.") {
                    current_section = Section::SpecificDep;
                    is_build = true;
                    is_platform = false;
                    current_dep_name = Some(
                        header
                            .strip_prefix("build-dependencies.")
                            .unwrap_or("")
                            .to_string(),
                    );
                } else if header.starts_with("target.") || header.starts_with("target.'") {
                    is_platform = true;
                    if header.contains("dependencies.") {
                        is_build = header.contains("build-dependencies.");
                        current_section = Section::SpecificDep;
                        // Extract the dep name after the last "dependencies."
                        if let Some(pos) = header.rfind("dependencies.") {
                            let after = &header[pos + "dependencies.".len()..];
                            current_dep_name = Some(after.to_string());
                        }
                    } else if header.ends_with("dependencies") {
                        is_build = header.contains("build-dependencies");
                        current_section = Section::Dependencies;
                    } else {
                        current_section = Section::None;
                    }
                } else {
                    current_section = Section::None;
                }
                continue;
            }

            // Skip comments and empty lines.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            match current_section {
                Section::Dependencies => {
                    // Lines like: serde = "1.0" or serde = { version = "1.0", ... }
                    if let Some((key, val)) = trimmed.split_once('=') {
                        let key = key.trim().trim_matches('"');
                        let val = val.trim();

                        let mut pkg_name = None;
                        let mut opt = false;

                        if val.starts_with('{') {
                            // Inline table — extract package and optional.
                            if let Some(p) = extract_field(val, "package") {
                                pkg_name = Some(p);
                            }
                            if extract_field(val, "optional")
                                .is_some_and(|v| v == "true")
                            {
                                opt = true;
                            }
                        }

                        let package_name =
                            pkg_name.unwrap_or_else(|| key.to_string());
                        deps.insert(
                            package_name.clone(),
                            DepMeta {
                                local_name: key.to_string(),
                                package_name,
                                optional: opt,
                                platform_conditional: is_platform,
                                build_dep: is_build,
                            },
                        );
                    }
                }
                Section::SpecificDep => {
                    // Lines within [dependencies.X] — look for package = "..." and optional = true.
                    if let Some((key, val)) = trimmed.split_once('=') {
                        let key = key.trim();
                        let val = val.trim().trim_matches('"');

                        if key == "package" {
                            current_package = Some(val.to_string());
                        } else if key == "optional" && val == "true" {
                            current_optional = true;
                        }
                    }
                }
                Section::None => {}
            }
        }

        // Flush last dep.
        flush(
            &mut deps,
            &current_dep_name,
            &current_package,
            current_optional,
            is_build,
            is_platform,
        );

        Self { deps }
    }

    /// Look up the local alias for a given package name.
    /// Returns the local_name if renamed, otherwise the normalized package name.
    pub fn local_alias(&self, package_name: &str) -> Option<String> {
        self.deps.get(package_name).map(|d| d.local_name.clone())
    }

    /// Check if a dependency is optional.
    pub fn is_optional(&self, package_name: &str) -> bool {
        self.deps
            .get(package_name)
            .is_some_and(|d| d.optional)
    }

    /// Check if a dependency is platform-conditional.
    pub fn is_platform_conditional(&self, package_name: &str) -> bool {
        self.deps
            .get(package_name)
            .is_some_and(|d| d.platform_conditional)
    }

    /// Check if a dependency is a build-dependency.
    pub fn is_build_dep(&self, package_name: &str) -> bool {
        self.deps
            .get(package_name)
            .is_some_and(|d| d.build_dep)
    }
}

#[derive(Debug, Clone, Copy)]
enum Section {
    None,
    Dependencies,
    SpecificDep,
}

/// Extract a field value from an inline TOML table string like `{ version = "1.0", package = "foo" }`.
fn extract_field(table_str: &str, field: &str) -> Option<String> {
    let pattern = format!("{field}");
    // Find "field = value" within the string.
    for part in table_str.split(',') {
        let part = part.trim().trim_matches(|c| c == '{' || c == '}');
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == pattern {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_rename() {
        let toml = r#"
[package]
name = "pango"
version = "0.18.3"

[dependencies.ffi]
version = "0.18"
package = "pango-sys"
"#;
        let info = CrateDepInfo::parse_toml_content(toml);
        assert_eq!(info.local_alias("pango-sys"), Some("ffi".to_string()));
    }

    #[test]
    fn test_parse_optional() {
        let toml = r#"
[dependencies]
serde = { version = "1.0", optional = true }
regex = "1.0"
"#;
        let info = CrateDepInfo::parse_toml_content(toml);
        assert!(info.is_optional("serde"));
        assert!(!info.is_optional("regex"));
    }

    #[test]
    fn test_parse_platform_conditional() {
        let toml = r#"
[target.'cfg(target_os = "linux")'.dependencies.wgpu-hal]
version = "28.0"
"#;
        let info = CrateDepInfo::parse_toml_content(toml);
        assert!(info.is_platform_conditional("wgpu-hal"));
    }

    #[test]
    fn test_parse_build_deps() {
        let toml = r#"
[build-dependencies]
serde_json = { version = "1.0", features = ["preserve_order"] }
"#;
        let info = CrateDepInfo::parse_toml_content(toml);
        assert!(info.is_build_dep("serde_json"));
    }

    #[test]
    fn test_inline_rename() {
        let toml = r#"
[dependencies]
webpki = { version = "0.103", package = "rustls-webpki" }
"#;
        let info = CrateDepInfo::parse_toml_content(toml);
        assert_eq!(
            info.local_alias("rustls-webpki"),
            Some("webpki".to_string())
        );
    }
}

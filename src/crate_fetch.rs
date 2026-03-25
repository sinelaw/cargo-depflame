use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Parsed crate specification from user input.
#[derive(Debug)]
pub struct CrateSpec {
    pub name: String,
    pub version: Option<String>,
}

/// Parse a crate spec from a string. Accepts:
/// - `crate_name` (latest version)
/// - `crate_name@1.2.3` (exact version)
/// - `https://crates.io/crates/crate_name` (latest version from URL)
/// - `https://crates.io/crates/crate_name/1.2.3` (exact version from URL)
pub fn parse_crate_spec(input: &str) -> Result<CrateSpec> {
    let input = input.trim().trim_end_matches('/');

    // Handle crates.io URLs.
    if input.starts_with("https://crates.io/crates/")
        || input.starts_with("http://crates.io/crates/")
    {
        let path = input
            .split("/crates/")
            .nth(1)
            .context("invalid crates.io URL")?;
        let parts: Vec<&str> = path.splitn(2, '/').collect();
        let name = parts[0].to_string();
        if name.is_empty() {
            bail!("empty crate name in URL");
        }
        let version = parts
            .get(1)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        return Ok(CrateSpec { name, version });
    }

    // Handle name@version.
    if let Some((name, version)) = input.split_once('@') {
        let name = name.trim().to_string();
        let version = version.trim().to_string();
        if name.is_empty() {
            bail!("empty crate name");
        }
        if version.is_empty() {
            bail!("empty version after @");
        }
        return Ok(CrateSpec {
            name,
            version: Some(version),
        });
    }

    // Plain crate name.
    if input.is_empty() {
        bail!("empty crate name");
    }
    Ok(CrateSpec {
        name: input.to_string(),
        version: None,
    })
}

/// Resolve the latest version of a crate from the crates.io API.
fn resolve_latest_version(name: &str) -> Result<String> {
    let url = format!("https://crates.io/api/v1/crates/{name}");
    let resp = ureq::get(&url)
        .set(
            "User-Agent",
            "cargo-depflame (https://github.com/sinelaw/cargo-depflame)",
        )
        .call()
        .with_context(|| format!("failed to fetch crate info for '{name}' from crates.io"))?;
    let body = resp
        .into_string()
        .context("failed to read crates.io API response")?;
    let response: serde_json::Value =
        serde_json::from_str(&body).context("failed to parse crates.io API response")?;

    let version = response["crate"]["max_stable_version"]
        .as_str()
        .or_else(|| response["crate"]["max_version"].as_str())
        .or_else(|| response["crate"]["newest_version"].as_str())
        .context("could not determine latest version from crates.io response")?;

    Ok(version.to_string())
}

/// Fetch a crate from crates.io and extract it into `dest_dir`.
/// Returns the path to the extracted crate root (containing Cargo.toml).
pub fn fetch_and_extract(spec: &CrateSpec, dest_dir: &Path) -> Result<PathBuf> {
    let version = match &spec.version {
        Some(v) => v.clone(),
        None => {
            eprintln!("Resolving latest version of '{}'...", spec.name);
            resolve_latest_version(&spec.name)?
        }
    };

    eprintln!("Downloading {} v{} from crates.io...", spec.name, version);

    let download_url = format!(
        "https://crates.io/api/v1/crates/{}/{}/download",
        spec.name, version
    );

    let response = ureq::get(&download_url)
        .set(
            "User-Agent",
            "cargo-depflame (https://github.com/sinelaw/cargo-depflame)",
        )
        .call()
        .with_context(|| format!("failed to download {} v{}", spec.name, version))?;

    // Read the response body (gzipped tarball).
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .context("failed to read download response")?;

    // Extract the tarball.
    let gz = flate2::read::GzDecoder::new(body.as_slice());
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest_dir)
        .context("failed to extract crate tarball")?;

    // The tarball extracts to a directory named `{name}-{version}`.
    let crate_dir = dest_dir.join(format!("{}-{}", spec.name, version));
    if !crate_dir.is_dir() {
        bail!(
            "expected extracted directory '{}' not found",
            crate_dir.display()
        );
    }

    eprintln!(
        "Extracted {} v{} to {}",
        spec.name,
        version,
        crate_dir.display()
    );
    Ok(crate_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_name() {
        let spec = parse_crate_spec("serde").unwrap();
        assert_eq!(spec.name, "serde");
        assert!(spec.version.is_none());
    }

    #[test]
    fn test_parse_name_at_version() {
        let spec = parse_crate_spec("serde@1.0.228").unwrap();
        assert_eq!(spec.name, "serde");
        assert_eq!(spec.version.as_deref(), Some("1.0.228"));
    }

    #[test]
    fn test_parse_crates_io_url() {
        let spec = parse_crate_spec("https://crates.io/crates/tokio").unwrap();
        assert_eq!(spec.name, "tokio");
        assert!(spec.version.is_none());
    }

    #[test]
    fn test_parse_crates_io_url_with_version() {
        let spec = parse_crate_spec("https://crates.io/crates/tokio/1.40.0").unwrap();
        assert_eq!(spec.name, "tokio");
        assert_eq!(spec.version.as_deref(), Some("1.40.0"));
    }

    #[test]
    fn test_parse_trailing_slash() {
        let spec = parse_crate_spec("https://crates.io/crates/tokio/").unwrap();
        assert_eq!(spec.name, "tokio");
        assert!(spec.version.is_none());
    }

    #[test]
    fn test_parse_empty_name_fails() {
        assert!(parse_crate_spec("").is_err());
    }

    #[test]
    fn test_parse_empty_version_fails() {
        assert!(parse_crate_spec("serde@").is_err());
    }
}

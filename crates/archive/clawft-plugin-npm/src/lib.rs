//! Node.js dependency parsing plugin for WeftOS.
//!
//! Parses `package.json` and `package-lock.json` files to extract dependency
//! information, detect outdated packages, and flag known vulnerable versions.

use serde::Deserialize;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parsed representation of a `package.json` file.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub dependencies: HashMap<String, String>,
    pub dev_dependencies: HashMap<String, String>,
    pub scripts: HashMap<String, String>,
}

/// Parsed representation of a `package-lock.json` file.
#[derive(Debug, Clone, PartialEq)]
pub struct LockInfo {
    pub name: String,
    pub version: String,
    pub packages: Vec<ResolvedPackage>,
}

/// A single resolved package entry from the lock file.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub resolved: Option<String>,
    pub dev: bool,
}

/// A package that may be outdated.
#[derive(Debug, Clone, PartialEq)]
pub struct OutdatedPackage {
    pub name: String,
    pub current: String,
    pub wanted: String,
}

/// A known vulnerability match.
#[derive(Debug, Clone, PartialEq)]
pub struct VulnInfo {
    pub package: String,
    pub version: String,
    pub severity: String,
    pub advisory: String,
}

// ---------------------------------------------------------------------------
// Internal serde helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawPackageJson {
    name: Option<String>,
    version: Option<String>,
    dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Option<HashMap<String, String>>,
    scripts: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct RawPackageLock {
    name: Option<String>,
    version: Option<String>,
    packages: Option<HashMap<String, RawLockEntry>>,
}

#[derive(Deserialize)]
struct RawLockEntry {
    version: Option<String>,
    resolved: Option<String>,
    dev: Option<bool>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a `package.json` string and extract project metadata.
pub fn parse_package_json(content: &str) -> Result<PackageInfo, String> {
    let raw: RawPackageJson =
        serde_json::from_str(content).map_err(|e| format!("invalid package.json: {e}"))?;

    Ok(PackageInfo {
        name: raw.name.unwrap_or_default(),
        version: raw.version.unwrap_or_default(),
        dependencies: raw.dependencies.unwrap_or_default(),
        dev_dependencies: raw.dev_dependencies.unwrap_or_default(),
        scripts: raw.scripts.unwrap_or_default(),
    })
}

/// Parse a `package-lock.json` string and extract all resolved packages.
pub fn parse_package_lock(content: &str) -> Result<LockInfo, String> {
    let raw: RawPackageLock =
        serde_json::from_str(content).map_err(|e| format!("invalid package-lock.json: {e}"))?;

    let mut packages = Vec::new();

    if let Some(map) = raw.packages {
        for (key, entry) in map {
            // The root entry has key "" -- skip it.
            if key.is_empty() {
                continue;
            }
            // Keys look like "node_modules/foo" or "node_modules/foo/node_modules/bar".
            let name = key
                .rsplit_once("node_modules/")
                .map(|(_, n)| n.to_string())
                .unwrap_or(key);

            packages.push(ResolvedPackage {
                name,
                version: entry.version.unwrap_or_default(),
                resolved: entry.resolved,
                dev: entry.dev.unwrap_or(false),
            });
        }
    }

    // Sort for deterministic output.
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(LockInfo {
        name: raw.name.unwrap_or_default(),
        version: raw.version.unwrap_or_default(),
        packages,
    })
}

/// List packages that may be outdated.
///
/// This is a stub implementation that simply lists every resolved package as
/// potentially outdated (wanted == current) since we have no registry to
/// compare against.
pub fn find_outdated(lock: &LockInfo) -> Vec<OutdatedPackage> {
    lock.packages
        .iter()
        .map(|p| OutdatedPackage {
            name: p.name.clone(),
            current: p.version.clone(),
            wanted: p.version.clone(), // stub: no registry lookup
        })
        .collect()
}

/// Detect known vulnerable package versions.
///
/// Stub implementation that pattern-matches a small set of well-known
/// vulnerable version ranges.
pub fn detect_vulnerabilities(lock: &LockInfo) -> Vec<VulnInfo> {
    // A handful of real-world advisories for demonstration purposes.
    let known_bad: &[(&str, &str, &str, &str)] = &[
        (
            "lodash",
            "4.17.20",
            "high",
            "CVE-2021-23337: command injection via template",
        ),
        (
            "minimist",
            "1.2.5",
            "critical",
            "CVE-2021-44906: prototype pollution",
        ),
        (
            "node-fetch",
            "2.6.1",
            "medium",
            "CVE-2022-0235: exposure of sensitive information",
        ),
        (
            "tar",
            "6.1.0",
            "high",
            "CVE-2021-37701: arbitrary file creation/overwrite",
        ),
    ];

    let mut vulns = Vec::new();
    for pkg in &lock.packages {
        for &(name, version, severity, advisory) in known_bad {
            if pkg.name == name && pkg.version == version {
                vulns.push(VulnInfo {
                    package: name.to_string(),
                    version: version.to_string(),
                    severity: severity.to_string(),
                    advisory: advisory.to_string(),
                });
            }
        }
    }
    vulns
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE_JSON: &str = r#"{
        "name": "my-app",
        "version": "1.0.0",
        "dependencies": {
            "express": "^4.18.0",
            "lodash": "^4.17.21"
        },
        "devDependencies": {
            "jest": "^29.0.0"
        },
        "scripts": {
            "start": "node index.js",
            "test": "jest"
        }
    }"#;

    const PACKAGE_LOCK: &str = r#"{
        "name": "my-app",
        "version": "1.0.0",
        "lockfileVersion": 3,
        "packages": {
            "": {
                "name": "my-app",
                "version": "1.0.0"
            },
            "node_modules/express": {
                "version": "4.18.2",
                "resolved": "https://registry.npmjs.org/express/-/express-4.18.2.tgz"
            },
            "node_modules/lodash": {
                "version": "4.17.20",
                "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.20.tgz"
            },
            "node_modules/jest": {
                "version": "29.7.0",
                "resolved": "https://registry.npmjs.org/jest/-/jest-29.7.0.tgz",
                "dev": true
            }
        }
    }"#;

    #[test]
    fn test_parse_package_json() {
        let info = parse_package_json(PACKAGE_JSON).unwrap();
        assert_eq!(info.name, "my-app");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.dependencies.len(), 2);
        assert_eq!(info.dependencies.get("express").unwrap(), "^4.18.0");
        assert_eq!(info.dev_dependencies.len(), 1);
        assert_eq!(info.dev_dependencies.get("jest").unwrap(), "^29.0.0");
        assert_eq!(info.scripts.len(), 2);
        assert_eq!(info.scripts.get("test").unwrap(), "jest");
    }

    #[test]
    fn test_parse_package_json_invalid() {
        assert!(parse_package_json("not json").is_err());
    }

    #[test]
    fn test_parse_package_lock() {
        let lock = parse_package_lock(PACKAGE_LOCK).unwrap();
        assert_eq!(lock.name, "my-app");
        assert_eq!(lock.version, "1.0.0");
        assert_eq!(lock.packages.len(), 3);

        let express = lock.packages.iter().find(|p| p.name == "express").unwrap();
        assert_eq!(express.version, "4.18.2");
        assert!(!express.dev);
        assert!(express.resolved.is_some());

        let jest = lock.packages.iter().find(|p| p.name == "jest").unwrap();
        assert!(jest.dev);
    }

    #[test]
    fn test_find_outdated() {
        let lock = parse_package_lock(PACKAGE_LOCK).unwrap();
        let outdated = find_outdated(&lock);
        assert_eq!(outdated.len(), 3);
        // Stub: wanted == current for all packages.
        for pkg in &outdated {
            assert_eq!(pkg.current, pkg.wanted);
        }
    }

    #[test]
    fn test_detect_vulnerabilities() {
        let lock = parse_package_lock(PACKAGE_LOCK).unwrap();
        let vulns = detect_vulnerabilities(&lock);
        assert_eq!(vulns.len(), 1);
        assert_eq!(vulns[0].package, "lodash");
        assert_eq!(vulns[0].version, "4.17.20");
        assert_eq!(vulns[0].severity, "high");
    }
}

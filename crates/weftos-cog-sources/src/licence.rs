//! Cognitum licence entitlement check (ADR-105 section 3).
//!
//! A licence is a declared record kept with the project: it names the
//! source, the cogs it covers, an account label and an optional expiry. The
//! check is presence, coverage and expiry. It does not verify payment and
//! it is not a cryptographic proof: Cognitum's 30% per cog is contractual
//! (ADR-100 section 6.4), so the code only stops an unlicensed install by
//! mistake and records which licence allowed an install.

use chrono::{DateTime, NaiveDate, Utc};

use crate::config::{CogLicence, LicensedCogs};
use crate::error::{Result, SourceError};

/// Parse an expiry: `YYYY-MM-DD` (valid through the end of that UTC day) or RFC 3339.
pub fn parse_expiry(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let end = d.and_hms_opt(23, 59, 59).expect("valid time");
        return Ok(DateTime::from_naive_utc_and_offset(end, Utc));
    }
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| SourceError::Config(format!("bad licence expiry {s:?} (use YYYY-MM-DD or RFC 3339)")))
}

impl CogLicence {
    /// True when this licence names `source` and covers `cog`.
    pub fn covers(&self, source: &str, cog: &str) -> bool {
        self.source == source
            && match &self.cogs {
                LicensedCogs::All(_) => true,
                LicensedCogs::List(v) => v.iter().any(|c| c == cog),
            }
    }
}

/// The licence that allows `cog` from `source` at `now`.
///
/// `Err(Unlicensed)` when no licence covers the cog; `Err(LicenceExpired)`
/// when a covering licence exists but every covering licence has expired.
pub fn entitlement<'a>(
    licences: &'a [CogLicence],
    source: &str,
    cog: &str,
    now: DateTime<Utc>,
) -> Result<&'a CogLicence> {
    let mut expired: Option<&CogLicence> = None;
    for l in licences.iter().filter(|l| l.covers(source, cog)) {
        match l.expires.as_deref().map(parse_expiry).transpose()? {
            Some(t) if t < now => expired = Some(l),
            _ => return Ok(l),
        }
    }
    match expired {
        Some(l) => Err(SourceError::LicenceExpired {
            source_name: source.into(),
            cog: cog.into(),
            expires: l.expires.clone().unwrap_or_default(),
        }),
        None => Err(SourceError::Unlicensed { source_name: source.into(), cog: cog.into() }),
    }
}

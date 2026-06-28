//! Security audit checks, hardening, and monitors for clawft.
//!
//! Provides the `weft security scan` engine with 50+ audit checks across
//! 10 categories:
//!
//! | Category | Check Count | Priority |
//! |----------|-------------|----------|
//! | Prompt Injection | 8+ | P0 |
//! | Exfiltration URL Detection | 5+ | P0 |
//! | Credential Literal Detection | 5+ | P0 |
//! | Permission Escalation | 5+ | P1 |
//! | Unsafe Shell Commands | 5+ | P1 |
//! | Supply Chain Risk | 5+ | P1 |
//! | DoS Patterns | 5+ | P2 |
//! | Indirect Prompt Injection | 5+ | P2 |
//! | Information Disclosure | 3+ | P2 |
//! | Cross-Agent Access Violations | 3+ | P2 |

pub mod checks;

pub use checks::{
    AuditCategory, AuditCheck, AuditFinding, AuditReport, AuditSeverity, SecurityScanner,
};

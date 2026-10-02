//! [`VerifiedProject`]: a project id whose provenance is cryptographic
//! (ADR-103 A6, Phase 2 packages A and I).
//!
//! Distinct from [`ClaimedProject`](crate::rpc_ext::ClaimedProject), which is
//! whatever the client wrote in `Request.project`. There is no conversion
//! between them and no public constructor: the field is private to this
//! module and the three `pub(crate)` builders are the only ways in.
//!
//! | Source | Builder | Why it is verified |
//! |---|---|---|
//! | the kernel's own bound project | [`VerifiedProject::from_bound`] | the socket you reached is the project's own |
//! | a validated token scoped to a project | [`VerifiedProject::from_token`] | the token authority checked the secret |
//! | a user-daemon-forwarded request | [`VerifiedProject::from_verified_forward`] | caller already checked the user-signed forward header |
//!
//! Honest limit: this is only as strong as the transport. A same-uid local
//! process can still lie in `Request.project`; the cryptographic sources are
//! the three above. It is an isolation guard between one user's projects,
//! not a boundary against a hostile local process (Phase 3 peer
//! credentials, Phase 4 sandboxes).
//!
//! A `ClaimedProject` cannot become a `VerifiedProject`:
//!
//! ```compile_fail
//! use clawft_weave::rpc_ext::ClaimedProject;
//! use clawft_weave::verified_project::VerifiedProject;
//! let claimed = ClaimedProject::from("01JB8Z3Q0V6X9KQ4M2N7T5R1WD");
//! let _: VerifiedProject = claimed.into();
//! ```

use clawft_kernel::governance::AttestedProject;
use clawft_kernel::token_authority::TokenInfo;

use crate::handshake_rpc::BoundProject;

/// A verified project id. See the module docs for the three sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProject(String);

impl VerifiedProject {
    /// The kernel's own bound project, if it has one. A child kernel's
    /// handshake `project_id` is verified by construction.
    pub(crate) fn from_bound(bound: &BoundProject) -> Option<Self> {
        bound.project_id.clone().map(Self)
    }

    /// The project a validated token is scoped to, if any. Call only with a
    /// `TokenInfo` the token authority returned for a secret it checked.
    pub(crate) fn from_token(info: &TokenInfo) -> Option<Self> {
        info.project.clone().map(Self)
    }

    /// A project id from a user-daemon forward header. Call only after the
    /// header's user signature, 5 s window and single use were checked
    /// (package I).
    pub(crate) fn from_verified_forward(project_id: String) -> Self {
        Self(project_id)
    }

    /// The verified project id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AttestedProject for VerifiedProject {
    fn project_id(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use chrono::Utc;
    use clawft_kernel::governance::GatePrincipal;
    use clawft_kernel::token_authority::TokenScope;

    use super::*;
    use crate::handshake_rpc::BoundProject;
    use crate::rpc_ext::ClaimedProject;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    // Compiles only while `T` does NOT implement `From<Src>`: with the impl
    // the call below would be ambiguous.
    trait AmbiguousIfFrom<A> {
        fn check() {}
    }
    impl<T: ?Sized, Src> AmbiguousIfFrom<(Src, ())> for PhantomData<(T, Src)> {}
    impl<T: ?Sized + From<Src>, Src> AmbiguousIfFrom<(Src, u8)> for PhantomData<(T, Src)> {}

    #[test]
    fn no_conversion_from_claimed_or_raw_strings() {
        PhantomData::<(VerifiedProject, ClaimedProject)>::check();
        PhantomData::<(VerifiedProject, String)>::check();
        PhantomData::<(VerifiedProject, &str)>::check();
    }

    #[test]
    fn builders_cover_the_three_sources_only() {
        let bound = BoundProject {
            project_id: Some(ID.into()),
            ..Default::default()
        };
        assert_eq!(VerifiedProject::from_bound(&bound).unwrap().as_str(), ID);
        assert!(VerifiedProject::from_bound(&BoundProject::default()).is_none());

        let mut info = TokenInfo {
            id: "t".into(),
            label: "l".into(),
            issued_at: Utc::now(),
            expires_at: Utc::now(),
            scope: TokenScope::Owner,
            project: Some(ID.into()),
        };
        assert_eq!(VerifiedProject::from_token(&info).unwrap().as_str(), ID);
        info.project = None;
        assert!(VerifiedProject::from_token(&info).is_none());

        assert_eq!(
            VerifiedProject::from_verified_forward(ID.into()).as_str(),
            ID
        );
    }

    #[test]
    fn stamps_a_principal_only_through_the_attested_trait() {
        let v = VerifiedProject::from_verified_forward(ID.into());
        let p = GatePrincipal::agent("a").with_project(&v);
        assert_eq!(p.project_id.as_deref(), Some(ID));
    }
}

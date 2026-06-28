//! Permission engine -- K1 ACL scaffold (WEFT-130).
//!
//! ## Status
//!
//! WEFT-130 ships the **scaffold** of the K1 ACL engine: a typed
//! `CapabilityChecker`, an `EffectiveAclCache` for repeated lookups,
//! and a tree-walk `check_permission` that honours explicit
//! grants / denies on a resource hierarchy. It deliberately stops
//! short of the full K1 design in
//! `.planning/sparc/weftos/13-exo-resource-tree.md` §5: there is no
//! `Did`/`exo_consent`/`BailmentPolicy` integration, no delegation
//! cert lifecycle, no risk-score evaluation, and no LRU eviction
//! beyond the simple "clear when full" pattern from the spec
//! (§5.2).
//!
//! The legacy free function `check()` (always-`Allow`) remains so
//! that existing K0 call sites compile unchanged. New code should
//! use [`CapabilityChecker`] and switch over once the K1 follow-up
//! lands the missing pieces.
//!
//! ## K1 follow-up
//!
//! See the 0.8.x Plane backlog for the deferred work:
//! - `Did`-based principals + ConsentProof emission
//! - DelegationCert grant/revoke/prune
//! - exo_consent::evaluate integration with risk scoring
//! - LRU eviction in EffectiveAclCache (currently bulk-clear)
//! - CLI: `weaver resource {grant, revoke, check}`
//!
//! ## Default policy
//!
//! `CapabilityChecker::new()` builds a checker whose root policy is
//! **deny-all**. Callers explicitly grant subtrees with
//! [`CapabilityChecker::grant`]. This matches the K1 design intent
//! ("Permission checks walk up the tree collecting policies")
//! without back-dooring the old always-`Allow` behaviour.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{Action, ResourceId, Role};

// ── Decision (back-compat with K0) ─────────────────────────────────

/// Permission decision returned by the legacy free function
/// [`check`] and the new [`CapabilityChecker::check_permission`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny,
    Delegate,
}

/// Check whether an agent with the given role may perform an action on a resource.
///
/// **K0 stub**: Always returns `Allow`. Preserved so existing K0
/// call sites compile unchanged. New code must use
/// [`CapabilityChecker::check_permission`] instead.
pub fn check(_agent_id: &str, _role: &Role, _action: &Action, _resource: &ResourceId) -> Decision {
    Decision::Allow
}

// ── Principal ──────────────────────────────────────────────────────

/// Identity of an actor making a permission request.
///
/// In the full K1 design this is a [`Did`](https://www.w3.org/TR/did-core/)
/// resolved through `exo_identity`. The scaffold uses opaque
/// strings so the engine can be wired up before the DID stack lands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Principal(pub String);

impl Principal {
    /// Construct from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The principal's identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Principal {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── ACL policy ─────────────────────────────────────────────────────

/// A single ACL entry attached to a resource.
///
/// Policies are evaluated from the leaf up. The first explicit
/// grant or deny that matches the principal/role for the requested
/// action wins. Children inherit unless overridden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclPolicy {
    /// Who this policy applies to. `None` matches any principal
    /// (used for role-based grants).
    pub principal: Option<Principal>,
    /// Required role. `None` matches any role.
    pub role: Option<Role>,
    /// Action this policy gates.
    pub action: Action,
    /// Whether to allow or deny.
    pub effect: Effect,
}

/// Effect of an ACL policy match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Allow,
    Deny,
}

impl AclPolicy {
    /// Convenience constructor for an Allow policy.
    pub fn allow(principal: Principal, action: Action) -> Self {
        Self {
            principal: Some(principal),
            role: None,
            action,
            effect: Effect::Allow,
        }
    }

    /// Convenience constructor for a Deny policy.
    pub fn deny(principal: Principal, action: Action) -> Self {
        Self {
            principal: Some(principal),
            role: None,
            action,
            effect: Effect::Deny,
        }
    }

    /// Whether this policy matches the given (principal, role, action).
    fn matches(&self, principal: &Principal, role: &Role, action: &Action) -> bool {
        if &self.action != action {
            return false;
        }
        if let Some(ref p) = self.principal
            && p != principal
        {
            return false;
        }
        if let Some(ref r) = self.role
            && r != role
        {
            return false;
        }
        true
    }
}

// ── EffectiveAclCache ──────────────────────────────────────────────

/// LRU-style cache of evaluated permission decisions.
///
/// Per the K1 spec (§5.2) the cache is keyed by
/// `(principal, resource_id, action)`. The scaffold uses the
/// "clear when full" eviction strategy from the spec (true LRU is
/// a follow-up). Hits and misses are tracked for tuning.
#[derive(Debug)]
pub struct EffectiveAclCache {
    entries: Mutex<HashMap<CacheKey, Decision>>,
    max_size: usize,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    principal: Principal,
    resource: ResourceId,
    action: Action,
}

impl EffectiveAclCache {
    /// Build a new cache with the given capacity.
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_size: max_size.max(1),
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Look up a cached decision.
    pub fn get(
        &self,
        principal: &Principal,
        resource: &ResourceId,
        action: &Action,
    ) -> Option<Decision> {
        let key = CacheKey {
            principal: principal.clone(),
            resource: resource.clone(),
            action: action.clone(),
        };
        let entries = self.entries.lock().ok()?;
        if let Some(d) = entries.get(&key) {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(d.clone())
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        }
    }

    /// Insert a decision. If the cache is full, the entire table is
    /// cleared (per spec §5.2 K1 scaffold). True LRU is a follow-up.
    pub fn put(
        &self,
        principal: Principal,
        resource: ResourceId,
        action: Action,
        decision: Decision,
    ) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= self.max_size {
            entries.clear();
        }
        entries.insert(
            CacheKey {
                principal,
                resource,
                action,
            },
            decision,
        );
    }

    /// Drop every entry whose resource matches `resource`.
    /// Invoked when a node's policies change.
    pub fn invalidate_resource(&self, resource: &ResourceId) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|k, _| &k.resource != resource);
        }
    }

    /// Drop every entry for `principal`. Invoked when delegation
    /// changes touch a principal (revoke / expiry sweep).
    pub fn invalidate_principal(&self, principal: &Principal) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|k, _| &k.principal != principal);
        }
    }

    /// Drop every entry. Invoked from bulk policy reload.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    /// Cumulative hit count since construction.
    pub fn hits(&self) -> u64 {
        self.hits.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Cumulative miss count since construction.
    pub fn misses(&self) -> u64 {
        self.misses.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for EffectiveAclCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

// ── CapabilityChecker ──────────────────────────────────────────────

/// Tree-walking permission checker (WEFT-130 scaffold).
///
/// The checker stores a per-resource policy list and walks from the
/// requested resource up to root, evaluating policies most-specific
/// first. The first explicit Allow or Deny that matches the
/// principal+action wins. With no matching policy on any node, the
/// **default-deny** root policy applies.
///
/// This is intentionally minimal: it does not yet hook into the
/// resource tree's `nodes` map (nodes carry no `rbac_policies` field
/// in K0), and it maps roles only when callers supply them
/// explicitly. Wiring it into `ResourceTree` and `CapabilityChecker`
/// in `clawft-kernel` is the next K1 step.
#[derive(Debug)]
pub struct CapabilityChecker {
    /// Per-resource ACLs. Missing entry = no policy at that level.
    policies: Mutex<HashMap<ResourceId, Vec<AclPolicy>>>,
    /// Decision cache for repeated lookups.
    cache: EffectiveAclCache,
}

impl CapabilityChecker {
    /// Build a checker whose default policy is **deny-all** at root.
    pub fn new() -> Self {
        Self {
            policies: Mutex::new(HashMap::new()),
            cache: EffectiveAclCache::new(1024),
        }
    }

    /// Direct access to the underlying decision cache (test helper /
    /// metrics path).
    pub fn cache(&self) -> &EffectiveAclCache {
        &self.cache
    }

    /// Attach a policy to a resource.
    pub fn add_policy(&self, resource: ResourceId, policy: AclPolicy) {
        if let Ok(mut policies) = self.policies.lock() {
            policies.entry(resource.clone()).or_default().push(policy);
        }
        // A new policy may flip a cached decision -- invalidate.
        self.cache.invalidate_resource(&resource);
    }

    /// Convenience: grant `action` on `resource` to `principal`.
    pub fn grant(&self, resource: ResourceId, principal: Principal, action: Action) {
        self.add_policy(resource, AclPolicy::allow(principal, action));
    }

    /// Convenience: deny `action` on `resource` for `principal`.
    pub fn deny(&self, resource: ResourceId, principal: Principal, action: Action) {
        self.add_policy(resource, AclPolicy::deny(principal, action));
    }

    /// Evaluate whether `principal` (with `role`) may perform
    /// `action` on `resource`.
    ///
    /// Walk: starts at `resource`, climbs to root, returning on the
    /// first matching policy. If no node has a matching policy the
    /// root **default-deny** wins.
    pub fn check_permission(
        &self,
        principal: &Principal,
        role: &Role,
        action: &Action,
        resource: &ResourceId,
    ) -> Decision {
        // Cache hit?
        if let Some(d) = self.cache.get(principal, resource, action) {
            return d;
        }

        let decision = self.evaluate_uncached(principal, role, action, resource);
        self.cache.put(
            principal.clone(),
            resource.clone(),
            action.clone(),
            decision.clone(),
        );
        decision
    }

    fn evaluate_uncached(
        &self,
        principal: &Principal,
        role: &Role,
        action: &Action,
        resource: &ResourceId,
    ) -> Decision {
        let policies = match self.policies.lock() {
            Ok(p) => p,
            Err(_) => return Decision::Deny, // poisoned -- fail closed
        };

        let mut current = Some(resource.clone());
        while let Some(rid) = current {
            if let Some(node_policies) = policies.get(&rid) {
                for policy in node_policies {
                    if policy.matches(principal, role, action) {
                        return match policy.effect {
                            Effect::Allow => Decision::Allow,
                            Effect::Deny => Decision::Deny,
                        };
                    }
                }
            }
            current = rid.parent();
        }

        // Default-deny root.
        Decision::Deny
    }
}

impl Default for CapabilityChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k0_stub_always_allows() {
        let decision = check(
            "agent-001",
            &Role::Viewer,
            &Action::Admin,
            &ResourceId::new("/kernel"),
        );
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn decision_serde_roundtrip() {
        for decision in [Decision::Allow, Decision::Deny, Decision::Delegate] {
            let json = serde_json::to_string(&decision).unwrap();
            let back: Decision = serde_json::from_str(&json).unwrap();
            assert_eq!(back, decision);
        }
    }

    // ── CapabilityChecker scaffold tests (WEFT-130) ───────────────

    /// With no policies, the root default-deny applies to every
    /// principal/action.
    #[test]
    fn root_deny_default() {
        let checker = CapabilityChecker::new();
        let decision = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(decision, Decision::Deny);
    }

    /// An explicit grant on a leaf permits that principal/action
    /// while leaving others denied.
    #[test]
    fn principal_grant_allows() {
        let checker = CapabilityChecker::new();
        checker.grant(
            ResourceId::new("/apps/secret"),
            Principal::new("alice"),
            Action::Read,
        );

        let allow = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(allow, Decision::Allow);

        // Different principal still denied.
        let deny = checker.check_permission(
            &Principal::new("bob"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(deny, Decision::Deny);

        // Different action still denied.
        let deny_write = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Write,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(deny_write, Decision::Deny);
    }

    /// A grant on a parent flows to children via tree walk.
    #[test]
    fn parent_inherit_grants() {
        let checker = CapabilityChecker::new();
        checker.grant(
            ResourceId::new("/apps"),
            Principal::new("alice"),
            Action::Read,
        );

        // Child resource inherits the grant.
        let inherited = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(inherited, Decision::Allow);
    }

    /// An explicit deny on a child overrides an inherited allow
    /// from a parent (most-specific-first walk).
    #[test]
    fn explicit_deny_overrides_inherit() {
        let checker = CapabilityChecker::new();
        checker.grant(
            ResourceId::new("/apps"),
            Principal::new("alice"),
            Action::Read,
        );
        checker.deny(
            ResourceId::new("/apps/secret"),
            Principal::new("alice"),
            Action::Read,
        );

        let decision = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/secret"),
        );
        assert_eq!(decision, Decision::Deny);

        // Sibling under /apps still inherits the grant.
        let sibling = checker.check_permission(
            &Principal::new("alice"),
            &Role::Viewer,
            &Action::Read,
            &ResourceId::new("/apps/public"),
        );
        assert_eq!(sibling, Decision::Allow);
    }

    /// Cache: repeated lookups must hit the cache rather than
    /// re-walking the tree.
    #[test]
    fn cache_records_hits_and_misses() {
        let checker = CapabilityChecker::new();
        checker.grant(
            ResourceId::new("/svc"),
            Principal::new("alice"),
            Action::Execute,
        );

        let p = Principal::new("alice");
        let r = ResourceId::new("/svc");
        let a = Action::Execute;

        // First call: miss + populate.
        assert_eq!(
            checker.check_permission(&p, &Role::Viewer, &a, &r),
            Decision::Allow
        );
        // Second call: hit.
        assert_eq!(
            checker.check_permission(&p, &Role::Viewer, &a, &r),
            Decision::Allow
        );

        assert!(
            checker.cache().hits() >= 1,
            "expected at least one cache hit, got {}",
            checker.cache().hits()
        );
        assert!(
            checker.cache().misses() >= 1,
            "expected at least one cache miss, got {}",
            checker.cache().misses()
        );
    }

    /// Adding a new policy must invalidate the cached decision for
    /// that resource so the next lookup sees the updated rules.
    #[test]
    fn add_policy_invalidates_cache() {
        let checker = CapabilityChecker::new();
        let p = Principal::new("alice");
        let r = ResourceId::new("/svc");

        // Initially deny (no policy).
        assert_eq!(
            checker.check_permission(&p, &Role::Viewer, &Action::Read, &r),
            Decision::Deny
        );

        // Grant after the deny was cached.
        checker.grant(r.clone(), p.clone(), Action::Read);

        // Cache must have been invalidated -- next call returns Allow.
        assert_eq!(
            checker.check_permission(&p, &Role::Viewer, &Action::Read, &r),
            Decision::Allow
        );
    }
}

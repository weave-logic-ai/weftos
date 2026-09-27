# Sansone board subscription proposal — 2026-09

## Decision needed

Sansone's OS Product Board remains the sole source of truth for its tasks and
goals. The `product-board-steward` is its sole row writer; project agents keep
development discussion in `docs/cards/SO-<n>.md` and do not add board comments.
The existing machine `GET /api/board/*` includes restricted tickets and is not
an acceptable source for a portfolio export.

The Sansone engagement lead must decide whether any board snapshot may leave
the Sansone boundary. `adr/ctoxos-021-machine-board-writes.md` leaves that C10
question open even for a key, status, title, and verdict snapshot to an
external store. Until it is resolved, the WeftOS dashboard subscription stays
unprovisioned and receives no Sansone IDs, titles, or goal records.

## Proposed contract for review

1. The engagement lead approves an explicit allowlist of source ticket IDs and
   goal slugs, plus each exportable title. An untagged item is not automatically
   public. The publisher rejects protected tags (`ctox`, `feesplit`,
   `directors-only`, `test-record`) and any ID missing from the source board.
2. The only candidate fields are item type, approved source ID, approved title,
   status, and a safe source link. No descriptions, comments, people, tags,
   references, fee split values, or linked document contents leave Sansone.
3. Sansone uses a client-owned, host-local `wfs_` credential for a read-only
   replace-all subscription scoped to its dashboard project. The dashboard
   stores the credential hash. The credential is never synced by Grokbot or
   committed to either repository.
4. A publisher uses a Sansone-approved bounded and paginated read path, builds
   the complete allowlisted snapshot, and refuses empty, partial, oversized,
   or newly ineligible results. It sends a snapshot only after the source read
   and the confidentiality checks succeed. The dashboard displays the source
   link and last-sync time; it never writes back to Sansone.
5. The engagement lead approves retention, revocation, and the first live
   snapshot before enrollment. Any later change to fields or eligibility goes
   through the same review.

## Build sequence after the decision

Record the lead's decision in Sansone's own ADR/card process. Implement the
approved export query and publisher in the Sansone repository, with a fixture
that includes protected and untagged sensitive titles. Verify that they cannot
be exported. Then create the scoped dashboard subscription, publish one
reviewed snapshot, and compare item count and IDs with the source allowlist.
Keep the Sansone subscription ticket on the WeftOS board open until that live
verification passes.

Source authority: Sansone `AGENTS.md`,
`adr/ctoxos-016-product-board-write-authority.md`,
`adr/ctoxos-021-machine-board-writes.md`,
`adr/ctoxos-004-c10-access-boundary.md`,
`adr/ctoxos-009-deliverable-format-and-confidentiality.md`,
`src/dashboard/lib/product-board/protected-tags.ts`, and `docs/board-api.md`.

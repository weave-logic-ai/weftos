# Sansone board subscription decision — 2026-09

## Decision

Sansone's OS Product Board remains the sole source of truth for its tasks and
goals. The `product-board-steward` is its sole row writer; project agents keep
development discussion in `docs/cards/SO-<n>.md` and do not add board comments.
The existing machine `GET /api/board/*` includes restricted tickets and caps
its list at 500, so it is not a complete source for a portfolio snapshot.

On 2026-09-27 the engagement lead explicitly authorized **all Sansone Product
Board tickets** for the WeftOS dashboard: “All tickets, it is our project we are
developing.” This resolves the export scope left open by
`adr/ctoxos-021-machine-board-writes.md` for this private WeftOS subscription.
The authorized fields are ticket number, title, status, and a link back to the
Sansone board. Descriptions, comments, people, tags, references, linked documents,
fee figures outside a ticket title, and goals are outside this snapshot.

## Publication contract

1. The complete ticket set is in scope, including cards with protected tags.
   The dashboard workspace is owner-only under Supabase row-level security; the
   subscription is a private development view, not a client-facing export.
2. The publisher projects only item type, source ticket number, title, status,
   and the Sansone Product Board deep link. No description, comment, person,
   tag, reference, or linked document contents leave Sansone.
3. Sansone uses a client-owned, host-local `wfs_` credential for a read-only
   replace-all subscription scoped to its dashboard project. The dashboard
   stores the credential hash. The credential is never synced by Grokbot or
   committed to either repository.
4. A publisher uses a bounded, paginated, read-only source transaction and
   refuses empty, partial, or oversized results. It sends one complete snapshot
   and checks the dashboard's returned count. The dashboard displays source links
   and last-sync time; it never writes back to Sansone.
5. Revoking the subscription prevents future updates; existing dashboard rows
   remain until explicitly removed. Any change to fields or destination access
   needs a new decision.

## Build and verification

Record this decision in Sansone's own docs. Implement the complete source query
and publisher in the Sansone repository, with tests proving no body, comment,
tag, or person field is transmitted and a capped read cannot replace a complete
snapshot. Compare the source and dashboard counts after first publication. Keep
the Sansone subscription ticket open until that live verification passes.

Source authority: Sansone `AGENTS.md`,
`adr/ctoxos-016-product-board-write-authority.md`,
`adr/ctoxos-021-machine-board-writes.md`,
`adr/ctoxos-004-c10-access-boundary.md`,
`adr/ctoxos-009-deliverable-format-and-confidentiality.md`,
`src/dashboard/lib/product-board/protected-tags.ts`, and `docs/board-api.md`.

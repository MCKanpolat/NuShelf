# Package ownership transfers

Transfer a locally published package from its current publisher profile to another existing profile. The transfer includes every version and future publication/management rights; package bytes, versions, authors, downloads and NuGet catalog history are preserved. Cached upstream packages cannot be transferred.

Both publisher keys need the explicit `package:transfer` permission and a prefix covering the package. Existing keys are not automatically granted this permission. Named-feed membership is also required; the recipient must be a publisher or feed administrator. A workspace administrator can request, accept or cancel on behalf of profiles.

1. Open the package page as its owner and choose **Request transfer** with the recipient's profile ID.
2. The recipient opens **Manage → Ownership transfers** in that feed and accepts the request.
3. Acceptance atomically switches the owner. Existing source-profile credentials immediately lose ownership rights; credentials are not moved to the new profile.

A request expires after seven days. Either participant or the workspace administrator can cancel. There is one pending request per package. Requests contain the expected current owner, and both request creation and acceptance share the publication lock; a stale owner or an already completed request returns 409. Source and recipient must differ. Scope, membership and current ownership are checked again when accepting.

## API

- `POST /api/packages/<id>/ownership` with `targetProfileId` and `expectedOwnerId`: create a request (201).
- `GET /api/ownership/transfers`: latest 200 requests involving the authenticated profile; administrators see all. Key prefixes filter results.
- `POST /api/ownership/transfers/<uuid>/accept`: accept (204).
- `POST /api/ownership/transfers/<uuid>/cancel`: cancel or decline (204).

Use the named-feed prefix for that feed's transfers. `package.ownership.requested`, `package.ownership.transferred` and `package.ownership.cancelled` audit events can be selected in webhook subscriptions. Failed attempts remain visible in the audit log with their HTTP result.

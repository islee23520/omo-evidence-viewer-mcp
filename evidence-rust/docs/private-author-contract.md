# Required authority-owned private author contract

This is a proposal, not an implemented endpoint or an accepted response field.
The existing private authorize request and service credential already identify the
current account, key/session, audience, method, route and current scopes. Preserve
that request and append an authority-owned optional `authorBinding` result only
after the current authorization succeeds:

```json
{
  "authorBinding": {
    "githubUserId": 123456,
    "githubHandle": "synthetic-author",
    "bindingVersion": 7
  }
}
```

An unlinked account returns `authorBinding: null`. The numeric ID and handle come
only from the current verified GitHub binding of the authorized user. Machines
resolve their current key owner's account; request bodies may not choose a user
or author. The authority checks binding issuer/subject against the same current
account identity used to authorize the request. A relink/unlink, disabled account,
revoked session/key, withdrawn grant or disabled service must be visible on the
very next request. No GitHub OAuth token or proof is returned. Existing public
PIN routes remain unchanged, and this field is never forwarded from a client.

Evidence needs a fresh author binding both before acceptance and immediately
before pointer commit. It records the immutable numeric ID and submission-time
handle in revision provenance, separately from submitted-by principal/key. Binding
changes never rewrite historical revisions. Missing, null, malformed or mismatched
binding denies new commits without replacing any healthy revision. Existing legacy
imports remain explicitly author-unknown; complete catalog activation is blocked
until evidence-backed attribution is resolved by the operator.

The authority owner must approve and implement this additive private response
contract with real PostgreSQL source tests for browser and key-owner resolution,
unlink/relink/revoke and incompatible subject denial. Evidence then adds exact
typed parsing and listening end-to-end author-ready commit coverage against that
source. A synthetic oracle that invents the field does not satisfy that gate.

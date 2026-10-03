# Required authority-owned private author contract

This additive response contract is implemented and independently reviewed in the
authority source at `6c5158a6e4118cf0a9cc6bb21fc5e738f0e628b0`, auth tree
`77459922ef4d0dc2914f29e2af0b87f859a6d8ae`. It is not present in the frozen
operating Task 11 image. Evidence consumes it only through private authorization.
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

The authority owner source tests cover browser and key-owner resolution,
unlink/relink/revoke and incompatible subject denial. Evidence's source-owned
listening E2E consumes that actual implementation, with exact typed parsing,
author-ready multipart commits and fresh pre-pointer rechecks. A synthetic oracle
that invents the private field does not satisfy this gate.

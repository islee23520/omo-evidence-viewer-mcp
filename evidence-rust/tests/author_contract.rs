use omo_evidence_storage::authority::Principal;
use serde_json::{Value, json};

#[test]
fn required_nullable_author_binding_is_exact_and_validated()
-> Result<(), Box<dyn std::error::Error>> {
    let base = json!({"user":{"id":"synthetic","isAdmin":false},"service":"evidence","expiresAt":1,"principalType":"browser","effectiveScopes":["evidence:upload"],"authorBinding":null});
    assert!(serde_json::from_value::<Principal>(base.clone()).is_ok());
    let mut missing = base.clone();
    missing.as_object_mut().map(|o| o.remove("authorBinding"));
    assert!(serde_json::from_value::<Principal>(missing).is_err());
    for author in [
        json!({}),
        json!({"githubUserId":0,"githubHandle":"synthetic","bindingVersion":1}),
        json!({"githubUserId":1,"githubHandle":"bad_handle","bindingVersion":1}),
        json!({"githubUserId":1,"githubHandle":"synthetic","bindingVersion":0}),
        json!({"githubUserId":1,"githubHandle":"synthetic","bindingVersion":1,"userId":"forged"}),
        json!({"githubUserId":"1","githubHandle":"synthetic","bindingVersion":1}),
    ] {
        let mut invalid = base.clone();
        invalid["authorBinding"] = author;
        assert!(serde_json::from_value::<Principal>(invalid).is_err());
    }
    let mut linked = base;
    linked["authorBinding"] =
        json!({"githubUserId":81001,"githubHandle":"synthetic","bindingVersion":1});
    let accepted: Principal = serde_json::from_value(linked.clone())?;
    let mut changed = linked;
    changed["authorBinding"]["bindingVersion"] = Value::from(2);
    let current = serde_json::from_value::<Principal>(changed);
    assert!(current.is_ok());
    if let Ok(current) = current {
        assert!(accepted.same_submission(&current).is_err());
    }
    Ok(())
}

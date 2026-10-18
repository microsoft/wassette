// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::PathBuf;

use policy::PolicyParser;

#[test]
fn example_policies_parse_and_validate() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut validated = 0;

    for entry in std::fs::read_dir(&examples).expect("failed to read examples directory") {
        let entry = entry.expect("failed to read examples directory entry");
        let policy_path = entry.path().join("policy.yaml");
        if !policy_path.is_file() {
            continue;
        }

        let policy = PolicyParser::parse_file(&policy_path)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error:#}", policy_path.display()));
        policy.validate().unwrap_or_else(|error| {
            panic!("failed to validate {}: {error:#}", policy_path.display())
        });
        validated += 1;
    }

    assert!(validated > 0, "expected at least one example policy");
}

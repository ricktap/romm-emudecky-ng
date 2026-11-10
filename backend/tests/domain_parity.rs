//! Deterministic Python-specification oracle captured during the Rust port.
//! Includes mixed region/revision/default/installed/bound choices and sibling edges.
use std::collections::{BTreeMap,BTreeSet};
use serde_json::{Value,json};
use romm_backend::domain::{sibling_group::compute_component_group_keys,sibling_resolution::{representative,canonical_name},sync_diff::collapse_sibling_groups};
#[test]
fn sibling_kernels_match_python_and_are_stable_under_fetch_order_reversal(){
    let cases:Vec<Value>=serde_json::from_str(include_str!("fixtures/sibling_parity.json")).unwrap();
    for (index,case) in cases.iter().enumerate(){
        let mut members=case["members"].as_array().unwrap().clone();
        let installed:BTreeSet<i64>=serde_json::from_value(case["installed"].clone()).unwrap();
        let registry:BTreeMap<i64,Value>=serde_json::from_value(case["registry"].clone()).unwrap();let bound=registry.keys().copied().collect();
        let preferred=case["preferred"].as_str().unwrap();
        assert_eq!(json!(representative(&members,&installed,&bound,preferred).unwrap()),case["representative"],"representative case {index}");
        assert_eq!(json!(canonical_name(&members,preferred).unwrap()),case["canonical"],"name case {index}");
        assert_eq!(json!(collapse_sibling_groups(&members,&registry,&installed,case["complete"].as_bool().unwrap(),preferred).unwrap()),case["collapsed"],"collapse case {index}");
        members.reverse();assert_eq!(json!(representative(&members,&installed,&bound,preferred).unwrap()),case["representative"],"reversed representative case {index}");assert_eq!(json!(canonical_name(&members,preferred).unwrap()),case["canonical"],"reversed name case {index}");
        let mut raw=case["raw"].as_array().unwrap().clone();let resident=serde_json::from_value(case["resident"].clone()).unwrap();
        assert_eq!(json!(compute_component_group_keys(&raw,&resident)),case["keys"],"component case {index}");raw.reverse();assert_eq!(json!(compute_component_group_keys(&raw,&resident)),case["keys"],"reversed component case {index}");
    }
}

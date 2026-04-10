//! Deterministic sample report generation for testing.
//!
//! The [`generate_sample_report`] function builds a hardcoded [`AnalysisReport`]
//! with representative data covering every [`RemovalStrategy`] variant, feature
//! data, dep tree edges, and all report sections. It has no filesystem or network
//! dependencies, making it suitable for both Rust and JavaScript test fixtures.

use std::collections::BTreeMap;

use crate::flamegraph::{DepTreeData, DepTreeEdge, DepTreeNode};
use crate::graph::EdgeMeta;
use crate::metrics::{Confidence, PackageInfo, RemovalStrategy, UpstreamTarget};
use crate::report::{AnalysisReport, DirectDepSummary, UnusedDirectDep};
use crate::scanner::{FileMatch, ScanResult};

/// Build a deterministic sample [`AnalysisReport`] for testing.
///
/// The report describes a fictitious workspace with two members (`my-app` and
/// `my-lib`) and a realistic dependency graph of ~15 nodes. It includes one
/// target for every [`RemovalStrategy`] variant so that both Rust and JS tests
/// can exercise all code paths.
pub fn generate_sample_report() -> AnalysisReport {
    AnalysisReport {
        tool_version: "0.1.0-sample".to_string(),
        timestamp: "epoch:1700000000".to_string(),
        workspace_root: "/sample/workspace".to_string(),
        threshold: 3.0,
        total_dependencies: 42,
        platform_dependencies: Some(38),
        phantom_dependencies: 4,
        heavy_nodes_found: 6,
        targets: sample_targets(),
        dep_tree: Some(sample_dep_tree()),
        unused_edges: vec![
            ("my-app".into(), "unused-dep".into()),
            ("my-lib".into(), "dead-dep".into()),
        ],
        unused_direct_deps: vec![UnusedDirectDep {
            from_crate: "my-app".into(),
            dep_name: "unused-dep".into(),
            dep_version: "0.3.0".into(),
            real_deps_saved: 5,
            is_test_example: false,
        }],
        direct_dep_summary: vec![
            DirectDepSummary {
                workspace_member: "my-app".into(),
                dep_name: "heavy-framework".into(),
                dep_version: "2.0.0".into(),
                unique_transitive_deps: 18,
                total_transitive_deps: 25,
                unique_ancestors: 1,
            },
            DirectDepSummary {
                workspace_member: "my-app".into(),
                dep_name: "serde".into(),
                dep_version: "1.0.200".into(),
                unique_transitive_deps: 3,
                total_transitive_deps: 5,
                unique_ancestors: 12,
            },
            DirectDepSummary {
                workspace_member: "my-lib".into(),
                dep_name: "regex".into(),
                dep_version: "1.10.0".into(),
                unique_transitive_deps: 2,
                total_transitive_deps: 8,
                unique_ancestors: 5,
            },
        ],
    }
}

fn sample_targets() -> Vec<UpstreamTarget> {
    vec![
        // 1. Remove — unused dep in workspace member
        UpstreamTarget {
            intermediate: pkg("my-app", "0.1.0"),
            heavy_dependency: pkg("unused-dep", "0.3.0"),
            w_transitive: 12,
            w_unique: 5,
            c_ref: 0,
            hurrs: None,
            confidence: Confidence::High,
            scan_result: empty_scan("unused-dep"),
            suggestion: RemovalStrategy::Remove,
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-app".into(), "unused-dep".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: true,
            is_standalone_integration: false,
            heavy_dep_loc: 800,
            heavy_dep_own_deps: 3,
            has_re_export_all: false,
        },
        // 2. FeatureGate — dep should be made optional
        UpstreamTarget {
            intermediate: pkg("my-app", "0.1.0"),
            heavy_dependency: pkg("heavy-framework", "2.0.0"),
            w_transitive: 25,
            w_unique: 18,
            c_ref: 3,
            hurrs: Some(8.3),
            confidence: Confidence::High,
            scan_result: scan_with_refs(
                "heavy-framework",
                3,
                vec![
                    file_match(
                        "/sample/workspace/src/main.rs",
                        10,
                        "use heavy_framework::App;",
                    ),
                    file_match(
                        "/sample/workspace/src/main.rs",
                        42,
                        "heavy_framework::init();",
                    ),
                    file_match(
                        "/sample/workspace/src/lib.rs",
                        5,
                        "use heavy_framework::Config;",
                    ),
                ],
            ),
            suggestion: RemovalStrategy::FeatureGate,
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-app".into(), "heavy-framework".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: true,
            is_standalone_integration: false,
            heavy_dep_loc: 15000,
            heavy_dep_own_deps: 12,
            has_re_export_all: false,
        },
        // 3. AlreadyGated — optional dep pulled in by default features
        UpstreamTarget {
            intermediate: pkg("serde", "1.0.200"),
            heavy_dependency: pkg("serde_derive", "1.0.200"),
            w_transitive: 5,
            w_unique: 3,
            c_ref: 8,
            hurrs: Some(0.6),
            confidence: Confidence::Medium,
            scan_result: scan_with_refs(
                "serde_derive",
                8,
                vec![file_match(
                    "/home/user/.cargo/registry/src/index.crates.io/serde-1.0.200/src/lib.rs",
                    15,
                    "#[derive(Serialize, Deserialize)]",
                )],
            ),
            suggestion: RemovalStrategy::AlreadyGated {
                detail: "already optional".into(),
                enabling_features: vec!["derive".into()],
                recommended_defaults: Some(vec!["std".into()]),
            },
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: true,
                platform_conditional: false,
            },
            dep_chain: vec!["my-app".into(), "serde".into(), "serde_derive".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: false,
            is_standalone_integration: false,
            heavy_dep_loc: 2500,
            heavy_dep_own_deps: 3,
            has_re_export_all: false,
        },
        // 4. ReplaceWithStd
        UpstreamTarget {
            intermediate: pkg("my-lib", "0.1.0"),
            heavy_dependency: pkg("once_cell", "1.19.0"),
            w_transitive: 1,
            w_unique: 1,
            c_ref: 2,
            hurrs: Some(0.5),
            confidence: Confidence::Medium,
            scan_result: scan_with_refs(
                "once_cell",
                2,
                vec![file_match(
                    "/sample/workspace/my-lib/src/lib.rs",
                    3,
                    "use once_cell::sync::Lazy;",
                )],
            ),
            suggestion: RemovalStrategy::ReplaceWithStd {
                suggestion: "std::sync::LazyLock (stable since 1.80)".into(),
            },
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-lib".into(), "once_cell".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: true,
            is_standalone_integration: false,
            heavy_dep_loc: 600,
            heavy_dep_own_deps: 0,
            has_re_export_all: false,
        },
        // 5. RequiredBySibling
        UpstreamTarget {
            intermediate: pkg("http-client", "3.0.0"),
            heavy_dependency: pkg("tokio", "1.35.0"),
            w_transitive: 20,
            w_unique: 0,
            c_ref: 15,
            hurrs: Some(1.3),
            confidence: Confidence::Noise,
            scan_result: scan_with_refs("tokio", 15, vec![]),
            suggestion: RemovalStrategy::RequiredBySibling {
                sibling: "hyper".into(),
            },
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-app".into(), "http-client".into(), "tokio".into()],
            required_by_sibling: Some("hyper".into()),
            phantom: false,
            intermediate_is_workspace_member: false,
            is_standalone_integration: false,
            heavy_dep_loc: 50000,
            heavy_dep_own_deps: 8,
            has_re_export_all: false,
        },
        // 6. InlineUpstream
        UpstreamTarget {
            intermediate: pkg("my-lib", "0.1.0"),
            heavy_dependency: pkg("tiny-helper", "0.2.0"),
            w_transitive: 4,
            w_unique: 2,
            c_ref: 1,
            hurrs: Some(4.0),
            confidence: Confidence::High,
            scan_result: scan_with_refs(
                "tiny-helper",
                1,
                vec![file_match(
                    "/sample/workspace/my-lib/src/util.rs",
                    7,
                    "use tiny_helper::slugify;",
                )],
            ),
            suggestion: RemovalStrategy::InlineUpstream {
                heavy_loc: 120,
                api_items_used: 1,
            },
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-lib".into(), "tiny-helper".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: true,
            is_standalone_integration: false,
            heavy_dep_loc: 120,
            heavy_dep_own_deps: 0,
            has_re_export_all: false,
        },
        // 7. MoveToDevDeps
        UpstreamTarget {
            intermediate: pkg("my-app", "0.1.0"),
            heavy_dependency: pkg("test-helpers", "1.0.0"),
            w_transitive: 8,
            w_unique: 4,
            c_ref: 3,
            hurrs: Some(2.7),
            confidence: Confidence::High,
            scan_result: ScanResult {
                heavy_crate_name: "test-helpers".into(),
                searched_names: vec!["test_helpers".into()],
                ref_count: 3,
                file_matches: vec![file_match(
                    "/sample/workspace/src/tests/integration.rs",
                    1,
                    "use test_helpers::setup;",
                )],
                files_with_matches: 1,
                generated_file_refs: 0,
                test_only_refs: 3,
                distinct_items: vec!["setup".into()],
                has_re_export_all: false,
            },
            suggestion: RemovalStrategy::MoveToDevDeps,
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec!["my-app".into(), "test-helpers".into()],
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: true,
            is_standalone_integration: false,
            heavy_dep_loc: 400,
            heavy_dep_own_deps: 2,
            has_re_export_all: false,
        },
    ]
}

/// Build a sample dependency tree with feature data and optional edges.
///
/// Tree structure (indices):
///   0: my-app (workspace)        children: [2, 3, 4, 8, 9, 15]
///   1: my-lib (workspace)        children: [5, 6, 10]
///   2: heavy-framework           children: [7, 11, 12]
///   3: serde                     children: [13]
///   4: unused-dep                children: [14]
///   5: regex                     children: []
///   6: tiny-helper               children: []
///   7: tokio                     children: []
///   8: http-client               children: [7]
///   9: test-helpers              children: []
///  10: once_cell                 children: []
///  11: heavy-sub-a               children: []
///  12: heavy-sub-b               children: []
///  13: serde_derive              children: []
///  14: dead-dep                  children: []
///  15: remote-lib                children: [16, 17]  (optional, not currently enabled)
///  16: remote-sub-a              children: []
///  17: remote-sub-b              children: []
fn sample_dep_tree() -> DepTreeData {
    let nodes = vec![
        // 0: my-app — "remote" feature gates remote-lib (not enabled by default)
        tree_node(
            "my-app",
            "0.1.0",
            42,
            true,
            vec![2, 3, 4, 8, 9, 15],
            vec!["default".into()],
            BTreeMap::from([
                ("default".into(), vec!["heavy-framework".into()]),
                ("remote".into(), vec!["dep:remote-lib".into()]),
            ]),
        ),
        // 1: my-lib
        tree_node(
            "my-lib",
            "0.1.0",
            15,
            true,
            vec![5, 6, 10],
            vec![],
            BTreeMap::new(),
        ),
        // 2: heavy-framework
        tree_node(
            "heavy-framework",
            "2.0.0",
            25,
            false,
            vec![7, 11, 12],
            vec!["default".into(), "async".into()],
            BTreeMap::from([
                ("default".into(), vec!["async".into()]),
                ("async".into(), vec!["dep:tokio".into()]),
            ]),
        ),
        // 3: serde
        tree_node(
            "serde",
            "1.0.200",
            5,
            false,
            vec![13],
            vec!["default".into(), "std".into(), "derive".into()],
            BTreeMap::from([
                ("default".into(), vec!["std".into()]),
                ("derive".into(), vec!["dep:serde_derive".into()]),
                ("std".into(), vec![]),
            ]),
        ),
        // 4: unused-dep
        tree_node(
            "unused-dep",
            "0.3.0",
            12,
            false,
            vec![14],
            vec![],
            BTreeMap::new(),
        ),
        // 5: regex
        tree_node(
            "regex",
            "1.10.0",
            8,
            false,
            vec![],
            vec!["default".into(), "std".into(), "unicode".into()],
            BTreeMap::from([
                ("default".into(), vec!["std".into(), "unicode".into()]),
                ("std".into(), vec![]),
                ("unicode".into(), vec![]),
            ]),
        ),
        // 6: tiny-helper
        tree_node(
            "tiny-helper",
            "0.2.0",
            1,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 7: tokio (shared: used by heavy-framework and http-client)
        tree_node(
            "tokio",
            "1.35.0",
            20,
            false,
            vec![],
            vec!["default".into(), "rt".into(), "net".into(), "macros".into()],
            BTreeMap::from([
                ("default".into(), vec!["rt".into()]),
                ("rt".into(), vec![]),
                ("net".into(), vec![]),
                ("macros".into(), vec![]),
            ]),
        ),
        // 8: http-client
        tree_node(
            "http-client",
            "3.0.0",
            22,
            false,
            vec![7],
            vec![],
            BTreeMap::new(),
        ),
        // 9: test-helpers
        tree_node(
            "test-helpers",
            "1.0.0",
            8,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 10: once_cell
        tree_node(
            "once_cell",
            "1.19.0",
            1,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 11: heavy-sub-a
        tree_node(
            "heavy-sub-a",
            "1.0.0",
            3,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 12: heavy-sub-b
        tree_node(
            "heavy-sub-b",
            "1.0.0",
            2,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 13: serde_derive
        tree_node(
            "serde_derive",
            "1.0.200",
            4,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 14: dead-dep
        tree_node(
            "dead-dep",
            "0.1.0",
            3,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 15: remote-lib (optional dep of my-app, gated by "remote" feature, NOT currently enabled)
        tree_node(
            "remote-lib",
            "2.0.0",
            3,
            false,
            vec![16, 17],
            vec![], // no enabled_features — not active in normal build
            BTreeMap::new(),
        ),
        // 16: remote-sub-a (transitive dep of remote-lib)
        tree_node(
            "remote-sub-a",
            "1.0.0",
            1,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
        // 17: remote-sub-b (transitive dep of remote-lib)
        tree_node(
            "remote-sub-b",
            "0.5.0",
            1,
            false,
            vec![],
            vec![],
            BTreeMap::new(),
        ),
    ];

    let edges = vec![
        // my-app -> heavy-framework (optional, gated by "default" feature)
        DepTreeEdge {
            from: 0,
            to: 2,
            is_optional: true,
            gating_feature: Some("default".into()),
            enabled_child_features: vec!["default".into(), "async".into()],
        },
        // my-app -> serde
        DepTreeEdge {
            from: 0,
            to: 3,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec!["derive".into(), "std".into()],
        },
        // my-app -> unused-dep
        DepTreeEdge {
            from: 0,
            to: 4,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // my-app -> http-client
        DepTreeEdge {
            from: 0,
            to: 8,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // my-app -> test-helpers
        DepTreeEdge {
            from: 0,
            to: 9,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // my-lib -> regex
        DepTreeEdge {
            from: 1,
            to: 5,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec!["default".into()],
        },
        // my-lib -> tiny-helper
        DepTreeEdge {
            from: 1,
            to: 6,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // my-lib -> once_cell
        DepTreeEdge {
            from: 1,
            to: 10,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // heavy-framework -> tokio (optional, gated by "async")
        DepTreeEdge {
            from: 2,
            to: 7,
            is_optional: true,
            gating_feature: Some("async".into()),
            enabled_child_features: vec!["rt".into(), "net".into()],
        },
        // heavy-framework -> heavy-sub-a
        DepTreeEdge {
            from: 2,
            to: 11,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // heavy-framework -> heavy-sub-b
        DepTreeEdge {
            from: 2,
            to: 12,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // serde -> serde_derive (optional, gated by "derive")
        DepTreeEdge {
            from: 3,
            to: 13,
            is_optional: true,
            gating_feature: Some("derive".into()),
            enabled_child_features: vec![],
        },
        // unused-dep -> dead-dep
        DepTreeEdge {
            from: 4,
            to: 14,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // http-client -> tokio
        DepTreeEdge {
            from: 8,
            to: 7,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec!["rt".into(), "net".into(), "macros".into()],
        },
        // my-app -> remote-lib (optional, gated by "remote" feature — NOT currently enabled)
        DepTreeEdge {
            from: 0,
            to: 15,
            is_optional: true,
            gating_feature: Some("remote".into()),
            enabled_child_features: vec![],
        },
        // remote-lib -> remote-sub-a
        DepTreeEdge {
            from: 15,
            to: 16,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
        // remote-lib -> remote-sub-b
        DepTreeEdge {
            from: 15,
            to: 17,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: vec![],
        },
    ];

    DepTreeData {
        nodes,
        root_indices: vec![0, 1],
        edges,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pkg(name: &str, version: &str) -> PackageInfo {
    PackageInfo {
        name: name.into(),
        version: version.into(),
    }
}

fn tree_node(
    name: &str,
    version: &str,
    weight: usize,
    is_workspace: bool,
    children: Vec<usize>,
    enabled_features: Vec<String>,
    available_features: BTreeMap<String, Vec<String>>,
) -> DepTreeNode {
    DepTreeNode {
        name: name.into(),
        version: version.into(),
        transitive_weight: weight,
        unique_ancestors: 0,
        is_workspace,
        children,
        enabled_features,
        available_features,
    }
}

fn empty_scan(name: &str) -> ScanResult {
    ScanResult {
        heavy_crate_name: name.into(),
        searched_names: vec![name.replace('-', "_")],
        ref_count: 0,
        file_matches: vec![],
        files_with_matches: 0,
        generated_file_refs: 0,
        test_only_refs: 0,
        distinct_items: vec![],
        has_re_export_all: false,
    }
}

fn scan_with_refs(name: &str, ref_count: usize, matches: Vec<FileMatch>) -> ScanResult {
    let files_with_matches = matches.len();
    let distinct_items: Vec<String> = matches
        .iter()
        .filter_map(|m| {
            m.line_content
                .split("::")
                .last()
                .and_then(|s| s.split(';').next())
                .map(|s| s.trim().to_string())
        })
        .collect();
    ScanResult {
        heavy_crate_name: name.into(),
        searched_names: vec![name.replace('-', "_")],
        ref_count,
        file_matches: matches,
        files_with_matches,
        generated_file_refs: 0,
        test_only_refs: 0,
        distinct_items,
        has_re_export_all: false,
    }
}

fn file_match(path: &str, line: usize, content: &str) -> FileMatch {
    FileMatch {
        path: path.into(),
        line_number: line,
        line_content: content.into(),
        in_generated_file: false,
        in_test_code: false,
    }
}

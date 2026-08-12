//! Two manifest invariants that no amount of Rust source can express.
//!
//! Identity is two-part, so the "folder == bin == id" invariant needs enforcing rather than
//! hoping: every trading-engine `[[bin]]` must be named `<strategy-id>-<te-id>` and live at
//! `strategies/<strategy-id>/<te-id>/main.rs`. That makes bin-name uniqueness structural, keeps the
//! `--config`-less default path resolvable from the bin name alone, and stops a trading engine from
//! silently writing into a sibling's data tree.
//!
//! And a headless trading engine must not need wgpu/winit/X11 merely to BUILD. An ungated
//! eframe target fails the build loudly, but making `eframe` non-optional — or letting any feature
//! pull `ui` in by default — puts the whole GUI stack back into every deployment with every gate
//! still green. That is the silent loss this pins.

use std::path::Path;

/// Targets outside `strategies/` that name no eframe type, and so must NOT gate on `ui` — gating
/// them would make an operator pass `--features ui` to run a headless tool, building the whole GUI
/// stack to do it.
///
/// Named one at a time on purpose. The guard below exists so that a new target cannot quietly
/// re-link eframe everywhere, and any rule shaped like "examples are exempt" would hand that back.
const NON_UI_TARGETS: [&str; 2] = ["poly-probe", "poly-recover"];

/// Parsed straight from the manifest text — pulling in a toml crate to read two tables would be a
/// new dependency earned for nothing.
#[derive(Default)]
struct TargetDeclaration {
    kind: String,
    name: String,
    path: String,
    required_features: String,
}

#[test]
fn every_trading_engine_bin_is_named_and_placed_by_its_two_part_identity() {
    let engines: Vec<TargetDeclaration> = declared_targets()
        .into_iter()
        .filter(|target| target.kind == "bin" && target.path.starts_with("strategies/"))
        .collect();
    assert!(
        !engines.is_empty(),
        "Cargo.toml declares no trading-engine [[bin]] — a strategy folder is not auto-discovered, \
         so a missing declaration means the trading engine cannot be built at all"
    );

    for engine in &engines {
        let (strategy_id, te_id) = split_identity(&engine.path);
        assert_eq!(
            engine.name,
            format!("{strategy_id}-{te_id}"),
            "bin name must be <strategy-id>-<te-id> for path {}",
            engine.path
        );
        assert_eq!(
            engine.path,
            format!("strategies/{strategy_id}/{te_id}/main.rs"),
            "bin {} must live at strategies/<strategy-id>/<te-id>/main.rs",
            engine.name
        );
        assert!(
            Path::new(&engine.path).is_file(),
            "bin {} points at {}, which does not exist",
            engine.name,
            engine.path
        );
        assert!(
            te_id.starts_with("te-"),
            "te id {te_id} must be prefixed te- so a strategy folder listing reads as its engines"
        );
    }
}

#[test]
fn the_default_build_links_no_eframe() {
    let manifest = read_manifest();
    assert!(
        dependency_line(&manifest, "eframe").contains("optional = true"),
        "eframe must stay optional, or every headless deployment builds wgpu/winit again"
    );
    assert_eq!(
        feature_definition(&manifest, "ui"),
        r#"["dep:eframe"]"#,
        "feature ui must map to dep:eframe alone"
    );
    assert!(
        feature_names(&manifest)
            .iter()
            .all(|feature| feature != "default"),
        "no default feature may exist: one that reached ui would re-link eframe everywhere"
    );

    let gated: Vec<TargetDeclaration> = declared_targets()
        .into_iter()
        .filter(|target| !target.path.starts_with("strategies/"))
        .collect();
    assert!(
        !gated.is_empty(),
        "the ui bin and the dom-fixture example are declared targets — an empty set means the \
         parser stopped seeing them, not that the invariant holds"
    );
    for exempt in NON_UI_TARGETS {
        assert!(
            gated.iter().any(|target| target.name == exempt),
            "{exempt} is exempted from the ui gate but is no longer a declared target — a stale \
             exemption is how the next target slips through"
        );
    }

    for target in gated {
        if NON_UI_TARGETS.contains(&target.name.as_str()) {
            assert!(
                target.required_features.is_empty(),
                "{} {} is a headless tool, so it must declare no required-features",
                target.kind,
                target.name
            );
            continue;
        }
        assert_eq!(
            target.required_features, r#"["ui"]"#,
            "{} {} names eframe, so it must declare required-features = [\"ui\"]",
            target.kind, target.name
        );
    }
}

/// The `(strategy-id, te-id)` pair the path claims, taken from the path so name and path are checked
/// against one another rather than both against the same source.
fn split_identity(path: &str) -> (&str, &str) {
    let segments: Vec<&str> = path.split('/').collect();
    assert_eq!(
        segments.len(),
        4,
        "bin path {path} must be strategies/<strategy-id>/<te-id>/main.rs"
    );
    (segments[1], segments[2])
}

fn read_manifest() -> String {
    std::fs::read_to_string("Cargo.toml").expect("Cargo.toml is readable")
}

fn declared_targets() -> Vec<TargetDeclaration> {
    let manifest = read_manifest();
    let mut targets: Vec<TargetDeclaration> = Vec::new();
    let mut kind = String::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            // Only bin/example tables are tracked; `[[test]]` carries name/path keys too, and
            // letting them through would silently overwrite the preceding target's fields.
            let table = line.trim_matches(['[', ']']);
            kind = match table {
                "bin" | "example" => table.to_owned(),
                _ => String::new(),
            };
            if !kind.is_empty() {
                targets.push(TargetDeclaration {
                    kind: kind.clone(),
                    ..TargetDeclaration::default()
                });
            }
            continue;
        }
        let Some(target) = targets.last_mut().filter(|_| !kind.is_empty()) else {
            continue;
        };
        if let Some(value) = value_of(line, "name") {
            target.name = unquote(value);
        }
        if let Some(value) = value_of(line, "path") {
            target.path = unquote(value);
        }
        if let Some(value) = value_of(line, "required-features") {
            target.required_features = value.to_owned();
        }
    }
    targets
}

fn dependency_line(manifest: &str, crate_name: &str) -> String {
    section_lines(manifest, "[dependencies]")
        .find_map(|line| value_of(line, crate_name).map(str::to_owned))
        .unwrap_or_else(|| panic!("[dependencies] declares no {crate_name}"))
}

fn feature_definition(manifest: &str, feature: &str) -> String {
    section_lines(manifest, "[features]")
        .find_map(|line| value_of(line, feature).map(str::to_owned))
        .unwrap_or_else(|| panic!("[features] declares no {feature}"))
}

fn feature_names(manifest: &str) -> Vec<String> {
    section_lines(manifest, "[features]")
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim().to_owned()))
        .collect()
}

fn section_lines<'a>(manifest: &'a str, header: &'a str) -> impl Iterator<Item = &'a str> {
    manifest
        .lines()
        .map(str::trim)
        .skip_while(move |line| *line != header)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
}

fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    Some(
        line.strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim(),
    )
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

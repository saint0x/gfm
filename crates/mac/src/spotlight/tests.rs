use super::*;

#[test]
fn reconciles_spotlight_enrichment_without_primary_dependency() {
    let primary = record("Report.md");
    let snapshot = parse_spotlight_fixture(
        &primary.path,
        "kMDItemDisplayName\tReport.md\nkMDItemKind\tMarkdown Document\nkMDItemFinderComment\tclient handoff\nkMDItemUserTags\tImportant|Client\n",
    )
    .unwrap();

    let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

    assert_eq!(report.snapshot.status, SpotlightStatus::Available);
    assert_eq!(report.enrichments(), 2);
    assert_eq!(report.conflicts(), 0);
    assert!(report.as_tsv().contains(
        "field\tfinder-comment\tprimary=-\tspotlight=client handoff\tdecision=enrich-from-spotlight"
    ));
}

#[test]
fn primary_display_name_wins_spotlight_conflict() {
    let primary = record("Primary.md");
    let snapshot =
        parse_spotlight_fixture(&primary.path, "kMDItemDisplayName\tStale.md\n").unwrap();

    let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

    assert_eq!(report.conflicts(), 1);
    assert!(report.as_tsv().contains(
        "field\tdisplay-name\tprimary=Primary.md\tspotlight=Stale.md\tdecision=conflict-primary-wins"
    ));
}

#[test]
fn unavailable_spotlight_never_blocks_primary_record() {
    let primary = record("Local.txt");
    let snapshot = SpotlightSnapshot::missing(&primary.path, "mdls could not find Local.txt");

    let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

    assert!(report
        .fields
        .iter()
        .all(|field| field.decision == SpotlightFieldDecision::SpotlightUnavailable));
    assert!(report.as_tsv().starts_with(
        "spotlight-reconciliation\t/tmp/Local.txt\t1:9\tprimary=filesystem\tspotlight=missing"
    ));
}

#[test]
fn converts_native_spotlight_snapshot_to_typed_fields() {
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "kMDItemDisplayName".to_string(),
        vec!["Native.md".to_string()],
    );
    attributes.insert("unknown".to_string(), vec!["ignored".to_string()]);

    let snapshot = native_snapshot(
        PathBuf::from("/tmp/Native.md"),
        gfm_mac_sys::NativeSpotlightSnapshot {
            status: NativeSpotlightStatus::Available,
            attributes,
            reason: None,
        },
    );

    assert_eq!(
        snapshot.attributes.get(&SpotlightField::DisplayName),
        Some(&vec!["Native.md".to_string()])
    );
    assert_eq!(snapshot.attributes.len(), 1);
}

#[test]
fn batched_reader_preserves_request_order_for_missing_paths() {
    let first = PathBuf::from("/tmp/gfm-spotlight-missing-one");
    let second = PathBuf::from("/tmp/gfm-spotlight-missing-two");

    let snapshots = SpotlightMetadataReader
        .read_paths([first.as_path(), second.as_path()])
        .unwrap();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].path, first);
    assert_eq!(snapshots[1].path, second);
    assert_eq!(snapshots[0].status, SpotlightStatus::Missing);
    assert_eq!(snapshots[1].status, SpotlightStatus::Missing);
}

#[test]
fn ingestion_plan_publishes_healthy_spotlight_metadata() {
    let records = vec![record("Report.md")];
    let snapshot = parse_spotlight_fixture(
        &records[0].path,
        "kMDItemDisplayName\tReport.md\nkMDItemFinderComment\tclient handoff\n",
    )
    .unwrap();

    let plan = SpotlightIngestionPlan::from_records(
        &records,
        &[snapshot],
        &SpotlightIngestionPolicy::default(),
    );

    assert_eq!(plan.health, SpotlightIndexHealth::Healthy);
    assert_eq!(plan.publishable, 1);
    assert_eq!(plan.decisions[0].action, SpotlightIngestionAction::Publish);
    assert_eq!(
        plan.decisions[0]
            .publishable_attributes
            .get(&SpotlightField::FinderComment),
        Some(&vec!["client handoff".to_string()])
    );
}

#[test]
fn ingestion_plan_exports_secondary_metadata_records() {
    let records = vec![record("Report.md"), record("Stale.md")];
    let accepted = parse_spotlight_fixture(
        &records[0].path,
        "kMDItemDisplayName\tReport.md\nkMDItemUserTags\tBlue,Important\nkMDItemFinderComment\tclient handoff\nkMDItemKind\tMarkdown Document\nkMDItemWhereFroms\thttps://example.com\n",
    )
    .unwrap();
    let stale =
        parse_spotlight_fixture(&records[1].path, "kMDItemDisplayName\tWrong.md\n").unwrap();
    let plan = SpotlightIngestionPlan::from_records(
        &records,
        &[accepted, stale],
        &SpotlightIngestionPolicy::default(),
    );

    let secondary = plan.secondary_metadata_records();

    assert_eq!(secondary.len(), 1);
    assert_eq!(secondary[0].id, records[0].id);
    assert_eq!(
        secondary[0].tags,
        vec!["Blue".to_string(), "Important".to_string()]
    );
    assert!(secondary[0]
        .comments
        .contains(&"Markdown Document".to_string()));
    assert!(secondary[0]
        .comments
        .contains(&"client handoff".to_string()));
    assert!(secondary[0]
        .comments
        .contains(&"https://example.com".to_string()));
}

#[test]
fn ingestion_plan_quarantines_stale_identity_conflicts() {
    let records = vec![record("Primary.md")];
    let snapshot =
        parse_spotlight_fixture(&records[0].path, "kMDItemDisplayName\tStale.md\n").unwrap();

    let plan = SpotlightIngestionPlan::from_records(
        &records,
        &[snapshot],
        &SpotlightIngestionPolicy::default(),
    );

    assert_eq!(plan.quarantined, 1);
    assert_eq!(
        plan.decisions[0].action,
        SpotlightIngestionAction::QuarantineStale
    );
    assert!(plan.decisions[0].publishable_attributes.is_empty());
}

#[test]
fn ingestion_plan_defers_records_after_per_volume_budget() {
    let mut first = record("One.md");
    let mut second = record("Two.md");
    first.id = FileId::new(VolumeId(7), 1);
    second.id = FileId::new(VolumeId(7), 2);
    let first_snapshot =
        parse_spotlight_fixture(&first.path, "kMDItemDisplayName\tOne.md\n").unwrap();
    let second_snapshot =
        parse_spotlight_fixture(&second.path, "kMDItemDisplayName\tTwo.md\n").unwrap();
    let policy = SpotlightIngestionPolicy {
        max_records_per_volume: 1,
        ..SpotlightIngestionPolicy::default()
    };

    let plan = SpotlightIngestionPlan::from_records(
        &[first, second],
        &[first_snapshot, second_snapshot],
        &policy,
    );

    assert_eq!(plan.publishable, 1);
    assert_eq!(plan.deferred, 1);
    assert_eq!(
        plan.decisions[1].action,
        SpotlightIngestionAction::DeferVolumeThrottle
    );
}

#[test]
fn ingestion_plan_reports_degraded_and_unavailable_health() {
    let records = vec![record("One.md"), record("Two.md")];
    let snapshots = vec![
        SpotlightSnapshot::unavailable(&records[0].path, "metadata server unavailable"),
        SpotlightSnapshot::available(&records[1].path, BTreeMap::new()),
    ];
    let degraded_policy = SpotlightIngestionPolicy {
        max_unavailable_fraction_bps: 4_999,
        ..SpotlightIngestionPolicy::default()
    };

    let degraded = SpotlightIngestionPlan::from_records(&records, &snapshots, &degraded_policy);

    assert_eq!(degraded.health, SpotlightIndexHealth::Degraded);
    assert_eq!(degraded.unavailable, 1);

    let unavailable_snapshots = vec![
        SpotlightSnapshot::unavailable(&records[0].path, "metadata server unavailable"),
        SpotlightSnapshot::unavailable(&records[1].path, "metadata server unavailable"),
    ];
    let unavailable = SpotlightIngestionPlan::from_records(
        &records,
        &unavailable_snapshots,
        &SpotlightIngestionPolicy::default(),
    );

    assert_eq!(unavailable.health, SpotlightIndexHealth::Unavailable);
    assert_eq!(unavailable.unavailable, 2);
    assert!(unavailable
        .decisions
        .iter()
        .all(|decision| decision.action == SpotlightIngestionAction::SkipUnavailable));
}

fn record(name: &str) -> FileRecord {
    FileRecord {
        id: FileId::new(VolumeId(1), 9),
        parent: None,
        path: PathBuf::from("/tmp").join(name),
        name: name.to_string(),
        kind: FileKind::File,
        len: 42,
        mode: 0,
        owner: 0,
        group: 0,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string(), "Client".to_string()],
        finder_comment: None,
    }
}

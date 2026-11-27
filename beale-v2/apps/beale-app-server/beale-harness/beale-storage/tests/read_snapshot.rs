use beale_storage::HostDatabase;
#[test]
fn read_snapshot_pins_commits_and_cannot_write_or_migrate() {
    let root =
        std::env::temp_dir().join(format!("beale-snapshot-example-{}", uuid::Uuid::new_v4()));
    let db = HostDatabase::open(&root).unwrap();
    db.migrate_schema(
        "example",
        &["CREATE TABLE example_counter(value INTEGER); INSERT INTO example_counter VALUES(1);"],
    )
    .unwrap();
    let snapshot = db.read_snapshot().unwrap();
    let copy = snapshot.clone();
    db.with_connection(|c| {
        c.execute("UPDATE example_counter SET value=2", [])?;
        Ok(())
    })
    .unwrap();
    let value = |db: &HostDatabase| {
        db.with_connection(|c| {
            Ok(c.query_row("SELECT value FROM example_counter", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .unwrap()
    };
    assert_eq!(value(&snapshot), 1);
    assert_eq!(value(&copy), 1);
    assert_eq!(value(&db), 2);
    snapshot
        .migrate_schema("example", &["existing schema"])
        .unwrap();
    assert!(
        snapshot
            .migrate_schema(
                "example",
                &["existing schema", "CREATE TABLE forbidden(value INTEGER)"]
            )
            .is_err()
    );
    assert!(
        snapshot
            .with_connection(|c| {
                c.execute("UPDATE example_counter SET value=3", [])?;
                Ok(())
            })
            .is_err()
    );
    drop(copy);
    drop(snapshot);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

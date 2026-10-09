use super::*;

const ULID: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1C";
const MESH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn names() -> MeshNames {
    let mut id = [0u8; 32];
    hex::decode_to_slice(MESH, &mut id).unwrap();
    MeshNames { mesh: Some(id) }
}

#[test]
fn a_name_resolves_only_on_its_own_mesh_and_only_to_a_paired_primary() {
    let d = tempfile::tempdir().unwrap();
    let n = names();
    let root = format!("weftos://{MESH}/projects/{ULID}");
    // No pairing yet: the uniform refusal.
    assert_eq!(resolve(&root, &n, Some(d.path())).unwrap_err(), UNKNOWN_NAME);
    crate::mesh_pairings::upsert(d.path(), "primary-node", "primary", &[ULID.to_owned()], "h:1").unwrap();
    assert_eq!(resolve(&root, &n, Some(d.path())).unwrap(), (ULID.to_owned(), ".".to_owned(), "primary-node".to_owned()));
    assert_eq!(resolve(&format!("weftos://{MESH}/projects/{ULID}/repos/tools"), &n, Some(d.path())).unwrap().1, "tools");
    // Another mesh, another project, a non-repository path: all the same words.
    for bad in [
        format!("weftos://{}/projects/{ULID}", MESH.replace('0', "1")),
        format!("weftos://{MESH}/projects/01K6ZQ8N3T4V5W6X7Y8Z9A0B1D"),
        format!("weftos://{MESH}/projects/{ULID}/files/x"),
        format!("weftos://{MESH}/cogs/{ULID}"),
    ] {
        assert_eq!(resolve(&bad, &n, Some(d.path())).unwrap_err(), UNKNOWN_NAME, "{bad}");
    }
    // A friendly mesh name is not an authority: a parse error, not a lookup.
    assert!(resolve(&format!("weftos://weavelogic/projects/{ULID}"), &n, Some(d.path())).unwrap_err().contains("authority"));
    assert!(resolve("weftos://M/projects/x", &n, Some(d.path())).unwrap_err().contains("authority"));
    // A node without a mesh id resolves nothing.
    assert_eq!(resolve(&root, &MeshNames::default(), Some(d.path())).unwrap_err(), UNKNOWN_NAME);
    // A member-role pairing does not serve.
    crate::mesh_pairings::remove(d.path(), "primary-node").unwrap();
    crate::mesh_pairings::upsert(d.path(), "member-node", "member", &[ULID.to_owned()], "h:2").unwrap();
    assert_eq!(resolve(&root, &n, Some(d.path())).unwrap_err(), UNKNOWN_NAME);
}

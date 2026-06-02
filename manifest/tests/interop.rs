//! Semantic interoperability with the .NET manifest format, anchored on a real
//! manifest produced by the .NET `rt` tool (`tests/fixtures/dotnet_manifest.json`).

use base64::prelude::*;
use repotools_manifest::hash::{self, HashType};
use repotools_manifest::{FileHash, Manifest};

const FIXTURE: &str = include_str!("fixtures/dotnet_manifest.json");

fn parse_fixture() -> Manifest {
    serde_json::from_str(FIXTURE).expect("parse .NET manifest fixture")
}

#[test]
fn parses_dotnet_manifest_fields() {
    let m = parse_fixture();

    assert_eq!(m.guid.to_string(), "887ff9ee-1cc3-402f-9f28-8bf72fa02e5a");
    assert_eq!(m.default_hash_method, Some(HashType::Md5));
    assert_eq!(m.name, None);
    assert_eq!(m.description, None);
    assert_eq!(m.count_files(), 3);
    assert_eq!(m.count_bytes(), 12 + 12 + 7);

    // Root files and a subdirectory file.
    let root = &m.root_directory;
    assert_eq!(root.name, ".");
    assert!(root.files.contains_key("a.txt"));
    assert!(root.files.contains_key("b.txt"));
    let sub = root.subdirectories.get("sub").expect("sub dir");
    assert!(sub.files.contains_key("c.txt"));

    // Dates parse, and the never-validated sentinel is the .NET min date.
    assert_eq!(
        m.last_validate_date_utc,
        repotools_manifest::serde_fmt::dotnet_min_date()
    );
    assert_eq!(
        m.inception_date_utc
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "2026-06-02T18:42:44.813Z"
    );

    // The ignore list is preserved verbatim (6 patterns from the .NET prototype).
    assert_eq!(m.ignore_list.len(), 6);
    assert_eq!(m.ignore_list[0], r"^\./\.repositoryManifest$");
}

#[test]
fn decodes_base64_hash_data() {
    let m = parse_fixture();
    let a = m.root_directory.files["a.txt"].file_hash.as_ref().unwrap();
    assert_eq!(a.hash_type, HashType::Md5);
    assert_eq!(
        a.hash_data,
        BASE64_STANDARD.decode("b1kCrCNwJL3QwXbLkwY9xA==").unwrap()
    );
    assert_eq!(a.hash_data.len(), 16); // MD5 is 16 bytes
}

/// Our hashing must match .NET byte-for-byte: the fixture's stored hashes are
/// the MD5s of the known file contents.
#[test]
fn hashing_matches_dotnet() {
    let cases: [(&str, &[u8]); 3] = [
        ("b1kCrCNwJL3QwXbLkwY9xA==", b"hello world\n"),
        ("PbIFD8+Eu2MdyuQX09tRjA==", b"second file\n"),
        ("aYO0zSEKqzOId95tOzPJJg==", b"nested\n"),
    ];
    for (b64, content) in cases {
        let expected = BASE64_STANDARD.decode(b64).unwrap();
        assert_eq!(hash::compute_bytes(content, HashType::Md5), expected);
    }
}

/// Round-trip: parse the .NET manifest, re-serialize, re-parse — the model must
/// be identical, proving our writer emits a form we can read back losslessly.
#[test]
fn round_trips_through_our_serializer() {
    let m1 = parse_fixture();
    let json = serde_json::to_string(&m1).unwrap();
    let m2: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(m1, m2);
}

/// Our serializer emits the .NET field names and encodings.
#[test]
fn serializes_with_dotnet_encodings() {
    let m = parse_fixture();
    let json = serde_json::to_string(&m).unwrap();

    assert!(json.contains(r#""Guid":"887ff9ee-1cc3-402f-9f28-8bf72fa02e5a""#));
    assert!(json.contains(r#""DefaultHashMethod":"MD5""#));
    assert!(json.contains(r#""HashType":"MD5""#));
    assert!(json.contains(r#""HashData":"b1kCrCNwJL3QwXbLkwY9xA==""#));
    assert!(json.contains(r#""FileLength":12"#));
    assert!(json.contains("2026-06-02T18:42:44.813Z"));
    assert!(json.contains(r#""Name":null"#)); // nulls included, like .NET
    assert!(json.contains("0001-01-01T00:00:00.000Z")); // min-date sentinel
}

/// A freshly created clean manifest uses MD5 and the documented default ignores.
#[test]
fn clean_manifest_defaults() {
    let m = Manifest::new_clean();
    assert_eq!(m.default_hash_method, Some(HashType::Md5));
    assert_eq!(
        m.ignore_list,
        vec![
            r"^\./\.repositoryManifest$".to_string(),
            r"^\./temp-repository/".to_string(),
        ]
    );
    assert_eq!(m.count_files(), 0);
    // A clean manifest serializes and parses cleanly.
    let json = serde_json::to_string(&m).unwrap();
    let _back: Manifest = serde_json::from_str(&json).unwrap();
}

/// FileHash equality respects both bytes and algorithm.
#[test]
fn file_hash_equality() {
    let a = FileHash::new(HashType::Md5, vec![1, 2, 3]);
    let b = FileHash::new(HashType::Md5, vec![1, 2, 3]);
    let c = FileHash::new(HashType::Sha256, vec![1, 2, 3]);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

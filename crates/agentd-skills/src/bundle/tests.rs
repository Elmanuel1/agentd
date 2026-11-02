use super::*;
use std::io::Write;

fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }
    buf
}

#[test]
fn parses_top_level_folder_and_frontmatter() {
    let zip =
        zip_with(&[("my-skill/SKILL.md", "---\nname: My Skill\ndescription: does a thing\n---\n# body\n")]);
    let b = parse_bundle(&zip).unwrap();
    assert_eq!(b.directory, "my-skill");
    assert_eq!(b.name, "My Skill");
    assert_eq!(b.description, "does a thing");
}

#[test]
fn rejects_root_level_skill_md() {
    let zip = zip_with(&[("SKILL.md", "# no folder")]);
    let err = parse_bundle(&zip).unwrap_err();
    assert!(err.message.starts_with("Zip must contain a top-level folder"));
}

#[test]
fn rejects_multiple_top_level_folders() {
    let zip = zip_with(&[("a/SKILL.md", "x"), ("b/other.txt", "y")]);
    assert!(parse_bundle(&zip).is_err());
}

use super::*;

fn files(entries: &[(&str, &str)]) -> Vec<UploadFile> {
    entries.iter().map(|(p, b)| ((*p).to_owned(), Bytes::from((*b).to_owned()))).collect()
}

const MD: &str = "---\nname: my-skill\ndescription: does a thing\n---\n# body\n";

#[test]
fn a_files_array_under_one_directory_parses() {
    let b = parse_bundle(files(&[("my-skill/SKILL.md", MD), ("my-skill/notes/a.md", "a")])).unwrap();
    assert_eq!(
        (b.directory.as_str(), b.name.as_str(), b.description.as_str()),
        ("my-skill", "my-skill", "does a thing")
    );
    let back = unzip(&b.archive).unwrap();
    assert_eq!(back.len(), 2);
}

#[test]
fn skill_md_may_sit_at_the_root() {
    let b = parse_bundle(files(&[("SKILL.md", MD)])).unwrap();
    assert_eq!(b.directory, "my-skill");
}

#[test]
fn a_zip_part_is_expanded() {
    let zipped = zip(&files(&[("z/SKILL.md", MD)])).unwrap();
    let b = parse_bundle(vec![("bundle.zip".into(), zipped)]).unwrap();
    assert_eq!(b.directory, "z");
}

#[test]
fn the_live_layout_and_frontmatter_errors() {
    let msg = |f: Vec<UploadFile>| parse_bundle(f).unwrap_err().message;
    assert_eq!(msg(vec![]), "files[]: Field required");
    assert_eq!(msg(files(&[("a/README.md", "x")])), "SKILL.md not found in uploaded files");
    assert_eq!(
        msg(files(&[("a/SKILL.md", MD), ("b/x.md", "x")])),
        "files must share a single top-level directory, or place `SKILL.md` at the upload root"
    );
    assert_eq!(msg(files(&[("a/SKILL.md", "# no frontmatter")])), FRONTMATTER);
    assert_eq!(msg(files(&[("a/SKILL.md", "---\nname: a\n---\n")])), FRONTMATTER);
}

use blake3::Hasher;
use ignore::{
    WalkBuilder,
    overrides::{Override, OverrideBuilder},
};
use std::{collections::BTreeMap, fs::File, os::unix::fs::MetadataExt, path::PathBuf};

const MAX_DIFF_TEXT_SIZE: u64 = 100 * 1024; // 100KB

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

impl Kind {
    fn from_meta(ft: std::fs::FileType) -> Self {
        if ft.is_dir() {
            Kind::Dir
        } else if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_file() {
            Kind::File
        } else {
            Kind::Other
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Ownership {
    uid: u32,
    gid: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Attrs {
    mode: u32,
    kind: Kind,
    symlink_target: Option<PathBuf>,
    ownership: Ownership,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    hash: Option<String>,
    contents: Option<String>,
    attrs: Attrs,
}

impl Entry {
    fn from_path(path: PathBuf) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)?;

        let len = metadata.len();
        let kind = Kind::from_meta(metadata.file_type());
        let mode = metadata.mode();
        let ownership = Ownership {
            uid: metadata.uid(),
            gid: metadata.gid(),
        };
        let mut hash: Option<String> = None;
        let mut contents: Option<String> = None;

        if kind == Kind::File {
            if len < MAX_DIFF_TEXT_SIZE {
                let bytes = std::fs::read(&path)?;
                hash = Some(blake3::hash(&bytes).to_hex().to_string());
                contents = String::from_utf8(bytes).ok()
            } else {
                // File is large, we should io stream to hash and set contents to None
                let mut file = File::open(&path)?;
                let mut hasher = Hasher::new();
                std::io::copy(&mut file, &mut hasher)?;
                hash = Some(hasher.finalize().to_hex().to_string());
                contents = None
            }
        };

        let symlink_target = if kind == Kind::Symlink {
            std::fs::read_link(&path).ok()
        } else {
            None
        };

        let attrs = Attrs {
            mode,
            kind,
            symlink_target,
            ownership,
        };

        Ok(Self {
            hash,
            contents,
            attrs,
        })
    }
}

type Snapshot = BTreeMap<PathBuf, Entry>;

fn build_overrides(dir: &String, patterns: &Vec<String>) -> anyhow::Result<Override> {
    let mut ob = OverrideBuilder::new(dir);
    for pattern in patterns {
        ob.add(&format!("!{pattern}"))?;
    }
    Ok(ob.build()?)
}

pub fn snapshot(
    dir: &String,
    ignore: &Vec<String>,
    hidden: bool,
) -> Result<Snapshot, anyhow::Error> {
    let overrides = build_overrides(dir, ignore)?;
    let walker = WalkBuilder::new(dir)
        .hidden(hidden)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .overrides(overrides)
        .parents(false)
        .follow_links(false)
        .build();

    let mut snapshot = Snapshot::new();

    for entry in walker {
        let entry = entry?;
        let path = entry.path().to_path_buf();

        match Entry::from_path(path.clone()) {
            Ok(e) => {
                snapshot.insert(path, e);
            }
            Err(e) => println!("whatchanged: {}: {e}", path.display()),
        }
    }

    Ok(snapshot)
}

pub fn diff(snapshot_pre: &Snapshot, snapshot_post: &Snapshot) {

}

pub fn run(
    dir: String,
    ignore: Vec<String>,
    _command: Vec<String>,
    hidden: bool,
) -> anyhow::Result<()> {

    let snapshot_pre = snapshot(&dir, &ignore, hidden)?;
    let snapshot_post = snapshot(&dir, &ignore, hidden)?;

    diff(&snapshot_pre, &snapshot_post);

    Ok(())
}

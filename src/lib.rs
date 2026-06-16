use blake3::Hasher;
use ignore::{
    WalkBuilder,
    overrides::{Override, OverrideBuilder},
};
use similar::TextDiff;
use std::{
    collections::BTreeMap, fmt::Display, fs::File, os::unix::fs::MetadataExt, path::PathBuf, process::Command,
};

const MAX_DIFF_TEXT_SIZE: u64 = 100 * 1024; // 100KB

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

impl Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Kind::File => "file",
                Kind::Dir => "dir",
                Kind::Symlink => "symlink",
                Kind::Other => "other",
            }
        )?;
        Ok(())
    }
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

impl Display for Ownership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.uid, self.gid)?;
        Ok(())
    }
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

    fn text_diff(&self, other: &Entry) -> Vec<String> {
        match (self.contents.as_deref(), other.contents.as_deref()) {
            (Some(cont1), Some(cont2)) => TextDiff::from_lines(cont2, cont1)
                .iter_all_changes()
                .map(|change| {
                    let sign = match change.tag() {
                        similar::ChangeTag::Delete => "-",
                        similar::ChangeTag::Insert => "+",
                        similar::ChangeTag::Equal => " ",
                    };
                    format!("{sign}{change}")
                })
                .collect(),
            _ => vec![],
        }
    }
}

type Snapshot = BTreeMap<PathBuf, Entry>;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum DiffVerb {
    Added,
    Deleted,
    Modified,
}

#[derive(Debug)]
pub struct Diff {
    path: PathBuf,
    verb: DiffVerb,
    old_attrs: Option<Attrs>,
    new_attrs: Option<Attrs>,
    text_diff: Vec<String>,
}

impl Display for Diff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mark = match self.verb {
            DiffVerb::Added => "A",
            DiffVerb::Deleted => "D",
            DiffVerb::Modified => "M",
        };

        writeln!(f, "{mark} {}", self.path.display())?;

        match (self.old_attrs.as_ref(), self.new_attrs.as_ref()) {
            (Some(a1), Some(a2)) => {
                if a1.kind != a2.kind {
                    writeln!(f, "- File type changed from {} to {}", a1.kind, a2.kind)?;
                }
                if a1.mode != a2.mode {
                    writeln!(f, "- File mode changed from {} to {}", a1.mode, a2.mode)?;
                }
                if a1.ownership != a2.ownership {
                    writeln!(
                        f,
                        "- File ownership changed from {} to {}",
                        a1.ownership, a2.ownership
                    )?;
                }
                if a1.symlink_target != a2.symlink_target {
                    writeln!(
                        f,
                        "- File symlink target changed from {} to {}",
                        a1.symlink_target.as_ref().unwrap().display(), a2.symlink_target.as_ref().unwrap().display()
                    )?;
                }
            }
            _ => {}
        };

        for line in &self.text_diff {
            write!(f, "{line}")?;
        }

        Ok(())
    }
}

fn build_overrides(dir: &str, patterns: &[String]) -> anyhow::Result<Override> {
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
        match entry {
            Ok(entry) => {
                let path = entry.path().to_path_buf();
                match Entry::from_path(path.clone()) {
                    Ok(e) => {
                        snapshot.insert(path, e);
                    }
                    Err(e) => eprintln!("whatchanged: {}: {e}", path.display()),
                }
            }
            Err(e) => eprintln!("whatchanged: {e}"),
        }
    }

    Ok(snapshot)
}

pub fn diff(mut snapshot_pre: Snapshot, snapshot_post: &Snapshot) -> Vec<Diff> {
    let mut diffs: Vec<Diff> = snapshot_post
        .iter()
        .filter_map(|(path, entry_post)| {
            match snapshot_pre.remove(path) {
                None => {
                    // added
                    Some(Diff {
                        path: path.clone(),
                        verb: DiffVerb::Added,
                        old_attrs: None,
                        new_attrs: None,
                        text_diff: vec![],
                    })
                }
                Some(entry_pre) => {
                    // diff the entries here
                    if entry_pre != *entry_post {
                        Some(Diff {
                            path: path.clone(),
                            verb: DiffVerb::Modified,
                            old_attrs: Some(entry_pre.attrs.clone()),
                            new_attrs: Some(entry_post.attrs.clone()),
                            text_diff: entry_post.text_diff(&entry_pre),
                        })
                    } else {
                        None
                    }
                }
            }
        })
        .collect();

    for (path, _) in snapshot_pre {
        diffs.push(Diff {
            path: path.clone(),
            verb: DiffVerb::Deleted,
            old_attrs: None,
            new_attrs: None,
            text_diff: vec![],
        });
    }

    diffs
}

pub fn run(
    dir: String,
    ignore: Vec<String>,
    _command: Vec<String>,
    hidden: bool,
) -> anyhow::Result<()> {
    let snapshot_pre = snapshot(&dir, &ignore, hidden)?;
    Command::new("touch").arg("vim.txt").status()?;
    Command::new("sh")
        .arg("-c")
        .arg("echo asdfasfdsafsdfasdfasf > 2.txt")
        .status()?;
    let snapshot_post = snapshot(&dir, &ignore, hidden)?;

    for diff in diff(snapshot_pre, &snapshot_post) {
        println!("{}", diff);
    }

    Ok(())
}

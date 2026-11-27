//! The Files app's On My iPhone (On My iPad) location: the `File Provider Storage` folder of the
//! `group.com.apple.FileProvider.LocalStorage` app group in the phone's data folder.
//!
//! Names are written decomposed (NFD). Foundation looks a file up by its decomposed name, as on HFS+, while the Linux
//! disk compares bytes, so a file copied in with a composed (NFC) name, as Linux tools write them, is listed in Files
//! but fails to open ("not found").

use crate::device::Device;
use crate::phone;
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const GROUP: &str = "group.com.apple.FileProvider.LocalStorage";
const METADATA: &str = ".com.apple.mobile_container_manager.metadata.plist";

fn storage(dev: &Device) -> Result<PathBuf> {
    storage_in(&phone::data_dir(dev).join("Containers/Shared/AppGroup"))
}

fn storage_in(groups: &Path) -> Result<PathBuf> {
    let rd = std::fs::read_dir(groups)
        .with_context(|| format!("{} not found: boot this phone once first", groups.display()))?;
    for e in rd.flatten() {
        let Ok(v) = plist::Value::from_file(e.path().join(METADATA)) else {
            continue;
        };
        let id = v
            .as_dictionary()
            .and_then(|d| d.get("MCMMetadataIdentifier"))
            .and_then(|x| x.as_string());
        if id == Some(GROUP) {
            let dir = e.path().join("File Provider Storage");
            std::fs::create_dir_all(&dir)?;
            return Ok(dir);
        }
    }
    bail!("this phone has no On My iPhone folder ({GROUP}) yet: boot it once first")
}

fn nfd(s: &str) -> String {
    s.nfd().collect()
}

fn utf8(name: &std::ffi::OsStr) -> Result<&str> {
    name.to_str()
        .with_context(|| format!("{}: iOS file names must be UTF-8", name.to_string_lossy()))
}

/// `rel` (a path inside On My iPhone, `/` or empty for its top) under `storage`, every component decomposed.
fn inside(storage: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = storage.to_path_buf();
    for c in Path::new(rel.trim_start_matches('/')).components() {
        match c {
            Component::Normal(s) => out.push(nfd(utf8(s)?)),
            Component::CurDir => {}
            _ => bail!("{rel:?}: give a path inside On My iPhone, without .."),
        }
    }
    Ok(out)
}

/// `rel` as stored: decomposed, or as given for a file something else wrote with a composed name.
fn existing(storage: &Path, rel: &str) -> Result<PathBuf> {
    let p = inside(storage, rel)?;
    if p.symlink_metadata().is_ok() {
        return Ok(p);
    }
    let raw = storage.join(rel.trim_start_matches('/'));
    if raw.symlink_metadata().is_ok() {
        return Ok(raw);
    }
    bail!("{rel:?} is not in On My iPhone")
}

fn shown(rel: Option<&str>) -> String {
    match rel.map(|r| r.trim_matches('/')).filter(|r| !r.is_empty()) {
        Some(r) => format!("On My iPhone/{r}"),
        None => "On My iPhone".into(),
    }
}

/// Copies files and folders into On My iPhone (into `to`, created if missing).
pub fn push(dev: &Device, paths: &[PathBuf], to: Option<&str>) -> Result<String> {
    push_into(&storage(dev)?, paths, to)
}

fn push_into(storage: &Path, paths: &[PathBuf], to: Option<&str>) -> Result<String> {
    if paths.is_empty() {
        bail!("files push needs at least one file or folder");
    }
    let dst_dir = inside(storage, to.unwrap_or(""))?;
    std::fs::create_dir_all(&dst_dir)?;
    let mut n = 0;
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("{}: not found", p.display()))?;
        let name = p
            .canonicalize()?
            .file_name()
            .map(|n| utf8(n).map(nfd))
            .transpose()?
            .with_context(|| format!("{}: has no file name", p.display()))?;
        n += copy(p, &dst_dir.join(name), meta.is_dir())?;
    }
    Ok(format!(
        "copied {n} file{} to {}",
        if n == 1 { "" } else { "s" },
        shown(to)
    ))
}

fn copy(src: &Path, dst: &Path, is_dir: bool) -> Result<usize> {
    if !is_dir {
        std::fs::copy(src, dst).with_context(|| format!("copying {}", src.display()))?;
        return Ok(1);
    }
    std::fs::create_dir_all(dst)?;
    let mut n = 0;
    for e in std::fs::read_dir(src)?.flatten() {
        let t = std::fs::metadata(e.path())?.file_type();
        if t.is_dir() || t.is_file() {
            n += copy(&e.path(), &dst.join(nfd(utf8(&e.file_name())?)), t.is_dir())?;
        }
    }
    Ok(n)
}

/// Lists a folder of On My iPhone, folders first (with a trailing `/`), names composed as people type them.
pub fn ls(dev: &Device, dir: Option<&str>) -> Result<String> {
    ls_in(&storage(dev)?, dir)
}

fn ls_in(storage: &Path, dir: Option<&str>) -> Result<String> {
    let d = existing(storage, dir.unwrap_or(""))?;
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(&d)?
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (!is_dir, e.file_name().to_string_lossy().nfc().collect())
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Ok(format!("{} is empty", shown(dir)));
    }
    Ok(entries
        .iter()
        .map(|(file, name)| {
            if *file {
                name.clone()
            } else {
                format!("{name}/")
            }
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Removes a file or folder from On My iPhone.
pub fn rm(dev: &Device, path: &str) -> Result<String> {
    rm_in(&storage(dev)?, path)
}

fn rm_in(storage: &Path, path: &str) -> Result<String> {
    if path.trim_matches('/').is_empty() {
        bail!("give a file or folder inside On My iPhone");
    }
    let p = existing(storage, path)?;
    if p.symlink_metadata()?.is_dir() {
        std::fs::remove_dir_all(&p)?;
    } else {
        std::fs::remove_file(&p)?;
    }
    Ok(format!("removed {}", shown(Some(path))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-sim-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn group(groups: &Path, uuid: &str, id: &str) {
        let g = groups.join(uuid);
        std::fs::create_dir_all(g.join("File Provider Storage")).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("MCMMetadataIdentifier".into(), id.into());
        plist::Value::Dictionary(d)
            .to_file_binary(g.join(METADATA))
            .unwrap();
    }

    #[test]
    fn storage_is_the_local_storage_group_not_photos_or_icloud() {
        let d = scratch("storage");
        group(
            &d,
            "A",
            "group.com.apple.mobileslideshow.PhotosFileProvider",
        );
        group(&d, "B", "group.com.apple.iCloudDrive");
        group(&d, "C", GROUP);
        assert_eq!(
            storage_in(&d).unwrap(),
            d.join("C").join("File Provider Storage")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pushed_names_are_decomposed_and_listed_composed() {
        let d = scratch("push");
        let storage = d.join("storage");
        std::fs::create_dir_all(&storage).unwrap();
        let src = d.join("Βιβλία");
        std::fs::create_dir_all(src.join("Ἑσπερινός")).unwrap();
        std::fs::write(src.join("Ἑσπερινός/Δοξαστικά.pdf"), b"%PDF").unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        assert_eq!(
            push_into(&storage, std::slice::from_ref(&src), Some("Μουσική")).unwrap(),
            "copied 2 files to On My iPhone/Μουσική"
        );
        let file = storage
            .join(nfd("Μουσική"))
            .join(nfd("Βιβλία"))
            .join(nfd("Ἑσπερινός"))
            .join(nfd("Δοξαστικά.pdf"));
        assert!(file.is_file());
        assert_eq!(
            ls_in(&storage, Some("Μουσική/Βιβλία")).unwrap(),
            "Ἑσπερινός/\na.txt"
        );
        assert_eq!(
            rm_in(&storage, "Μουσική/Βιβλία/Ἑσπερινός").unwrap(),
            "removed On My iPhone/Μουσική/Βιβλία/Ἑσπερινός"
        );
        assert_eq!(ls_in(&storage, Some("Μουσική/Βιβλία")).unwrap(), "a.txt");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn paths_stay_inside_on_my_iphone() {
        let d = scratch("inside");
        assert!(inside(&d, "../etc").is_err());
        assert!(inside(&d, "a/../../b").is_err());
        assert_eq!(inside(&d, "/a/./b").unwrap(), d.join("a/b"));
        assert!(rm_in(&d, "/").is_err());
        assert!(rm_in(&d, "missing").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_composed_name_written_by_something_else_is_still_found() {
        let d = scratch("raw");
        let composed: String = "Ψαλτήριο.pdf".nfc().collect();
        std::fs::write(d.join(&composed), b"x").unwrap();
        assert_eq!(ls_in(&d, None).unwrap(), composed);
        rm_in(&d, &composed).unwrap();
        assert_eq!(ls_in(&d, None).unwrap(), "On My iPhone is empty");
        let _ = std::fs::remove_dir_all(&d);
    }
}

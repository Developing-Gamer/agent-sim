//! Turning what the user has (an .ipa built for devices, or a simulator .app) into an .app x86Sim installs.
//!
//! A device build runs in the simulator once its binaries say so: every Mach-O's LC_BUILD_VERSION platform goes from
//! iOS (2) to iOS simulator (7), and Info.plist names the simulator as the platform. The code is the same arm64 code
//! (x86Sim translates it either way). App Store downloads are encrypted (FairPlay) and cannot run anywhere else:
//! those are refused.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct App {
    pub path: PathBuf,
    pub bundle_id: String,
}

const LC_BUILD_VERSION: u32 = 0x32;
const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;
const LC_ENCRYPTION_INFO_64: u32 = 0x2c;
const PLATFORM_IOS: u32 = 2;
const PLATFORM_IOSSIMULATOR: u32 = 7;
const FAT_MAGIC: u32 = 0xCAFE_BABE;
const FAT_MAGIC_64: u32 = 0xCAFE_BABF;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000C;

#[derive(Debug, PartialEq)]
pub enum Retag {
    NotMachO,
    AlreadySimulator,
    Retagged,
}

fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u32be(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64be(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Extracts the arm64 slice from a universal (fat) Mach-O in place.
///
/// Universal simulator builds (such as Flutter's `flutter build ios --simulator` or Xcode universal builds) carry
/// both x86_64 and arm64 slices; Darling picks the x86_64 slice first and fails with an architecture mismatch
/// against `dyld_sim`. Returns `Ok(true)` when thinned, `Ok(false)` when the file is not a fat Mach-O.
pub fn thin_arm64(bytes: &mut Vec<u8>, name: &str) -> Result<bool> {
    let fat64 = match u32be(bytes, 0) {
        Some(FAT_MAGIC) => false,
        Some(FAT_MAGIC_64) => true,
        _ => return Ok(false),
    };
    let n = u32be(bytes, 4).context("truncated fat header")? as usize;
    // Java .class files also start with 0xCAFEBABE, with class major version >= 45 in bytes 6..8.
    if n == 0 || n > 32 {
        return Ok(false);
    }
    let entry_size = if fat64 { 32 } else { 20 };
    if 8 + n * entry_size > bytes.len() {
        bail!("{name}: truncated fat header");
    }
    for i in 0..n {
        let base = 8 + i * entry_size;
        let cpu = u32be(bytes, base).context("truncated fat header")?;
        let (off, size) = if fat64 {
            (
                usize::try_from(u64be(bytes, base + 8).context("truncated fat header")?)
                    .context("fat slice offset overflow")?,
                usize::try_from(u64be(bytes, base + 16).context("truncated fat header")?)
                    .context("fat slice size overflow")?,
            )
        } else {
            (
                u32be(bytes, base + 8).context("truncated fat header")? as usize,
                u32be(bytes, base + 12).context("truncated fat header")? as usize,
            )
        };
        if cpu == CPU_TYPE_ARM64 {
            let end = off
                .checked_add(size)
                .filter(|&e| e <= bytes.len())
                .with_context(|| format!("{name}: arm64 fat slice out of bounds"))?;
            *bytes = bytes[off..end].to_vec();
            return Ok(true);
        }
    }
    bail!("{name} has no arm64 slice (x86Sim runs arm64 simulator builds)")
}

/// Retags one 64-bit Mach-O image in place (at `base` in the file); refuses encrypted ones.
fn retag_image(b: &mut [u8], base: usize, name: &str) -> Result<Retag> {
    if u32le(b, base) != Some(0xFEED_FACF) {
        return Ok(Retag::NotMachO);
    }
    if u32le(b, base + 4) == Some(CPU_TYPE_X86_64) {
        bail!("{name} is x86_64 only (x86Sim runs arm64 simulator builds)");
    }
    let ncmds = u32le(b, base + 16).context("truncated Mach-O")? as usize;
    let mut off = base + 32;
    let mut result = Retag::AlreadySimulator;
    let mut saw_build = false;
    for _ in 0..ncmds {
        let cmd = u32le(b, off).context("truncated Mach-O")?;
        let size = u32le(b, off + 4).context("truncated Mach-O")? as usize;
        if size < 8 || off + size > b.len() {
            bail!("{name}: malformed load command");
        }
        match cmd {
            LC_ENCRYPTION_INFO_64 if u32le(b, off + 16).unwrap_or(0) != 0 => bail!(
                "{name} is encrypted (an App Store download): use an unencrypted build (Xcode's Ad Hoc, Development or \
                 Enterprise export, or a simulator build)"
            ),
            LC_BUILD_VERSION => {
                saw_build = true;
                let platform = u32le(b, off + 8).unwrap_or(0);
                if platform == PLATFORM_IOS {
                    b[off + 8..off + 12].copy_from_slice(&PLATFORM_IOSSIMULATOR.to_le_bytes());
                    result = Retag::Retagged;
                } else if platform != PLATFORM_IOSSIMULATOR {
                    bail!("{name} is built for platform {platform}, not iOS");
                }
            }
            LC_VERSION_MIN_IPHONEOS if !saw_build => bail!(
                "{name} predates LC_BUILD_VERSION (built for iOS 11 or older); rebuild it with a current Xcode"
            ),
            _ => {}
        }
        off += size;
    }
    Ok(result)
}

/// Retags a Mach-O file (thin, or every 64-bit slice of a fat one).
pub fn retag(bytes: &mut [u8], name: &str) -> Result<Retag> {
    match u32be(bytes, 0) {
        Some(FAT_MAGIC) => {
            let n = u32be(bytes, 4).context("truncated fat header")? as usize;
            if n == 0 || n > 32 {
                return Ok(Retag::NotMachO);
            }
            let mut any = Retag::NotMachO;
            for i in 0..n {
                let o = u32be(bytes, 8 + i * 20 + 8).context("truncated fat header")? as usize;
                match retag_image(bytes, o, name)? {
                    Retag::Retagged => any = Retag::Retagged,
                    Retag::AlreadySimulator if any == Retag::NotMachO => {
                        any = Retag::AlreadySimulator
                    }
                    _ => {}
                }
            }
            Ok(any)
        }
        _ => retag_image(bytes, 0, name),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        let t = e.file_type()?;
        if t.is_dir() {
            walk(&e.path(), out)?;
        } else if t.is_file() {
            out.push(e.path());
        }
    }
    Ok(())
}

fn bundle_id(app: &Path) -> Result<String> {
    let v = plist::Value::from_file(app.join("Info.plist"))
        .with_context(|| format!("reading {}/Info.plist", app.display()))?;
    v.as_dictionary()
        .and_then(|d| d.get("CFBundleIdentifier"))
        .and_then(|s| s.as_string())
        .map(str::to_string)
        .context("Info.plist has no CFBundleIdentifier")
}

/// Info.plist (of the app and of each extension) names the simulator as its platform.
fn retag_plist(path: &Path) -> Result<()> {
    let mut v = plist::Value::from_file(path)?;
    let Some(d) = v.as_dictionary_mut() else {
        return Ok(());
    };
    let mut changed = false;
    if let Some(plist::Value::Array(a)) = d.get_mut("CFBundleSupportedPlatforms")
        && a.iter().any(|x| x.as_string() == Some("iPhoneOS"))
    {
        *a = vec!["iPhoneSimulator".into()];
        changed = true;
    }
    if d.get("DTPlatformName").and_then(|x| x.as_string()) == Some("iphoneos") {
        d.insert("DTPlatformName".into(), "iphonesimulator".into());
        changed = true;
    }
    if changed {
        v.to_file_xml(path)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Archive {
    Zip,
    Gzip,
    Tar,
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// Downloads `url` into the installs folder, keeping the name its path ends in so the archive type stays visible.
fn download(url: &str) -> Result<PathBuf> {
    let dir = crate::phone::home()?.join("installs").join(".downloads");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let dst = dir.join(url_file_name(url));
    eprintln!("downloading {url}");
    let ok = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&dst)
        .arg(url)
        .status()
        .context("downloading (curl)")?;
    if !ok.success() {
        bail!("could not download {url}");
    }
    Ok(dst)
}

fn url_file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    let name = name.trim_matches('.');
    if name.is_empty() || name.contains("..") || path.matches('/').count() < 3 {
        "download".into()
    } else {
        name.into()
    }
}

/// What a file is by its first bytes, since a download's name need not say.
fn archive_kind(path: &Path) -> Result<Archive> {
    let mut head = [0u8; 262];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let mut n = 0;
        while n < head.len() {
            match f.read(&mut head[n..])? {
                0 => break,
                k => n += k,
            }
        }
        n
    };
    match &head[..n] {
        [b'P', b'K', 3, 4, ..] => Ok(Archive::Zip),
        [0x1f, 0x8b, ..] => Ok(Archive::Gzip),
        h if h.len() >= 262 && &h[257..262] == b"ustar" => Ok(Archive::Tar),
        _ => bail!(
            "{} is not an .ipa or an archive (zip, tar or tar.gz); install also takes an .app folder or a URL",
            path.display()
        ),
    }
}

fn archive_stem(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let lower = name.to_lowercase();
    for ext in [".tar.gz", ".tgz", ".tar", ".zip", ".ipa"] {
        if lower.ends_with(ext) && lower.len() > ext.len() {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.into_owned()
}

fn unpack(path: &Path, kind: Archive, work: &Path) -> Result<()> {
    let ok = match kind {
        Archive::Zip => Command::new("python3")
            .args(["-I", "-m", "zipfile", "-e"])
            .arg(path)
            .arg(work)
            .status()
            .context("unzipping (python3)")?,
        Archive::Gzip | Archive::Tar => Command::new("tar")
            .arg(if kind == Archive::Gzip { "-xzf" } else { "-xf" })
            .arg(path)
            .arg("-C")
            .arg(work)
            .status()
            .context("unpacking (tar)")?,
    };
    if !ok.success() {
        bail!("{} could not be unpacked", path.display());
    }
    Ok(())
}

/// The one .app in an unpacked archive: at its top, in an .ipa's Payload, or a folder or two down.
fn find_app(dir: &Path) -> Result<PathBuf> {
    fn look(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || e.file_name() == "__MACOSX" {
                continue;
            }
            if p.extension().is_some_and(|x| x == "app") {
                out.push(p);
            } else if depth < 3 {
                look(&p, depth + 1, out);
            }
        }
    }
    let mut apps = Vec::new();
    look(dir, 1, &mut apps);
    apps.sort();
    match apps.len() {
        0 => bail!("no .app inside"),
        1 => Ok(apps.remove(0)),
        _ => bail!(
            "more than one .app inside: {}",
            apps.iter()
                .map(|a| a.strip_prefix(dir).unwrap_or(a).display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Makes an .app the simulator runs from `path`: an .app directory, an .ipa, an archive holding an .app (.tar.gz, .tgz,
/// .tar or .zip, as EAS Build and CI artifacts come), or an http(s) URL of any of those.
pub fn prepare(path: &Path) -> Result<App> {
    let downloaded;
    let path = match path.to_str().filter(|s| is_url(s)) {
        Some(url) => {
            downloaded = download(url)?;
            downloaded.as_path()
        }
        None => path,
    };
    let path =
        std::fs::canonicalize(path).with_context(|| format!("{} not found", path.display()))?;
    let app = if path.is_dir() {
        if path.extension().is_none_or(|e| e != "app") {
            bail!("install takes an .app folder, an .ipa, an archive holding an .app, or a URL");
        }
        // retag a copy, so a device .app the user points at is left as it was
        let work = crate::phone::home()?.join("installs");
        std::fs::create_dir_all(&work)?;
        let dst = work.join(path.file_name().unwrap());
        let _ = std::fs::remove_dir_all(&dst);
        let ok = Command::new("cp").arg("-a").arg(&path).arg(&dst).status()?;
        if !ok.success() {
            bail!("copying {}", path.display());
        }
        dst
    } else {
        let kind = archive_kind(&path)?;
        let work = crate::phone::home()?
            .join("installs")
            .join(archive_stem(&path));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)?;
        unpack(&path, kind, &work)?;
        find_app(&work).with_context(|| format!("installing {}", path.display()))?
    };
    let mut files = Vec::new();
    walk(&app, &mut files)?;
    let mut retagged = 0;
    let mut thinned = 0;
    for f in &files {
        if f.file_name().is_some_and(|n| n == "Info.plist") {
            retag_plist(f).with_context(|| format!("updating {}", f.display()))?;
            continue;
        }
        let Ok(meta) = std::fs::metadata(f) else {
            continue;
        };
        if meta.len() < 32 {
            continue;
        }
        let mut b = std::fs::read(f)?;
        let name = f.strip_prefix(&app).unwrap_or(f).display().to_string();
        let did_thin = thin_arm64(&mut b, &name)?;
        let did_retag = retag(&mut b, &name)? == Retag::Retagged;
        if did_thin || did_retag {
            std::fs::write(f, &b)?;
            if did_thin {
                thinned += 1;
            }
            if did_retag {
                retagged += 1;
            }
        }
    }
    match (retagged, thinned) {
        (r, 0) if r > 0 => eprintln!("prepared a device build for the simulator ({r} binaries)"),
        (r, t) if r > 0 => {
            eprintln!(
                "prepared a device build for the simulator ({r} binaries, {t} thinned to arm64)"
            )
        }
        (0, t) if t > 0 => eprintln!("thinned universal build to arm64 ({t} binaries)"),
        _ => {}
    }
    let bundle_id = bundle_id(&app)?;
    Ok(App {
        path: app,
        bundle_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macho_with_cpu(cpu: u32, cmds: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0xFEED_FACFu32.to_le_bytes());
        b[4..8].copy_from_slice(&cpu.to_le_bytes());
        b[16..20].copy_from_slice(&(cmds.len() as u32).to_le_bytes());
        for (cmd, body) in cmds {
            b.extend_from_slice(&cmd.to_le_bytes());
            b.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
            b.extend_from_slice(body);
        }
        b
    }
    fn macho(cmds: &[(u32, Vec<u8>)]) -> Vec<u8> {
        macho_with_cpu(CPU_TYPE_ARM64, cmds)
    }
    fn fat(slices: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let header_len = 8 + slices.len() * 20;
        let mut out = Vec::new();
        out.extend_from_slice(&FAT_MAGIC.to_be_bytes());
        out.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut off = header_len;
        for (cpu, data) in slices {
            out.extend_from_slice(&cpu.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(off as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
            off += data.len();
        }
        for (_, data) in slices {
            out.extend_from_slice(data);
        }
        out
    }
    fn fat64(slices: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let header_len = 8 + slices.len() * 32;
        let mut out = Vec::new();
        out.extend_from_slice(&FAT_MAGIC_64.to_be_bytes());
        out.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut off = header_len as u64;
        for (cpu, data) in slices {
            out.extend_from_slice(&cpu.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&off.to_be_bytes());
            out.extend_from_slice(&(data.len() as u64).to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
            off += data.len() as u64;
        }
        for (_, data) in slices {
            out.extend_from_slice(data);
        }
        out
    }
    fn build_version(platform: u32) -> Vec<u8> {
        let mut v = platform.to_le_bytes().to_vec();
        v.extend_from_slice(&[0u8; 12]);
        v
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-sim-ipa-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn download_names_come_from_the_url_path() {
        assert_eq!(
            url_file_name("https://expo.dev/artifacts/eas/abc123.tar.gz"),
            "abc123.tar.gz"
        );
        assert_eq!(url_file_name("https://x.io/b/App.ipa?sig=1#f"), "App.ipa");
        assert_eq!(url_file_name("https://x.io"), "download");
        assert_eq!(url_file_name("https://x.io/b/"), "download");
        assert_eq!(url_file_name("https://x.io/b/.."), "download");
    }

    #[test]
    fn archive_stems_drop_the_archive_extension() {
        assert_eq!(archive_stem(Path::new("/d/Isokratis.tar.gz")), "Isokratis");
        assert_eq!(archive_stem(Path::new("/d/App.TGZ")), "App");
        assert_eq!(archive_stem(Path::new("/d/App.ipa")), "App");
        assert_eq!(archive_stem(Path::new("/d/download")), "download");
    }

    #[test]
    fn archives_are_told_apart_by_their_first_bytes() {
        let d = scratch("kind");
        let write = |n: &str, b: &[u8]| {
            let p = d.join(n);
            std::fs::write(&p, b).unwrap();
            p
        };
        assert_eq!(
            archive_kind(&write("a", b"PK\x03\x04rest")).unwrap(),
            Archive::Zip
        );
        assert_eq!(
            archive_kind(&write("b", &[0x1f, 0x8b, 8, 0])).unwrap(),
            Archive::Gzip
        );
        let mut tar = vec![0u8; 512];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(archive_kind(&write("c", &tar)).unwrap(), Archive::Tar);
        assert!(archive_kind(&write("d", b"<html>not found</html>")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_eas_simulator_tarball_unpacks_to_its_app() {
        let d = scratch("tgz");
        let app = d.join("src/Isokratis.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Info.plist"), b"x").unwrap();
        let tgz = d.join("build.tar.gz");
        let ok = Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(d.join("src"))
            .arg("Isokratis.app")
            .status()
            .unwrap();
        assert!(ok.success());
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        unpack(&tgz, archive_kind(&tgz).unwrap(), &out).unwrap();
        assert_eq!(find_app(&out).unwrap(), out.join("Isokratis.app"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn find_app_looks_in_payload_and_refuses_two() {
        let d = scratch("find");
        std::fs::create_dir_all(d.join("Payload/Runner.app/Watch/W.app")).unwrap();
        std::fs::create_dir_all(d.join("__MACOSX/Other.app")).unwrap();
        assert_eq!(find_app(&d).unwrap(), d.join("Payload/Runner.app"));
        std::fs::create_dir_all(d.join("Second.app")).unwrap();
        assert!(find_app(&d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn device_binaries_become_simulator_ones() {
        let mut b = macho(&[(LC_BUILD_VERSION, build_version(PLATFORM_IOS))]);
        assert_eq!(retag(&mut b, "x").unwrap(), Retag::Retagged);
        assert_eq!(u32le(&b, 40), Some(PLATFORM_IOSSIMULATOR));
        assert_eq!(retag(&mut b, "x").unwrap(), Retag::AlreadySimulator);
    }

    #[test]
    fn encrypted_binaries_are_refused() {
        let mut enc = vec![0u8; 12];
        enc[8..12].copy_from_slice(&1u32.to_le_bytes());
        let mut b = macho(&[
            (LC_BUILD_VERSION, build_version(PLATFORM_IOS)),
            (LC_ENCRYPTION_INFO_64, enc),
        ]);
        assert!(retag(&mut b, "x").is_err());
    }

    #[test]
    fn other_files_are_left_alone() {
        let mut b = b"not a mach-o file at all, just text".to_vec();
        assert!(!thin_arm64(&mut b, "x").unwrap());
        assert_eq!(retag(&mut b, "x").unwrap(), Retag::NotMachO);
    }

    #[test]
    fn universal_simulator_binary_is_thinned_to_arm64() {
        let x64 = macho_with_cpu(
            CPU_TYPE_X86_64,
            &[(LC_BUILD_VERSION, build_version(PLATFORM_IOSSIMULATOR))],
        );
        let arm = macho(&[(LC_BUILD_VERSION, build_version(PLATFORM_IOSSIMULATOR))]);
        let mut b = fat(&[(CPU_TYPE_X86_64, x64), (CPU_TYPE_ARM64, arm.clone())]);
        assert!(thin_arm64(&mut b, "Runner").unwrap());
        assert_eq!(b, arm);
        assert_eq!(retag(&mut b, "Runner").unwrap(), Retag::AlreadySimulator);
    }

    #[test]
    fn universal_device_binary_is_thinned_and_retagged() {
        let x64 = macho_with_cpu(
            CPU_TYPE_X86_64,
            &[(LC_BUILD_VERSION, build_version(PLATFORM_IOS))],
        );
        let arm = macho(&[(LC_BUILD_VERSION, build_version(PLATFORM_IOS))]);
        let mut b = fat(&[(CPU_TYPE_X86_64, x64), (CPU_TYPE_ARM64, arm)]);
        assert!(thin_arm64(&mut b, "Runner").unwrap());
        assert_eq!(retag(&mut b, "Runner").unwrap(), Retag::Retagged);
        assert_eq!(u32le(&b, 4), Some(CPU_TYPE_ARM64));
        assert_eq!(u32le(&b, 40), Some(PLATFORM_IOSSIMULATOR));
    }

    #[test]
    fn fat64_binary_is_thinned_to_arm64() {
        let x64 = macho_with_cpu(
            CPU_TYPE_X86_64,
            &[(LC_BUILD_VERSION, build_version(PLATFORM_IOSSIMULATOR))],
        );
        let arm = macho(&[(LC_BUILD_VERSION, build_version(PLATFORM_IOSSIMULATOR))]);
        let mut b = fat64(&[(CPU_TYPE_X86_64, x64), (CPU_TYPE_ARM64, arm.clone())]);
        assert!(thin_arm64(&mut b, "Runner").unwrap());
        assert_eq!(b, arm);
    }

    #[test]
    fn x86_64_only_binaries_are_refused() {
        let x64 = macho_with_cpu(
            CPU_TYPE_X86_64,
            &[(LC_BUILD_VERSION, build_version(PLATFORM_IOSSIMULATOR))],
        );
        let mut fat_x64 = fat(&[(CPU_TYPE_X86_64, x64.clone())]);
        assert!(thin_arm64(&mut fat_x64, "Runner").is_err());

        let mut thin_x64 = x64;
        assert!(retag(&mut thin_x64, "Runner").is_err());
    }

    #[test]
    fn java_class_magic_is_not_treated_as_fat_macho() {
        let mut b = vec![0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x41];
        b.resize(64, 0);
        assert!(!thin_arm64(&mut b, "Foo.class").unwrap());
        assert_eq!(retag(&mut b, "Foo.class").unwrap(), Retag::NotMachO);
    }
}

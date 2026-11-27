use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const OWNER: &str = "suppaman80";
const MAX_BYTES: u64 = 512 * 1024 * 1024;

pub struct App {
    pub repo: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static [u8],
}

pub const APPS: [App; 2] = [
    App {
        repo: "Suppa-Terminal",
        title: "Suppa-Terminal",
        description: "Terminale Linux con guide ai comandi.",
        icon: include_bytes!("../assets/suppa-terminal.png"),
    },
    App {
        repo: "Suppa-Usb-Analyzer",
        title: "Suppa USB Analyzer",
        description: "Esplora periferiche USB e porte seriali.",
        icon: include_bytes!("../assets/suppa-usb-analyzer.png"),
    },
];

impl App {
    pub fn site(&self) -> String {
        format!("https://{OWNER}.github.io/{}/", self.repo)
    }

    fn filename(&self, version: &Version) -> String {
        format!("{}_{version}_x86_64.AppImage", self.repo)
    }

    pub fn installed(&self, root: &Path) -> Option<(Version, PathBuf)> {
        fs::read_dir(root.join(self.repo))
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let version = name
                    .to_str()?
                    .strip_prefix(&format!("{}_", self.repo))?
                    .strip_suffix("_x86_64.AppImage")?
                    .parse::<Version>()
                    .ok()?;
                let file = entry.path();
                let metadata = fs::symlink_metadata(&file).ok()?;
                (version.pre.is_empty()
                    && version.build.is_empty()
                    && metadata.is_file()
                    && metadata.permissions().mode() & 0o100 != 0
                    && is_appimage(&file))
                .then_some((version, file))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
    }
}

pub fn is_appimage(path: &Path) -> bool {
    let mut header = [0; 11];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && &header[..4] == b"\x7fELF"
        && &header[8..] == b"AI\x02"
}

#[derive(Clone)]
pub struct Available {
    pub version: Version,
    name: String,
    tag: String,
    size: u64,
    digest: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
}

impl Release {
    fn select(self, app: &App) -> Result<Available> {
        if self.draft || self.prerelease {
            return Err("La release non e' stabile.".into());
        }
        let version = Version::parse(self.tag_name.strip_prefix('v').ok_or("Tag non valido.")?)?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err("Il tag non rappresenta una versione stabile.".into());
        }
        let name = app.filename(&version);
        let asset = self
            .assets
            .into_iter()
            .find(|asset| asset.name == name)
            .ok_or("Nessuna AppImage x86_64 compatibile nella release.")?;
        let digest = asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("GitHub non fornisce un checksum SHA-256 valido.")?;
        if asset.size < 11 || asset.size > MAX_BYTES {
            return Err("Dimensione dell'AppImage non valida.".into());
        }
        Ok(Available {
            version,
            name,
            tag: self.tag_name,
            size: asset.size,
            digest: digest.to_ascii_lowercase(),
        })
    }
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("SuppaworldLauncher/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

pub fn latest(app: &App) -> Result<Available> {
    let url = format!(
        "https://api.github.com/repos/{OWNER}/{}/releases/latest",
        app.repo
    );
    let reply = client()?.get(url).send()?;
    if reply.status() == 429
        || (reply.status() == 403
            && reply
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|v| v == "0"))
    {
        return Err("Limite GitHub raggiunto. Riprova tra qualche minuto.".into());
    }
    let release: Release = reply.error_for_status()?.json()?;
    release.select(app)
}

pub fn install(app: &App, release: &Available, root: &Path, progress: impl Fn(f32)) -> Result<()> {
    let directory = root.join(app.repo);
    fs::create_dir_all(&directory)?;
    let url = format!(
        "https://github.com/{OWNER}/{}/releases/download/{}/{}",
        app.repo, release.tag, release.name
    );
    let reply = client()?.get(url).send()?.error_for_status()?;
    save_verified(reply, release, &directory, progress)
}

fn save_verified(
    mut input: impl Read,
    release: &Available,
    directory: &Path,
    progress: impl Fn(f32),
) -> Result<()> {
    let mut part = tempfile::NamedTempFile::new_in(directory)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut total = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > release.size {
            return Err("Il file supera la dimensione annunciata.".into());
        }
        hash.update(&buffer[..count]);
        part.write_all(&buffer[..count])?;
        progress(total as f32 / release.size as f32);
    }
    if total != release.size || format!("{:x}", hash.finalize()) != release.digest {
        return Err("Download non integro: dimensione o checksum non corrispondente.".into());
    }
    part.flush()?;
    if !is_appimage(part.path()) {
        return Err("Il file scaricato non e' una AppImage valida.".into());
    }
    part.as_file()
        .set_permissions(fs::Permissions::from_mode(0o700))?;
    part.as_file().sync_all()?;
    part.persist(directory.join(&release.name))?;
    Ok(())
}

pub fn xdg(var: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(absolute) = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(absolute);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(fallback))
        .ok_or_else(|| io::Error::other("Impossibile trovare la cartella personale."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifica_integrita_prima_di_sostituire_il_file() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = b"\x7fELF\0\0\0\0AI\x02test";
        let app = &APPS[0];
        let name = app.filename(&Version::new(0, 1, 0));
        let file = directory.path().join(&name);
        std::fs::write(&file, bytes).unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&file, perms).unwrap();
        assert!(is_appimage(&file));
    }
}
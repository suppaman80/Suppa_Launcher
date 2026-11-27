use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::PathBuf};

use crate::catalog::{Result, is_appimage, xdg};

pub fn entry() -> Result<PathBuf> {
    Ok(
        xdg("XDG_CONFIG_HOME", ".config")?
            .join("autostart/io.github.suppaman80.Suppaworld.desktop"),
    )
}

fn exec_arg(value: &str) -> String {
    // I valori Desktop Entry e la riga Exec hanno due livelli di escaping.
    let mut escaped = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                escaped.push_str("\\\\");
                escaped.push(ch);
            }
            '%' => escaped.push_str("%%"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

pub fn set(enabled: bool) -> Result<()> {
    let entry = entry()?;
    if !enabled {
        return match fs::remove_file(entry) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        };
    }
    let source = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && is_appimage(path))
        .ok_or("Apri il launcher dalla sua AppImage per attivare l'avvio automatico.")?;
    let directory = xdg("XDG_DATA_HOME", ".local/share")?.join("suppaworld/launcher");
    let target = directory.join("Suppaworld-Launcher.AppImage");
    let path = target
        .to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or("Il percorso non e' compatibile con l'avvio automatico.")?;
    fs::create_dir_all(&directory)?;
    let mut package = tempfile::NamedTempFile::new_in(&directory)?;
    std::io::copy(&mut fs::File::open(source)?, &mut package)?;
    package
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o700))?;
    package.as_file().sync_all()?;
    package.persist(target.as_path())?;

    let desktop_directory = entry.parent().ok_or("Cartella di avvio non valida.")?;
    fs::create_dir_all(desktop_directory)?;
    let mut desktop = tempfile::NamedTempFile::new_in(desktop_directory)?;
    writeln!(
        desktop,
        "[Desktop Entry]\nType=Application\nName=Suppaworld Launcher\nExec={}\nTerminal=false",
        exec_arg(path)
    )?;
    desktop.persist(entry)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protegge_spazi_e_codici_desktop() {
        assert_eq!(
            exec_arg("/home/my folder/100%.AppImage"),
            "\"/home/my folder/100%%.AppImage\""
        );
        assert_eq!(exec_arg("a\\b$`\""), "\"a\\\\\\\\b\\\\$\\\\`\\\\\"\"");
    }
}
use std::fs;
use std::path::{Path, PathBuf};

use nix::sys::statvfs::statvfs;
use serde::{Deserialize, Serialize};

use crate::{MarketError, Result};

pub const FILL_LIMIT: f64 = 0.9;
const IDENTITY: &str = "volume.json";
const PARTIAL: &str = ".partial";

#[derive(Serialize, Deserialize)]
struct Identity {
    id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextVolume {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    pub available: u64,
}

impl Space {
    fn used_share(self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        (self.total - self.available.min(self.total)) as f64 / self.total as f64
    }
}

#[derive(Clone, Debug)]
pub struct TextVolumes {
    volumes: Vec<TextVolume>,
    fill_limit: f64,
}

impl Default for TextVolumes {
    fn default() -> Self {
        Self {
            volumes: Vec::new(),
            fill_limit: FILL_LIMIT,
        }
    }
}

impl TextVolumes {
    pub fn open(roots: &[PathBuf], fill_limit: f64) -> Result<Self> {
        let mut volumes = Vec::new();
        for root in roots {
            let volume = identify(root)?;
            if volumes
                .iter()
                .any(|known: &TextVolume| known.id == volume.id)
            {
                return Err(MarketError::Text(format!(
                    "{} is the same volume as another text directory",
                    root.display()
                )));
            }
            let partial = root.join(PARTIAL);
            if partial.exists() {
                fs::remove_dir_all(&partial)?;
            }
            volumes.push(volume);
        }
        Ok(Self {
            volumes,
            fill_limit,
        })
    }

    pub fn fill_limit(&self) -> f64 {
        self.fill_limit
    }

    pub fn volumes(&self) -> &[TextVolume] {
        &self.volumes
    }

    pub fn root(&self, id: &str) -> Option<&Path> {
        self.volumes
            .iter()
            .find(|volume| volume.id == id)
            .map(|volume| volume.root.as_path())
    }

    pub fn space(volume: &TextVolume) -> Result<Space> {
        let stats = statvfs(&volume.root)
            .map_err(|error| MarketError::Text(format!("{}: {error}", volume.root.display())))?;
        let block = stats.fragment_size();
        Ok(Space {
            total: u64::from(stats.blocks()) * block,
            available: u64::from(stats.blocks_available()) * block,
        })
    }

    pub fn choose(&self) -> Result<&TextVolume> {
        let mut roomiest: Option<(&TextVolume, u64)> = None;
        for volume in &self.volumes {
            let space = Self::space(volume)?;
            if space.used_share() < self.fill_limit
                && roomiest.is_none_or(|(_, available)| space.available > available)
            {
                roomiest = Some((volume, space.available));
            }
        }
        roomiest.map(|(volume, _)| volume).ok_or_else(|| {
            MarketError::Text(format!(
                "every text volume is at least {:.0}% full",
                self.fill_limit * 100.0
            ))
        })
    }

    pub fn place(volume: &TextVolume, written: &Path, relative: &str) -> Result<u64> {
        let target = volume.root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(written, &target)?;
        Ok(fs::metadata(&target)?.len())
    }

    pub fn scratch(volume: &TextVolume, name: &str) -> Result<PathBuf> {
        let partial = volume.root.join(PARTIAL);
        fs::create_dir_all(&partial)?;
        Ok(partial.join(name))
    }
}

fn identify(root: &Path) -> Result<TextVolume> {
    if !root.is_dir() {
        return Err(MarketError::Text(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let path = root.join(IDENTITY);
    let id = match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<Identity>(&bytes)
                .map_err(|error| MarketError::Text(format!("{}: {error}", path.display())))?
                .id
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = format!("{:016x}", rand::random::<u64>());
            let written = root.join(format!("{IDENTITY}.partial"));
            fs::write(
                &written,
                serde_json::to_vec(&Identity { id: id.clone() })
                    .map_err(|error| MarketError::Text(error.to_string()))?,
            )?;
            fs::rename(&written, &path)?;
            id
        }
        Err(error) => return Err(error.into()),
    };
    Ok(TextVolume {
        id,
        root: root.to_owned(),
    })
}

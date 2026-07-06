//! v3 project registry. Committed identity (project.toml per project dir),
//! machine-local path map (state/projects.json), auto-registration from
//! session cwd, and exclusion with cleanup.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Store, slugify};
use crate::errors::CoreError;

/// Committed per-project identity (knowledge/projects/<slug>/project.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub slug: String,
    #[serde(default)]
    pub remote_url: Option<String>,
    /// YYYY-MM-DD of first registration.
    pub registered: String,
}

/// Machine-local slug -> absolute path map (state/projects.json). Rebuildable:
/// re-derived from observed sessions, so losing it only delays resolution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathMap {
    #[serde(default)]
    pub paths: BTreeMap<String, String>,
}

impl PathMap {
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = store_root.join("state").join("projects.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(PathMap::default()),
        }
    }

    pub fn save(&self, store_root: &Path) -> Result<(), CoreError> {
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        let dir = store_root.join("state");
        std::fs::create_dir_all(&dir).map_err(io)?;
        let json =
            serde_json::to_string_pretty(self).map_err(|e| CoreError::Parse(e.to_string()))?;
        let tmp = dir.join("projects.json.tmp");
        std::fs::write(&tmp, json).map_err(io)?;
        std::fs::rename(&tmp, dir.join("projects.json")).map_err(io)
    }
}

pub struct Registration {
    pub slug: String,
    pub newly_registered: bool,
}

fn git_in(dir: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn read_meta(store: &Store, slug: &str) -> Option<ProjectMeta> {
    let path = store
        .knowledge_dir()
        .join("projects")
        .join(slug)
        .join("project.toml");
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn write_meta(store: &Store, meta: &ProjectMeta) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let dir = store.knowledge_dir().join("projects").join(&meta.slug);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let content = toml::to_string_pretty(meta).map_err(|e| CoreError::Parse(e.to_string()))?;
    std::fs::write(dir.join("project.toml"), content).map_err(io)
}

fn all_metas(store: &Store) -> Vec<ProjectMeta> {
    let mut metas = Vec::new();
    let projects_dir = store.knowledge_dir().join("projects");
    if let Ok(read) = std::fs::read_dir(&projects_dir) {
        for item in read.flatten() {
            if item.path().is_dir() {
                if let Some(slug) = item.file_name().to_str() {
                    if let Some(meta) = read_meta(store, slug) {
                        metas.push(meta);
                    }
                }
            }
        }
    }
    metas
}

/// Register (or recognize) the project containing `cwd`. Resolution:
/// git root of cwd (falls back to cwd for non-git dirs) -> match existing
/// registrations by remote_url, then by recorded path, else create new.
/// Never call this for excluded paths — check `is_excluded` first.
pub fn register(store: &Store, cwd: &str) -> Result<Registration, CoreError> {
    let root = git_in(cwd, &["rev-parse", "--show-toplevel"]).unwrap_or_else(|| {
        std::fs::canonicalize(cwd)
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .unwrap_or_else(|| cwd.to_string())
    });
    let remote = git_in(&root, &["remote", "get-url", "origin"]);

    let mut map = PathMap::load(store.root())?;

    // 1. remote_url match (stable identity)
    if let Some(ref url) = remote {
        if let Some(meta) = all_metas(store)
            .into_iter()
            .find(|m| m.remote_url.as_deref() == Some(url.as_str()))
        {
            if map.paths.get(&meta.slug).map(String::as_str) != Some(root.as_str()) {
                map.paths.insert(meta.slug.clone(), root.clone());
                map.save(store.root())?;
            }
            return Ok(Registration {
                slug: meta.slug,
                newly_registered: false,
            });
        }
    }
    // 2. recorded-path match (non-git dirs, or repos without remotes)
    if let Some((slug, _)) = map.paths.iter().find(|(_, p)| p.as_str() == root) {
        return Ok(Registration {
            slug: slug.clone(),
            newly_registered: false,
        });
    }

    // 3. new registration
    let base = Path::new(&root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let mut slug = slugify(base);
    let mut i = 2;
    while read_meta(store, &slug).is_some() {
        slug = format!("{}-{}", slugify(base), i);
        i += 1;
    }
    write_meta(
        store,
        &ProjectMeta {
            slug: slug.clone(),
            remote_url: remote,
            registered: chrono::Utc::now().date_naive().to_string(),
        },
    )?;
    map.paths.insert(slug.clone(), root);
    map.save(store.root())?;
    Ok(Registration {
        slug,
        newly_registered: true,
    })
}

/// Path-prefix exclusion against `config.privacy.exclude_projects`.
/// A prefix matches the directory itself or anything under it.
/// Both sides are canonicalized when possible so symlink spellings
/// (e.g. /tmp vs /private/tmp on macOS) cannot bypass an exclusion.
pub fn is_excluded(path: &str, exclude_projects: &[String]) -> bool {
    let canon = |s: &str| -> String {
        std::fs::canonicalize(s)
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .unwrap_or_else(|| s.to_string())
    };
    let path = canon(path);
    exclude_projects.iter().any(|prefix| {
        let prefix = canon(prefix);
        path == prefix || path.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
    })
}

/// Exclusion cleanup: delete the project's knowledge subtree (recoverable via
/// store git history), drop it from the path map, and remove its
/// CLAUDE.local.md (the whole file — it is retro-owned build output).
pub fn cleanup_excluded(
    store: &Store,
    slug: &str,
    project_path: Option<&str>,
) -> Result<(), CoreError> {
    let dir = store.knowledge_dir().join("projects").join(slug);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    let mut map = PathMap::load(store.root())?;
    if map.paths.remove(slug).is_some() {
        map.save(store.root())?;
    }
    if let Some(path) = project_path {
        let local_md = Path::new(path).join("CLAUDE.local.md");
        if local_md.exists() {
            std::fs::remove_file(&local_md).map_err(|e| CoreError::Io(e.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git_project(dir: &Path, remote: Option<&str>) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init"]);
        if let Some(url) = remote {
            run(&["remote", "add", "origin", url]);
        }
    }

    #[test]
    fn register_new_project_creates_meta_and_notifies() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj_tmp = TempDir::new().unwrap();
        let proj = proj_tmp.path().join("My API-Service");
        std::fs::create_dir_all(&proj).unwrap();
        git_project(&proj, Some("git@github.com:me/my-api.git"));

        let reg = register(&store, proj.to_str().unwrap()).unwrap();
        assert!(reg.newly_registered);
        assert_eq!(reg.slug, "my-api-service");
        // committed identity file exists
        let meta_path = store_tmp
            .path()
            .join("knowledge/projects/my-api-service/project.toml");
        let meta = std::fs::read_to_string(meta_path).unwrap();
        assert!(meta.contains("git@github.com:me/my-api.git"));
        // second registration is a no-op
        let again = register(&store, proj.to_str().unwrap()).unwrap();
        assert!(!again.newly_registered);
        assert_eq!(again.slug, "my-api-service");
    }

    #[test]
    fn register_matches_by_remote_url_when_path_moved() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let a = TempDir::new().unwrap();
        git_project(a.path(), Some("git@github.com:me/stable.git"));
        let a_canon = std::fs::canonicalize(a.path()).unwrap();
        let first = register(&store, a_canon.to_str().unwrap()).unwrap();

        // same repo cloned elsewhere (different path, same remote)
        let b = TempDir::new().unwrap();
        git_project(b.path(), Some("git@github.com:me/stable.git"));
        let b_canon = std::fs::canonicalize(b.path()).unwrap();
        let second = register(&store, b_canon.to_str().unwrap()).unwrap();
        assert!(!second.newly_registered);
        assert_eq!(second.slug, first.slug);
        // path map updated to the new location
        let map = PathMap::load(store_tmp.path()).unwrap();
        assert_eq!(map.paths[&first.slug], b_canon.to_str().unwrap());
    }

    #[test]
    fn non_git_directory_registers_by_path() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        let reg = register(&store, proj.path().to_str().unwrap()).unwrap();
        assert!(reg.newly_registered);
        let again = register(&store, proj.path().to_str().unwrap()).unwrap();
        assert!(!again.newly_registered);
    }

    #[test]
    fn non_git_dir_two_spellings_register_once() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        let raw = proj.path().to_str().unwrap().to_string();
        let canonical = std::fs::canonicalize(proj.path())
            .unwrap()
            .display()
            .to_string();
        let first = register(&store, &raw).unwrap();
        let second = register(&store, &canonical).unwrap();
        assert!(!second.newly_registered, "same dir, different spelling");
        assert_eq!(first.slug, second.slug);
    }

    #[test]
    fn is_excluded_survives_symlink_spelling() {
        let real = TempDir::new().unwrap();
        let raw = real.path().to_str().unwrap().to_string();
        let canonical = std::fs::canonicalize(real.path())
            .unwrap()
            .display()
            .to_string();
        // exclude written in one spelling, path arrives in the other
        assert!(is_excluded(&canonical, &[raw.clone()]));
        assert!(is_excluded(&raw, &[canonical]));
    }

    #[test]
    fn is_excluded_matches_path_prefixes() {
        let excludes = vec!["/Users/me/private".to_string()];
        assert!(is_excluded("/Users/me/private/notes", &excludes));
        assert!(is_excluded("/Users/me/private", &excludes));
        assert!(!is_excluded("/Users/me/privateer", &excludes));
        assert!(!is_excluded("/Users/me/work/app", &excludes));
    }

    #[test]
    fn cleanup_excluded_removes_knowledge_and_local_md_block() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        git_project(proj.path(), None);
        let reg = register(&store, proj.path().to_str().unwrap()).unwrap();

        // seed a projected CLAUDE.local.md
        let dir = store_tmp.path().join("knowledge/projects").join(&reg.slug);
        assert!(dir.is_dir());
        std::fs::write(
            proj.path().join("CLAUDE.local.md"),
            "<!-- retro:managed:start -->\n- old rule\n<!-- retro:managed:end -->\n",
        )
        .unwrap();

        cleanup_excluded(&store, &reg.slug, Some(proj.path().to_str().unwrap())).unwrap();
        assert!(!dir.exists());
        assert!(!proj.path().join("CLAUDE.local.md").exists());
        let map = PathMap::load(store_tmp.path()).unwrap();
        assert!(!map.paths.contains_key(&reg.slug));
    }
}

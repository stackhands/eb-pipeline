//! Read-only consumption of bundles written by `eb-stack package plan`.

use crate::exit::Fail;
use crate::plan::module_rel;
use crate::util::{list_word, ListLine};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// One ecosystem-native check associated with an emitted recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Run `pip check` with the recipe's module loaded.
    Pip {
        /// Emitted easyconfig whose installed module provides `pip`.
        recipe: PathBuf,
    },
    /// Check the installed package's DESCRIPTION dependencies with R.
    R {
        /// Emitted easyconfig whose installed module provides the R package.
        recipe: PathBuf,
        /// Package name passed to R.
        package: String,
    },
}

impl Verification {
    /// The module name derived from the emitted easyconfig.
    pub fn module(&self) -> Result<String, Fail> {
        let recipe = match self {
            Self::Pip { recipe } | Self::R { recipe, .. } => recipe,
        };
        module_rel(&recipe.display().to_string()).ok_or_else(|| {
            Fail::usage(format!(
                "package-plan recipe has no conventional EasyBuild name: {}",
                recipe.display()
            ))
        })
    }
}

/// Validated package-plan input and the build lines derived from it.
#[derive(Clone, Debug)]
pub struct Input {
    /// Absolute bundle directory.
    pub bundle: PathBuf,
    /// SHA-256 of `package.plan.json` exactly as written by eb-stack.
    pub digest: String,
    /// Recipes in the bundle's declared build order.
    pub lines: Vec<ListLine>,
    /// Checks implied by package origins, one per emitted recipe.
    pub verification: Vec<Verification>,
}

fn read_json(path: &Path) -> Result<(Vec<u8>, Value), Fail> {
    let bytes = std::fs::read(path)
        .map_err(|e| Fail::usage(format!("cannot read {}: {e}", path.display())))?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|e| Fail::usage(format!("cannot parse {}: {e}", path.display())))?;
    Ok((bytes, value))
}

fn schema_one(path: &Path, value: &Value) -> Result<(), Fail> {
    match value.get("schema_version").and_then(Value::as_u64) {
        Some(1) => Ok(()),
        Some(version) => Err(Fail::usage(format!(
            "{} has unsupported schema_version {version}",
            path.display()
        ))),
        None => Err(Fail::usage(format!(
            "{} has no integer schema_version",
            path.display()
        ))),
    }
}

fn safe_relative(bundle: &Path, raw: &str, what: &str) -> Result<PathBuf, Fail> {
    let relative = Path::new(raw);
    if raw.is_empty()
        || relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Fail::usage(format!(
            "{what} must be a non-empty bundle-relative path: {raw}"
        )));
    }
    let path = bundle.join(relative.components().collect::<PathBuf>());
    if path.exists() {
        ensure_contained(bundle, &path, what)?;
    }
    Ok(path)
}

fn ensure_contained(bundle: &Path, path: &Path, what: &str) -> Result<PathBuf, Fail> {
    let canonical_bundle = std::fs::canonicalize(bundle)
        .map_err(|e| Fail::usage(format!("cannot resolve {}: {e}", bundle.display())))?;
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| Fail::usage(format!("cannot resolve {}: {e}", path.display())))?;
    if !canonical.starts_with(&canonical_bundle) {
        return Err(Fail::usage(format!(
            "{what} escapes the package-plan bundle: {}",
            path.display()
        )));
    }
    Ok(canonical)
}

fn discover(bundle: &Path, root: &Path, extension: &str) -> Result<Vec<PathBuf>, Fail> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let canonical_root = ensure_contained(bundle, root, "package-plan directory")?;
    let mut directories = vec![root.to_path_buf()];
    let mut seen_directories = BTreeSet::from([canonical_root]);
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|e| Fail::usage(format!("cannot read {}: {e}", directory.display())))?;
        for entry in entries {
            let path = entry
                .map_err(|e| Fail::usage(format!("cannot read {}: {e}", directory.display())))?
                .path();
            if path.is_dir() {
                let canonical = ensure_contained(bundle, &path, "package-plan directory")?;
                if seen_directories.insert(canonical) {
                    directories.push(path);
                }
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                ensure_contained(bundle, &path, "package-plan file")?;
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn declared_recipes(bundle: &Path) -> Result<Vec<PathBuf>, Fail> {
    let order = bundle.join("build-order.json");
    if !order.is_file() {
        return discover(bundle, &bundle.join("easyconfigs"), "eb");
    }
    let (_, value) = read_json(&order)?;
    schema_one(&order, &value)?;
    let recipes = value
        .get("recipes")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::usage(format!("{} has no recipes array", order.display())))?;
    let mut seen = BTreeSet::new();
    let mut declared = Vec::with_capacity(recipes.len());
    for entry in recipes {
        let raw = entry
            .as_str()
            .ok_or_else(|| Fail::usage(format!("{} recipes must be strings", order.display())))?;
        let path = safe_relative(bundle, raw, "build-order recipe")?;
        if !path.is_file() {
            return Err(Fail::usage(format!(
                "build-order recipe does not exist: {}",
                path.display()
            )));
        }
        if !seen.insert(path.clone()) {
            return Err(Fail::usage(format!(
                "{} lists recipe more than once: {raw}",
                order.display()
            )));
        }
        declared.push(path);
    }
    Ok(declared)
}

fn plan_identity(path: &Path) -> Result<(String, String), Fail> {
    let (_, value) = read_json(path)?;
    schema_one(path, &value)?;
    let origin = value
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(|| Fail::usage(format!("{} has no origin", path.display())))?;
    let package = value
        .pointer("/package/name")
        .and_then(Value::as_str)
        .ok_or_else(|| Fail::usage(format!("{} has no package.name", path.display())))?;
    Ok((origin.to_string(), package.to_string()))
}

fn verification(bundle: &Path, recipes: &[PathBuf]) -> Result<Vec<Verification>, Fail> {
    let closure = bundle.join("closure.plan.json");
    if !closure.is_file() {
        let (origin, package) = plan_identity(&bundle.join("package.plan.json"))?;
        return Ok(for_recipes(&origin, &package, recipes));
    }
    let (_, value) = read_json(&closure)?;
    schema_one(&closure, &value)?;
    let root = value.get("root").ok_or_else(|| {
        Fail::usage(format!(
            "{} has no root package reference",
            closure.display()
        ))
    })?;
    let mut refs = vec![root];
    refs.extend(
        value
            .get("companions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    );
    let mut out = Vec::new();
    for entry in refs {
        let manifest = entry
            .get("manifest")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Fail::usage(format!(
                    "{} package reference has no manifest",
                    closure.display()
                ))
            })?;
        let plan = safe_relative(bundle, manifest, "closure manifest")?;
        let (origin, package) = plan_identity(&plan)?;
        let paths = entry
            .get("recipes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Fail::usage(format!(
                    "{} package reference has no recipes",
                    closure.display()
                ))
            })?
            .iter()
            .map(|recipe| {
                let raw = recipe.as_str().ok_or_else(|| {
                    Fail::usage(format!("{} recipes must be strings", closure.display()))
                })?;
                let path = safe_relative(bundle, raw, "closure recipe")?;
                if !recipes.contains(&path) {
                    return Err(Fail::usage(format!(
                        "{} recipe is absent from build-order.json: {raw}",
                        closure.display()
                    )));
                }
                Ok(path)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for check in for_recipes(&origin, &package, &paths) {
            if !out.contains(&check) {
                out.push(check);
            }
        }
    }
    Ok(out)
}

fn for_recipes(origin: &str, package: &str, recipes: &[PathBuf]) -> Vec<Verification> {
    recipes
        .iter()
        .filter_map(|recipe| match origin {
            "pypi" => Some(Verification::Pip {
                recipe: recipe.clone(),
            }),
            "cran" => Some(Verification::R {
                recipe: recipe.clone(),
                package: package.to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// Read an eb-stack package-plan bundle without modifying it.
pub fn load(path: &Path) -> Result<Input, Fail> {
    let bundle = std::path::absolute(path)
        .map_err(|e| Fail::usage(format!("cannot make {} absolute: {e}", path.display())))?;
    if !bundle.is_dir() {
        return Err(Fail::usage(format!(
            "package-plan bundle is not a directory: {}",
            bundle.display()
        )));
    }
    let manifest = bundle.join("package.plan.json");
    let (bytes, value) = read_json(&manifest)?;
    schema_one(&manifest, &value)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let recipes = declared_recipes(&bundle)?;
    if recipes.is_empty() {
        return Err(Fail::usage(format!(
            "package-plan bundle has no emitted recipes under {}",
            bundle.join("easyconfigs").display()
        )));
    }
    let robot = format!("--robot={}", bundle.join("easyconfigs").display());
    let easyblocks = discover(&bundle, &bundle.join("easyblocks"), "py")?;
    let include = (!easyblocks.is_empty()).then(|| {
        format!(
            "--include-easyblocks={}",
            easyblocks
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    });
    let mut lines = Vec::with_capacity(recipes.len());
    for recipe in &recipes {
        let mut words = vec![recipe.display().to_string(), robot.clone()];
        if let Some(option) = &include {
            words.push(option.clone());
        }
        let raw = words
            .iter()
            .map(|word| list_word(word))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        lines.push(ListLine {
            raw,
            easyconfig: words[0].clone(),
            args: words[1..].to_vec(),
        });
    }
    let verification = verification(&bundle, &recipes)?;
    Ok(Input {
        bundle,
        digest,
        lines,
        verification,
    })
}

/// The R expression used for an installed package's DESCRIPTION dependency check.
pub const R_DESCRIPTION_CHECK: &str = r#"p <- commandArgs(TRUE)[1]; i <- installed.packages(); if (!(p %in% rownames(i))) stop(sprintf("package %s is not installed", p)); d <- tools::package_dependencies(p, db=i, which=c("Depends","Imports","LinkingTo"), recursive=FALSE)[[p]]; m <- setdiff(d, rownames(i)); if (length(m)) stop(sprintf("%s has missing DESCRIPTION dependencies: %s", p, paste(m, collapse=", ")))"#;

#[cfg(test)]
mod tests {
    use super::load;

    #[test]
    fn closure_references_verify_each_recipe_once() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("easyconfigs")).unwrap();
        std::fs::write(
            root.path().join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("companion.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"companion"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("build-order.json"),
            r#"{"schema_version":1,"recipes":["easyconfigs/demo-1.0.eb"]}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("easyconfigs/demo-1.0.eb"), "").unwrap();
        std::fs::write(
            root.path().join("closure.plan.json"),
            r#"{"schema_version":1,"root":{"manifest":"package.plan.json","recipes":["easyconfigs/demo-1.0.eb"]},"companions":[{"manifest":"companion.plan.json","recipes":["easyconfigs/demo-1.0.eb"]}]}"#,
        )
        .unwrap();

        let input = load(root.path()).unwrap();
        assert_eq!(input.verification.len(), 1, "{:?}", input.verification);
    }

    #[test]
    fn closure_recipes_must_appear_in_the_build_order() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("easyconfigs")).unwrap();
        std::fs::write(
            root.path().join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("build-order.json"),
            r#"{"schema_version":1,"recipes":["easyconfigs/A-1.0.eb"]}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("easyconfigs/A-1.0.eb"), "").unwrap();
        std::fs::write(root.path().join("easyconfigs/B-1.0.eb"), "").unwrap();
        std::fs::write(
            root.path().join("closure.plan.json"),
            r#"{"schema_version":1,"root":{"manifest":"package.plan.json","recipes":["easyconfigs/B-1.0.eb"]},"companions":[]}"#,
        )
        .unwrap();

        let error = load(root.path()).unwrap_err();
        assert_eq!(
            error.msg,
            format!(
                "{} recipe is absent from build-order.json: easyconfigs/B-1.0.eb",
                root.path().join("closure.plan.json").display()
            )
        );
    }

    #[test]
    fn build_order_cannot_escape_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("build-order.json"),
            r#"{"schema_version":1,"recipes":["../outside.eb"]}"#,
        )
        .unwrap();
        let error = load(root.path()).unwrap_err();
        assert!(error.msg.contains("bundle-relative"), "{}", error.msg);
    }

    #[test]
    fn build_order_cannot_repeat_a_recipe() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("easyconfigs")).unwrap();
        std::fs::write(
            root.path().join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("easyconfigs/demo.eb"), "name = 'demo'\n").unwrap();
        std::fs::write(
            root.path().join("build-order.json"),
            r#"{"schema_version":1,"recipes":["easyconfigs/demo.eb","easyconfigs/demo.eb"]}"#,
        )
        .unwrap();

        let error = load(root.path()).unwrap_err();
        assert!(
            error.msg.contains("lists recipe more than once"),
            "{}",
            error.msg
        );
    }

    #[test]
    fn build_order_recipe_cannot_escape_through_a_symbolic_link() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("bundle");
        std::fs::create_dir_all(bundle.join("easyconfigs")).unwrap();
        std::fs::write(
            bundle.join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(
            bundle.join("build-order.json"),
            r#"{"schema_version":1,"recipes":["easyconfigs/demo.eb"]}"#,
        )
        .unwrap();
        let outside = root.path().join("outside.eb");
        std::fs::write(&outside, "name = 'outside'\n").unwrap();
        std::os::unix::fs::symlink(&outside, bundle.join("easyconfigs/demo.eb")).unwrap();

        let error = load(&bundle).unwrap_err();
        assert!(
            error.msg.contains("escapes the package-plan bundle"),
            "{}",
            error.msg
        );
    }

    #[test]
    fn discovered_recipe_tree_cannot_escape_through_a_symbolic_link() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("bundle");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            bundle.join("package.plan.json"),
            r#"{"schema_version":1,"origin":"pypi","package":{"name":"demo"}}"#,
        )
        .unwrap();
        std::fs::write(outside.join("demo.eb"), "name = 'outside'\n").unwrap();
        std::os::unix::fs::symlink(&outside, bundle.join("easyconfigs")).unwrap();

        let error = load(&bundle).unwrap_err();
        assert!(
            error.msg.contains("escapes the package-plan bundle"),
            "{}",
            error.msg
        );
    }
}

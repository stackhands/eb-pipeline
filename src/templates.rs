//! Built-in site file templates.
//!
//! Each template is a complete, valid site file with placeholder values only
//! (the same lint that guards the repository runs over them). `eb-pipeline
//! init --template NAME` writes one as the starting point for a new site.

/// One template: its name, a one-line summary, and the file text.
pub struct Template {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

/// Every built-in template, by name.
pub const TEMPLATES: &[Template] = &[
    Template {
        name: "small-cluster",
        summary: "Slurm, one CPU target; no EESSI, inode quota or licences",
        text: include_str!("../templates/small-cluster.toml"),
    },
    Template {
        name: "eessi-site",
        summary:
            "Slurm, CPU and GPU targets on an EESSI site layer, with quota, caches and licences",
        text: include_str!("../templates/eessi-site.toml"),
    },
];

/// The template called `name`.
pub fn get(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

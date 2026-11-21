//! Renders the user guide's generated partials from the code they
//! document, so the book can't drift from it:
//!
//! - `cli.md`: the `ctrlassist` command tree, the CLI reference.
//! - `release-version.txt`: the release the install commands download.
//!
//! Run `gen-docs <output dir>`; `just docs-gen` writes the partials into
//! `docs/book/src/generated/` and `just docs-gen-check` fails when the
//! committed copies are stale. Every partial is written hook-clean (no
//! trailing whitespace, one final newline) so a regeneration and the
//! committed file always agree.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let out = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: gen-docs <output dir>")?;
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    write(&out, "cli.md", &cli_reference())?;
    write(
        &out,
        "release-version.txt",
        &format!("export VERSION={}\n", env!("CARGO_PKG_VERSION")),
    )
}

fn write(dir: &Path, name: &str, contents: &str) -> Result<()> {
    let path = dir.join(name);
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}

/// The clap tree as Markdown, starting at "Command Overview": the CLI
/// page supplies its own heading and introduction, while clap-markdown
/// always writes a title and a preamble, so that leading run is dropped
/// and the tool's footer turned off.
fn cli_reference() -> String {
    let markdown = clap_markdown::help_markdown_custom::<ctrlassist_cli::Cli>(
        &clap_markdown::MarkdownOptions::new().show_footer(false),
    );
    // Trimmed per line: clap-markdown leaves a trailing space after the
    // dash of a subcommand with no description, which the whitespace gate
    // would strip right back.
    let body = markdown
        .lines()
        .skip_while(|line| {
            line.trim().is_empty()
                || line.starts_with("# ")
                || line.starts_with("This document contains the help content")
        })
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference names every verb a person can type, and carries
    /// nothing the whitespace gate would rewrite.
    #[test]
    fn the_cli_reference_covers_the_command_and_is_clean() {
        let reference = cli_reference();
        assert!(
            reference.starts_with("**Command Overview:**"),
            "{reference}"
        );
        for verb in [
            "ctrlassist status",
            "ctrlassist pads",
            "ctrlassist identify",
            "ctrlassist profile put",
            "ctrlassist start",
            "ctrlassist select",
            "ctrlassist update",
            "ctrlassist service tray",
        ] {
            assert!(
                reference.contains(&format!("`{verb}`")),
                "{verb} is missing"
            );
        }
        assert!(reference.lines().all(|line| line == line.trim_end()));
        assert!(reference.ends_with('\n') && !reference.ends_with("\n\n"));
    }

    #[test]
    fn the_partials_are_written_where_asked() {
        let dir = std::env::temp_dir().join(format!("ctrlassist-docgen-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        write(&dir, "release-version.txt", "export VERSION=1.2.3\n").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("release-version.txt")).unwrap(),
            "export VERSION=1.2.3\n"
        );
        fs::remove_dir_all(&dir).unwrap();
        assert!(write(&dir.join("gone"), "x", "y").is_err());
    }
}
